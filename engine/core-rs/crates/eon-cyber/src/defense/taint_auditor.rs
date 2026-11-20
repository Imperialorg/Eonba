//! Bounded structural review of one caller-supplied source unit. Locations are
//! byte ranges in that unit, not snippets, names, runtime traces or proof of a CWE.

use std::ops::Range;

use tree_sitter::Node;

use crate::cpg::{Lang, parse};
use crate::remediation::patch_isolated_python_token_check;

use super::{Assessment, Gap, MAX_SOURCE_BYTES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    UncheckedIndex,
    InterpolatedQuery,
    SplitAtomicUpdate,
    EarlyTokenMismatch,
}

impl Rule {
    pub fn cwe(self) -> &'static str {
        match self {
            Self::UncheckedIndex => "CWE-119",
            Self::InterpolatedQuery => "CWE-89",
            Self::SplitAtomicUpdate => "CWE-362",
            Self::EarlyTokenMismatch => "CWE-208",
        }
    }

    pub fn recommendation(self) -> &'static str {
        match self {
            Self::UncheckedIndex => "Review the index invariant; prefer checked slice access with an explicit failure path.",
            Self::InterpolatedQuery => "Confirm this is a database call; pass values as bound parameters, not query text.",
            Self::SplitAtomicUpdate => "Confirm the receiver is shared and atomic; review a single atomic update or a lock.",
            Self::EarlyTokenMismatch => "Review the isolated bytes comparison proposal using hmac.compare_digest.",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SourceCheck {
    pub rule: Rule,
    /// First matching syntax in this source; no match is not a clean verdict.
    pub evidence: Assessment<Range<usize>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SourceReport {
    pub language: Lang,
    pub checks: [SourceCheck; 4],
}

fn rust_method<'a>(call: Node<'a>, src: &str) -> Option<(Node<'a>, &str, Node<'a>)> {
    if call.kind() != "call_expression" {
        return None;
    }
    let field = call.child_by_field_name("function")?;
    let receiver = field.child_by_field_name("value")?;
    let name = field.child_by_field_name("field")?;
    Some((receiver, &src[name.byte_range()], call.child_by_field_name("arguments")?))
}

fn rust_rule(call: Node<'_>, src: &str) -> Option<Rule> {
    let (receiver, name, args) = rust_method(call, src)?;
    if matches!(name, "get_unchecked" | "get_unchecked_mut") && args.named_child_count() == 1 {
        return Some(Rule::UncheckedIndex);
    }
    // Same plain receiver and a direct load in the value stored. Do not join
    // calls solely by method name or across aliases, functions or branches.
    if name != "store" || args.named_child_count() != 2 || receiver.kind() != "identifier" {
        return None;
    }
    let value = args.named_child(0)?;
    if value.kind() != "binary_expression" {
        return None;
    }
    ["left", "right"]
        .into_iter()
        .filter_map(|field| rust_method(value.child_by_field_name(field)?, src))
        .any(|(other, method, params)| {
            method == "load" && params.named_child_count() == 1 && other.kind() == "identifier"
                && src[other.byte_range()] == src[receiver.byte_range()]
        })
        .then_some(Rule::SplitAtomicUpdate)
}

fn python_query(call: Node<'_>, src: &str) -> Option<Rule> {
    if call.kind() != "call" {
        return None;
    }
    let callee = call.child_by_field_name("function")?;
    let name = callee.child_by_field_name("attribute")?;
    let arg = call.child_by_field_name("arguments")?.named_child(0)?;
    if !matches!(&src[name.byte_range()], "execute" | "executemany") || arg.kind() != "string" {
        return None;
    }
    arg.named_children(&mut arg.walk())
        .any(|n| n.kind() == "interpolation")
        .then_some(Rule::InterpolatedQuery)
}

pub fn review_source(language: Lang, source: &str) -> SourceReport {
    use Rule::*;
    let checks = [UncheckedIndex, InterpolatedQuery, SplitAtomicUpdate, EarlyTokenMismatch];
    let unverified = |gap| SourceReport {
        language,
        checks: checks.map(|rule| SourceCheck { rule, evidence: Assessment::Unverified(gap) }),
    };
    if source.len() > MAX_SOURCE_BYTES {
        return unverified(Gap::LimitExceeded);
    }
    let Ok(tree) = parse(language, source) else {
        return unverified(Gap::InvalidInput);
    };
    let root = tree.root_node();
    if root.has_error() {
        return unverified(Gap::InvalidInput);
    }
    let mut report = unverified(Gap::Unsupported);
    for check in &mut report.checks {
        if matches!((language, check.rule),
            (Lang::Rust, UncheckedIndex | SplitAtomicUpdate) | (Lang::Python, InterpolatedQuery | EarlyTokenMismatch))
        {
            check.evidence = Assessment::Unverified(Gap::OutsideRule);
        }
    }
    if language == Lang::Python {
        match patch_isolated_python_token_check(source) {
            Ok(Some(patch)) => report.checks[3].evidence = Assessment::Observed(patch.flow.branch_condition),
            Err(_) => report.checks[3].evidence = Assessment::Unverified(Gap::InvalidInput),
            Ok(None) => {}
        }
    }
    let mut cursor = root.walk();
    for _ in 0..8192 {
        let node = cursor.node();
        let rule = match language {
            Lang::Rust => rust_rule(node, source),
            Lang::Python => python_query(node, source),
        };
        if let Some(check) = rule.and_then(|r| report.checks.iter_mut().find(|c| c.rule == r))
            && !matches!(check.evidence, Assessment::Observed(_))
        {
            check.evidence = Assessment::Observed(node.byte_range());
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return report;
            }
        }
    }
    unverified(Gap::LimitExceeded)
}
