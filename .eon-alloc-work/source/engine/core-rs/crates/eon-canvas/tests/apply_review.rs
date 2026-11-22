//! Applying review decisions to source: applied changes take the new text, applied removals are deleted, applied additions
//! are appended, and NACKed changes keep the old text.

use eon_canvas::diff::{apply_review, match_blocks, ApplyError, Decision, Review};
use eon_cyber::cpg::{function_blocks, Lang};

#[test]
fn decisions_are_applied_to_the_source_in_their_own_way() {
    let old_src = "fn a() { 1 }\nfn b() { 2 }\nfn gone() {}\n";
    let new_src = "fn a() { 3 }\nfn b() { 9 }\nfn added() {}\n";
    let old = function_blocks(Lang::Rust, old_src).unwrap();
    let new = function_blocks(Lang::Rust, new_src).unwrap();
    let pairings = match_blocks(&old, &new);
    let mut review = Review::new(&pairings);
    assert!(review.decide(0, Decision::Applied));
    assert!(review.decide(1, Decision::Nacked));
    assert!(review.decide(2, Decision::Applied));
    assert!(review.decide(3, Decision::Applied));

    let patched = apply_review(old_src, &old, &new, &pairings, &review).unwrap();
    let result = function_blocks(Lang::Rust, &patched).unwrap();
    let names: Vec<&str> = result.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "added"]);
    assert_eq!(result[0].text, "fn a() { 3 }");
    assert_eq!(result[1].text, "fn b() { 2 }");
}

#[test]
fn a_pending_review_refuses_to_apply() {
    let old_src = "fn a() { 1 }\n";
    let new_src = "fn a() { 2 }\n";
    let old = function_blocks(Lang::Rust, old_src).unwrap();
    let new = function_blocks(Lang::Rust, new_src).unwrap();
    let pairings = match_blocks(&old, &new);
    let review = Review::new(&pairings);
    assert_eq!(
        apply_review(old_src, &old, &new, &pairings, &review),
        Err(ApplyError::Pending { index: 0 })
    );
}
