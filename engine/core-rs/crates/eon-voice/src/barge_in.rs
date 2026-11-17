//! Barge-in signal shared between the speech detector and the playback callback (docs/implementation/04, section 3).
//!
//! When the user starts speaking over the assistant, the detector calls `trigger`. The next playback callback sees the
//! flag, flushes the TTS ring in one index update, and writes silence. The detector never touches the ring itself,
//! so it never blocks the audio thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct BargeIn {
    flag: Arc<AtomicBool>,
}

impl BargeIn {
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests a flush. Idempotent until the playback side takes the request.
    pub fn trigger(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Takes a pending request, clearing it. Returns true at most once per trigger.
    pub fn take(&self) -> bool {
        self.flag.swap(false, Ordering::AcqRel)
    }

    pub fn is_pending(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trigger_is_taken_exactly_once() {
        let barge = BargeIn::new();
        assert!(!barge.take());
        barge.trigger();
        barge.trigger();
        assert!(barge.is_pending());
        assert!(barge.take());
        assert!(!barge.take());
    }

    #[test]
    fn clones_share_the_same_request() {
        let detector = BargeIn::new();
        let playback = detector.clone();
        detector.trigger();
        assert!(playback.take());
    }
}
