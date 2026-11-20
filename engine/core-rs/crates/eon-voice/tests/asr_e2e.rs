//! End-to-end ASR test: pinned Zipformer streaming transducer over the upstream 16 kHz `en.wav` fixture.
//!
//! The model directory comes from `scripts/models.lock.json` (`zipformer-*`). Put the files at the repo path below,
//! or set `EON_ZIPFORMER_DIR`.

use std::path::PathBuf;

mod common;

use common::read_pcm16_mono_16k;
use eon_voice::asr::{MAX_UTTERANCE_SAMPLES, ZipformerAsr};

fn model_dir() -> PathBuf {
    if let Ok(path) = std::env::var("EON_ZIPFORMER_DIR") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models/asr")
}

fn load_model() -> ZipformerAsr {
    let dir = model_dir();
    assert!(
        dir.join("tokens.txt").exists(),
        "missing Zipformer models in {}; run: python3 scripts/download_models.py --ids zipformer-en-2023-06-26-encoder-int8 zipformer-en-2023-06-26-decoder zipformer-en-2023-06-26-joiner-int8 zipformer-en-2023-06-26-tokens",
        dir.display()
    );
    ZipformerAsr::load(dir).expect("pinned Zipformer models must load")
}

#[test]
fn upstream_en_wav_transcribes_exactly() {
    let mut asr = load_model();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/en.wav");
    let samples = read_pcm16_mono_16k(&fixture);
    let text = asr.transcribe(&samples).expect("transcription must succeed");
    assert_eq!(
        text,
        "THE DRIBAL CHIEFTAIN CALLED FOR THE BOY AND PRESENTED HIM THE FIFTY PIECES OF GOLD"
    );
}

#[test]
fn silence_yields_no_text() {
    let mut asr = load_model();
    let text = asr.transcribe(&vec![0.0; 16_000]).expect("transcription must succeed");
    assert!(text.is_empty(), "silence produced {text:?}");
}

#[test]
fn rejected_chunks_do_not_advance_the_utterance() {
    let mut asr = load_model();
    let mut stream = asr.start_utterance().unwrap();
    assert!(stream.push(&[0.0; 320]).unwrap().is_none());
    assert!(stream.push(&vec![0.0; MAX_UTTERANCE_SAMPLES - 319]).is_err());
    for bad in [f32::NAN, f32::INFINITY, -1.01, 1.01] {
        assert!(stream.push(&[0.0, bad]).is_err());
    }
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/en.wav");
    for samples in read_pcm16_mono_16k(&fixture).chunks(317) {
        stream.push(samples).unwrap();
    }
    assert_eq!(
        stream.finish().unwrap(),
        "THE DRIBAL CHIEFTAIN CALLED FOR THE BOY AND PRESENTED HIM THE FIFTY PIECES OF GOLD"
    );
}

#[test]
fn released_recording_emits_partials_before_end_of_input() {
    // Both the speech and its independent transcript come with the digest-pinned
    // Zipformer release. Do not silently substitute synthetic speech.
    let archive_fixture = std::env::var_os("EON_ZIPFORMER_TEST_WAV_0")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/0.wav"));
    let samples = read_pcm16_mono_16k(&archive_fixture);
    let mut asr = load_model();
    let mut stream = asr.start_utterance().unwrap();
    let mut first_partial = None;
    for (i, chunk) in samples.chunks(320).enumerate() {
        if let Some(partial) = stream.push(chunk).unwrap() {
            assert!(!partial.text.is_empty());
            first_partial.get_or_insert(i * 320 + chunk.len());
        }
    }
    assert!(first_partial.expect("expected a partial transcript") < samples.len() / 2);
    let transcript = stream.finish().unwrap();
    assert_eq!(
        transcript,
        "AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE SQUALID QUARTER OF THE BROTHELS"
    );
}
