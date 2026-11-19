//! End-to-end TTS: the pinned Kokoro-82M int8 model and the af_heart voice, with espeak-ng for phonemes.
//!
//! The check is a round trip. The synthesized speech is resampled to 16 kHz and transcribed by the pinned Zipformer
//! ASR. Enough content words must come back, so the test fails on silence or on speech that is not intelligible.

use std::path::PathBuf;

use eon_voice::asr::ZipformerAsr;
use eon_voice::resample::resample_mono;
use eon_voice::tts::{KokoroTts, SAMPLE_RATE_HZ};
use eon_voice::{
    LeaseId,
    barge_in::BargeIn,
    commitment::{Bridge, BridgeCache, CommitmentGate},
};

fn repo_models() -> PathBuf {
    if let Some(path) = std::env::var_os("EON_VOICE_MODELS") {
        return path.into();
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models")
}

#[test]
fn synthesized_speech_is_intelligible_to_the_pinned_asr() {
    let models = repo_models();
    let tts_model = models.join("tts/kokoro-v1.0-int8.onnx");
    let voice = models.join("tts/voices/af_heart.bin");
    assert!(
        tts_model.exists() && voice.exists(),
        "missing Kokoro assets; run: python3 scripts/download_models.py --ids kokoro-v1.0-int8 kokoro-v1.0-voice-af_heart"
    );
    let mut tts = KokoroTts::load(&tts_model, &voice).expect("pinned Kokoro model must load");
    let speech = tts
        .synthesize("Hello world, this is a test of the voice.", 1.0)
        .expect("synthesis must succeed");

    let seconds = speech.len() as f32 / SAMPLE_RATE_HZ as f32;
    assert!(seconds > 1.5, "speech is only {seconds:.2} s");
    let peak = speech.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "speech is silent (peak {peak})");

    let at_16k = resample_mono(&speech, SAMPLE_RATE_HZ, 16_000).expect("resampling must succeed");
    let asr_model = models.join("asr");
    let mut asr = ZipformerAsr::load(&asr_model).expect("pinned Zipformer models must load");
    let transcript = asr
        .transcribe(&at_16k)
        .expect("transcription must succeed")
        .to_uppercase();

    let content_words = ["HELLO", "WORLD", "THIS", "TEST", "VOICE"];
    let found = content_words.iter().filter(|w| transcript.contains(*w)).count();
    eprintln!("ROUND_TRIP transcript={transcript:?} found={found}");
    assert!(found >= 3, "round trip recovered {found} of 5 words: {transcript:?}");
}

#[test]
fn streaming_delivers_the_first_clause_before_the_reply_is_finished() {
    let models = repo_models();
    let mut tts = KokoroTts::load(
        models.join("tts/kokoro-v1.0-int8.onnx"),
        models.join("tts/voices/af_heart.bin"),
    )
    .expect("pinned Kokoro model must load");
    let mut callbacks = 0usize;
    let mut total_samples = 0usize;
    let timing = tts
        .synthesize_streaming(
            "Affirmative. The report is ready, and the tests all passed. Shall I summarize it now?",
            1.0,
            |samples| {
                callbacks += 1;
                total_samples += samples.len();
            },
        )
        .expect("streaming synthesis must succeed");

    assert_eq!(timing.clauses, 4, "four clauses: commas and full stops both split");
    assert_eq!(callbacks, 4, "one callback per clause");
    let seconds = total_samples as f32 / SAMPLE_RATE_HZ as f32;
    assert!(seconds > 2.0, "streamed audio is only {seconds:.2} s");
    assert!(
        timing.first_audio < timing.total.mul_f32(0.7),
        "first audio at {:?} is not clearly before the full reply at {:?}",
        timing.first_audio,
        timing.total
    );
}

#[test]
fn cached_bridge_and_committed_clause_meet_synthesis_budgets() {
    let models = repo_models();
    let mut tts = KokoroTts::load(
        models.join("tts/kokoro-v1.0-int8.onnx"),
        models.join("tts/voices/af_heart.bin"),
    )
    .unwrap();
    let cache = BridgeCache::warm(&mut tts, 1.0).unwrap();
    let start = std::time::Instant::now();
    let mut gate = CommitmentGate::new(1, BargeIn::new());
    gate.queue("The test report is ready.");
    let bridge = gate.bridge(&cache, Bridge::Report).unwrap();
    let first = start.elapsed();
    assert!(bridge.iter().any(|s| s.abs() > 0.05));
    assert!(gate.synthesize_next(&mut tts, 1.0).unwrap().is_none());
    gate.lease_acquired(1, LeaseId::new());
    let synthesis = std::time::Instant::now();
    let clause = gate.synthesize_next(&mut tts, 1.0).unwrap().unwrap();
    let rtf = synthesis.elapsed().as_secs_f32() / (clause.len() as f32 / SAMPLE_RATE_HZ as f32);
    eprintln!("Kokoro cached-prefix available={first:?}, uncached clause RTF={rtf:.3}");
    // Software availability only, not endpoint-to-speaker TTFA.
    assert!(first < std::time::Duration::from_millis(170));
    assert!(rtf < 0.15);
}
