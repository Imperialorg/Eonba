//! Runs a ready task inside a cognitive epoch (docs/implementation/01, sections 4 and 5).
//!
//! The step stages writes into an epoch. On success the writes commit through the event sink, and then the task
//! completes. A failing step rolls back and leaves the task pending. If the completion record fails after the writes
//! committed, the task stays ready and a later run commits under a newer epoch. The semantics are therefore
//! at-least-once for the step, so steps should be idempotent or keyed by their epoch.

use eon_primitives::{EonError, EonResult, TaskId};

use crate::epoch::{CognitiveEpoch, SharedState};
use crate::events::{complete_recorded, EventSink};
use crate::workflow::Workflow;

pub fn run_task_in_epoch<F>(
    workflow: &mut Workflow,
    state: &mut SharedState,
    task: &TaskId,
    epoch_id: u64,
    sink: &mut dyn EventSink,
    step: F,
) -> EonResult<()>
where
    F: FnOnce(&mut CognitiveEpoch) -> EonResult<()>,
{
    if !workflow.ready().contains(task) {
        return Err(EonError::ValidationError(format!("task {task} is not ready")));
    }
    let mut epoch = state.begin(epoch_id)?;
    if let Err(err) = step(&mut epoch) {
        epoch.rollback();
        return Err(err);
    }
    if let Err(err) = state.commit_recorded(&mut epoch, sink) {
        epoch.rollback();
        return Err(err);
    }
    complete_recorded(workflow, task, sink)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{KernelEvent, VecSink};
    use crate::workflow::Task;

    fn one_task() -> (Workflow, TaskId) {
        let mut wf = Workflow::new();
        let id = TaskId::new();
        wf.insert(Task::new(id.clone(), [])).unwrap();
        (wf, id)
    }

    #[test]
    fn successful_step_commits_then_completes_and_is_recorded_in_order() {
        let (mut wf, id) = one_task();
        let mut state = SharedState::new();
        let mut sink = VecSink::default();
        run_task_in_epoch(&mut wf, &mut state, &id, 1, &mut sink, |epoch| epoch.stage("plan", "draft")).unwrap();

        assert_eq!(state.get("plan"), Some("draft"));
        assert_eq!(state.fence(), 1);
        assert!(matches!(wf.get(&id).unwrap().state, crate::workflow::TaskState::Completed));
        assert!(matches!(sink.events[0], KernelEvent::EpochCommitted { id: 1, .. }));
        assert!(matches!(sink.events[1], KernelEvent::TaskTransition { .. }));
    }

    #[test]
    fn failing_step_rolls_back_and_leaves_the_task_pending() {
        let (mut wf, id) = one_task();
        let mut state = SharedState::new();
        let mut sink = VecSink::default();
        let outcome = run_task_in_epoch(&mut wf, &mut state, &id, 1, &mut sink, |epoch| {
            epoch.stage("plan", "half-written")?;
            Err(EonError::OperationFailed {
                operation: "step".into(),
                reason: "model timed out".into(),
            })
        });

        assert!(outcome.is_err());
        assert_eq!(state.get("plan"), None);
        assert_eq!(state.fence(), 0);
        assert!(sink.events.is_empty());
        assert!(matches!(wf.get(&id).unwrap().state, crate::workflow::TaskState::Pending));
    }

    #[test]
    fn a_task_with_unfinished_dependencies_never_runs_its_step() {
        let mut wf = Workflow::new();
        let parent = TaskId::new();
        let child = TaskId::new();
        wf.insert(Task::new(parent.clone(), [])).unwrap();
        wf.insert(Task::new(child.clone(), [parent])).unwrap();
        let mut state = SharedState::new();
        let mut sink = VecSink::default();
        let mut ran = false;
        let outcome = run_task_in_epoch(&mut wf, &mut state, &child, 1, &mut sink, |_| {
            ran = true;
            Ok(())
        });
        assert!(outcome.is_err());
        assert!(!ran);
    }

    #[test]
    fn a_fenced_epoch_id_is_refused_before_the_step_runs() {
        let (mut wf, id) = one_task();
        let mut state = SharedState::new();
        let mut sink = VecSink::default();
        let mut earlier = state.begin(5).unwrap();
        state.commit(&mut earlier).unwrap();
        let mut ran = false;
        let outcome = run_task_in_epoch(&mut wf, &mut state, &id, 3, &mut sink, |_| {
            ran = true;
            Ok(())
        });
        assert!(outcome.is_err());
        assert!(!ran);
    }

    struct RejectsTransitions(Vec<KernelEvent>);
    impl EventSink for RejectsTransitions {
        fn record(&mut self, event: KernelEvent) -> EonResult<()> {
            if matches!(event, KernelEvent::TaskTransition { .. }) {
                return Err(EonError::OperationFailed {
                    operation: "test.sink".into(),
                    reason: "disk full".into(),
                });
            }
            self.0.push(event);
            Ok(())
        }
    }

    #[test]
    fn completion_failure_after_commit_keeps_the_task_ready_for_a_newer_epoch() {
        let (mut wf, id) = one_task();
        let mut state = SharedState::new();
        let mut sink = RejectsTransitions(Vec::new());
        let outcome = run_task_in_epoch(&mut wf, &mut state, &id, 1, &mut sink, |epoch| epoch.stage("plan", "v1"));
        assert!(outcome.is_err());
        assert_eq!(state.get("plan"), Some("v1"), "writes committed before completion failed");
        assert!(matches!(wf.get(&id).unwrap().state, crate::workflow::TaskState::Pending));

        let mut retry_sink = VecSink::default();
        run_task_in_epoch(&mut wf, &mut state, &id, 2, &mut retry_sink, |epoch| epoch.stage("plan", "v2")).unwrap();
        assert_eq!(state.get("plan"), Some("v2"));
        assert!(matches!(wf.get(&id).unwrap().state, crate::workflow::TaskState::Completed));
    }
}
