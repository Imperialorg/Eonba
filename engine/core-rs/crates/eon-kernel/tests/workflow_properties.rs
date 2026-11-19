//! Property tests for the workflow DAG (docs/implementation/01, section 4.2).

use std::collections::BTreeSet;

use eon_kernel::workflow::{Task, Workflow};
use eon_primitives::TaskId;
use proptest::prelude::*;

/// Builds a random DAG where task `i` may only depend on earlier tasks, so it is acyclic by construction.
fn random_dag(edges: &[Vec<bool>]) -> (Workflow, Vec<TaskId>) {
    let ids: Vec<TaskId> = (0..edges.len()).map(|_| TaskId::new()).collect();
    let mut wf = Workflow::new();
    for (i, row) in edges.iter().enumerate() {
        let deps = (0..i).filter(|&j| row.get(j).copied().unwrap_or(false)).map(|j| ids[j].clone());
        wf.insert(Task::new(ids[i].clone(), deps)).unwrap();
    }
    (wf, ids)
}

fn dag_strategy() -> impl Strategy<Value = Vec<Vec<bool>>> {
    prop::collection::vec(prop::collection::vec(any::<bool>(), 0..12), 1..16)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    /// Running the workflow to completion must run every task exactly once and never start a task early.
    #[test]
    fn execution_runs_each_task_once_and_respects_dependencies(edges in dag_strategy()) {
        let (mut wf, ids) = random_dag(&edges);
        let mut completed: BTreeSet<TaskId> = BTreeSet::new();

        loop {
            let ready = wf.ready();
            if ready.is_empty() {
                break;
            }
            for id in &ready {
                let deps = &wf.get(id).unwrap().dependencies;
                prop_assert!(deps.iter().all(|d| completed.contains(d)), "{id} ran before its dependencies");
            }
            for id in ready {
                prop_assert!(completed.insert(id.clone()), "{id} was ready twice");
                wf.complete(&id).unwrap();
            }
        }
        prop_assert_eq!(completed.len(), ids.len(), "some tasks never became ready");
    }

    /// A valid splice keeps the graph acyclic, adds the children, and moves downstream waits onto sinks.
    #[test]
    fn valid_splices_keep_the_graph_acyclic(
        edges in dag_strategy(),
        parent_index in any::<prop::sample::Index>(),
        child_count in 1usize..5,
        child_edges in prop::collection::vec(prop::collection::vec(any::<bool>(), 0..5), 1..5),
    ) {
        let (mut wf, ids) = random_dag(&edges);
        let parent = ids[parent_index.index(ids.len())].clone();
        wf.complete(&parent).unwrap();

        let children: Vec<TaskId> = (0..child_count).map(|_| TaskId::new()).collect();
        let specs: Vec<Task> = children
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let row = child_edges.get(i).cloned().unwrap_or_default();
                let deps = (0..i).filter(|&j| row.get(j).copied().unwrap_or(false)).map(|j| children[j].clone());
                Task::new(id.clone(), deps)
            })
            .collect();

        let downstream: Vec<TaskId> = ids
            .iter()
            .filter(|id| wf.get(id).unwrap().dependencies.contains(&parent))
            .cloned()
            .collect();

        wf.splice_subgraph(&parent, specs).unwrap();
        prop_assert!(wf.validate_topological_order().is_ok());
        for child in &children {
            prop_assert!(wf.get(child).is_some());
        }
        for id in downstream {
            prop_assert!(!wf.get(&id).unwrap().dependencies.contains(&parent));
        }
    }

    /// A splice that would close a cycle must be rejected and leave the workflow byte-for-byte unchanged.
    #[test]
    fn cyclic_splices_leave_the_workflow_untouched(edges in dag_strategy()) {
        let (mut wf, ids) = random_dag(&edges);
        let parent = ids[0].clone();
        wf.complete(&parent).unwrap();
        let before = wf.clone();

        let a = TaskId::new();
        let b = TaskId::new();
        let cyclic = vec![Task::new(a.clone(), [b.clone()]), Task::new(b, [a])];
        prop_assert!(wf.splice_subgraph(&parent, cyclic).is_err());
        prop_assert_eq!(wf, before);
    }
}
