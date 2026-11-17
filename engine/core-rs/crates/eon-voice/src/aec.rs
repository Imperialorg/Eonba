//! Acoustic echo cancellation with WebRTC AEC3 (docs/implementation/04, echo path).
//!
//! The playback signal (render) is analyzed first, then the microphone signal (capture) is cleaned in place. Frames
//! are 10 ms, so 160 samples at 16 kHz. The stream delay is the measured time between the render reaching the
//! speaker and its echo reaching the microphone. Fixing it avoids depending on AEC3's delay estimator, which needs
//! time to converge.

use eon_primitives::{EonError, EonResult};
use webrtc_audio_processing::{Config, Processor};
use webrtc_audio_processing_config::{EchoCanceller, HighPassFilter};

fn aec_err(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("aec.{operation}"),
        reason: reason.to_string(),
    }
}

pub struct AcousticEchoCanceller {
    processor: Processor,
    frame_samples: usize,
}

impl AcousticEchoCanceller {
    /// Creates a mono AEC3 instance. `stream_delay_ms` is the render-to-capture delay. `None` lets AEC3 estimate it.
    pub fn new(sample_rate_hz: u32, stream_delay_ms: Option<u16>) -> EonResult<Self> {
        let processor = Processor::new(sample_rate_hz).map_err(|e| aec_err("create", e))?;
        processor.set_config(Config {
            echo_canceller: Some(EchoCanceller::Full { stream_delay_ms }),
            high_pass_filter: Some(HighPassFilter::default()),
            ..Default::default()
        });
        let frame_samples = processor.num_samples_per_frame();
        Ok(Self {
            processor,
            frame_samples,
        })
    }

    /// Samples in one frame (10 ms). Every call below takes exactly this many samples.
    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// Feeds the playback frame that is about to reach the speaker. The signal is not modified.
    pub fn analyze_render(&self, render: &[f32]) -> EonResult<()> {
        self.check_frame(render)?;
        self.processor
            .analyze_render_frame([render])
            .map_err(|e| aec_err("render", e))
    }

    /// Removes echo from one microphone frame in place.
    pub fn process_capture(&self, capture: &mut [f32]) -> EonResult<()> {
        self.check_frame(capture)?;
        self.processor
            .process_capture_frame([capture])
            .map_err(|e| aec_err("capture", e))
    }

    fn check_frame(&self, frame: &[f32]) -> EonResult<()> {
        if frame.len() != self.frame_samples {
            return Err(EonError::ValidationError(format!(
                "AEC frames are {} samples, got {}",
                self.frame_samples,
                frame.len()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    fn energy(samples: &[f32]) -> f32 {
        samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32
    }

    fn render_signal(seconds: f32) -> Vec<f32> {
        let n = (RATE as f32 * seconds) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin() * 0.4)
            .collect()
    }

    /// Runs the render and capture paths frame by frame and returns the cleaned capture signal.
    fn run(aec: &AcousticEchoCanceller, render: &[f32], mut capture: Vec<f32>) -> Vec<f32> {
        let frame = aec.frame_samples();
        for start in (0..render.len()).step_by(frame) {
            aec.analyze_render(&render[start..start + frame]).unwrap();
            aec.process_capture(&mut capture[start..start + frame]).unwrap();
        }
        capture
    }

    #[test]
    fn echo_is_cancelled_after_convergence() {
        let delay_samples = 45 * RATE as usize / 1000;
        let render = render_signal(3.0);
        let mut echo = vec![0.0f32; render.len()];
        for i in delay_samples..render.len() {
            echo[i] = render[i - delay_samples] * 0.5;
        }
        let aec = AcousticEchoCanceller::new(RATE, Some(45)).unwrap();
        let cleaned = run(&aec, &render, echo.clone());

        let tail = render.len() * 2 / 3..render.len();
        let before = energy(&echo[tail.clone()]);
        let after = energy(&cleaned[tail]);
        let erle_db = 10.0 * (before / after.max(1e-12)).log10();
        assert!(erle_db > 15.0, "echo reduced by only {erle_db:.1} dB");
    }

    #[test]
    fn near_end_speech_without_echo_passes_through() {
        let speech: Vec<f32> = render_signal(2.0).iter().map(|s| s * 0.8).collect();
        let silence_render = vec![0.0f32; speech.len()];
        let aec = AcousticEchoCanceller::new(RATE, Some(45)).unwrap();
        let cleaned = run(&aec, &silence_render, speech.clone());

        let tail = speech.len() / 2..speech.len();
        let ratio = energy(&cleaned[tail.clone()]) / energy(&speech[tail]);
        assert!(ratio > 0.7, "near-end speech attenuated to {ratio:.2} of its energy");
    }

    #[test]
    fn wrong_frame_length_is_refused_before_processing() {
        let aec = AcousticEchoCanceller::new(RATE, None).unwrap();
        assert!(aec.analyze_render(&[0.0; 80]).is_err());
        assert!(aec.process_capture(&mut [0.0; 320]).is_err());
    }
}
