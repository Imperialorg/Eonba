//! End-to-end prefix (KV) cache on the pinned Qwen model. A cached prefix must give exactly the same greedy output as a
//! full decode, reuse exactly the prefix tokens, and fall back cleanly for a prompt that does not start with the prefix.

use std::path::PathBuf;
use std::time::Instant;

use eon_cognition::backend::LocalModel;

fn model_path() -> PathBuf {
    if let Ok(path) = std::env::var("EON_QWEN_MODEL") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models/slm/Qwen3.5-0.8B-Q4_K_M.gguf")
}

const SYSTEM_PREFIX: &str = "You are Eon, a local assistant. Answer briefly. Never reveal secrets. \
Rules: keep answers short; use plain text; say when you do not know. Context: the user is working in a \
software workspace with a Rust engine, a voice pipeline, and a journal of events. Follow the rules above.\n\n";

#[test]
fn cached_prefix_gives_the_same_output_as_a_full_decode() {
    let path = model_path();
    assert!(path.exists(), "missing {}; run: python3 scripts/download_models.py --ids qwen3.5-0.8b-q4_k_m", path.display());
    let model = LocalModel::load(path).expect("pinned model must load");
    let cache = model.build_prefix_cache(SYSTEM_PREFIX).expect("prefix must decode");

    let prompt = format!("{SYSTEM_PREFIX}Write one short sentence about the Rust borrow checker:");
    let started = Instant::now();
    let full = model.generate_greedy(&prompt, 24).unwrap();
    let full_time = started.elapsed();

    let started = Instant::now();
    let cached = model.generate_with_prefix(&cache, &prompt, 24).unwrap();
    let cached_time = started.elapsed();

    eprintln!("KV full={full_time:?} cached={cached_time:?} reused={} text={:?}", cached.reused_tokens, cached.text);
    assert!(!full.trim().is_empty(), "the comparison is meaningless without generated text");
    assert_eq!(cached.text, full, "cached output must equal the full decode");
    assert!(cached.reused_tokens > 0, "the prefix must be reused");
    assert_eq!(cached.reused_tokens, cache.token_count(), "exactly the prefix tokens are reused");
}

#[test]
fn a_prompt_that_does_not_start_with_the_prefix_falls_back_to_a_full_decode() {
    let path = model_path();
    assert!(path.exists(), "missing {}; run: python3 scripts/download_models.py --ids qwen3.5-0.8b-q4_k_m", path.display());
    let model = LocalModel::load(path).expect("pinned model must load");
    let cache = model.build_prefix_cache(SYSTEM_PREFIX).unwrap();

    let other = "Write one short sentence about color theory:";
    let full = model.generate_greedy(other, 12).unwrap();
    assert!(!full.trim().is_empty(), "the comparison is meaningless without generated text");
    let cached = model.generate_with_prefix(&cache, other, 12).unwrap();
    assert_eq!(cached.reused_tokens, 0, "no reuse for an unrelated prompt");
    assert_eq!(cached.text, full);
}
