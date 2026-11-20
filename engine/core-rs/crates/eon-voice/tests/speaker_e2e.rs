//! End-to-end speaker check with the pinned WeSpeaker ResNet34 model.
//!
//! Real speech from the upstream `en.wav` fixture is split into two halves. Both halves come from the same speaker, so
//! they must score above the gate threshold. The assistant's own Kokoro voice is a different speaker, and it must score
//! below the threshold. A two-way split of one recording is weaker evidence than two recordings, and the test records
//! that. Use a fixed threshold, never one fitted to the candidates under test.

use std::path::PathBuf;

mod common;

use eon_voice::resample::resample_mono;
use eon_voice::speaker::{SpeakerEmbedder, SpeakerGate, cosine};
use eon_voice::tts::{KokoroTts, SAMPLE_RATE_HZ as TTS_RATE};
use eon_voice::{
    TaskId,
    barge_in::BargeIn,
    coordinator::{BargeInCoordinator, DuplexReflex},
    ring::pcm_ring,
    vad::SileroVad,
};

fn models() -> PathBuf {
    if let Some(path) = std::env::var_os("EON_VOICE_MODELS") {
        return path.into();
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models")
}

fn fixture() -> Vec<f32> {
    common::read_pcm16_mono_16k(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/en.wav"))
}

#[test]
fn same_speaker_scores_above_the_gate_and_the_assistant_voice_below_it() {
    let mut embedder = SpeakerEmbedder::load(models().join("speaker/wespeaker_resnet34.onnx"))
        .expect("pinned WeSpeaker model must load");
    let speech = fixture();
    let half = speech.len() / 2;
    let owner = embedder.embed(&speech[..half]).unwrap();
    let same_speaker = embedder.embed(&speech[half..]).unwrap();

    let mut tts = KokoroTts::load(
        models().join("tts/kokoro-v1.0-int8.onnx"),
        models().join("tts/voices/af_heart.bin"),
    )
    .expect("pinned Kokoro model must load");
    let assistant = tts
        .synthesize(
            "Affirmative. I am running the test now, and the report follows shortly.",
            1.0,
        )
        .unwrap();
    let assistant_16k = resample_mono(&assistant, TTS_RATE, 16_000).unwrap();
    let other_speaker = embedder.embed(&assistant_16k).unwrap();

    let same = cosine(&owner, &same_speaker);
    let other = cosine(&owner, &other_speaker);
    eprintln!("SPEAKER same={same:.3} assistant={other:.3}");
    assert!(
        same > other + 0.2,
        "same speaker {same:.3} must clearly exceed the assistant voice {other:.3}"
    );

    let gate = SpeakerGate::enroll(&owner, 0.35).unwrap();
    assert!(gate.accepts(&same_speaker));
    assert!(!gate.accepts(&other_speaker));

    // Real model-to-render integration, not a device latency measurement. The
    // non-owner here is an independent near-end voice, not just removable echo.
    let signal = BargeIn::new();
    let task = TaskId::new();
    let coordinator = BargeInCoordinator::new(task.clone(), signal.clone(), gate);
    let vad = SileroVad::load(models().join("vad/silero_vad.onnx")).unwrap();
    let mut reflex = DuplexReflex::new(coordinator, vad, embedder, Some(45)).unwrap();
    let (mut tx, mut rx) = pcm_ring(19_200 + assistant_16k.len() + speech.len());
    // Use real TTS as the played reference; cover the entire candidate window.
    while tx.push_slice(&assistant_16k) == assistant_16k.len() {}
    let mut out = [0.0; 160];
    let mut journal = Vec::new();
    let mut echo = eon_voice::delay::DelayLine::new(720);
    let silence = [0.0; 160];
    for frame in std::iter::repeat_n(&silence, 120).chain(assistant_16k.as_chunks::<160>().0) {
        assert_eq!(signal.render(&mut rx, &mut out).samples, 160);
        let mut mic = out;
        echo.process_block(&mut mic);
        for (s, near) in mic.iter_mut().zip(frame) {
            *s = 0.5 * *s + near;
        }
        assert!(!reflex.process(&out, &mut mic, 3, &mut journal).unwrap());
    }
    assert!(journal.is_empty());
    let mut interrupted = false;
    for frame in speech[half..].as_chunks::<160>().0 {
        assert_eq!(signal.render(&mut rx, &mut out).samples, 160);
        let mut mic = out;
        echo.process_block(&mut mic);
        for (s, near) in mic.iter_mut().zip(frame) {
            *s = 0.5 * *s + near;
        }
        if reflex.process(&out, &mut mic, 7, &mut journal).unwrap() {
            interrupted = true;
            break;
        }
    }
    assert!(interrupted, "sustained verified near-end speech must cancel TTS");
    let result = signal.render(&mut rx, &mut out);
    assert!(result.acknowledged && result.flushed > 0);
    assert_eq!(out, [0.0; 160]);
    let event: serde_json::Value = serde_json::from_slice(&journal).unwrap();
    assert_eq!(event["event"], "INTERRUPTED_BY_USER");
    assert_eq!(event["task_id"], task.as_str());
    assert_eq!(event["token_offset"], 7);
}
