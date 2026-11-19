//! Path-condition solving with Z3 (docs/implementation/03, concrete inputs for branch conditions).
//!
//! A branch condition from the code graph is expressed as a [`Constraint`] over named integer inputs. [`solve`] returns a
//! concrete assignment that satisfies it, or `Unsat` when no input can reach the branch. The result can be checked with
//! [`holds`], which evaluates the constraint directly, so a model is never trusted on the solver's word alone.

use std::collections::BTreeMap;

use eon_primitives::{EonError, EonResult};
use z3::ast::{Bool, Int};
use z3::{SatResult, Solver};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    Eq(String, i64),
    Ne(String, i64),
    Gt(String, i64),
    Lt(String, i64),
    And(Vec<Constraint>),
    Or(Vec<Constraint>),
    Not(Box<Constraint>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Solution {
    Sat(BTreeMap<String, i64>),
    Unsat,
}

fn solver_err(reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: "solver.solve".into(),
        reason: reason.to_string(),
    }
}

fn variables(constraint: &Constraint, out: &mut Vec<String>) {
    match constraint {
        Constraint::Eq(v, _) | Constraint::Ne(v, _) | Constraint::Gt(v, _) | Constraint::Lt(v, _) => {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
        Constraint::And(parts) | Constraint::Or(parts) => parts.iter().for_each(|p| variables(p, out)),
        Constraint::Not(inner) => variables(inner, out),
    }
}

fn encode(constraint: &Constraint, vars: &BTreeMap<String, Int>) -> Bool {
    let var = |name: &String| vars[name].clone();
    match constraint {
        Constraint::Eq(v, c) => var(v).eq(Int::from_i64(*c)),
        Constraint::Ne(v, c) => var(v).ne(Int::from_i64(*c)),
        Constraint::Gt(v, c) => var(v).gt(Int::from_i64(*c)),
        Constraint::Lt(v, c) => var(v).lt(Int::from_i64(*c)),
        Constraint::And(parts) => {
            let encoded: Vec<Bool> = parts.iter().map(|p| encode(p, vars)).collect();
            let refs: Vec<&Bool> = encoded.iter().collect();
            Bool::and(&refs)
        }
        Constraint::Or(parts) => {
            let encoded: Vec<Bool> = parts.iter().map(|p| encode(p, vars)).collect();
            let refs: Vec<&Bool> = encoded.iter().collect();
            Bool::or(&refs)
        }
        Constraint::Not(inner) => encode(inner, vars).not(),
    }
}

/// Finds integer inputs that satisfy `constraint`, or reports that none exist.
pub fn solve(constraint: &Constraint) -> EonResult<Solution> {
    let mut names = Vec::new();
    variables(constraint, &mut names);
    let vars: BTreeMap<String, Int> = names.iter().map(|n| (n.clone(), Int::new_const(n.as_str()))).collect();

    let solver = Solver::new();
    solver.assert(encode(constraint, &vars));
    match solver.check() {
        SatResult::Unsat => Ok(Solution::Unsat),
        SatResult::Unknown => Err(solver_err("the solver returned unknown")),
        SatResult::Sat => {
            let model = solver.get_model().ok_or_else(|| solver_err("sat without a model"))?;
            let mut assignment = BTreeMap::new();
            for name in &names {
                let value = model
                    .eval(&vars[name], true)
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| solver_err(format!("no value for {name} in the model")))?;
                assignment.insert(name.clone(), value);
            }
            Ok(Solution::Sat(assignment))
        }
    }
}

/// Evaluates `constraint` on a concrete assignment. A variable missing from `inputs` makes the constraint false.
pub fn holds(constraint: &Constraint, inputs: &BTreeMap<String, i64>) -> bool {
    let value = |name: &String| inputs.get(name).copied();
    match constraint {
        Constraint::Eq(v, c) => value(v) == Some(*c),
        Constraint::Ne(v, c) => value(v).is_some_and(|x| x != *c),
        Constraint::Gt(v, c) => value(v).is_some_and(|x| x > *c),
        Constraint::Lt(v, c) => value(v).is_some_and(|x| x < *c),
        Constraint::And(parts) => parts.iter().all(|p| holds(p, inputs)),
        Constraint::Or(parts) => parts.iter().any(|p| holds(p, inputs)),
        Constraint::Not(inner) => !holds(inner, inputs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str) -> String {
        name.to_owned()
    }

    #[test]
    fn a_satisfiable_branch_yields_inputs_that_hold() {
        let branch = Constraint::And(vec![Constraint::Gt(var("len"), 8), Constraint::Lt(var("len"), 12)]);
        let Solution::Sat(inputs) = solve(&branch).unwrap() else {
            panic!("the branch is reachable");
        };
        assert!(holds(&branch, &inputs), "the solver's model must satisfy the constraint: {inputs:?}");
    }

    #[test]
    fn an_impossible_branch_is_unsat() {
        let dead = Constraint::And(vec![Constraint::Eq(var("x"), 3), Constraint::Eq(var("x"), 4)]);
        assert_eq!(solve(&dead).unwrap(), Solution::Unsat);
    }

    #[test]
    fn disjunction_and_negation_are_encoded() {
        let either = Constraint::Or(vec![Constraint::Eq(var("mode"), 7), Constraint::Ne(var("mode"), 0)]);
        let not_zero = Constraint::Not(Box::new(Constraint::Eq(var("mode"), 0)));
        let combined = Constraint::And(vec![either, not_zero]);
        let Solution::Sat(inputs) = solve(&combined).unwrap() else {
            panic!("reachable");
        };
        assert!(holds(&combined, &inputs));
        assert_ne!(inputs["mode"], 0);
    }

    #[test]
    fn holds_treats_a_missing_input_as_false() {
        assert!(!holds(&Constraint::Gt(var("x"), 0), &BTreeMap::new()));
    }
}
