//! Copy-on-write shadow workspaces with fuse-overlayfs (docs/implementation/01, section 4.3).
//!
//! Each task mounts the shared base tree read-only as the lower layer. Its edits land in a private upper layer, so
//! tasks never touch the base and never each other. The mount is unprivileged, which avoids a root helper. The merge
//! reads the merged view with `worktree::snapshot`, so overlay and worktree shadows feed the same three-way merge.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use eon_primitives::{EonError, EonResult};

use crate::shadow::Tree;
use crate::worktree::snapshot;

fn overlay_err(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("overlay.{operation}"),
        reason: reason.to_string(),
    }
}

/// True when `fuse-overlayfs` and a FUSE unmount helper are on PATH. Callers can fall back to worktrees otherwise.
pub fn available() -> bool {
    tool_exists("fuse-overlayfs") && (tool_exists("fusermount3") || tool_exists("fusermount"))
}

fn tool_exists(name: &str) -> bool {
    Command::new(name).arg("--version").output().is_ok()
}

pub struct ShadowOverlay {
    merged: PathBuf,
    mounted: bool,
}

impl ShadowOverlay {
    /// Mounts `lower` under `state_dir` as a copy-on-write view. `state_dir` must not exist yet. Its `upper` directory
    /// keeps the task's edits after unmount, for audit.
    pub fn mount(lower: &Path, state_dir: &Path) -> EonResult<Self> {
        let upper = state_dir.join("upper");
        let work = state_dir.join("work");
        let merged = state_dir.join("merged");
        for dir in [&upper, &work, &merged] {
            fs::create_dir_all(dir).map_err(|e| overlay_err("mkdir", e))?;
        }
        let options = format!(
            "lowerdir={},upperdir={},workdir={}",
            lower.display(),
            upper.display(),
            work.display()
        );
        let status = Command::new("fuse-overlayfs")
            .arg("-o")
            .arg(&options)
            .arg(&merged)
            .status()
            .map_err(|e| overlay_err("spawn", e))?;
        if !status.success() {
            return Err(overlay_err("mount", format!("fuse-overlayfs exited with {status}")));
        }
        let mut overlay = Self { merged, mounted: true };
        if !overlay.is_mounted() {
            overlay.mounted = false;
            return Err(overlay_err("mount", "fuse-overlayfs returned success without a mount"));
        }
        Ok(overlay)
    }

    fn is_mounted(&self) -> bool {
        let Ok(table) = fs::read_to_string("/proc/self/mounts") else {
            return false;
        };
        let target = self.merged.display().to_string();
        table.lines().any(|line| line.split_whitespace().nth(1) == Some(target.as_str()))
    }

    pub fn path(&self) -> &Path {
        &self.merged
    }

    /// The task's full view of the tree: the base with its edits applied.
    pub fn view(&self) -> EonResult<Tree> {
        snapshot(&self.merged)
    }

    /// Unmounts the view. The upper layer stays on disk.
    pub fn unmount(mut self) -> EonResult<()> {
        self.unmount_inner()
    }

    fn unmount_inner(&mut self) -> EonResult<()> {
        if !self.mounted {
            return Ok(());
        }
        let target = self.merged.display().to_string();
        let attempt = |tool: &str| Command::new(tool).args(["-u", target.as_str()]).status();
        let status = attempt("fusermount3").or_else(|_| attempt("fusermount"));
        match status {
            Ok(s) if s.success() => {
                self.mounted = false;
                Ok(())
            }
            Ok(s) => Err(overlay_err("unmount", format!("fusermount exited with {s}"))),
            Err(e) => Err(overlay_err("unmount", e)),
        }
    }
}

impl Drop for ShadowOverlay {
    fn drop(&mut self) {
        let _ = self.unmount_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow::merge_trees;

    fn base(dir: &Path) {
        fs::write(dir.join("a.rs"), "one\n\nfn helper() {}\n").unwrap();
        fs::write(dir.join("b.rs"), "two\n").unwrap();
    }

    #[test]
    fn overlay_edits_stay_private_and_merge_back() {
        if !available() {
            eprintln!("skipping: fuse-overlayfs and fusermount3 are not installed");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let lower = root.path().join("lower");
        fs::create_dir_all(&lower).unwrap();
        base(&lower);
        let base_tree = snapshot(&lower).unwrap();

        let task_a = ShadowOverlay::mount(&lower, &root.path().join("task-a")).unwrap();
        let task_b = ShadowOverlay::mount(&lower, &root.path().join("task-b")).unwrap();
        fs::write(task_a.path().join("b.rs"), "TWO-from-a\n").unwrap();
        fs::write(task_b.path().join("a.rs"), "one\n\nfn helper_renamed() {}\n").unwrap();

        assert_eq!(fs::read_to_string(lower.join("b.rs")).unwrap(), "two\n", "base must not change");
        assert_eq!(fs::read_to_string(task_b.path().join("b.rs")).unwrap(), "two\n", "task B must not see A's edit");

        let merged = merge_trees(&base_tree, &task_a.view().unwrap(), &task_b.view().unwrap()).unwrap();
        assert_eq!(merged["b.rs"], b"TWO-from-a\n".to_vec());
        assert_eq!(merged["a.rs"], b"one\n\nfn helper_renamed() {}\n".to_vec());

        assert_eq!(fs::read_to_string(root.path().join("task-a/upper/b.rs")).unwrap(), "TWO-from-a\n");
        assert!(!root.path().join("task-a/upper/a.rs").exists(), "task A never touched a.rs");
        task_a.unmount().unwrap();
        task_b.unmount().unwrap();
    }
}
