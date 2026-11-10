use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EonError {
    InvalidId {
        expected_prefix: &'static str,
        actual: String,
    },
    ValidationError(String),
    ResourceExhausted {
        resource: String,
        requested: u64,
        available: u64,
    },
    ResourceLeaseTimeout {
        lease_id: String,
    },
    SecurityViolation(String),
    CapabilityNotFound {
        capability_id: String,
    },
    OperationFailed {
        operation: String,
        reason: String,
    },
    InternalError(String),
}

impl fmt::Display for EonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId {
                expected_prefix,
                actual,
            } => {
                write!(
                    f,
                    "Invalid ID: expected prefix '{}_', found '{}'",
                    expected_prefix, actual
                )
            }
            Self::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            Self::ResourceExhausted {
                resource,
                requested,
                available,
            } => write!(
                f,
                "Resource '{}' exhausted: requested {}, available {}",
                resource, requested, available
            ),
            Self::ResourceLeaseTimeout { lease_id } => {
                write!(f, "Resource lease '{}' expired or timed out", lease_id)
            }
            Self::SecurityViolation(msg) => write!(f, "Security policy violation: {}", msg),
            Self::CapabilityNotFound { capability_id } => {
                write!(f, "Capability not registered: {}", capability_id)
            }
            Self::OperationFailed { operation, reason } => {
                write!(f, "Operation '{}' failed: {}", operation, reason)
            }
            Self::InternalError(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for EonError {}
