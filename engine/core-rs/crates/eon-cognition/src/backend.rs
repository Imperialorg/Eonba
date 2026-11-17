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
        let ctx_size = NonZeroU32::new(CONTEXT_TOKENS).expect("nonzero constant");
        let mut ctx = self
            .model
            .new_context(shared_backend()?, LlamaContextParams::default().with_n_ctx(Some(ctx_size)))
            .map_err(|e| llm_err("context", e))?;

        let vocab = self.model.vocab();
        let prompt_tokens = vocab.tokenize(prompt.as_bytes(), true, false);
        if prompt_tokens.is_empty() || prompt_tokens.len() + max_tokens > CONTEXT_TOKENS as usize {
            return Err(EonError::ValidationError("prompt does not fit the context window".into()));
        }

        let mut batch = LlamaBatch::new(prompt_tokens.len().max(1), 1);
        let last = prompt_tokens.len() - 1;
        for (i, token) in prompt_tokens.iter().enumerate() {
            batch
                .add(*token, i as i32, &[0], i == last)
                .map_err(|e| llm_err("batch", e))?;
        }
        ctx.decode(&mut batch).map_err(|e| llm_err("decode", e))?;

        let mut sampler = LlamaSampler::greedy();
        let mut output = Vec::new();
        let mut logits_index = batch.n_tokens() - 1;
        for position in (prompt_tokens.len() as i32..).take(max_tokens) {
            let token = sampler.sample(&ctx, logits_index);
            sampler.accept(token);
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
}
