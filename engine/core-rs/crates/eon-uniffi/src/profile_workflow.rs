use std::time::Instant;

use eon_cognition::profile::ModelProfile;
use eon_kernel::events::{KernelEvent, VecSink};
use eon_kernel::fsm::TaskState;
use eon_kernel::model::{Task, TaskSpec, TaskType, Workflow};
use eon_kernel::resource::ResourceManager;
use eon_kernel::scheduler::{Dispatch, Outcome, TaskScheduler};
use eon_primitives::{ObjectiveId, TaskId, WorkflowId};

use crate::{BridgeError, invalid};

#[derive(Debug)]
pub struct MobileWorkflowReceipt {
    pub profile: String,
    pub completed_tasks: u32,
}

fn unavailable(reason: impl ToString) -> BridgeError {
    BridgeError::WorkflowUnavailable {
        reason: reason.to_string(),
    }
}

fn submit(task_id: &TaskId, memory_gb: f64) -> Result<TaskScheduler<VecSink>, BridgeError> {
    const EPOCH: u64 = 1; // Each call owns a fresh workflow and sink.
    let memory = serde_json::Number::from_f64(memory_gb)
        .ok_or_else(|| invalid("device memory must be finite"))?;
    let workflow_id = WorkflowId::new();
    let task = Task::try_from(TaskSpec {
        id: task_id.clone(),
        workflow_id: workflow_id.clone(),
        task_type: TaskType::AtomicExecution,
        capability: "diagnostics".into(),
        operation: "select_model_profile".into(),
        parameters: serde_json::json!({ "memory_gb": memory }),
        dependencies: Default::default(),
        resource_requirements: vec![],
        timeout_ms: 5_000,
        is_idempotent: true,
    })
    .map_err(unavailable)?;
    let workflow =
        Workflow::new(workflow_id, ObjectiveId::new(), 0, EPOCH, [task]).map_err(unavailable)?;
    TaskScheduler::new(workflow, EPOCH, ResourceManager::new(), VecSink::default())
        .map_err(unavailable)
}

fn diagnose(permit: &Dispatch) -> Result<ModelProfile, BridgeError> {
    let memory_gb = permit.contract().parameters["memory_gb"]
        .as_f64()
        .ok_or_else(|| invalid("expected numeric memory_gb"))?;
    let profile =
        ModelProfile::select_for_memory_gb(memory_gb).map_err(|e| invalid(e.to_string()))?;
    // Verify the output against the profile table before acknowledging success.
    if memory_gb < profile.min_device_memory_gb()
        || ModelProfile::ALL
            .iter()
            .any(|&p| p > profile && p.min_device_memory_gb() <= memory_gb)
    {
        return Err(unavailable("selected profile does not match device memory"));
    }
    Ok(profile)
}

fn run_task(
    scheduler: &mut TaskScheduler<VecSink>,
    task: &TaskId,
) -> Result<MobileWorkflowReceipt, BridgeError> {
    let permits = scheduler.tick(Instant::now()).map_err(unavailable)?;
    let [permit] = permits.as_slice() else {
        return Err(unavailable("expected one diagnostic dispatch"));
    };
    if permit.task() != task || !permit.is_valid() {
        return Err(unavailable("missing valid diagnostic lease"));
    }
    let result = diagnose(permit);
    let outcome = if result.is_ok() {
        Outcome::Succeeded
    } else {
        Outcome::Failed
    };
    scheduler
        .finish(permit, outcome, Instant::now())
        .map_err(unavailable)?;
    let profile = result?;
    let completed = scheduler.sink().events.iter().filter(|event| matches!(
        event, KernelEvent::TaskTransition { task: id, from: TaskState::Running, to: TaskState::Completed }
        if id == task
    )).count();
    if completed != 1 || scheduler.state(task) != Some(TaskState::Completed) {
        return Err(unavailable("missing unique diagnostic completion"));
    }
    Ok(MobileWorkflowReceipt {
        profile: profile.lock_key().into(),
        completed_tasks: completed as u32,
    })
}

/// Run one fixed, permissionless diagnostic. All task state and evidence die with this call.
pub fn run_profile_workflow(memory_gb: f64) -> Result<MobileWorkflowReceipt, BridgeError> {
    let task = TaskId::new();
    run_task(&mut submit(&task, memory_gb)?, &task)
}

#[cfg(test)]
mod codegen_tests {
    use super::*;

    #[test]
    fn receipts_are_local_to_each_submission_and_use_the_dispatched_input() {
        for (gb, profile) in [
            (12.0, "p2_workstation_16g"),
            (4.0, "p0_mobile_ultralight"),
            (16.0, "p3_workstation_heavy"),
            (8.0, "p1_mobile_flagship"),
        ] {
            let receipt = run_profile_workflow(gb).unwrap();
            assert_eq!(receipt.profile, profile);
            assert_eq!(receipt.completed_tasks, 1);
        }
    }

    #[test]
    fn failed_diagnostic_records_failure_not_completion() {
        let task = TaskId::new();
        let mut scheduler = submit(&task, -1.0).unwrap();
        assert!(matches!(
            run_task(&mut scheduler, &task),
            Err(BridgeError::InvalidInput { .. })
        ));
        assert_eq!(scheduler.state(&task), Some(TaskState::Failed));
        assert!(matches!(
            scheduler.sink().events.last(),
            Some(KernelEvent::TaskTransition { task: id, from: TaskState::Running, to: TaskState::Failed })
                if id == &task
        ));
        for gb in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                run_profile_workflow(gb),
                Err(BridgeError::InvalidInput { .. })
            ));
        }
    }

    #[test]
    fn completed_task_cannot_issue_a_second_receipt() {
        let task = TaskId::new();
        let mut scheduler = submit(&task, 4.0).unwrap();
        run_task(&mut scheduler, &task).unwrap();
        assert!(matches!(
            run_task(&mut scheduler, &task),
            Err(BridgeError::WorkflowUnavailable { .. })
        ));
    }
}
