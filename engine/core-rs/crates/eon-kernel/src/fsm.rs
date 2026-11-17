//! Task state machine with an explicit transition table (docs/implementation/01, section 4).
//!
//! Every state change goes through `next_state`. Illegal transitions are errors, never silent no-ops,
//! so a corrupted or replayed event cannot move a task somewhere the table does not allow.

use eon_primitives::{EonError, EonResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskState {
    Pending,
    Ready,
    Running,
    Completed,
    Failed,
    Compensating,
    Compensated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskEvent {
    DependenciesMet,
    Start,
    Succeed,
    Fail,
    Rollback,
    CompensationDone,
}

pub const ALL_STATES: [TaskState; 7] = [
    TaskState::Pending,
    TaskState::Ready,
    TaskState::Running,
    TaskState::Completed,
    TaskState::Failed,
    TaskState::Compensating,
    TaskState::Compensated,
];

pub const ALL_EVENTS: [TaskEvent; 6] = [
    TaskEvent::DependenciesMet,
    TaskEvent::Start,
    TaskEvent::Succeed,
    TaskEvent::Fail,
    TaskEvent::Rollback,
    TaskEvent::CompensationDone,
];

/// The complete legal transition table. Anything not listed here is rejected.
pub fn next_state(from: TaskState, event: TaskEvent) -> EonResult<TaskState> {
    use TaskEvent as E;
    use TaskState as S;
    let to = match (from, event) {
        (S::Pending, E::DependenciesMet) => S::Ready,
        (S::Ready, E::Start) => S::Running,
        (S::Running, E::Succeed) => S::Completed,
        (S::Running, E::Fail) => S::Failed,
        (S::Failed, E::Rollback) | (S::Completed, E::Rollback) => S::Compensating,
        (S::Compensating, E::CompensationDone) => S::Compensated,
        _ => {
            return Err(EonError::ValidationError(format!(
                "illegal task transition: {from:?} on {event:?}"
            )));
        }
    };
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_reaches_completed() {
        let mut s = TaskState::Pending;
        for e in [TaskEvent::DependenciesMet, TaskEvent::Start, TaskEvent::Succeed] {
            s = next_state(s, e).unwrap();
        }
        assert_eq!(s, TaskState::Completed);
    }

    #[test]
    fn failure_path_compensates_and_settles() {
        let mut s = TaskState::Pending;
        for e in [TaskEvent::DependenciesMet, TaskEvent::Start, TaskEvent::Fail, TaskEvent::Rollback, TaskEvent::CompensationDone] {
            s = next_state(s, e).unwrap();
        }
        assert_eq!(s, TaskState::Compensated);
    }

    #[test]
    fn the_table_has_exactly_seven_legal_edges() {
        let legal: Vec<(TaskState, TaskEvent)> = ALL_STATES
            .iter()
            .flat_map(|&s| ALL_EVENTS.iter().map(move |&e| (s, e)))
            .filter(|&(s, e)| next_state(s, e).is_ok())
            .collect();
        assert_eq!(legal.len(), 7, "legal edges: {legal:?}");
    }

    #[test]
    fn terminal_states_accept_no_event() {
        for event in ALL_EVENTS {
            assert!(next_state(TaskState::Compensated, event).is_err());
        }
    }

    #[test]
    fn replayed_completion_cannot_restart_a_finished_task() {
        assert!(next_state(TaskState::Completed, TaskEvent::Start).is_err());
        assert!(next_state(TaskState::Completed, TaskEvent::Succeed).is_err());
    }
}
