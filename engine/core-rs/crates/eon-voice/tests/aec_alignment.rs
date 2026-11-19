//! Delay alignment of the AEC reference. The same echo is cancelled twice: once with the playback reference delayed by
//! the echo delay (aligned), and once with the reference undelayed while AEC3 is told the delay (misaligned). The echo
//! delay is 400 ms, a realistic playback-path latency. At 45 ms AEC3's adaptive filter absorbs the offset and alignment
//! barely matters, so that case is not used here.

use eon_voice::aec::AcousticEchoCanceller;
use eon_voice::delay::DelayLine;

const RATE: u32 = 16_000;
const DELAY_MS: usize = 400;

fn erle_db(aligned_reference: bool) -> f32 {
    let delay = DELAY_MS * RATE as usize / 1000;
    let total = (RATE as f32 * 3.0) as usize;
    let render: Vec<f32> = (0..total)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin() * 0.4)
        .collect();
    let mic: Vec<f32> = (0..total)
        .map(|i| if i >= delay { render[i - delay] * 0.5 } else { 0.0 })
        .collect();

    let aec = AcousticEchoCanceller::new(RATE, Some(DELAY_MS as u16)).unwrap();
    let frame = aec.frame_samples();
    let mut reference = DelayLine::new(if aligned_reference { 0 } else { delay });
    let mut capture = mic.clone();
    let mut frame_reference = vec![0.0f32; frame];
    for start in (0..total).step_by(frame) {
        let end = start + frame;
        if end > total {
            break;
        }
        frame_reference.copy_from_slice(&render[start..end]);
        reference.process_block(&mut frame_reference);
        aec.analyze_render(&frame_reference).unwrap();
        aec.process_capture(&mut capture[start..end]).unwrap();
    }
    let tail = total * 2 / 3..total;
    let before: f32 = mic[tail.clone()].iter().map(|s| s * s).sum();
    let after: f32 = capture[tail].iter().map(|s| s * s).sum();
    10.0 * (before / after.max(1e-12)).log10()
}

#[test]
fn aligning_the_playback_reference_is_what_makes_cancellation_work() {
    let aligned = erle_db(true);
    let misaligned = erle_db(false);
    assert!(aligned > 15.0, "aligned reference cancels only {aligned:.1} dB");
    assert!(
        aligned > misaligned + 10.0,
        "alignment must matter: aligned {aligned:.1} dB, misaligned {misaligned:.1} dB"
    );
}
