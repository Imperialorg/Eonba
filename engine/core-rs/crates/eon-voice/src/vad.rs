//! Silero VAD v5 inference over ONNX Runtime (docs/implementation/04, sections 2 and 3).
//!
//! The model takes 512 samples (32 ms at 16 kHz) plus a recurrent state, and returns a speech probability and the
//! next state. The state must be carried between frames of one stream, so each stream owns its own `SileroVad`.

use std::path::Path;

use eon_primitives::{EonError, EonResult};
use ort::session::Session;
use ort::value::Tensor;

use crate::geometry::{SAMPLE_RATE_HZ, VAD_FRAME_SAMPLES};

const STATE_LEN: usize = 2 * 128;

pub struct SileroVad {
    session: Session,
    state: Vec<f32>,
}

fn onnx_err(context: &str, e: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("silero.{context}"),
        reason: e.to_string(),
    }
}

impl SileroVad {
    /// Loads the model from `path`. The file is expected to be the digest-verified artifact from `models.lock.json`.
    pub fn load(path: impl AsRef<Path>) -> EonResult<Self> {
        let session = Session::builder(crate::runtime::environment()?)
            .map_err(|e| onnx_err("builder", e))?
            .with_intra_threads(1)
            .map_err(|e| onnx_err("threads", e))?
            .commit_from_file(path)
            .map_err(|e| onnx_err("load", e))?;
        Ok(Self {
            session,
            state: vec![0.0; STATE_LEN],
        })
    }

    /// Resets the recurrent state, for example at the start of a new utterance stream.
    pub fn reset(&mut self) {
        self.state.iter_mut().for_each(|x| *x = 0.0);
    }

    /// Returns the speech probability in [0, 1] for one 32 ms frame of 16 kHz mono samples.
    pub fn speech_probability(&mut self, frame: &[f32]) -> EonResult<f32> {
        if frame.len() != VAD_FRAME_SAMPLES {
            return Err(EonError::ValidationError(format!(
                "Silero v5 needs {VAD_FRAME_SAMPLES} samples per frame, got {}",
                frame.len()
            )));
        }
        let input = Tensor::from_array(([1usize, VAD_FRAME_SAMPLES], frame.to_vec())).map_err(|e| onnx_err("input", e))?;
        let state = Tensor::from_array(([2usize, 1, 128], self.state.clone())).map_err(|e| onnx_err("state", e))?;
        let sr = Tensor::from_array(((), vec![SAMPLE_RATE_HZ as i64])).map_err(|e| onnx_err("sr", e))?;
        let outputs = self
            .session
            .run(ort::inputs!["input" => input, "state" => state, "sr" => sr])
            .map_err(|e| onnx_err("run", e))?;
        let (_, probability) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| onnx_err("output", e))?;
        let (_, next_state) = outputs["stateN"]
            .try_extract_tensor::<f32>()
            .map_err(|e| onnx_err("stateN", e))?;
        if next_state.len() != STATE_LEN {
            return Err(EonError::InternalError(format!("unexpected Silero state size {}", next_state.len())));
        }
        self.state.copy_from_slice(next_state);
        probability
            .first()
            .copied()
            .ok_or_else(|| EonError::InternalError("empty Silero output".into()))
    }
}
