//! End-to-end ASR test: pinned Zipformer streaming transducer over the upstream 16 kHz `en.wav` fixture.
//!
//! The model directory comes from `scripts/models.lock.json` (`zipformer-*`). Put the files at the repo path below,
//! or set `EON_ZIPFORMER_DIR`.

use std::path::PathBuf;

mod common;

use common::read_pcm16_mono_16k;
use eon_voice::asr::ZipformerAsr;

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
