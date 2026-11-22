//! Lowers Rust `if` conditions into solver constraints (docs/implementation/03, branch conditions for concrete inputs).
//!
//! Supported: comparisons of a plain identifier with an integer literal (`==`, `!=`, `<`, `>`), combined with `&&`, `||`,
//! `!`, and parentheses. Anything else (calls, arithmetic, field access, two variables) is reported as unsupported with
//! `None`, not approximated, so a solver answer is never about a condition the source did not state.

use tree_sitter::{Language, Node, Parser};

use eon_primitives::{EonError, EonResult};

use crate::cpg::{FunctionBlock, Lang};
use crate::solver::Constraint;

/// One `if` condition in a function, with the function it belongs to and its one-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCondition {
    pub function: String,
    pub line: usize,
    /// `None` when the condition uses a construct outside the supported subset.
    pub constraint: Option<Constraint>,
}

fn parse_error(reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: "branches.parse".into(),
        reason: reason.to_string(),
    }
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

/// The operator of a unary expression. The grammar leaves it as an anonymous first child, not a named field.
fn unary_operator<'a>(node: Node, source: &'a str) -> Option<&'a str> {
    node.child(0).map(|op| text(op, source))
}

fn integer(node: Node, source: &str) -> Option<i64> {
    match node.kind() {
        "integer_literal" => text(node, source).replace('_', "").parse().ok(),
        "unary_expression" if unary_operator(node, source) == Some("-") => {
            integer(node.named_child(0)?, source).map(|v| -v)
        }
        "parenthesized_expression" => integer(node.named_child(0)?, source),
        _ => None,
    }
}

fn lower(node: Node, source: &str) -> Option<Constraint> {
    match node.kind() {
        "parenthesized_expression" => lower(node.named_child(0)?, source),
        "unary_expression" => {
            (unary_operator(node, source)? == "!").then_some(())?;
            Some(Constraint::Not(Box::new(lower(node.named_child(0)?, source)?)))
        }
        "binary_expression" => {
            let op = text(node.child_by_field_name("operator")?, source);
            let left = node.child_by_field_name("left")?;
            let right = node.child_by_field_name("right")?;
            match op {
                "&&" => Some(Constraint::And(vec![lower(left, source)?, lower(right, source)?])),
                "||" => Some(Constraint::Or(vec![lower(left, source)?, lower(right, source)?])),
                "==" | "!=" | "<" | ">" => {
                    let (name, value, flipped) = match (left.kind(), right.kind()) {
                        ("identifier", _) => (text(left, source), integer(right, source)?, false),
                        (_, "identifier") => (text(right, source), integer(left, source)?, true),
                        _ => return None,
                    };
                    let name = name.to_owned();
                    // With the literal on the left, `5 < x` means `x > 5`.
                    let op = match (op, flipped) {
                        ("<", true) => ">",
                        (">", true) => "<",
                        other => other.0,
                    };
                    Some(match op {
                        "==" => Constraint::Eq(name, value),
                        "!=" => Constraint::Ne(name, value),
                        "<" => Constraint::Lt(name, value),
                        ">" => Constraint::Gt(name, value),
                        _ => return None,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn collect(node: Node, source: &str, function: &str, out: &mut Vec<BranchCondition>) {
    if node.kind() == "if_expression"
        && let Some(condition) = node.child_by_field_name("condition")
    {
        out.push(BranchCondition {
            function: function.to_owned(),
            line: node.start_position().row + 1,
            constraint: lower(condition, source),
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, source, function, out);
    }
}

/// Every `if` condition in `source`, attributed to the innermost function that contains it. Conditions outside any
/// function are attributed to `<module>`.
pub fn branch_conditions(source: &str) -> EonResult<Vec<BranchCondition>> {
    let mut parser = Parser::new();
    parser
        .set_language(&Language::new(tree_sitter_rust::LANGUAGE))
        .map_err(parse_error)?;
    let tree = parser.parse(source, None).ok_or_else(|| parse_error("parser produced no tree"))?;
    let blocks: Vec<FunctionBlock> = crate::cpg::function_blocks(Lang::Rust, source)?;
    let mut out = Vec::new();
    collect(tree.root_node(), source, "<module>", &mut out);
    for condition in &mut out {
        let line = condition.line;
        let enclosing = blocks.iter().filter(|b| {
            b.start_line <= line && line <= b.start_line + b.text.lines().count().saturating_sub(1)
        });
        if let Some(block) = enclosing.max_by_key(|b| b.start_line) {
            condition.function = block.name.clone();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::{solve, Solution};

    #[test]
    fn comparisons_with_literals_lower_to_constraints_with_the_literal_on_either_side() {
        let source = "fn check(x: i64, y: i64) {\n    if x > 5 && 3 == y { }\n    if 9 < x { }\n}\n";
        let conditions = branch_conditions(source).unwrap();
        assert_eq!(conditions.len(), 2);
        assert_eq!(conditions[0].function, "check");
        assert_eq!(conditions[0].line, 2);
        assert_eq!(
            conditions[0].constraint,
            Some(Constraint::And(vec![
                Constraint::Gt("x".into(), 5),
                Constraint::Eq("y".into(), 3),
            ]))
        );
        assert_eq!(conditions[1].constraint, Some(Constraint::Gt("x".into(), 9)));
    }

    #[test]
    fn negation_disjunction_and_parentheses_are_supported() {
        let source = "fn f(n: i64) { if !(n == 0) || n < -2 { } }\n";
        let condition = &branch_conditions(source).unwrap()[0];
        assert_eq!(
            condition.constraint,
            Some(Constraint::Or(vec![
                Constraint::Not(Box::new(Constraint::Eq("n".into(), 0))),
                Constraint::Lt("n".into(), -2),
            ]))
        );
    }

    #[test]
    fn unsupported_conditions_are_reported_not_approximated() {
        let source = "fn f(s: &str, a: i64, b: i64) {\n    if s.is_empty() { }\n    if a + 1 > b { }\n    if a == b { }\n}\n";
        let conditions = branch_conditions(source).unwrap();
        assert!(conditions.iter().all(|c| c.constraint.is_none()), "{conditions:?}");
    }

    #[test]
    fn a_lowered_condition_is_reachable_exactly_when_the_solver_says_so() {
        let source = "fn f(x: i64) { if x > 10 && x < 12 { } if x > 3 && x < 2 { } }\n";
        let conditions = branch_conditions(source).unwrap();
        let reachable = solve(conditions[0].constraint.as_ref().unwrap()).unwrap();
        assert!(matches!(reachable, Solution::Sat(ref m) if m["x"] == 11));
        let dead = solve(conditions[1].constraint.as_ref().unwrap()).unwrap();
        assert_eq!(dead, Solution::Unsat);
    }
}
