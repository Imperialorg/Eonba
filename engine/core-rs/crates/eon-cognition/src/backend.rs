//! Local LLM generation over llama.cpp (docs/implementation/02, section 3).
//!
//! Greedy decoding only, so output is deterministic for a fixed model, prompt, and context size. That property is
//! what the end-to-end test relies on. Sampling policies come later and must not change this path.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::OnceLock;

use eon_primitives::{EonError, EonResult};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{LlamaModel, params::LlamaModelParams};
use llama_cpp_2::sampling::LlamaSampler;

const CONTEXT_TOKENS: u32 = 2048;

/// llama.cpp allows one backend initialization per process, so every model shares this one.
fn shared_backend() -> EonResult<&'static LlamaBackend> {
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| LlamaBackend::init().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|reason| llm_err("backend", reason))
}

/// A decoded prompt prefix and the context state after it.
pub struct PrefixCache {
    tokens: Vec<llama_cpp_2::token::LlamaToken>,
    state: Vec<u8>,
}

impl PrefixCache {
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedGeneration {
    pub text: String,
    pub reused_tokens: usize,
}

pub struct LocalModel {
    model: LlamaModel,
}

fn llm_err(context: &str, e: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("llm.{context}"),
        reason: e.to_string(),
    }
}

impl LocalModel {
    pub fn load(path: impl AsRef<Path>) -> EonResult<Self> {
        let model = LlamaModel::load_from_file(shared_backend()?, path, &LlamaModelParams::default())
            .map_err(|e| llm_err("load", e))?;
        Ok(Self { model })
    }

    /// Generates up to `max_tokens` tokens greedily after `prompt`, stopping at end-of-generation.
    pub fn generate_greedy(&self, prompt: &str, max_tokens: usize) -> EonResult<String> {
        self.generate_with(prompt, max_tokens, LlamaSampler::greedy())
    }

    /// Generates greedily under a GBNF grammar, so every token keeps the output inside the grammar. The grammar's
    /// root rule is `root`. Generation stops when the grammar is complete or `max_tokens` is reached.
    pub fn generate_constrained(&self, prompt: &str, grammar: &str, max_tokens: usize) -> EonResult<String> {
        let constraint = LlamaSampler::grammar(&self.model, grammar, "root")
            .map_err(|e| llm_err("grammar", e))?;
        self.generate_with(prompt, max_tokens, LlamaSampler::chain_simple([constraint, LlamaSampler::greedy()]))
    }

    fn new_context(&self) -> EonResult<llama_cpp_2::context::LlamaContext<'_>> {
        let ctx_size = NonZeroU32::new(CONTEXT_TOKENS).expect("nonzero constant");
        self.model
            .new_context(shared_backend()?, LlamaContextParams::default().with_n_ctx(Some(ctx_size)))
            .map_err(|e| llm_err("context", e))
    }

    /// Decodes `tokens[from..]` as one batch, with logits for the last token only. Returns the batch index of those
    /// logits. `from` must leave at least one token.
    fn decode_from(
        ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
        tokens: &[llama_cpp_2::token::LlamaToken],
        from: usize,
    ) -> EonResult<i32> {
        if from >= tokens.len() {
            return Err(EonError::ValidationError("at least one token must follow the cached prefix".into()));
        }
        let mut batch = LlamaBatch::new(tokens.len() - from, 1);
        let last = tokens.len() - 1;
        for (i, token) in tokens.iter().enumerate().skip(from) {
            batch.add(*token, i as i32, &[0], i == last).map_err(|e| llm_err("batch", e))?;
        }
        ctx.decode(&mut batch).map_err(|e| llm_err("decode", e))?;
        Ok(batch.n_tokens() - 1)
    }

    /// Samples from the logits at `logits_index` of the last decode, then keeps decoding one token at a time until
    /// end-of-generation or `max_tokens`. The context must hold `prompt_len` tokens already.
    fn sample_loop(
        &self,
        ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
        mut sampler: LlamaSampler,
        prompt_len: usize,
        max_tokens: usize,
        logits_index: i32,
    ) -> EonResult<String> {
        let vocab = self.model.vocab();
        let mut batch = LlamaBatch::new(1, 1);
        let mut output = Vec::new();
        let mut logits_index = logits_index;
        for position in (prompt_len as i32..).take(max_tokens) {
            // `sample` also accepts the token into the sampler chain, so it must not be accepted again.
            let token = sampler.sample(ctx, logits_index);
            if vocab.is_eog(token) {
                break;
            }
            output.extend(vocab.token_to_piece(token, false, None));

            batch.clear();
            batch.add(token, position, &[0], true).map_err(|e| llm_err("batch", e))?;
            ctx.decode(&mut batch).map_err(|e| llm_err("decode", e))?;
            logits_index = 0;
        }
        String::from_utf8(output).map_err(|e| llm_err("utf8", e))
    }

    fn generate_with(&self, prompt: &str, max_tokens: usize, sampler: LlamaSampler) -> EonResult<String> {
        let tokens = self.model.vocab().tokenize(prompt.as_bytes(), true, false);
        if tokens.is_empty() || tokens.len() + max_tokens > CONTEXT_TOKENS as usize {
            return Err(EonError::ValidationError("prompt does not fit the context window".into()));
        }
        let mut ctx = self.new_context()?;
        let logits = Self::decode_from(&mut ctx, &tokens, 0)?;
        self.sample_loop(&mut ctx, sampler, tokens.len(), max_tokens, logits)
    }

    /// Decodes a fixed prefix once and captures the resulting context state. Later prompts that start with the same
    /// tokens reuse it, so only their new tokens are decoded (docs/implementation/02, KV pre-cognition).
    pub fn build_prefix_cache(&self, prefix: &str) -> EonResult<PrefixCache> {
        let tokens = self.model.vocab().tokenize(prefix.as_bytes(), true, false);
        if tokens.is_empty() {
            return Err(EonError::ValidationError("prefix is empty".into()));
        }
        let mut ctx = self.new_context()?;
        Self::decode_from(&mut ctx, &tokens, 0)?;
        let mut state = vec![0u8; ctx.get_state_size()];
        // SAFETY: `state` has exactly `get_state_size()` bytes, which is the maximum llama.cpp writes.
        let written = unsafe { ctx.copy_state_data(state.as_mut_ptr()) };
        state.truncate(written);
        Ok(PrefixCache { tokens, state })
    }

    /// Greedy generation for `prompt`, reusing `cache` when the prompt starts with its tokens. Returns the text and the
    /// number of prompt tokens that were served from the cache (zero when it did not apply).
    pub fn generate_with_prefix(&self, cache: &PrefixCache, prompt: &str, max_tokens: usize) -> EonResult<CachedGeneration> {
        let tokens = self.model.vocab().tokenize(prompt.as_bytes(), true, false);
        if tokens.len() + max_tokens > CONTEXT_TOKENS as usize {
            return Err(EonError::ValidationError("prompt does not fit the context window".into()));
        }
        let reusable = tokens.len() > cache.tokens.len() && tokens.starts_with(&cache.tokens);
        let mut ctx = self.new_context()?;
        let reused = if reusable {
            // SAFETY: the state was produced by `copy_state_data` on a context of the same model and parameters.
            let read = unsafe { ctx.set_state_data(&cache.state) };
            if read != cache.state.len() {
                return Err(llm_err("state", format!("restored {read} of {} bytes", cache.state.len())));
            }
            cache.tokens.len()
        } else {
            0
        };
        let logits = Self::decode_from(&mut ctx, &tokens, reused)?;
        let text = self.sample_loop(&mut ctx, LlamaSampler::greedy(), tokens.len(), max_tokens, logits)?;
        Ok(CachedGeneration {
            text,
            reused_tokens: reused,
        })
    }
}
