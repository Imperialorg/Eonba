//! Write-back of a reviewed patch through a shadow checkout. Uses real git repositories in temporary directories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use eon_canvas::diff::{apply_review, match_blocks, Decision, Review};
use eon_canvas::writeback::{write_back, WriteBackError};
use eon_cyber::cpg::{function_blocks, Lang};
use eon_kernel::shadow::{ShadowError, Tree, VerifyGate};

struct AcceptAll;

impl VerifyGate for AcceptAll {
    fn verify(&self, _: &Tree) -> Result<(), String> {
        Ok(())
    }
}

struct RefuseAll;

impl VerifyGate for RefuseAll {
    fn verify(&self, _: &Tree) -> Result<(), String> {
        Err("the verification gate refuses this patch".into())
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A temporary repository with `lib.rs` committed as `source`, and the shadow path for it.
fn fixture(name: &str, source: &str) -> (PathBuf, PathBuf, String) {
    let base = std::env::temp_dir().join(format!("eon-writeback-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let shadow = base.join("shadow");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("lib.rs"), source).unwrap();
    git(&repo, &["add", "lib.rs"]);
    git(
        &repo,
        &["-c", "user.name=test", "-c", "user.email=test@example.com", "commit", "-q", "-m", "base"],
    );
    let commit = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    (repo, shadow, commit)
}

fn applied_patch(old_src: &str, new_src: &str) -> String {
    let old = function_blocks(Lang::Rust, old_src).unwrap();
    let new = function_blocks(Lang::Rust, new_src).unwrap();
    let pairings = match_blocks(&old, &new);
    let mut review = Review::new(&pairings);
    for index in 0..pairings.len() {
        review.decide(index, Decision::Applied);
    }
    apply_review(old_src, &old, &new, &pairings, &review).unwrap()
}

#[test]
fn an_accepted_patch_lands_in_the_repository_and_the_shadow_is_removed() {
    let old_src = "fn a() { 1 }\n";
    let patched = applied_patch(old_src, "fn a() { 2 }\n");
    let (repo, shadow, commit) = fixture("accepted", old_src);

    write_back(&repo, &commit, &shadow, "lib.rs", &patched, &AcceptAll).unwrap();

    assert_eq!(fs::read_to_string(repo.join("lib.rs")).unwrap(), patched);
    assert!(!shadow.exists(), "the shadow checkout must be removed");
    fs::remove_dir_all(repo.parent().unwrap()).unwrap();
}

#[test]
fn a_refused_patch_leaves_the_repository_untouched_and_removes_the_shadow() {
    let old_src = "fn a() { 1 }\n";
    let patched = applied_patch(old_src, "fn a() { 2 }\n");
    let (repo, shadow, commit) = fixture("refused", old_src);

    let err = write_back(&repo, &commit, &shadow, "lib.rs", &patched, &RefuseAll).unwrap_err();

    assert!(matches!(err, WriteBackError::Shadow(ShadowError::VerificationFailed(_))));
    assert_eq!(fs::read_to_string(repo.join("lib.rs")).unwrap(), old_src);
    assert!(!shadow.exists(), "the shadow checkout must be removed even when refused");
    fs::remove_dir_all(repo.parent().unwrap()).unwrap();
}
