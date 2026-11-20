//! Zipformer streaming transducer ASR over ONNX Runtime (docs/implementation/04, speech recognition section).
//!
//! The encoder consumes 45-frame chunks of 80-bin Kaldi fbank and carries its cache tensors from one chunk to the
//! next. Greedy transducer search runs the joiner on every encoder frame and advances a stateful decoder over the
//! last two emitted tokens. Token 0 is blank.

use std::collections::HashMap;
use std::path::Path;

use eon_primitives::{EonError, EonResult};

use crate::endpoint::ReflexHint;
use crate::features::{NUM_MEL_BINS, SAMPLE_RATE_HZ, online_fbank};
use kaldi_native_fbank::OnlineFeature;
use ort::session::{Session, SessionInputValue};
use ort::value::{Tensor, TensorElementType, ValueType};

const CHUNK_FRAMES: usize = 45;
const CHUNK_SHIFT: usize = 32;
const CONTEXT_SIZE: usize = 2;
const BLANK_ID: i64 = 0;
const WORD_BOUNDARY: char = '\u{2581}';

const ENCODER_FILE: &str = "encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx";
const DECODER_FILE: &str = "decoder-epoch-99-avg-1-chunk-16-left-128.onnx";
const JOINER_FILE: &str = "joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx";
const TOKENS_FILE: &str = "tokens.txt";
/// The feature extractor retains an utterance's history until endpointing.
pub const MAX_UTTERANCE_SAMPLES: usize = 120 * 16_000;

enum CacheData {
    F32(Vec<f32>),
    I64(Vec<i64>),
}

struct CacheTensor {
    name: String,
    shape: Vec<i64>,
    data: CacheData,
}

pub struct ZipformerAsr {
    encoder: Session,
    decoder: Session,
    joiner: Session,
    tokens: HashMap<i64, String>,
    caches: Vec<CacheTensor>,
}

fn onnx_err(context: &str, e: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("zipformer.{context}"),
        reason: e.to_string(),
    }
}

fn load_session(path: &Path) -> EonResult<Session> {
    Session::builder(crate::runtime::environment()?)
        .map_err(|e| onnx_err("builder", e))?
        .with_intra_threads(1)
        .map_err(|e| onnx_err("threads", e))?
        .commit_from_file(path)
        .map_err(|e| onnx_err("load", e))
}

fn load_tokens(path: &Path) -> EonResult<HashMap<i64, String>> {
    let text = std::fs::read_to_string(path).map_err(|e| onnx_err("tokens", e))?;
    text.lines()
        .filter_map(|line| line.rsplit_once(' '))
        .map(|(symbol, id)| {
            let id = id.parse::<i64>().map_err(|e| onnx_err("tokens", e))?;
            Ok((id, symbol.to_owned()))
        })
        .collect()
}

/// Zero-initialises every encoder input except the audio features. Dynamic dimensions are sized to 1, which matches
/// the batch size used at inference.
fn initial_caches(encoder: &Session) -> EonResult<Vec<CacheTensor>> {
    encoder
        .inputs()
        .iter()
        .filter(|input| input.name() != "x")
        .map(|input| {
            let ValueType::Tensor { ty, shape, .. } = input.dtype() else {
                return Err(onnx_err("caches", format!("non-tensor input {}", input.name())));
            };
            let dims: Vec<i64> = shape.iter().map(|&d| if d <= 0 { 1 } else { d }).collect();
            let len = dims.iter().product::<i64>() as usize;
            let data = match ty {
                TensorElementType::Float32 => CacheData::F32(vec![0.0; len]),
                TensorElementType::Int64 => CacheData::I64(vec![0; len]),
                other => {
                    return Err(onnx_err(
                        "caches",
                        format!("unsupported {other:?} for {}", input.name()),
                    ));
                }
            };
            Ok(CacheTensor {
                name: input.name().to_owned(),
                shape: dims,
                data,
            })
        })
        .collect()
}

fn argmax(values: &[f32]) -> i64 {
    values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i as i64)
        .unwrap_or(BLANK_ID)
}

impl ZipformerAsr {
    /// Loads the three ONNX graphs and token table from `model_dir`. Files must be the digest-verified artifacts from
    /// `scripts/models.lock.json` (`zipformer-*`).
    pub fn load(model_dir: impl AsRef<Path>) -> EonResult<Self> {
        let dir = model_dir.as_ref();
        let encoder = load_session(&dir.join(ENCODER_FILE))?;
        let caches = initial_caches(&encoder)?;
        Ok(Self {
            decoder: load_session(&dir.join(DECODER_FILE))?,
            joiner: load_session(&dir.join(JOINER_FILE))?,
            tokens: load_tokens(&dir.join(TOKENS_FILE))?,
            encoder,
            caches,
        })
    }

    /// Transcribes one utterance of 16 kHz mono samples in [-1, 1]. Caches are reset on every call.
    pub fn transcribe(&mut self, samples: &[f32]) -> EonResult<String> {
        let mut stream = self.start_utterance()?;
        stream.push(samples)?;
        stream.finish()
    }

    pub fn start_utterance(&mut self) -> EonResult<ZipformerStream<'_>> {
        self.caches = initial_caches(&self.encoder)?;
        let context = [BLANK_ID; CONTEXT_SIZE];
        let decoder_out = self.run_decoder(context)?;
        Ok(ZipformerStream {
            model: self,
            features: online_fbank()?,
            next_frame: 0,
            context,
            decoder_out,
            tokens: Vec::new(),
            input_samples: 0,
        })
    }
}

/// One utterance. Both the overlapping fbank windows and the transducer state
/// survive arbitrary PCM push boundaries. End padding is added only by `finish`.
pub struct ZipformerStream<'a> {
    model: &'a mut ZipformerAsr,
    features: OnlineFeature,
    next_frame: usize,
    context: [i64; CONTEXT_SIZE],
    decoder_out: Vec<f32>,
    tokens: Vec<i64>,
    input_samples: usize,
}

impl ZipformerStream<'_> {
    /// Returns the current partial transcript only when new tokens were emitted.
    pub fn push(&mut self, samples: &[f32]) -> EonResult<Option<PartialTranscript>> {
        if samples.len() > MAX_UTTERANCE_SAMPLES - self.input_samples {
            return Err(onnx_err("input", "utterance exceeds the two-minute limit"));
        }
        if !samples.iter().all(|s| (-1.0..=1.0).contains(s)) {
            return Err(onnx_err("input", "expected finite, normalized mono PCM"));
        }
        self.input_samples += samples.len();
        self.features.accept_waveform(SAMPLE_RATE_HZ, samples);
        let old_len = self.tokens.len();
        self.decode_ready()?;
        Ok((self.tokens.len() > old_len).then(|| {
            let text = self.model.detokenize(&self.tokens);
            PartialTranscript {
                hint: ReflexHint::from_partial(&text),
                text,
            }
        }))
    }

    pub fn finish(mut self) -> EonResult<String> {
        self.features.input_finished();
        self.features
            .features
            .extend(std::iter::repeat_with(|| vec![0.0; NUM_MEL_BINS]).take(CHUNK_FRAMES));
        self.decode_ready()?;
        Ok(self.model.detokenize(&self.tokens))
    }

    fn decode_ready(&mut self) -> EonResult<()> {
        while self.next_frame + CHUNK_FRAMES <= self.features.num_frames_ready() {
            let chunk = self.features.features[self.next_frame..self.next_frame + CHUNK_FRAMES].concat();
            let (encoded, frames) = self.model.run_encoder(&chunk)?;
            let dim = encoded.len() / frames;
            for frame in encoded.chunks_exact(dim) {
                let token = self.model.run_joiner(frame, &self.decoder_out)?;
                if token != BLANK_ID {
                    self.tokens.push(token);
                    self.context = [self.context[1], token];
                    self.decoder_out = self.model.run_decoder(self.context)?;
                }
            }
            self.next_frame += CHUNK_SHIFT;
        }
        Ok(())
    }
}

impl ZipformerAsr {
    /// Returns the flattened encoder output and its frame count, and advances the caches.
    fn run_encoder(&mut self, chunk: &[f32]) -> EonResult<(Vec<f32>, usize)> {
        let mut feed: Vec<(String, SessionInputValue<'static>)> = Vec::with_capacity(self.caches.len() + 1);
        let features = Tensor::from_array(([1usize, CHUNK_FRAMES, NUM_MEL_BINS], chunk.to_vec()))
            .map_err(|e| onnx_err("input", e))?;
        feed.push(("x".to_owned(), features.into()));
        for cache in &self.caches {
            let value: SessionInputValue<'static> = match &cache.data {
                CacheData::F32(v) => Tensor::from_array((cache.shape.clone(), v.clone()))
                    .map_err(|e| onnx_err("cache", e))?
                    .into(),
                CacheData::I64(v) => Tensor::from_array((cache.shape.clone(), v.clone()))
                    .map_err(|e| onnx_err("cache", e))?
                    .into(),
            };
            feed.push((cache.name.clone(), value));
        }

        let outputs = self.encoder.run(feed).map_err(|e| onnx_err("encoder", e))?;
        let (shape, encoder_out) = outputs["encoder_out"]
            .try_extract_tensor::<f32>()
            .map_err(|e| onnx_err("encoder_out", e))?;
        let frames = shape[1] as usize;
        let encoder_out = encoder_out.to_vec();

        for cache in &mut self.caches {
            let name = format!("new_{}", cache.name);
            let next = &outputs[name.as_str()];
            match &mut cache.data {
                CacheData::F32(v) => {
                    let (_, data) = next.try_extract_tensor::<f32>().map_err(|e| onnx_err("cache", e))?;
                    v.clear();
                    v.extend_from_slice(data);
                }
                CacheData::I64(v) => {
                    let (_, data) = next.try_extract_tensor::<i64>().map_err(|e| onnx_err("cache", e))?;
                    v.clear();
                    v.extend_from_slice(data);
                }
            }
        }
        Ok((encoder_out, frames))
    }

    fn run_decoder(&mut self, context: [i64; CONTEXT_SIZE]) -> EonResult<Vec<f32>> {
        let y = Tensor::from_array(([1usize, CONTEXT_SIZE], context.to_vec())).map_err(|e| onnx_err("decoder", e))?;
        let outputs = self
            .decoder
            .run(ort::inputs!["y" => y])
            .map_err(|e| onnx_err("decoder", e))?;
        let (_, out) = outputs["decoder_out"]
            .try_extract_tensor::<f32>()
            .map_err(|e| onnx_err("decoder_out", e))?;
        Ok(out.to_vec())
    }

    fn run_joiner(&mut self, encoder_frame: &[f32], decoder_out: &[f32]) -> EonResult<i64> {
        let encoder = Tensor::from_array(([1usize, encoder_frame.len()], encoder_frame.to_vec()))
            .map_err(|e| onnx_err("joiner", e))?;
        let decoder = Tensor::from_array(([1usize, decoder_out.len()], decoder_out.to_vec()))
            .map_err(|e| onnx_err("joiner", e))?;
        let outputs = self
            .joiner
            .run(ort::inputs!["encoder_out" => encoder, "decoder_out" => decoder])
            .map_err(|e| onnx_err("joiner", e))?;
        let (_, logits) = outputs["logit"]
            .try_extract_tensor::<f32>()
            .map_err(|e| onnx_err("logit", e))?;
        Ok(argmax(logits))
    }

    fn detokenize(&self, ids: &[i64]) -> String {
        ids.iter()
            .filter_map(|id| self.tokens.get(id))
            .map(String::as_str)
            .collect::<String>()
            .replace(WORD_BOUNDARY, " ")
            .trim()
            .to_owned()
    }
}

pub struct PartialTranscript {
    pub text: String,
    pub hint: Option<ReflexHint>,
}
