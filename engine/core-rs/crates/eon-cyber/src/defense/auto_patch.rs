//! Checked source proposals for human review, never live hotpatches.

use std::{fmt, ops::Range};

use crate::cpg::{Lang, parse};

use super::{Assessment, Gap, MAX_SOURCE_BYTES};

pub use crate::remediation::patch_isolated_python_token_check;

pub struct BoundsPatch {
    /// Byte range of the original expression in the supplied Rust source.
    pub replaced: Range<usize>,
    source: String,
}

impl BoundsPatch {
    pub fn patched_source(&self) -> &str {
        &self.source
    }
}

impl fmt::Debug for BoundsPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundsPatch")
            .field("replaced", &self.replaced)
            .finish_non_exhaustive()
    }
}

/// One isolated Rust module, standard prelude, immutable byte slice, `usize`
/// index and `Option<u8>` return contract. In-bounds reads still yield `Some`;
/// out-of-bounds now yields `None`. Never invent a fallback for other contracts.
pub fn propose_checked_byte_lookup(source: &str) -> Assessment<BoundsPatch> {
    if source.len() > MAX_SOURCE_BYTES {
        return Assessment::Unverified(Gap::LimitExceeded);
    }
    let Ok(tree) = parse(Lang::Rust, source) else {
        return Assessment::Unverified(Gap::InvalidInput);
    };
    let root = tree.root_node();
    if root.has_error() {
        return Assessment::Unverified(Gap::InvalidInput);
    }
    let proposal = || {
        let function = root.named_child(0)?;
        let params = function.child_by_field_name("parameters")?;
        let param = |i| params.named_child(i)?.child_by_field_name("pattern");
        let name = |n: tree_sitter::Node<'_>| {
            let s = &source[n.byte_range()];
            (n.kind() == "identifier"
                && s.starts_with(|c: char| c.is_ascii_lowercase())
                && s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'))
            .then_some(s)
        };
        let function_name = name(function.child_by_field_name("name")?)?;
        let bytes = name(param(0)?)?;
        let index = name(param(1)?)?;
        // A whole-module match rules out shadowed types/methods, attributes,
        // other statements or observable side effects in the index expression.
        let original = format!("Some(unsafe {{ *{bytes}.get_unchecked({index}) }})");
        let signature = format!("fn {function_name}({bytes}: &[u8], {index}: usize) -> Option<u8>");
        let admitted = format!("{signature} {{ {original} }}");
        if bytes == index || !source.split_ascii_whitespace().eq(admitted.split_ascii_whitespace()) {
            return None;
        }
        let expr = function.child_by_field_name("body")?.named_child(0)?;
        let replaced = expr.byte_range();
        let checked = format!("{bytes}.get({index}).copied()");
        let mut patched = source.to_owned();
        patched.replace_range(replaced.clone(), &checked);
        let after = parse(Lang::Rust, &patched).ok()?;
        let expected = format!("{signature} {{ {checked} }}");
        (!after.root_node().has_error() && patched.split_ascii_whitespace().eq(expected.split_ascii_whitespace()))
            .then_some(BoundsPatch { replaced, source: patched })
    };
    proposal().map_or(Assessment::Unverified(Gap::OutsideRule), Assessment::Observed)
}
