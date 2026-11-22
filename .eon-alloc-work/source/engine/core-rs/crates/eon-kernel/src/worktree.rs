//! Shadow git worktrees for parallel tasks (docs/implementation/01, section 4.3).
//!
//! Each task edits its own detached checkout of a common base commit. `snapshot` reads a checkout into a `Tree`, which
//! `shadow::merge_trees` then merges. Worktrees are real working copies, not copy-on-write overlays. Overlay storage
//! is deferred.

use std::fs;
use std::path::Path;
use std::process::Command;

use eon_primitives::{EonError, EonResult};

use crate::shadow::Tree;

fn operation_failed(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: operation.to_owned(),
        reason: reason.to_string(),
    }
}

fn path_arg(path: &Path) -> EonResult<&str> {
    path.to_str()
        .ok_or_else(|| operation_failed("worktree.path", format!("non-UTF-8 path {}", path.display())))
}

fn git(repo: &Path, args: &[&str]) -> EonResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| operation_failed("worktree.git", e))?;
    if !output.status.success() {
        return Err(operation_failed(
            &format!("git {}", args.join(" ")),
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Creates a detached shadow checkout of `base_commit` at `dest`. `dest` must not exist yet.
pub fn create_shadow(repo: &Path, base_commit: &str, dest: &Path) -> EonResult<()> {
    git(repo, &["worktree", "add", "--detach", path_arg(dest)?, base_commit]).map(|_| ())
}

/// Removes a shadow checkout and its worktree registration.
pub fn remove_shadow(repo: &Path, dest: &Path) -> EonResult<()> {
    git(repo, &["worktree", "remove", "--force", path_arg(dest)?]).map(|_| ())
}

/// Reads every regular file under `dir` (skipping `.git` and symlinks) into a tree keyed by `/`-separated paths.
pub fn snapshot(dir: &Path) -> EonResult<Tree> {
    let mut tree = Tree::new();
    collect(dir, dir, &mut tree)?;
    Ok(tree)
}

fn collect(root: &Path, dir: &Path, tree: &mut Tree) -> EonResult<()> {
    let io = |e: std::io::Error| operation_failed("worktree.snapshot", e);
    for entry in fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let path = entry.path();
        if entry.file_name() == ".git" {
            continue;
        }
        let file_type = entry.file_type().map_err(io)?;
        if file_type.is_dir() {
            collect(root, &path, tree)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| operation_failed("worktree.snapshot", e))?;
            let key = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            tree.insert(key, fs::read(&path).map_err(io)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow::{merge_trees, ShadowError};

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "eon-test")
            .env("GIT_AUTHOR_EMAIL", "eon-test@example.invalid")
            .env("GIT_COMMITTER_NAME", "eon-test")
            .env("GIT_COMMITTER_EMAIL", "eon-test@example.invalid")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn base_repo(root: &Path) -> (std::path::PathBuf, String) {
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join("a.rs"), "one\n\nfn helper() {}\n").unwrap();
        fs::write(repo.join("b.rs"), "two\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-q", "-m", "base"]);
        let head = git(&repo, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        (repo, head)
    }

    #[test]
    fn two_shadow_edits_merge_back_through_real_worktrees() {
        let root = tempfile::tempdir().unwrap();
        let (repo, base) = base_repo(root.path());

        let base_dir = root.path().join("base");
        create_shadow(&repo, &base, &base_dir).unwrap();
        let base_tree = snapshot(&base_dir).unwrap();

        let task_a = root.path().join("task-a");
        let task_b = root.path().join("task-b");
        create_shadow(&repo, &base, &task_a).unwrap();
        create_shadow(&repo, &base, &task_b).unwrap();
        fs::write(task_a.join("b.rs"), "TWO-from-a\n").unwrap();
        fs::write(task_b.join("a.rs"), "one\n\nfn helper_renamed() {}\n").unwrap();

        let merged = merge_trees(&base_tree, &snapshot(&task_a).unwrap(), &snapshot(&task_b).unwrap()).unwrap();
        assert_eq!(merged["b.rs"], b"TWO-from-a\n".to_vec());
        assert_eq!(merged["a.rs"], b"one\n\nfn helper_renamed() {}\n".to_vec());
        assert!(!merged.contains_key(".git"));

        for dir in [&base_dir, &task_a, &task_b] {
            remove_shadow(&repo, dir).unwrap();
            assert!(!dir.exists(), "{} should be removed", dir.display());
        }
    }

    #[test]
    fn overlapping_edits_in_real_worktrees_conflict() {
        let root = tempfile::tempdir().unwrap();
        let (repo, base) = base_repo(root.path());
        let base_dir = root.path().join("base");
        let task_a = root.path().join("task-a");
        let task_b = root.path().join("task-b");
        create_shadow(&repo, &base, &base_dir).unwrap();
        create_shadow(&repo, &base, &task_a).unwrap();
        create_shadow(&repo, &base, &task_b).unwrap();
        fs::write(task_a.join("b.rs"), "from-a\n").unwrap();
        fs::write(task_b.join("b.rs"), "from-b\n").unwrap();

        let outcome = merge_trees(&snapshot(&base_dir).unwrap(), &snapshot(&task_a).unwrap(), &snapshot(&task_b).unwrap());
        assert_eq!(outcome, Err(ShadowError::Conflicts(vec!["b.rs".to_owned()])));
    }
}
