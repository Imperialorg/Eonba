//! End-to-end grammar-constrained decoding on the pinned Qwen model. Constrained output must be a tool call with an
//! allowed name and an object of arguments, for any prompt, because the grammar masks every other token.

use std::path::PathBuf;

use eon_cognition::backend::LocalModel;
use eon_cognition::grammar::tool_call_grammar;

fn model_path() -> PathBuf {
    if let Ok(path) = std::env::var("EON_QWEN_MODEL") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models/slm/Qwen3.5-0.8B-Q4_K_M.gguf")
}

#[test]
fn constrained_output_is_always_an_allowed_tool_call() {
    let path = model_path();
    assert!(path.exists(), "missing {}; run: python3 scripts/download_models.py --ids qwen3.5-0.8b-q4_k_m", path.display());
    let model = LocalModel::load(path).expect("pinned model must load");
    let allowed = ["shell.exec", "fs.read"];
    let grammar = tool_call_grammar(&allowed).unwrap();
    let prompt = "Task: list the files in the current directory.\nAnswer with one tool call as JSON.\n";

    let unconstrained = model.generate_greedy(prompt, 60).unwrap();
    eprintln!("UNCONSTRAINED {unconstrained:?}");

    let constrained = model.generate_constrained(prompt, &grammar, 96).expect("constrained generation must run");
    eprintln!("CONSTRAINED {constrained:?}");
    let call: serde_json::Value =
        serde_json::from_str(&constrained).unwrap_or_else(|e| panic!("constrained output is not JSON ({e}): {constrained:?}"));
    let name = call["name"].as_str().expect("name must be a string");
    assert!(allowed.contains(&name), "tool {name:?} is outside the allow-list");
    assert!(call["arguments"].is_object(), "arguments must be an object: {constrained:?}");
}
