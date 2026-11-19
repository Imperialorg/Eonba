//! Shadow-workspace merge for parallel tasks (docs/implementation/01, section 4.3).
//!
//! Two tasks edit copies of one base tree. Disjoint edits are taken as they are. Identical edits are accepted once.
//! Overlapping text edits go through a line-level three-way merge, and anything it cannot resolve is a conflict.
//! The merge is all-or-nothing: on any conflict no tree is returned. A clean text merge is still not trusted, so the
//! merged tree must pass a `VerifyGate` (for example `cargo check`) before it is accepted.
//!
//! Structural CST merge (mergiraf or tree-sitter) and real CoW worktrees are not implemented yet. Line-level merge
//! refuses more than a structural merge would, so it fails closed rather than producing wrong output.

use std::collections::{BTreeMap, BTreeSet};

/// A file tree: path to content. A path missing from the map does not exist.
pub type Tree = BTreeMap<String, Vec<u8>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowError {
    /// Paths that could not be merged automatically. No merged tree is produced.
    Conflicts(Vec<String>),
    /// The merged tree failed the verification gate. The reason comes from the gate.
    VerificationFailed(String),
}

/// Runs the post-merge verification (for example `cargo check --workspace`) before a merge is accepted.
pub trait VerifyGate {
    fn verify(&self, merged: &Tree) -> Result<(), String>;
}

/// Paths where `tree` differs from `base`, including files that exist only on one side.
fn changed_paths(base: &Tree, tree: &Tree) -> BTreeSet<String> {
    base.keys()
        .chain(tree.keys())
        .filter(|path| base.get(*path) != tree.get(*path))
        .cloned()
        .collect()
}

fn merge_one(path: &str, base: Option<&Vec<u8>>, ours: Option<&Vec<u8>>, theirs: Option<&Vec<u8>>) -> Result<Option<Vec<u8>>, String> {
    match (ours, theirs) {
        (o, t) if o == t => Ok(o.cloned()),
        (None, _) | (_, None) => Err(path.to_owned()),
        (Some(o), Some(t)) => {
            let ancestor = base.ok_or_else(|| path.to_owned())?;
            let (Ok(ancestor), Ok(o), Ok(t)) = (
                std::str::from_utf8(ancestor),
                std::str::from_utf8(o),
                std::str::from_utf8(t),
            ) else {
                return Err(path.to_owned());
            };
            diffy::merge(ancestor, o, t)
                .map(|text| Some(text.into_bytes()))
                .map_err(|_| path.to_owned())
        }
    }
}

/// Three-way merges two shadow trees against their common base. Returns the merged tree, or every conflicting path.
pub fn merge_trees(base: &Tree, ours: &Tree, theirs: &Tree) -> Result<Tree, ShadowError> {
    let ours_changed = changed_paths(base, ours);
    let theirs_changed = changed_paths(base, theirs);
    let mut merged = base.clone();
    let mut conflicts = Vec::new();

    for path in ours_changed.union(&theirs_changed) {
        let result = merge_one(path, base.get(path), ours.get(path), theirs.get(path));
        match result {
            Ok(Some(content)) => {
                merged.insert(path.clone(), content);
            }
            Ok(None) => {
                merged.remove(path);
            }
            Err(conflict) => conflicts.push(conflict),
        }
    }

    if conflicts.is_empty() {
        Ok(merged)
    } else {
        Err(ShadowError::Conflicts(conflicts))
    }
}

/// Merges two shadow trees and accepts the result only if `gate` verifies it.
pub fn merge_and_verify(
    base: &Tree,
    ours: &Tree,
    theirs: &Tree,
    gate: &dyn VerifyGate,
) -> Result<Tree, ShadowError> {
    let merged = merge_trees(base, ours, theirs)?;
    gate.verify(&merged).map_err(ShadowError::VerificationFailed)?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> Tree {
        files
            .iter()
            .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
            .collect()
    }

    struct AcceptAll;
    impl VerifyGate for AcceptAll {
        fn verify(&self, _: &Tree) -> Result<(), String> {
            Ok(())
        }
    }

    struct RejectWith(&'static str);
    impl VerifyGate for RejectWith {
        fn verify(&self, _: &Tree) -> Result<(), String> {
            Err(self.0.to_owned())
        }
    }

    #[test]
    fn disjoint_files_are_both_taken() {
        let base = tree(&[("a.rs", "a"), ("b.rs", "b")]);
        let ours = tree(&[("a.rs", "a2"), ("b.rs", "b")]);
        let theirs = tree(&[("a.rs", "a"), ("b.rs", "b2")]);
        let merged = merge_trees(&base, &ours, &theirs).unwrap();
        assert_eq!(merged, tree(&[("a.rs", "a2"), ("b.rs", "b2")]));
    }

    #[test]
    fn identical_edits_are_accepted_once() {
        let base = tree(&[("a.rs", "old\n")]);
        let both = tree(&[("a.rs", "new\n")]);
        assert_eq!(merge_trees(&base, &both, &both).unwrap(), both);
    }

    #[test]
    fn non_overlapping_hunks_in_one_file_merge() {
        let base_text = "one\ntwo\nthree\nfour\nfive\nsix\n";
        let base = tree(&[("f.rs", base_text)]);
        let ours = tree(&[("f.rs", "ONE\ntwo\nthree\nfour\nfive\nsix\n")]);
        let theirs = tree(&[("f.rs", "one\ntwo\nthree\nfour\nfive\nSIX\n")]);
        let merged = merge_trees(&base, &ours, &theirs).unwrap();
        assert_eq!(merged["f.rs"], b"ONE\ntwo\nthree\nfour\nfive\nSIX\n".to_vec());
    }

    #[test]
    fn overlapping_hunks_conflict_and_return_no_tree() {
        let base = tree(&[("f.rs", "x\n")]);
        let ours = tree(&[("f.rs", "ours\n")]);
        let theirs = tree(&[("f.rs", "theirs\n")]);
        assert_eq!(
            merge_trees(&base, &ours, &theirs),
            Err(ShadowError::Conflicts(vec!["f.rs".to_owned()]))
        );
    }

    #[test]
    fn delete_against_modify_is_a_conflict() {
        let base = tree(&[("f.rs", "x\n")]);
        let ours = Tree::new();
        let theirs = tree(&[("f.rs", "changed\n")]);
        assert_eq!(
            merge_trees(&base, &ours, &theirs),
            Err(ShadowError::Conflicts(vec!["f.rs".to_owned()]))
        );
    }

    #[test]
    fn non_utf8_overlap_fails_closed() {
        let base = Tree::from([("bin".to_owned(), vec![0u8, 1, 2])]);
        let ours = Tree::from([("bin".to_owned(), vec![0u8, 9, 2])]);
        let theirs = Tree::from([("bin".to_owned(), vec![0u8, 7, 2])]);
        assert!(matches!(merge_trees(&base, &ours, &theirs), Err(ShadowError::Conflicts(_))));
    }

    #[test]
    fn clean_text_merge_is_refused_when_verification_fails() {
        // A textually clean merge can still break the build: one side renames a helper the other side newly calls.
        let base = tree(&[("lib.rs", "fn helper() {}\n\nfn main() {}\n")]);
        let ours = tree(&[("lib.rs", "fn helper_renamed() {}\n\nfn main() {}\n")]);
        let theirs = tree(&[("lib.rs", "fn helper() {}\n\nfn main() { helper(); }\n")]);
        let outcome = merge_and_verify(&base, &ours, &theirs, &RejectWith("cargo check: unresolved `helper`"));
        assert_eq!(
            outcome,
            Err(ShadowError::VerificationFailed("cargo check: unresolved `helper`".to_owned()))
        );
        assert!(merge_and_verify(&base, &ours, &theirs, &AcceptAll).is_ok());
    }
}
