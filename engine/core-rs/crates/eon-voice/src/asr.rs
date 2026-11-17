//! Zipformer streaming transducer ASR over ONNX Runtime (docs/implementation/04, speech recognition section).
//!
//! The encoder consumes 45-frame chunks of 80-bin Kaldi fbank and carries its cache tensors from one chunk to the
//! next. Greedy transducer search runs the joiner on every encoder frame and advances a stateful decoder over the
//! last two emitted tokens. Token 0 is blank.

use std::collections::HashMap;
use std::path::Path;

use eon_primitives::{EonError, EonResult};
use kaldi_native_fbank::online::FeatureComputer;
use kaldi_native_fbank::{FbankComputer, FbankOptions, OnlineFeature};
use ort::session::{Session, SessionInputValue};
use ort::value::{Tensor, TensorElementType, ValueType};

const SAMPLE_RATE_HZ: f32 = 16_000.0;
const NUM_MEL_BINS: usize = 80;
const CHUNK_FRAMES: usize = 45;
const CHUNK_SHIFT: usize = 32;
const CONTEXT_SIZE: usize = 2;
const BLANK_ID: i64 = 0;
const WORD_BOUNDARY: char = '\u{2581}';

const ENCODER_FILE: &str = "encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx";
const DECODER_FILE: &str = "decoder-epoch-99-avg-1-chunk-16-left-128.onnx";
const JOINER_FILE: &str = "joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx";
const TOKENS_FILE: &str = "tokens.txt";

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
                other => return Err(onnx_err("caches", format!("unsupported {other:?} for {}", input.name()))),
            };
            Ok(CacheTensor {
                name: input.name().to_owned(),
                shape: dims,
                data,
            })
        })
        .collect()
}

fn fbank(samples: &[f32]) -> EonResult<Vec<Vec<f32>>> {
    let mut opts = FbankOptions::default();
    opts.frame_opts.dither = 0.0;
    opts.frame_opts.snip_edges = false;
    opts.mel_opts.num_bins = NUM_MEL_BINS;
    opts.use_energy = false;
    let computer = FbankComputer::new(opts).map_err(|e| onnx_err("fbank", e))?;
    let mut features = OnlineFeature::new(FeatureComputer::Fbank(computer));
    features.accept_waveform(SAMPLE_RATE_HZ, samples);
    features.input_finished();
    (0..features.num_frames_ready())
        .map(|i| {
            features
                .get_frame(i)
                .map(<[f32]>::to_vec)
                .ok_or_else(|| onnx_err("fbank", format!("missing frame {i}")))
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
        self.caches = initial_caches(&self.encoder)?;

        let mut frames = fbank(samples)?;
        frames.extend(std::iter::repeat_with(|| vec![0.0; NUM_MEL_BINS]).take(CHUNK_FRAMES));

        let mut context = [BLANK_ID; CONTEXT_SIZE];
        let mut decoder_out = self.run_decoder(context)?;
        let mut hypothesis = Vec::new();
        let mut start = 0;
        while start + CHUNK_FRAMES <= frames.len() {
            let chunk: Vec<f32> = frames[start..start + CHUNK_FRAMES].concat();
            let (encoder_out, encoder_frames) = self.run_encoder(&chunk)?;
            let dim = encoder_out.len() / encoder_frames;
            for t in 0..encoder_frames {
                let token = self.run_joiner(&encoder_out[t * dim..(t + 1) * dim], &decoder_out)?;
                if token != BLANK_ID {
                    hypothesis.push(token);
                    context = [context[1], token];
                    decoder_out = self.run_decoder(context)?;
                }
            }
            start += CHUNK_SHIFT;
        }
        Ok(self.detokenize(&hypothesis))
    }

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
        let outputs = self.decoder.run(ort::inputs!["y" => y]).map_err(|e| onnx_err("decoder", e))?;
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
