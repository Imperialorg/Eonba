//! End-to-end VAD test: pinned Silero v5 ONNX model over a real 16 kHz speech recording.
//!
//! The model comes from `scripts/models.lock.json` (`silero-vad-v5`). Put it at the repo path below, or set
//! `EON_SILERO_MODEL`. The WAV fixture is the upstream `test_wavs/en.wav` (sha256 eb1eb008...).

use std::path::PathBuf;

mod common;

use common::read_pcm16_mono_16k;
use eon_voice::endpoint::{Completeness, Endpointer};
use eon_voice::geometry::VAD_FRAME_SAMPLES;
use eon_voice::vad::SileroVad;

fn model_path() -> PathBuf {
    if let Ok(path) = std::env::var("EON_SILERO_MODEL") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models/vad/silero_vad.onnx")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn load_model() -> SileroVad {
    let path = model_path();
    assert!(
        path.exists(),
        "missing {}; run: python3 scripts/download_models.py --ids silero-vad-v5",
        path.display()
    );
    SileroVad::load(path).expect("pinned Silero model must load")
}

#[test]
fn silence_scores_low() {
    let mut vad = load_model();
    let silence = vec![0.0f32; VAD_FRAME_SAMPLES];
    let p = vad.speech_probability(&silence).unwrap();
    assert!((0.0..=1.0).contains(&p));
    assert!(p < 0.1, "silence scored {p}");
}

#[test]
fn wrong_frame_size_is_refused_before_inference() {
    let mut vad = load_model();
    assert!(vad.speech_probability(&[0.0; 480]).is_err());
}

#[test]
fn reset_makes_inference_repeatable() {
    let mut vad = load_model();
    let frame: Vec<f32> = (0..VAD_FRAME_SAMPLES).map(|i| ((i as f32) * 0.05).sin() * 0.3).collect();
    let first = vad.speech_probability(&frame).unwrap();
    vad.reset();
    let again = vad.speech_probability(&frame).unwrap();
    assert_eq!(first, again);
}

#[test]
fn real_speech_is_detected_and_its_pauses_are_scored_as_silence() {
    let mut vad = load_model();
    let samples = read_pcm16_mono_16k(&fixture("en.wav"));
    let probabilities: Vec<f32> = samples
        .as_chunks::<VAD_FRAME_SAMPLES>()
        .0
        .iter()
        .map(|frame| vad.speech_probability(frame).unwrap())
        .collect();

    let speech = probabilities.iter().filter(|&&p| p > 0.5).count();
    let quiet = probabilities.iter().filter(|&&p| p < 0.2).count();
    let total = probabilities.len();
    assert!(speech as f32 / total as f32 > 0.3, "only {speech}/{total} frames detected as speech");
    assert!(quiet > 0, "no frames scored as silence; the VAD cannot tell pauses apart");

    let mut endpointer = Endpointer::new();
    let endpoints = probabilities
        .iter()
        .filter(|&&p| endpointer.push_frame(32, p > 0.5, Completeness::Complete).is_some())
        .count();
    assert!(endpoints >= 1, "the endpointer never closed a turn on real speech");
}
