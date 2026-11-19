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

/// Fires once after `required_frames` consecutive frames whose RMS is at or above `threshold_rms`. Run it on the
/// echo-cancelled capture signal, so the assistant's own voice does not count as the user speaking.
#[derive(Debug, Clone)]
pub struct SpeechOnsetDetector {
    threshold_rms: f32,
    required_frames: usize,
    run: usize,
    fired: bool,
}

impl SpeechOnsetDetector {
    pub fn new(threshold_rms: f32, required_frames: usize) -> Self {
        Self {
            threshold_rms,
            required_frames: required_frames.max(1),
            run: 0,
            fired: false,
        }
    }

    /// Returns true only on the frame that completes a qualifying run. Later frames return false until `reset`.
    pub fn push_frame(&mut self, frame: &[f32]) -> bool {
        if self.fired || frame.is_empty() {
            return false;
        }
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        if rms >= self.threshold_rms {
            self.run += 1;
            if self.run >= self.required_frames {
                self.fired = true;
                return true;
            }
        } else {
            self.run = 0;
        }
        false
    }

    pub fn reset(&mut self) {
        self.run = 0;
        self.fired = false;
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

    #[test]
    fn onset_needs_a_sustained_run_above_the_threshold() {
        let loud = vec![0.5f32; 160];
        let quiet = vec![0.001f32; 160];
        let mut detector = SpeechOnsetDetector::new(0.1, 3);
        assert!(!detector.push_frame(&loud));
        assert!(!detector.push_frame(&loud));
        assert!(!detector.push_frame(&quiet), "a dip resets the run");
        assert!(!detector.push_frame(&loud));
        assert!(!detector.push_frame(&loud));
        assert!(detector.push_frame(&loud), "third consecutive loud frame fires");
        assert!(!detector.push_frame(&loud), "fires once per utterance");
        detector.reset();
        assert!(!detector.push_frame(&loud));
    }
}
