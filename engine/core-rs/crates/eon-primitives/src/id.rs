use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generates a Crockford Base32 26-character pseudo-ULID string from timestamp + monotonic counter.
pub fn generate_ulid_string() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let count = COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut result = [b'0'; 26];

    // Encode 48-bit timestamp (first 10 chars)
    let mut ts = now;
    for i in (0..10).rev() {
        result[i] = CROCKFORD_ALPHABET[(ts & 0x1F) as usize];
        ts >>= 5;
    }

    // Encode 80-bit randomness/entropy (last 16 chars)
    let mut entropy = count;
    for i in (10..26).rev() {
        result[i] = CROCKFORD_ALPHABET[(entropy & 0x1F) as usize];
        entropy >>= 5;
    }

    // Safety: CROCKFORD_ALPHABET contains only valid ASCII uppercase characters and numbers.
    unsafe { String::from_utf8_unchecked(result.to_vec()) }
}

macro_rules! define_typed_id {
    ($type_name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $type_name(String);

        impl $type_name {
            pub const PREFIX: &'static str = $prefix;

            pub fn new() -> Self {
                Self(format!("{}_{}", Self::PREFIX, generate_ulid_string()))
            }

            pub fn from_raw(raw: String) -> Result<Self, crate::error::EonError> {
                if !raw.starts_with(concat!($prefix, "_")) {
                    return Err(crate::error::EonError::InvalidId {
                        expected_prefix: Self::PREFIX,
                        actual: raw,
                    });
                }
                Ok(Self(raw))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $type_name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $type_name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $type_name {
            type Err = crate::error::EonError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::from_raw(s.to_string())
            }
        }
    };
}

define_typed_id!(ObjectiveId, "obj");
define_typed_id!(TaskId, "task");
define_typed_id!(WorkflowId, "wf");
define_typed_id!(LeaseId, "lease");
define_typed_id!(IdentityId, "id");
define_typed_id!(ExecutionId, "exec");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_objective_id_prefix() {
        let id = ObjectiveId::new();
        assert!(id.as_str().starts_with("obj_"));
        assert_eq!(id.as_str().len(), 4 + 26); // "obj_" + 26 chars
    }

    #[test]
    fn test_task_id_prefix() {
        let id = TaskId::new();
        assert!(id.as_str().starts_with("task_"));
        assert_eq!(id.as_str().len(), 5 + 26); // "task_" + 26 chars
    }

    #[test]
    fn test_invalid_prefix_rejected() {
        let err = ObjectiveId::from_raw("task_0123456789ABCDEFGHJKMNPQRS".to_string()).unwrap_err();
        match err {
            crate::error::EonError::InvalidId { expected_prefix, .. } => {
                assert_eq!(expected_prefix, "obj");
            }
            _ => panic!("Expected InvalidId error"),
        }
    }
}

