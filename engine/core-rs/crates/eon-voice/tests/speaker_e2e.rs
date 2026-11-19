//! End-to-end speaker check with the pinned WeSpeaker ResNet34 model.
//!
//! Real speech from the upstream `en.wav` fixture is split into two halves. Both halves come from the same speaker, so
//! they must score above the gate threshold. The assistant's own Kokoro voice is a different speaker, and it must score
//! below the threshold. A two-way split of one recording is weaker evidence than two recordings, and the test records
//! that. The gate threshold is the midpoint between the measured same-speaker and different-speaker scores.

use std::path::PathBuf;

use eon_voice::resample::resample_mono;
use eon_voice::speaker::{cosine, SpeakerEmbedder, SpeakerGate};
use eon_voice::tts::{KokoroTts, SAMPLE_RATE_HZ as TTS_RATE};

fn models() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models")
}

fn fixture() -> Vec<f32> {
    let bytes = std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/en.wav")).unwrap();
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            return bytes[pos + 8..pos + 8 + size]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
                .collect();
        }
        pos += 8 + size + (size & 1);
    }
    panic!("no data chunk");
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
        .synthesize("Affirmative. I am running the test now, and the report follows shortly.", 1.0)
        .unwrap();
    let assistant_16k = resample_mono(&assistant, TTS_RATE, 16_000).unwrap();
    let other_speaker = embedder.embed(&assistant_16k).unwrap();

    let same = cosine(&owner, &same_speaker);
    let other = cosine(&owner, &other_speaker);
    eprintln!("SPEAKER same={same:.3} assistant={other:.3}");
    assert!(same > other + 0.2, "same speaker {same:.3} must clearly exceed the assistant voice {other:.3}");

    let gate = SpeakerGate::enroll(&owner, (same + other) / 2.0).unwrap();
    assert!(gate.accepts(&same_speaker));
    assert!(!gate.accepts(&other_speaker));
}
