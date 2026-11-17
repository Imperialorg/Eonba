//! Durable kernel events: encodes `KernelEvent` into journal payloads and writes them through the group-commit
//! journal (docs/implementation/01, sections 3.1 and 4).
//!
//! Payload format is one line of tab-separated fields: `T<TAB>task<TAB>from<TAB>to`, or
//! `S<TAB>parent<TAB>child,child,...`. The format is versioned by its leading tag so future event kinds can be
//! added without breaking replay.

use eon_foundation::group_commit::GroupCommitJournal;
use eon_primitives::{EonError, EonResult, TaskId};

use crate::events::{EventSink, KernelEvent};
use crate::fsm::TaskState;

fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Pending => "pending",
        TaskState::Ready => "ready",
        TaskState::Running => "running",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::Compensating => "compensating",
        TaskState::Compensated => "compensated",
    }
}

fn parse_state(name: &str) -> EonResult<TaskState> {
    Ok(match name {
        "pending" => TaskState::Pending,
        "ready" => TaskState::Ready,
        "running" => TaskState::Running,
        "completed" => TaskState::Completed,
        "failed" => TaskState::Failed,
        "compensating" => TaskState::Compensating,
        "compensated" => TaskState::Compensated,
        other => return Err(corrupt(format!("unknown state '{other}'"))),
    })
}

fn corrupt(reason: String) -> EonError {
    EonError::InternalError(format!("corrupt kernel event: {reason}"))
}

pub fn encode(event: &KernelEvent) -> Vec<u8> {
    let line = match event {
        KernelEvent::TaskTransition { task, from, to } => {
            format!("T\t{task}\t{}\t{}", state_name(*from), state_name(*to))
        }
        KernelEvent::SubgraphSpliced { parent, added } => {
            let children: Vec<String> = added.iter().map(|id| id.to_string()).collect();
            format!("S\t{parent}\t{}", children.join(","))
        }
    };
    line.into_bytes()
}

pub fn decode(bytes: &[u8]) -> EonResult<KernelEvent> {
    let text = std::str::from_utf8(bytes).map_err(|e| corrupt(e.to_string()))?;
    let fields: Vec<&str> = text.split('\t').collect();
    match fields.as_slice() {
        ["T", task, from, to] => Ok(KernelEvent::TaskTransition {
            task: task.parse::<TaskId>().map_err(|e| corrupt(e.to_string()))?,
            from: parse_state(from)?,
            to: parse_state(to)?,
        }),
        ["S", parent, children] => {
            let added = if children.is_empty() {
                Vec::new()
            } else {
                children
                    .split(',')
                    .map(|id| id.parse::<TaskId>().map_err(|e| corrupt(e.to_string())))
                    .collect::<EonResult<Vec<_>>>()?
            };
            Ok(KernelEvent::SubgraphSpliced {
                parent: parent.parse::<TaskId>().map_err(|e| corrupt(e.to_string()))?,
                added,
            })
        }
        _ => Err(corrupt(format!("unrecognized record with {} fields", fields.len()))),
    }
}

/// Writes each event to the group-commit journal and returns only after the event is fsynced.
pub struct JournalSink<'a> {
    journal: &'a GroupCommitJournal,
}

impl<'a> JournalSink<'a> {
    pub fn new(journal: &'a GroupCommitJournal) -> Self {
        Self { journal }
    }
}

impl EventSink for JournalSink<'_> {
    fn record(&mut self, event: KernelEvent) -> EonResult<()> {
        self.journal.append_durable(encode(&event)).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use eon_foundation::journal::replay;

    use super::*;
    use crate::events::{complete_recorded, splice_recorded};
    use crate::workflow::{Task, Workflow};

    #[test]
    fn every_event_kind_round_trips_through_its_encoding() {
        let a = TaskId::new();
        let b = TaskId::new();
        let events = [
            KernelEvent::TaskTransition { task: a.clone(), from: TaskState::Ready, to: TaskState::Running },
            KernelEvent::SubgraphSpliced { parent: a.clone(), added: vec![b.clone()] },
            KernelEvent::SubgraphSpliced { parent: a.clone(), added: vec![] },
        ];
        for event in events {
            assert_eq!(decode(&encode(&event)).unwrap(), event);
        }
    }

    #[test]
    fn corrupt_records_are_refused() {
        assert!(decode(b"X\tnope").is_err());
        assert!(decode(b"T\ttask_bad\tpending\tcompleted").is_err());
        assert!(decode(b"T\t01HZ\tbogus\tcompleted").is_err());
        assert!(decode(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn completion_and_splice_are_durable_and_replayable() {
        let dir = tempfile::tempdir().unwrap();
        let journal = GroupCommitJournal::spawn(dir.path(), 16).unwrap();
        let mut wf = Workflow::new();
        let parent = TaskId::new();
        let child = TaskId::new();
        wf.insert(Task::new(parent.clone(), [])).unwrap();

        complete_recorded(&mut wf, &parent, &mut JournalSink::new(&journal)).unwrap();
        splice_recorded(
            &mut wf,
            &parent,
            vec![Task::new(child.clone(), [])],
            &mut JournalSink::new(&journal),
        )
        .unwrap();
        drop(journal);

        let decoded: Vec<KernelEvent> = replay(dir.path())
            .unwrap()
            .into_iter()
            .map(|frame| decode(&frame.payload).unwrap())
            .collect();
        assert_eq!(
            decoded,
            vec![
                KernelEvent::TaskTransition {
                    task: parent.clone(),
                    from: TaskState::Pending,
                    to: TaskState::Completed,
                },
                KernelEvent::SubgraphSpliced { parent, added: vec![child] },
            ]
        );
    }
}
