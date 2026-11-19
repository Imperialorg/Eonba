//! Silero VAD v5 inference over ONNX Runtime (docs/implementation/04, sections 2 and 3).
//!
//! The model takes 512 samples (32 ms at 16 kHz) plus a recurrent state, and returns a speech probability and the
//! next state. The state must be carried between frames of one stream, so each stream owns its own `SileroVad`.

use std::path::Path;

use eon_primitives::{EonError, EonResult};
use ort::session::Session;
use ort::value::Tensor;

use crate::endpoint::{Completeness, Endpoint, Endpointer, RESUME_HOLDOFF_MS};
use crate::geometry::{SAMPLE_RATE_HZ, VAD_FRAME_SAMPLES};

const STATE_LEN: usize = 2 * 128;
const CONTEXT: usize = 64;

/// Native VAD/endpointer for listening turns. Barge-in detection during active
/// playback runs separately; this holdoff starts only *after* a reply finishes.
pub struct VadEndpoint {
    vad: SileroVad,
    endpoint: Endpointer,
    holdoff_samples: usize,
    session: eon_primitives::SessionId,
}

impl VadEndpoint {
    pub fn new(vad: SileroVad) -> Self {
        Self {
            vad,
            endpoint: Endpointer::new(),
            holdoff_samples: 0,
            session: eon_primitives::SessionId::new(),
        }
    }

    pub fn session(&self) -> &eon_primitives::SessionId {
        &self.session
    }

    pub fn reply_finished(&mut self) {
        self.vad.reset();
        self.endpoint = Endpointer::new();
        self.holdoff_samples = crate::geometry::samples_for_ms(RESUME_HOLDOFF_MS, SAMPLE_RATE_HZ);
        self.session = eon_primitives::SessionId::new();
    }

    pub fn push(&mut self, frame: &[f32; VAD_FRAME_SAMPLES], partial: &str) -> EonResult<Option<Endpoint>> {
        if self.holdoff_samples != 0 {
            self.holdoff_samples = self.holdoff_samples.saturating_sub(frame.len());
            return Ok(None);
        }
        let p = self.vad.speech_probability(frame)?;
        let frame_ms = crate::geometry::ms_for_samples(frame.len(), SAMPLE_RATE_HZ);
        Ok(self
            .endpoint
            .push_frame(frame_ms, p >= 0.35, Completeness::from_partial(partial)))
    }
}

pub struct SileroVad {
    session: Session,
    state: Vec<f32>,
    context: [f32; CONTEXT],
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
            context: [0.0; CONTEXT],
        })
    }

    /// Resets the recurrent state, for example at the start of a new utterance stream.
    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
    }

    /// Returns the speech probability in [0, 1] for one 32 ms frame of 16 kHz mono samples.
    pub fn speech_probability(&mut self, frame: &[f32]) -> EonResult<f32> {
        if frame.len() != VAD_FRAME_SAMPLES {
            return Err(EonError::ValidationError(format!(
                "Silero v5 needs {VAD_FRAME_SAMPLES} samples per frame, got {}",
                frame.len()
            )));
        }
        // The v5 wrapper prepends the previous frame's 64-sample STFT context at 16 kHz.
        let input = Tensor::from_array((
            [1usize, CONTEXT + VAD_FRAME_SAMPLES],
            [self.context.as_slice(), frame].concat(),
        ))
        .map_err(|e| onnx_err("input", e))?;
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
            return Err(EonError::InternalError(format!(
                "unexpected Silero state size {}",
                next_state.len()
            )));
        }
        self.state.copy_from_slice(next_state);
        self.context.copy_from_slice(&frame[VAD_FRAME_SAMPLES - CONTEXT..]);
        probability
            .first()
            .copied()
            .ok_or_else(|| EonError::InternalError("empty Silero output".into()))
    }
}
