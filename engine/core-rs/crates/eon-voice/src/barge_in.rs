//! Barge-in signal shared between the speech detector and the playback callback (docs/implementation/04, section 3).
//!
//! When the user starts speaking over the assistant, the detector calls `trigger`. The next playback callback sees the
//! request, flushes the TTS ring and writes silence. The detector never touches the ring itself,
//! so it never blocks the audio thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::ring::PcmConsumer;

const IDLE: u8 = 0;
const PLAYING: u8 = 1;
const REQUESTED: u8 = 2;
const MUTED: u8 = 3;

/// One reply's playback signal. A cancelled reply cannot resume (including late producer writes).
/// Start a fresh ring and signal for the next reply.
#[derive(Debug, Clone, Default)]
pub struct BargeIn {
    state: Arc<AtomicU8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub samples: usize,
    pub flushed: usize,
    pub acknowledged: bool,
    pub muted: bool,
}

impl BargeIn {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels unconditionally. Only the first request needs acknowledgment.
    pub fn trigger(&self) {
        self.state.fetch_max(REQUESTED, Ordering::AcqRel);
    }

    #[cfg(feature = "native")]
    pub(crate) fn interrupt_playing(&self) -> bool {
        self.state
            .compare_exchange(PLAYING, REQUESTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Acknowledges the request without unmuting the reply.
    pub fn take(&self) -> bool {
        self.state
            .compare_exchange(REQUESTED, MUTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) == REQUESTED
    }

    pub fn is_muted(&self) -> bool {
        self.state.load(Ordering::Acquire) >= REQUESTED
    }

    pub fn is_playing(&self) -> bool {
        self.state.load(Ordering::Acquire) == PLAYING
    }

    #[cfg(feature = "native")]
    pub(crate) fn playback_stopped(&self) {
        let _ = self
            .state
            .compare_exchange(PLAYING, IDLE, Ordering::AcqRel, Ordering::Acquire);
    }

    /// The mono portion of the real render callback, also usable by non-CPAL drivers.
    pub fn render(&self, consumer: &mut PcmConsumer, out: &mut [f32]) -> Rendered {
        let samples = if self.is_muted() { 0 } else { consumer.pop_slice(out) };
        let (from, to) = if samples > 0 { (IDLE, PLAYING) } else { (PLAYING, IDLE) };
        // Never overwrite a simultaneous detection. Check again after copying samples.
        let _ = self
            .state
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire);
        if self.is_muted() {
            out.fill(0.0);
            return Rendered {
                samples: 0,
                flushed: samples + consumer.flush(),
                acknowledged: self.take(),
                muted: true,
            };
        }
        out[samples..].fill(0.0);
        Rendered {
            samples,
            ..Rendered::default()
        }
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
