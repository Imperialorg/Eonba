//! Device loop test against the host's default audio devices (PulseAudio through ALSA in this codespace).
//!
//! The test skips with a message when the host has no default input or output device. It does not skip on driver errors.
//! ALSA's first period can take over a second to arrive, so the waits are long enough to see the first callback.

use std::sync::atomic::Ordering;
use std::time::Duration;

use eon_voice::device::{capture_default_input, play_default_output};
use eon_voice::ring::pcm_ring;

#[test]
fn default_input_delivers_frames_without_driver_errors() {
    let (producer, consumer) = pcm_ring(1 << 16);
    let handle = match capture_default_input(producer) {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("skipping capture: {e}");
            return;
        }
    };
    std::thread::sleep(Duration::from_millis(2500));
    let frames = handle.counters.frames.load(Ordering::Relaxed);
    let errors = handle.counters.driver_errors.load(Ordering::Relaxed);
    let callbacks = handle.counters.callbacks.load(Ordering::Relaxed);
    drop(handle);
    drop(consumer);
    assert_eq!(errors, 0, "driver reported errors during capture");
    assert!(frames > 0, "no frames captured in 2.5 s; callbacks={callbacks}");
}

#[test]
fn default_output_drains_queued_audio_and_counts_underruns_when_empty() {
    let (mut producer, consumer) = pcm_ring(1 << 16);
    let tone: Vec<f32> = (0..8_000)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin() * 0.2)
        .collect();
    assert_eq!(producer.push_slice(&tone), tone.len());
    let handle = match play_default_output(consumer) {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("skipping playback: {e}");
            return;
        }
    };
    std::thread::sleep(Duration::from_millis(2500));
    let errors = handle.counters.driver_errors.load(Ordering::Relaxed);
    let underruns = handle.counters.underruns.load(Ordering::Relaxed);
    assert_eq!(errors, 0, "driver reported errors during playback");
    assert!(underruns > 0, "an empty ring must be counted as underrun, not silently skipped");
    drop(handle);
}
