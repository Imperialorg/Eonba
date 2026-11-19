//! Saga execution with reverse compensation (docs/implementation/01, section 4).
//!
//! Steps run in order. On the first failure, every completed step is compensated in reverse order.
//! A failing compensation is recorded and the remaining compensations still run, so one bad undo
//! cannot strand the rest of the rollback.

use eon_primitives::{EonError, EonResult};

pub type Action = Box<dyn FnMut() -> EonResult<()> + Send>;

pub struct Step {
    pub name: String,
    pub action: Action,
    pub compensate: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SagaOutcome {
    Completed,
    Compensated {
        failed_step: String,
        cause: String,
        compensation_errors: Vec<String>,
    },
}

pub fn run_saga(mut steps: Vec<Step>) -> SagaOutcome {
    let mut completed: Vec<usize> = Vec::new();
    for index in 0..steps.len() {
        let result: EonResult<()> = (steps[index].action)();
        match result {
            Ok(()) => completed.push(index),
            Err(err) => {
                let compensation_errors = completed
                    .iter()
                    .rev()
                    .filter_map(|&i| (steps[i].compensate)().err().map(|e| format!("{}: {e}", steps[i].name)))
                    .collect();
                return SagaOutcome::Compensated {
                    failed_step: steps[index].name.clone(),
                    cause: describe(&err),
                    compensation_errors,
                };
            }
        }
    }
    SagaOutcome::Completed
}

fn describe(err: &EonError) -> String {
    err.to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    type Log = Arc<Mutex<Vec<String>>>;

    fn step(name: &str, log: &Log, fail_action: bool, fail_comp: bool) -> Step {
        let (a_log, c_log) = (log.clone(), log.clone());
        let (a_name, c_name) = (name.to_string(), name.to_string());
        Step {
            name: name.to_string(),
            action: Box::new(move || {
                a_log.lock().unwrap().push(format!("do:{a_name}"));
                if fail_action {
                    Err(EonError::OperationFailed { operation: a_name.clone(), reason: "boom".into() })
                } else {
                    Ok(())
                }
            }),
            compensate: Box::new(move || {
                c_log.lock().unwrap().push(format!("undo:{c_name}"));
                if fail_comp {
                    Err(EonError::InternalError("undo failed".into()))
                } else {
                    Ok(())
                }
            }),
        }
    }

    #[test]
    fn all_steps_succeed_without_compensation() {
        let log: Log = Arc::default();
        let outcome = run_saga(vec![step("a", &log, false, false), step("b", &log, false, false)]);
        assert_eq!(outcome, SagaOutcome::Completed);
        assert_eq!(*log.lock().unwrap(), vec!["do:a", "do:b"]);
    }

    #[test]
    fn failure_compensates_completed_steps_in_reverse_and_skips_the_rest() {
        let log: Log = Arc::default();
        let outcome = run_saga(vec![
            step("a", &log, false, false),
            step("b", &log, false, false),
            step("c", &log, true, false),
            step("d", &log, false, false),
        ]);
        assert_eq!(
            outcome,
            SagaOutcome::Compensated {
                failed_step: "c".into(),
                cause: "Operation 'c' failed: boom".into(),
                compensation_errors: vec![],
            }
        );
        assert_eq!(*log.lock().unwrap(), vec!["do:a", "do:b", "do:c", "undo:b", "undo:a"]);
    }

    #[test]
    fn a_failing_compensation_does_not_stop_the_rest_of_the_rollback() {
        let log: Log = Arc::default();
        let outcome = run_saga(vec![
            step("a", &log, false, false),
            step("b", &log, false, true),
            step("c", &log, true, false),
        ]);
        let SagaOutcome::Compensated { compensation_errors, .. } = outcome else {
            panic!("expected compensation");
        };
        assert_eq!(compensation_errors, vec!["b: Internal error: undo failed".to_string()]);
        assert!(log.lock().unwrap().contains(&"undo:a".to_string()), "a must still be undone");
    }

    #[test]
    fn first_step_failure_has_nothing_to_compensate() {
        let log: Log = Arc::default();
        let outcome = run_saga(vec![step("a", &log, true, false), step("b", &log, false, false)]);
        assert!(matches!(outcome, SagaOutcome::Compensated { compensation_errors, .. } if compensation_errors.is_empty()));
        assert_eq!(*log.lock().unwrap(), vec!["do:a"]);
    }
}
