//! Bounded structural review of one caller-supplied source unit. Locations are
//! byte ranges in that unit, not snippets, names, runtime traces or proof of a CWE.

use std::ops::Range;

use tree_sitter::Node;

use crate::cpg::{Lang, parse};
use crate::remediation::patch_isolated_python_token_check;

use super::{Assessment, Gap, MAX_SOURCE_BYTES};

const MAX_TREE_NODES: usize = 8192;

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
            Self::UncheckedIndex => {
                "Review the index invariant; prefer checked slice access with an explicit failure path."
            }
            Self::InterpolatedQuery => {
                "Confirm this is a database call; pass values as bound parameters, not query text."
            }
            Self::SplitAtomicUpdate => {
                "Confirm the receiver is shared and atomic; review a single atomic update or a lock."
            }
            Self::EarlyTokenMismatch => {
                "Review the isolated bytes comparison proposal using hmac.compare_digest."
            }
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

fn rust_method<'tree, 'src>(
    call: Node<'tree>,
    src: &'src str,
) -> Option<(Node<'tree>, &'src str, Node<'tree>)> {
    if call.kind() != "call_expression" {
        return None;
    }
    let field = call.child_by_field_name("function")?;
    if field.kind() != "field_expression" {
        return None;
    }
    let receiver = field.child_by_field_name("value")?;
    let name = field.child_by_field_name("field")?;
    Some((
        receiver,
        &src[name.byte_range()],
        call.child_by_field_name("arguments")?,
    ))
}

fn rust_arguments<const N: usize>(args: Node<'_>) -> Option<[Node<'_>; N]> {
    let mut cursor = args.walk();
    let mut children = args.named_children(&mut cursor).filter(|n| !n.is_extra());
    let mut result = [args; N];
    for arg in &mut result {
        *arg = children.next()?;
    }
    children.next().is_none().then_some(result)
}

fn rust_rule(call: Node<'_>, src: &str) -> Option<Rule> {
    let (receiver, name, args) = rust_method(call, src)?;
    if matches!(name, "get_unchecked" | "get_unchecked_mut") && rust_arguments::<1>(args).is_some()
    {
        return Some(Rule::UncheckedIndex);
    }
    // Same plain receiver and a direct load in the value stored. Do not join
    // calls solely by method name or across aliases, functions or branches.
    if name != "store" || receiver.kind() != "identifier" {
        return None;
    }
    let [value, _] = rust_arguments(args)?;
    if value.kind() != "binary_expression" {
        return None;
    }
    ["left", "right"]
        .into_iter()
        .filter_map(|field| rust_method(value.child_by_field_name(field)?, src))
        .any(|(other, method, params)| {
            method == "load"
                && rust_arguments::<1>(params).is_some()
                && other.kind() == "identifier"
                && src[other.byte_range()] == src[receiver.byte_range()]
        })
        .then_some(Rule::SplitAtomicUpdate)
}

fn python_query(call: Node<'_>, src: &str) -> Option<Rule> {
    if call.kind() != "call" {
        return None;
    }
    let callee = call.child_by_field_name("function")?;
    let args = call.child_by_field_name("arguments")?;
    if callee.kind() != "attribute" || args.kind() != "argument_list" {
        return None;
    }
    let name = callee.child_by_field_name("attribute")?;
    let arg = args
        .named_children(&mut args.walk())
        .find(|n| !n.is_extra())?;
    if !matches!(&src[name.byte_range()], "execute" | "executemany") || arg.kind() != "string" {
        return None;
    }
    arg.named_children(&mut arg.walk())
        .any(|n| n.kind() == "interpolation")
        .then_some(Rule::InterpolatedQuery)
}

pub fn review_source(language: Lang, source: &str) -> SourceReport {
    use Rule::*;
    let checks = [
        UncheckedIndex,
        InterpolatedQuery,
        SplitAtomicUpdate,
        EarlyTokenMismatch,
    ];
    let unverified = |gap| SourceReport {
        language,
        checks: checks.map(|rule| SourceCheck {
            rule,
            evidence: Assessment::Unverified(gap),
        }),
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
        check.evidence = match (language, check.rule) {
            (Lang::Python, EarlyTokenMismatch) => match patch_isolated_python_token_check(source) {
                Ok(Some(patch)) => Assessment::Observed(patch.flow.branch_condition),
                Err(_) => Assessment::Unverified(Gap::InvalidInput),
                Ok(None) => Assessment::Unverified(Gap::OutsideRule),
            },
            (Lang::Rust, UncheckedIndex | SplitAtomicUpdate)
            | (Lang::Python, InterpolatedQuery) => Assessment::Unverified(Gap::OutsideRule),
            _ => Assessment::Unverified(Gap::Unsupported),
        };
    }
    let mut cursor = root.walk();
    for _ in 0..MAX_TREE_NODES {
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
