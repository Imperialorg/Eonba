//! Recorded handoff controls for verification context. No live devices or acoustic timing.

use std::path::PathBuf;

use super::{common, fixture, load_speaker, models};
use eon_voice::TaskId;
use eon_voice::barge_in::BargeIn;
use eon_voice::coordinator::{BargeInCoordinator, CANDIDATE_SAMPLES, DuplexReflex};
use eon_voice::ring::pcm_ring;
use eon_voice::speaker::{SpeakerEmbedding, SpeakerGate};
use eon_voice::vad::SileroVad;

const BLOCK: usize = 160;

fn rms(pcm: &[f32]) -> f32 {
    (pcm.iter().map(|x| x * x).sum::<f32>() / pcm.len() as f32).sqrt()
}

// Splice into active speech, not a leading silence that would clear the VAD
// history. The rule is the same for both recordings and uses no identity scores.
fn speech_onset(pcm: &[f32]) -> &[f32] {
    let floor = rms(pcm);
    let block = pcm.as_chunks::<BLOCK>().0.iter().position(|x| rms(x) >= floor).unwrap();
    &pcm[block * BLOCK..]
}

// Owner -> stranger -> held-out owner, with uninterrupted render and a single
// VAD, echo path, speaker gate and audit slot across both splices.
fn handoff(owner: &SpeakerEmbedding, prelude: &[f32], stranger: &[f32], returning: &[f32]) -> Option<(u64, u64, usize)> {
    let signal = BargeIn::new();
    let gate = SpeakerGate::enroll(owner, 0.35).unwrap();
    let coordinator = BargeInCoordinator::new(TaskId::new(), signal.clone(), gate);
    let audit = coordinator.audit();
    let speaker = load_speaker();
    let vad = SileroVad::load(models().join("vad/silero_vad.onnx")).unwrap();
    let mut reflex = DuplexReflex::new(coordinator, vad, speaker, Some(0)).unwrap();
    let (mut tx, mut rx) = pcm_ring(BLOCK);
    let tone: [f32; BLOCK] = std::array::from_fn(|i| 0.1 * (std::f32::consts::TAU * i as f32 / 32.0).sin());
    let mut out = [0.0; BLOCK];
    let mut journal = Vec::new();
    let mut rejected = [0; 2];
    for (phase, pcm) in [prelude, stranger, returning].into_iter().enumerate() {
        let before = reflex.rejected_candidates();
        for (i, near) in pcm.as_chunks::<BLOCK>().0.iter().enumerate() {
            assert_eq!(tx.push_slice(&tone), BLOCK);
            assert_eq!(signal.render(&mut rx, &mut out).samples, BLOCK);
            let mut mic = std::array::from_fn(|j| near[j] + 0.25 * out[j]);
            if reflex.process(&out, &mut mic, phase, &mut journal).unwrap() {
                match phase {
                    0 => return None, // Legitimate owner decision before the cut: not a negative trial.
                    1 => panic!(
                        "playback interrupted {} ms into the stranger after {} ms of owner",
                        (i + 1) * 10,
                        prelude.len() / 16
                    ),
                    _ => {
                        assert_eq!(audit.record().unwrap().token_offset, phase);
                        assert_eq!(tx.push_slice(&tone), BLOCK);
                        assert!(signal.render(&mut rx, &mut out).acknowledged);
                        assert_eq!(out, [0.0; BLOCK]);
                        return Some((rejected[0], rejected[1], (i + 1) * 10));
                    }
                }
            }
        }
        assert!(audit.record().is_none() && journal.is_empty());
        if phase < 2 {
            rejected[phase] = reflex.rejected_candidates() - before;
            if phase == 1 {
                assert!(rejected[1] > 0, "stranger never reached the speaker gate");
            }
        }
    }
    panic!("returning clean owner never interrupted");
}

#[test]
fn active_speaker_switches_do_not_credit_a_departed_owner() {
    let owner = fixture();
    let stranger = common::read_pcm16_mono_16k(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/0.wav"));
    let half = owner.len() / 2;
    let mut embedder = load_speaker();
    let enrollment = embedder.embed(&owner[..half]).unwrap();
    let returning = speech_onset(&owner[half..]);
    let stranger = speech_onset(&stranger);
    // Equal-energy broadband noise on both sides of the switch, normalized by
    // the complete clip, not the score of an individual verification window.
    let mut seed = 1u32;
    let mut bed = |len| -> Vec<f32> {
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                seed as f32 / u32::MAX as f32 - 0.5
            })
            .collect()
    };
    let mix = |speech: &[f32], noise: &[f32]| -> Vec<f32> {
        let gain = rms(speech) / rms(noise);
        speech.iter().zip(noise).map(|(s, n)| 0.5 * (s + n * gain)).collect()
    };
    let owner_noisy = mix(returning, &bed(returning.len()));
    let stranger_noisy = mix(stranger, &bed(stranger.len()));
    let mut unresolved_history = false;
    for (label, prelude, other) in [("clean", returning, stranger), ("noise", &owner_noisy, &stranger_noisy)] {
        let mut exercised = 0;
        // Half-window offsets: never align the identities to verification boundaries.
        for halves in [1, 3, 5, 7] {
            let cut = (CANDIDATE_SAMPLES * halves / 2 / BLOCK) * BLOCK;
            if let Some((pre, negative, owner_ms)) = handoff(&enrollment, &prelude[..cut], other, returning) {
                eprintln!(
                    "switch {label} after {} ms: {pre} owner embeddings before cut, \
                     {negative} rejections during stranger, returning owner at {owner_ms} ms",
                    cut / 16
                );
                unresolved_history |= pre > 0;
                exercised += 1;
            }
        }
        assert!(exercised > 0, "all {label} trials already latched before the cut");
    }
    assert!(
        unresolved_history,
        "no switch exercised a previously rejected owner candidate"
    );
}
