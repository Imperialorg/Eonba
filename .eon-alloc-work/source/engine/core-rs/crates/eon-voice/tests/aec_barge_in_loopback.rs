//! Loopback: assistant speech plays while its echo reaches the microphone, and the owner starts talking at 1.2 s.
//!
//! The render is a 440 Hz stand-in for TTS. The microphone hears a 45 ms, half-amplitude echo of it, plus the owner's
//! broadband speech-like signal from 1.2 s on. The barge-in detector runs on the NLMS residual, which removes only the
//! linear echo. Two things are checked: no trigger while only the assistant speaks, and a trigger shortly after the
//! owner starts. The AEC3 path is not used here, because its suppressor removes near-end speech while the render is
//! active (see `linear_echo` docs).

use eon_voice::barge_in::{BargeIn, SpeechOnsetDetector};
use eon_voice::linear_echo::NlmsCanceller;

const RATE: u32 = 16_000;
const OWNER_START_S: f32 = 1.2;
const TOTAL_S: f32 = 3.0;
const FRAME_MS: usize = 10;
const DELAY_MS: usize = 45;
const TAPS: usize = 1024;
const MU: f32 = 0.2;

fn owner_voice(sample: usize, start: usize, lcg: &mut u64) -> f32 {
    *lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    if sample < start {
        return 0.0;
    }
    let t = (sample - start) as f32 / RATE as f32;
    let envelope = (2.0 * std::f32::consts::PI * 3.0 * t).sin().abs();
    ((*lcg >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.2 * envelope
}

#[test]
fn owner_speech_over_echo_triggers_barge_in_and_the_assistant_alone_does_not() {
    let total = (RATE as f32 * TOTAL_S) as usize;
    let owner_start = (RATE as f32 * OWNER_START_S) as usize;
    let delay = DELAY_MS * RATE as usize / 1000;
    let frame = RATE as usize * FRAME_MS / 1000;

    let render: Vec<f32> = (0..total)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin() * 0.4)
        .collect();
    let mut lcg = 99u64;
    let owner: Vec<f32> = (0..total).map(|i| owner_voice(i, owner_start, &mut lcg)).collect();
    let mic: Vec<f32> = (0..total)
        .map(|i| if i >= delay { render[i - delay] * 0.5 } else { 0.0 } + owner[i])
        .collect();

    let mut canceller = NlmsCanceller::new(TAPS, MU);
    let mut detector = SpeechOnsetDetector::new(0.05, 3);
    let barge = BargeIn::new();
    let mut first_trigger: Option<usize> = None;
    let mut residual_echo_power = 0.0f32;
    let mut residual_count = 0usize;
    let mut residual_frame = vec![0.0f32; frame];

    for start in (0..total).step_by(frame) {
        let end = (start + frame).min(total);
        if end - start < frame {
            break;
        }
        for i in start..end {
            canceller.push_render(render[i]);
            residual_frame[i - start] = canceller.process(mic[i]);
        }
        if start >= owner_start / 2 && end <= owner_start {
            residual_echo_power += residual_frame.iter().map(|s| s * s).sum::<f32>();
            residual_count += frame;
        }
        if detector.push_frame(&residual_frame) && first_trigger.is_none() {
            first_trigger = Some(start);
            barge.trigger();
        }
    }

    let echo_power = residual_echo_power / residual_count.max(1) as f32;
    assert!(echo_power < 1e-4, "residual echo power {echo_power:e} before the owner spoke");
    assert!(
        first_trigger.is_none_or(|at| at >= owner_start),
        "false barge-in at {:.3} s while only the assistant was speaking",
        first_trigger.unwrap_or(0) as f32 / RATE as f32
    );
    let trigger_at = first_trigger.expect("owner speech must trigger barge-in");
    let latency_ms = (trigger_at - owner_start) as f32 * 1000.0 / RATE as f32;
    assert!(latency_ms < 200.0, "barge-in took {latency_ms:.0} ms after the owner started");
    assert!(barge.is_pending(), "the trigger must reach the playback side");
}
