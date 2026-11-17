//! Credential handling: secrets never reach argv or logs (docs/implementation/01, section 3.3).

use std::collections::HashMap;

use eon_primitives::{EonError, EonResult};
use zeroize::{Zeroize, ZeroizeOnDrop};

const SECRET_SCHEME: &str = "secret://";

/// Secret bytes that are wiped on drop. Deliberately has no `Debug`, `Display`, `Clone`, or `Serialize`.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct EphemeralSecret(Vec<u8>);

impl EphemeralSecret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Only for binding into a pipe, fd, or header buffer. Never format or log the result.
    pub fn expose_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Default)]
pub struct CredentialBroker {
    secrets: HashMap<String, EphemeralSecret>,
}

impl CredentialBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: impl Into<String>, secret: EphemeralSecret) {
        self.secrets.insert(key.into(), secret);
    }

    pub fn get(&self, key: &str) -> Option<&EphemeralSecret> {
        self.secrets.get(key)
    }

    /// Rejects any argv entry that names a secret. Callers must bind secrets through stdin or an fd instead.
    pub fn check_argv(args: &[&str]) -> EonResult<()> {
        match args.iter().find(|arg| arg.contains(SECRET_SCHEME)) {
            None => Ok(()),
            Some(_) => Err(EonError::SecurityViolation(
                "secret:// references are not allowed in argv; bind via stdin or fd".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_with_secret_reference_is_refused() {
        let err =
            CredentialBroker::check_argv(&["curl", "-H", "secret://github/token"]).unwrap_err();
        assert!(matches!(err, EonError::SecurityViolation(_)));
    }

    #[test]
    fn plain_argv_is_allowed() {
        assert!(CredentialBroker::check_argv(&["git", "status", "--short"]).is_ok());
    }

    #[test]
    fn broker_returns_inserted_secret_bytes() {
        let mut broker = CredentialBroker::new();
        broker.insert("github", EphemeralSecret::new(b"token-bytes".to_vec()));
        assert_eq!(broker.get("github").unwrap().expose_bytes(), b"token-bytes");
        assert!(broker.get("missing").is_none());
    }
}
