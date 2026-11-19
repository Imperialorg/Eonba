//! WeSpeaker ResNet34 speaker embeddings and the owner gate (docs/implementation/04, biometric gate).
//!
//! Audio goes through the shared 80-bin fbank front end, then per-utterance mean normalization over time, then the
//! network. The 256-dim embedding is L2-normalized, so cosine similarity is a dot product. The gate compares a candidate
//! against the enrolled owner embedding. The assistant's own echo should score low, which is why the gate sits behind
//! barge-in detection.

use std::path::Path;

use eon_primitives::{EonError, EonResult};
use ort::session::Session;
use ort::value::Tensor;

use crate::features::{kaldi_fbank, NUM_MEL_BINS};

pub const EMBEDDING_DIM: usize = 256;
/// Shortest audio the embedder accepts: 0.5 s, which is 50 fbank frames.
const MIN_FRAMES: usize = 50;

fn speaker_err(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("speaker.{operation}"),
        reason: reason.to_string(),
    }
}

pub struct SpeakerEmbedder {
    session: Session,
}

impl SpeakerEmbedder {
    pub fn load(model: impl AsRef<Path>) -> EonResult<Self> {
        let session = Session::builder(crate::runtime::environment()?)
            .map_err(|e| speaker_err("builder", e))?
            .with_intra_threads(2)
            .map_err(|e| speaker_err("threads", e))?
            .commit_from_file(model)
            .map_err(|e| speaker_err("load", e))?;
        Ok(Self { session })
    }

    /// Returns the L2-normalized embedding of 16 kHz mono speech of at least 0.5 s.
    pub fn embed(&mut self, samples: &[f32]) -> EonResult<Vec<f32>> {
        let frames = kaldi_fbank(samples)?;
        if frames.len() < MIN_FRAMES {
            return Err(EonError::ValidationError(format!(
                "speaker embedding needs at least {MIN_FRAMES} frames, got {}",
                frames.len()
            )));
        }
        let t = frames.len();
        let mut mean = vec![0.0f32; NUM_MEL_BINS];
        for frame in &frames {
            for (m, x) in mean.iter_mut().zip(frame) {
                *m += x;
            }
        }
        mean.iter_mut().for_each(|m| *m /= t as f32);
        let flat: Vec<f32> = frames
            .iter()
            .flat_map(|frame| frame.iter().zip(&mean).map(|(x, m)| x - m))
            .collect();

        let input = Tensor::from_array(([1usize, t, NUM_MEL_BINS], flat)).map_err(|e| speaker_err("input", e))?;
        let outputs = self
            .session
            .run(ort::inputs!["feats" => input])
            .map_err(|e| speaker_err("run", e))?;
        let (_, embedding) = outputs["embs"]
            .try_extract_tensor::<f32>()
            .map_err(|e| speaker_err("embs", e))?;
        if embedding.len() != EMBEDDING_DIM {
            return Err(speaker_err("embs", format!("expected {EMBEDDING_DIM} values, got {}", embedding.len())));
        }
        Ok(normalize(embedding))
    }
}

pub fn normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

/// Cosine similarity of two embedding vectors. Equal-length inputs are required.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Accepts a candidate only if its cosine similarity to the enrolled owner is at or above `threshold`.
pub struct SpeakerGate {
    owner: Vec<f32>,
    threshold: f32,
}

impl SpeakerGate {
    pub fn enroll(owner: &[f32], threshold: f32) -> EonResult<Self> {
        if owner.len() != EMBEDDING_DIM {
            return Err(EonError::ValidationError(format!(
                "owner embedding must have {EMBEDDING_DIM} values, got {}",
                owner.len()
            )));
        }
        Ok(Self {
            owner: normalize(owner),
            threshold,
        })
    }

    pub fn score(&self, candidate: &[f32]) -> f32 {
        cosine(&self.owner, candidate)
    }

    pub fn accepts(&self, candidate: &[f32]) -> bool {
        self.score(candidate) >= self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(index: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBEDDING_DIM];
        v[index] = 1.0;
        v
    }

    #[test]
    fn cosine_of_a_vector_with_itself_is_one_and_orthogonal_is_zero() {
        let a = normalize(&[3.0, 4.0, 0.0]);
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
        assert!(cosine(&unit(0), &unit(1)).abs() < 1e-6);
    }

    #[test]
    fn gate_accepts_the_owner_and_refuses_an_orthogonal_speaker() {
        let gate = SpeakerGate::enroll(&unit(0), 0.5).unwrap();
        assert!(gate.accepts(&unit(0)));
        assert!(!gate.accepts(&unit(1)));
    }

    #[test]
    fn enrollment_refuses_a_wrong_dimension() {
        assert!(SpeakerGate::enroll(&[0.1; 128], 0.5).is_err());
    }
}
