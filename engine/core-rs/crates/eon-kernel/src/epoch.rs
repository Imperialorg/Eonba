//! Cognitive epochs with staged commit and fencing (docs/implementation/01, section 4).
//!
//! An epoch stages writes against the shared state. A commit applies them atomically and closes the epoch. A
//! rollback discards them. A newer epoch fences older ones, so a slow, retried, or replayed cognition step cannot
//! overwrite state that a later epoch already committed.

use std::collections::BTreeMap;

use eon_primitives::{EonError, EonResult};

#[derive(Debug, Default)]
pub struct SharedState {
    values: BTreeMap<String, String>,
    /// Highest epoch that has committed. Any epoch at or below this is fenced.
    fence: u64,
}

impl SharedState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn fence(&self) -> u64 {
        self.fence
    }

    /// Opens a staging area for epoch `id`. Opening an epoch at or below the fence is refused.
    pub fn begin(&self, id: u64) -> EonResult<CognitiveEpoch> {
        if id <= self.fence {
            return Err(EonError::ValidationError(format!(
                "epoch {id} is fenced by committed epoch {}",
                self.fence
            )));
        }
        Ok(CognitiveEpoch {
            id,
            staged: BTreeMap::new(),
            closed: false,
        })
    }

    /// Applies an epoch's staged writes atomically. Refused if a newer epoch has already committed.
    pub fn commit(&mut self, epoch: &mut CognitiveEpoch) -> EonResult<()> {
        if epoch.closed {
            return Err(EonError::ValidationError(format!("epoch {} is already closed", epoch.id)));
        }
        if epoch.id <= self.fence {
            epoch.closed = true;
            return Err(EonError::ValidationError(format!(
                "epoch {} was fenced by committed epoch {} before commit",
                epoch.id, self.fence
            )));
        }
        for (key, value) in std::mem::take(&mut epoch.staged) {
            self.values.insert(key, value);
        }
        self.fence = epoch.id;
        epoch.closed = true;
        Ok(())
    }
}

#[derive(Debug)]
pub struct CognitiveEpoch {
    id: u64,
    staged: BTreeMap<String, String>,
    closed: bool,
}

impl CognitiveEpoch {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Stages a write. Nothing is visible to other readers until commit.
    pub fn stage(&mut self, key: impl Into<String>, value: impl Into<String>) -> EonResult<()> {
        if self.closed {
            return Err(EonError::ValidationError(format!("epoch {} is closed", self.id)));
        }
        self.staged.insert(key.into(), value.into());
        Ok(())
    }

    /// Discards every staged write and closes the epoch.
    pub fn rollback(&mut self) {
        self.staged.clear();
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_writes_are_invisible_until_commit() {
        let mut state = SharedState::new();
        let mut epoch = state.begin(1).unwrap();
        epoch.stage("plan", "draft").unwrap();
        assert_eq!(state.get("plan"), None);
        state.commit(&mut epoch).unwrap();
        assert_eq!(state.get("plan"), Some("draft"));
    }

    #[test]
    fn rollback_discards_staged_writes() {
        let mut state = SharedState::new();
        let mut epoch = state.begin(1).unwrap();
        epoch.stage("plan", "bad").unwrap();
        epoch.rollback();
        assert_eq!(state.get("plan"), None);
        assert!(state.commit(&mut epoch).is_err(), "a rolled-back epoch cannot commit");
    }

    #[test]
    fn a_later_commit_fences_an_earlier_epoch() {
        let mut state = SharedState::new();
        let mut slow = state.begin(1).unwrap();
        let mut fast = state.begin(2).unwrap();
        slow.stage("answer", "stale").unwrap();
        fast.stage("answer", "fresh").unwrap();
        state.commit(&mut fast).unwrap();
        assert!(state.commit(&mut slow).is_err());
        assert_eq!(state.get("answer"), Some("fresh"), "the stale epoch must not overwrite");
    }

    #[test]
    fn a_replayed_epoch_id_cannot_be_reopened_after_commit() {
        let mut state = SharedState::new();
        let mut epoch = state.begin(5).unwrap();
        state.commit(&mut epoch).unwrap();
        assert!(state.begin(5).is_err());
        assert!(state.begin(4).is_err());
        assert!(state.begin(6).is_ok());
    }

    #[test]
    fn a_closed_epoch_rejects_further_writes_and_double_commit() {
        let mut state = SharedState::new();
        let mut epoch = state.begin(1).unwrap();
        state.commit(&mut epoch).unwrap();
        assert!(epoch.stage("x", "y").is_err());
        assert!(state.commit(&mut epoch).is_err());
    }
}
