//! Warm-up on the pinned Qwen model. Warming must not change what the model generates afterwards.

use std::path::PathBuf;

use eon_cognition::backend::LocalModel;

fn model_path() -> PathBuf {
    if let Ok(path) = std::env::var("EON_QWEN_MODEL") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models/slm/Qwen3.5-0.8B-Q4_K_M.gguf")
}

#[test]
fn warm_up_does_not_change_later_greedy_output() {
    let path = model_path();
    assert!(path.exists(), "missing {}; run: python3 scripts/download_models.py --ids qwen3.5-0.8b-q4_k_m", path.display());
    let model = LocalModel::load(path).expect("pinned model must load");

    let cold = model.generate_greedy("Name one Rust keyword.", 8).unwrap();
    model.warm_up().expect("warm-up must succeed");
    let warm = model.generate_greedy("Name one Rust keyword.", 8).unwrap();

    assert!(!cold.trim().is_empty());
    assert_eq!(cold, warm);
}
