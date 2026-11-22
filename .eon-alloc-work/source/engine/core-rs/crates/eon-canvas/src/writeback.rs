//! Writes a reviewed patch back to a repository through a shadow checkout (docs/implementation/01, section 4.3).
//!
//! The patch goes into a fresh shadow checkout of the commit the review was made against. The shadow's tree is merged
//! with the repository's current tree, and the merge is accepted only if the verification gate passes. The file is written
//! into the repository only after that. The shadow is removed on every path.

use std::fs;
use std::path::Path;

use eon_kernel::shadow::{merge_and_verify, ShadowError, Tree, VerifyGate};
use eon_kernel::worktree::{create_shadow, remove_shadow, snapshot};
use eon_primitives::EonError;

#[derive(Debug)]
pub enum WriteBackError {
    Kernel(EonError),
    Shadow(ShadowError),
    Io(String),
}

/// Writes `patched` to `rel_path` in `repo`, merging against the repository's current tree through a shadow checkout of
/// `base_commit` at `shadow`. `shadow` must not exist yet.
pub fn write_back(
    repo: &Path,
    base_commit: &str,
    shadow: &Path,
    rel_path: &str,
    patched: &str,
    gate: &dyn VerifyGate,
) -> Result<Tree, WriteBackError> {
    create_shadow(repo, base_commit, shadow).map_err(WriteBackError::Kernel)?;
    let merged = merge_into_repo(repo, shadow, rel_path, patched, gate);
    let removed = remove_shadow(repo, shadow);
    let merged = merged?;
    removed.map_err(WriteBackError::Kernel)?;

    let bytes = merged
        .get(rel_path)
        .ok_or_else(|| WriteBackError::Io(format!("merged tree has no {rel_path}")))?;
    let target = repo.join(rel_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| WriteBackError::Io(e.to_string()))?;
    }
    fs::write(&target, bytes).map_err(|e| WriteBackError::Io(e.to_string()))?;
    Ok(merged)
}

fn merge_into_repo(
    repo: &Path,
    shadow: &Path,
    rel_path: &str,
    patched: &str,
    gate: &dyn VerifyGate,
) -> Result<Tree, WriteBackError> {
    let base = snapshot(shadow).map_err(WriteBackError::Kernel)?;
    let target = shadow.join(rel_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| WriteBackError::Io(e.to_string()))?;
    }
    fs::write(&target, patched).map_err(|e| WriteBackError::Io(e.to_string()))?;
    let ours = snapshot(shadow).map_err(WriteBackError::Kernel)?;
    let theirs = snapshot(repo).map_err(WriteBackError::Kernel)?;
    merge_and_verify(&base, &ours, &theirs, gate).map_err(WriteBackError::Shadow)
}
