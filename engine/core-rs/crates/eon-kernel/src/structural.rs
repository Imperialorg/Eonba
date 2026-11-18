//! Function-level structural merge for Rust and Python (docs/implementation/01, section 4.3).
//!
//! Each side is compared with the base one function at a time, using the tree-sitter function blocks from `eon-cyber`.
//! Edits to different functions merge cleanly, and a function changed differently on both sides is a conflict. Text
//! outside function definitions (imports, constants, comments) is kept from the base. This is not a CST merge: two edits to
//! the same function always conflict, even when they touch different lines.

use eon_cyber::cpg::{function_blocks, FunctionBlock, Lang};
use eon_primitives::{EonError, EonResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    /// Names of functions that both sides changed differently, or added differently under one name.
    Conflicts(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    Unchanged,
    Replaced(String),
    Removed,
}

fn parse_error(reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: "structural.merge".into(),
        reason: reason.to_string(),
    }
}

fn blocks(lang: Lang, source: &str) -> EonResult<Vec<FunctionBlock>> {
    function_blocks(lang, source).map_err(parse_error)
}

fn find<'a>(blocks: &'a [FunctionBlock], name: &str) -> Option<&'a FunctionBlock> {
    blocks.iter().find(|block| block.name == name)
}

/// Combines one side's change with the other's. `None` means the two changes conflict.
fn resolve(ours: Change, theirs: Change) -> Option<Change> {
    match (ours, theirs) {
        (Change::Unchanged, other) | (other, Change::Unchanged) => Some(other),
        (a, b) if a == b => Some(a),
        _ => None,
    }
}

/// Merges `ours` and `theirs`, both derived from `base`, and returns the merged source.
pub fn merge_functions(lang: Lang, base: &str, ours: &str, theirs: &str) -> EonResult<Result<String, MergeError>> {
    let base_blocks = blocks(lang, base)?;
    let ours_blocks = blocks(lang, ours)?;
    let theirs_blocks = blocks(lang, theirs)?;

    let side_change = |side: &[FunctionBlock], block: &FunctionBlock| match find(side, &block.name) {
        None => Change::Removed,
        Some(current) if current.text == block.text => Change::Unchanged,
        Some(current) => Change::Replaced(current.text.clone()),
    };

    let mut conflicts = Vec::new();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for block in &base_blocks {
        let ours_change = side_change(&ours_blocks, block);
        let theirs_change = side_change(&theirs_blocks, block);
        match resolve(ours_change, theirs_change) {
            None => conflicts.push(block.name.clone()),
            Some(Change::Unchanged) => {}
            Some(Change::Removed) => edits.push((block.start_byte, block.end_byte, String::new())),
            Some(Change::Replaced(text)) => edits.push((block.start_byte, block.end_byte, text)),
        }
    }

    let mut additions: Vec<(String, String)> = Vec::new();
    for side in [&ours_blocks, &theirs_blocks] {
        for block in side {
            if find(&base_blocks, &block.name).is_some() {
                continue;
            }
            match additions.iter().find(|(name, _)| *name == block.name) {
                Some((_, text)) if *text != block.text => conflicts.push(block.name.clone()),
                Some(_) => {}
                None => additions.push((block.name.clone(), block.text.clone())),
            }
        }
    }
    if !conflicts.is_empty() {
        return Ok(Err(MergeError::Conflicts(conflicts)));
    }

    edits.sort_by_key(|edit| edit.0);
    let mut merged = String::with_capacity(base.len());
    let mut cursor = 0;
    for (start, end, replacement) in edits {
        merged.push_str(&base[cursor..start]);
        merged.push_str(&replacement);
        cursor = end;
    }
    merged.push_str(&base[cursor..]);
    for (_, text) in &additions {
        merged.push_str("\n\n");
        merged.push_str(text);
    }
    if !additions.is_empty() {
        merged.push('\n');
    }
    Ok(Ok(merged))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "fn a() { 1 }\nfn b() { 2 }\n";

    #[test]
    fn edits_to_different_functions_merge_cleanly() {
        let ours = "fn a() { 3 }\nfn b() { 2 }\n";
        let theirs = "fn a() { 1 }\nfn b() { 4 }\n";
        let merged = merge_functions(Lang::Rust, BASE, ours, theirs).unwrap().unwrap();
        let names: Vec<String> = function_blocks(Lang::Rust, &merged)
            .unwrap()
            .into_iter()
            .map(|b| b.text)
            .collect();
        assert_eq!(names, vec!["fn a() { 3 }", "fn b() { 4 }"]);
    }

    #[test]
    fn the_same_function_changed_differently_on_both_sides_conflicts() {
        let ours = "fn a() { 3 }\nfn b() { 2 }\n";
        let theirs = "fn a() { 5 }\nfn b() { 2 }\n";
        assert_eq!(
            merge_functions(Lang::Rust, BASE, ours, theirs).unwrap(),
            Err(MergeError::Conflicts(vec!["a".into()]))
        );
    }

    #[test]
    fn a_removal_on_one_side_applies_when_the_other_side_is_unchanged() {
        let ours = "fn b() { 2 }\n";
        let theirs = BASE;
        let merged = merge_functions(Lang::Rust, BASE, ours, theirs).unwrap().unwrap();
        let names: Vec<String> = function_blocks(Lang::Rust, &merged)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names, vec!["b"]);
    }

    #[test]
    fn additions_from_both_sides_are_kept_and_the_same_name_added_differently_conflicts() {
        let ours = "fn a() { 1 }\nfn b() { 2 }\nfn from_ours() {}\n";
        let theirs = "fn a() { 1 }\nfn b() { 2 }\nfn from_theirs() {}\n";
        let merged = merge_functions(Lang::Rust, BASE, ours, theirs).unwrap().unwrap();
        let names: Vec<String> = function_blocks(Lang::Rust, &merged)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names, vec!["a", "b", "from_ours", "from_theirs"]);

        let clash_ours = "fn a() { 1 }\nfn b() { 2 }\nfn new_fn() { 1 }\n";
        let clash_theirs = "fn a() { 1 }\nfn b() { 2 }\nfn new_fn() { 2 }\n";
        assert_eq!(
            merge_functions(Lang::Rust, BASE, clash_ours, clash_theirs).unwrap(),
            Err(MergeError::Conflicts(vec!["new_fn".into()]))
        );
    }
}
