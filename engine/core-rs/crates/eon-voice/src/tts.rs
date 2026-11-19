//! Kokoro-82M text-to-speech (docs/implementation/04, TTS section).
//!
//! The pipeline is text to phonemes (espeak-ng, en-us, IPA with stress marks), phonemes to token ids through the
//! model's vocabulary, then one ONNX call that returns a 24 kHz waveform. The voice file holds one style vector for each
//! token count, so the style row is picked by the number of tokens, as in the reference implementation.
//!
//! espeak-ng reads the text from stdin and is never given it as an argument, so text cannot inject options.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use eon_primitives::{EonError, EonResult};
use ort::session::Session;
use ort::value::Tensor;

pub const SAMPLE_RATE_HZ: u32 = 24_000;
/// Longest phoneme sequence the model accepts. The reference truncates to this before tokenizing.
const MAX_TOKENS: usize = 509;
const STYLE_DIM: usize = 256;
const VOICE_ROWS: usize = 510;
const SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2.0;
const VOCAB_JSON: &str = include_str!("../assets/kokoro_vocab.json");

fn tts_err(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("tts.{operation}"),
        reason: reason.to_string(),
    }
}

fn load_vocab() -> EonResult<HashMap<char, i64>> {
    let root: serde_json::Value = serde_json::from_str(VOCAB_JSON).map_err(|e| tts_err("vocab", e))?;
    let entries = root
        .get("vocab")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| tts_err("vocab", "missing vocab object"))?;
    entries
        .iter()
        .map(|(symbol, id)| {
            let mut chars = symbol.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return Err(tts_err("vocab", format!("entry {symbol:?} is not one character")));
            };
            let id = id.as_i64().ok_or_else(|| tts_err("vocab", format!("id for {symbol:?} is not an integer")))?;
            Ok((c, id))
        })
        .collect()
}

/// Converts text to en-us IPA phonemes with stress marks. Newlines become spaces and runs of spaces collapse.
pub fn phonemize(text: &str) -> EonResult<String> {
    let mut child = Command::new("espeak-ng")
        .args(["-v", "en-us", "-q", "--ipa=3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tts_err("phonemize.spawn", format!("espeak-ng is required on PATH: {e}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| tts_err("phonemize.stdin", "no stdin pipe"))?
        .write_all(text.as_bytes())
        .map_err(|e| tts_err("phonemize.write", e))?;
    let output = child.wait_with_output().map_err(|e| tts_err("phonemize.wait", e))?;
    if !output.status.success() {
        return Err(tts_err(
            "phonemize",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    Ok(raw.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub struct KokoroTts {
    session: Session,
    vocab: HashMap<char, i64>,
    voice: Vec<f32>,
}

impl KokoroTts {
    /// Loads the model and a voice file. Both must be the digest-verified artifacts from `scripts/models.lock.json`.
    pub fn load(model: impl AsRef<Path>, voice: impl AsRef<Path>) -> EonResult<Self> {
        let session = Session::builder(crate::runtime::environment()?)
            .map_err(|e| tts_err("builder", e))?
            .with_intra_threads(2)
            .map_err(|e| tts_err("threads", e))?
            .commit_from_file(model)
            .map_err(|e| tts_err("load", e))?;
        let bytes = std::fs::read(voice).map_err(|e| tts_err("voice", e))?;
        let expected = VOICE_ROWS * STYLE_DIM * 4;
        if bytes.len() != expected {
            return Err(tts_err("voice", format!("expected {expected} bytes, found {}", bytes.len())));
        }
        let voice = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        Ok(Self {
            session,
            vocab: load_vocab()?,
            voice,
        })
    }

    /// Synthesizes `text` at `speed` (0.5 to 2.0) and returns mono samples at `SAMPLE_RATE_HZ`.
    /// Synthesizes clause by clause and hands each clause's samples to `on_clause` as soon as they exist. This lets
    /// playback start before the whole reply is synthesized.
    pub fn synthesize_streaming(
        &mut self,
        text: &str,
        speed: f32,
        mut on_clause: impl FnMut(&[f32]),
    ) -> EonResult<StreamTiming> {
        let started = std::time::Instant::now();
        let clauses = split_clauses(text);
        if clauses.is_empty() {
            return Err(EonError::ValidationError("text has no speakable clauses".into()));
        }
        let mut first_audio = None;
        for clause in &clauses {
            let samples = self.synthesize(clause, speed)?;
            first_audio.get_or_insert_with(|| started.elapsed());
            on_clause(&samples);
        }
        Ok(StreamTiming {
            clauses: clauses.len(),
            first_audio: first_audio.unwrap_or_default(),
            total: started.elapsed(),
        })
    }

    pub fn synthesize(&mut self, text: &str, speed: f32) -> EonResult<Vec<f32>> {
        if !SPEED_RANGE.contains(&speed) {
            return Err(EonError::ValidationError(format!("speed {speed} is outside 0.5 to 2.0")));
        }
        let phonemes = phonemize(text)?;
        let tokens: Vec<i64> = phonemes
            .chars()
            .filter_map(|c| self.vocab.get(&c).copied())
            .take(MAX_TOKENS)
            .collect();
        if tokens.is_empty() {
            return Err(EonError::ValidationError("text has no speakable phonemes".into()));
        }
        let row = tokens.len();
        let style = self.voice[row * STYLE_DIM..(row + 1) * STYLE_DIM].to_vec();
        let mut ids = Vec::with_capacity(tokens.len() + 2);
        ids.push(0);
        ids.extend(tokens);
        ids.push(0);

        let input_ids = Tensor::from_array(([1usize, ids.len()], ids)).map_err(|e| tts_err("input", e))?;
        let style = Tensor::from_array(([1usize, STYLE_DIM], style)).map_err(|e| tts_err("input", e))?;
        let speed = Tensor::from_array(([1usize], vec![speed])).map_err(|e| tts_err("input", e))?;
        let outputs = self
            .session
            .run(ort::inputs!["input_ids" => input_ids, "style" => style, "speed" => speed])
            .map_err(|e| tts_err("run", e))?;
        let (_, waveform) = outputs["waveform"]
            .try_extract_tensor::<f32>()
            .map_err(|e| tts_err("waveform", e))?;
        Ok(waveform.to_vec())
    }
}

/// Splits text into clauses after sentence and clause punctuation. Each clause keeps its punctuation. Empty pieces are
/// dropped, so every returned clause has at least one speakable character.
pub fn split_clauses(text: &str) -> Vec<String> {
    let mut clauses = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        current.push(c);
        if matches!(c, '.' | '!' | '?' | ';' | ':' | ',') {
            let trimmed = current.trim();
            if trimmed.chars().any(char::is_alphanumeric) {
                clauses.push(trimmed.to_owned());
            }
            current.clear();
        }
    }
    let rest = current.trim();
    if rest.chars().any(char::is_alphanumeric) {
        clauses.push(rest.to_owned());
    }
    clauses
}

/// Timing of one streamed synthesis. `first_audio` is the time until the first clause's samples were delivered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamTiming {
    pub clauses: usize,
    pub first_audio: std::time::Duration,
    pub total: std::time::Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clauses_keep_their_punctuation_and_drop_empty_pieces() {
        assert_eq!(
            split_clauses("Affirmative. I am running the test now, and the report follows shortly."),
            vec!["Affirmative.", "I am running the test now,", "and the report follows shortly."]
        );
        assert_eq!(split_clauses("  ... ,  "), Vec::<String>::new());
        assert_eq!(split_clauses("no punctuation here"), vec!["no punctuation here"]);
    }

    #[test]
    fn vocab_maps_single_characters_to_the_reference_ids() {
        let vocab = load_vocab().unwrap();
        assert_eq!(vocab[&'a'], 43);
        assert_eq!(vocab[&' '], 16);
        assert_eq!(vocab[&'ˈ'], 156);
        assert_eq!(vocab.len(), 114);
    }
}
