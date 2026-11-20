use std::io;

use eon_cyber::cpg::{Lang, checked_top_level_function};
use eon_cyber::function_review::{Decision, Review, apply_review, match_blocks};

use super::{MAX_DOCUMENT, invalid, snapshot};
use crate::workspace::Workspace;

/// In-memory proposal. The expected digest covers the entire original snapshot
/// and is suitable for `Workspace::edit` after separate operator approval.
#[derive(Debug)]
pub struct ReplacementPreview {
    pub relative_path: String,
    pub expected_sha256: [u8; 32],
    pub before: String,
    pub after: String,
}

/// Replace exactly one top-level Rust/Python function, not an arbitrary text
/// match. Comments/imports/sibling definitions outside that node cannot change.
pub fn preview_replacement(
    workspace: &Workspace,
    path: &str,
    symbol: &str,
    replacement: &str,
) -> io::Result<ReplacementPreview> {
    let lang = match std::path::Path::new(path).extension().and_then(|s| s.to_str()) {
        Some("rs") => Lang::Rust,
        Some("py") => Lang::Python,
        _ => return Err(invalid("unsupported structural editing language")),
    };
    if replacement.len() > MAX_DOCUMENT || symbol.is_empty() || symbol.len() > 256 {
        return Err(invalid("replacement or symbol exceeds limits"));
    }
    let (before, expected_sha256) = snapshot(workspace, path)?;
    let find = |src: &str| checked_top_level_function(lang, src, symbol).map_err(io::Error::other);
    let old = [find(&before)?];
    let new = [find(replacement)?];
    if new[0].text != replacement.trim_end() || before.len() - old[0].text.len() + new[0].text.len() > MAX_DOCUMENT {
        return Err(invalid(
            "replacement must consist of one complete function within budget",
        ));
    }
    let pairings = match_blocks(&old, &new);
    let mut review = Review::new(&pairings);
    if !review.decide(0, Decision::Applied) {
        return Err(invalid("replacement does not change the function"));
    }
    let after = apply_review(&before, &old, &new, &pairings, &review).map_err(|_| invalid("unreviewed function"))?;
    let actual = find(&after)?;
    if actual.start_byte != old[0].start_byte || actual.text != new[0].text {
        return Err(invalid("result did not preserve the selected function boundary"));
    }
    Ok(ReplacementPreview {
        relative_path: path.into(),
        expected_sha256,
        before,
        after,
    })
}
