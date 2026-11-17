//! UniFFI bridge (docs/implementation/05, section 4). Exposes the cross-platform decision surface.
//!
//! Only pure decisions cross the boundary: model profile selection and endpointing thresholds. Audio buffers,
//! journal handles, and capability dispatch stay inside Rust, so the foreign side cannot bypass a gate.

use eon_cognition::profile::ModelProfile;
use eon_voice::endpoint::{Completeness as VoiceCompleteness, silence_threshold_ms};

uniffi::setup_scaffolding!();

#[derive(Debug, uniffi::Error)]
pub enum BridgeError {
    /// Named `reason` rather than `message`: Kotlin's `Exception.message` would clash with a generated property.
    InvalidInput { reason: String },
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput { reason } => write!(f, "invalid input: {reason}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Completeness {
    Complete,
    Ambiguous,
    Incomplete,
}

impl From<Completeness> for VoiceCompleteness {
    fn from(value: Completeness) -> Self {
        match value {
            Completeness::Complete => Self::Complete,
            Completeness::Ambiguous => Self::Ambiguous,
            Completeness::Incomplete => Self::Incomplete,
        }
    }
}

/// Returns the `scripts/models.lock.json` profile key for the device memory in GB.
#[uniffi::export]
pub fn select_model_profile(memory_gb: f64) -> Result<String, BridgeError> {
    ModelProfile::select_for_memory_gb(memory_gb)
        .map(|profile| profile.lock_key().to_string())
        .map_err(|e| BridgeError::InvalidInput { reason: e.to_string() })
}

/// End-of-turn silence threshold in milliseconds for a transcript completeness tier.
#[uniffi::export]
pub fn endpoint_silence_ms(completeness: Completeness) -> u32 {
    silence_threshold_ms(completeness.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_selection_returns_lock_keys() {
        assert_eq!(select_model_profile(4.0).unwrap(), "p0_mobile_ultralight");
        assert_eq!(select_model_profile(16.0).unwrap(), "p3_workstation_heavy");
    }

    #[test]
    fn invalid_memory_is_a_typed_error_not_a_panic() {
        assert!(matches!(select_model_profile(f64::NAN), Err(BridgeError::InvalidInput { .. })));
    }

    #[test]
    fn endpoint_thresholds_match_the_voice_crate() {
        assert_eq!(endpoint_silence_ms(Completeness::Complete), 300);
        assert_eq!(endpoint_silence_ms(Completeness::Incomplete), 1150);
    }
}
