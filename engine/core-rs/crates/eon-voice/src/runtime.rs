//! Process-wide ONNX Runtime environment shared by every session in this crate.

use std::sync::OnceLock;

use eon_primitives::{EonError, EonResult};
use ort::environment::Environment;

pub fn environment() -> EonResult<&'static Environment> {
    static ENVIRONMENT: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| {
            ort::init()
                .with_name("eon-voice")
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|reason| EonError::OperationFailed {
            operation: "onnx.environment".into(),
            reason: reason.clone(),
        })
}
