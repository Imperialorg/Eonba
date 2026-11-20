//! Compile explicit reads against one caller-authorized workspace. Keywords are
//! ASCII case-insensitive, paths are case-sensitive, and `then` sequences reads:
//!
//! ```text
//! read file README.md then read 80 bytes from src/lib.rs at offset 128
//! ```
//!
//! `read file` requests the first 4096 bytes; `at offset` defaults to zero.
//! Unrecognized input is never forwarded to an inference engine, shell or tool.

mod preview;

use eon_kernel::model::{Objective, Task, TaskSpec, TaskType, Workflow};
use eon_kernel::resource::{CanonicalResourceKey, Demand, Requirement, ResourceRank};
use eon_primitives::{EonError, EonResult, TaskId, WorkflowId};
use serde_json::json;

pub use preview::{
    Attention, CONFIRMATION_WINDOW, Confirmation, ConfirmedPreview, Decision, ShadowRead,
    StagedPreview,
};

pub const MAX_INTENT_BYTES: usize = 2048;
pub const MAX_READS: usize = 8;
pub const MAX_READ_BYTES: usize = 4096;

/// A range is resolved against a workspace root supplied by the host, never by the objective.
#[derive(Debug, PartialEq, Eq)]
pub struct ReadRequest {
    pub task_id: TaskId,
    pub path: String,
    pub offset: u64,
    pub length: usize,
}

/// No deserialization or raw-workflow constructor: kernel DAG validation alone
/// does not authorize an arbitrary capability or its parameters.
pub struct CompiledObjective {
    workflow: Workflow,
    reads: Vec<ReadRequest>,
}

impl CompiledObjective {
    pub fn reads(&self) -> &[ReadRequest] {
        &self.reads
    }
}

/// Compile the whole objective or reject it. No partial plan escapes, even if a
/// later clause is unsupported. Only IDs are freshly allocated on each submission.
pub fn compile(objective: &Objective) -> EonResult<CompiledObjective> {
    let intent = &objective.description;
    if intent.len() > MAX_INTENT_BYTES || !intent.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
    {
        return Err(invalid("expected a bounded single-line ASCII intent"));
    }
    let words: Vec<_> = intent.split_ascii_whitespace().collect();
    let mut reads = Vec::new();
    let mut tasks = Vec::new();
    let workflow_id = WorkflowId::new();
    let mut predecessor = None;
    for clause in words.split(|w| w.eq_ignore_ascii_case("then")) {
        if reads.len() >= MAX_READS {
            return Err(invalid("too many read steps"));
        }
        let read = parse_read(clause)?;
        tasks.push(Task::try_from(TaskSpec {
            id: read.task_id.clone(),
            workflow_id: workflow_id.clone(),
            task_type: TaskType::AtomicExecution,
            capability: "workspace.read@v1".into(),
            operation: "read_range".into(),
            parameters: json!({"path": read.path, "offset": read.offset, "length": read.length}),
            dependencies: predecessor.take().into_iter().collect(),
            resource_requirements: vec![Requirement {
                key: CanonicalResourceKey {
                    rank: ResourceRank::FileLock,
                    id: format!("workspace:///{}", read.path),
                },
                demand: Demand::SharedRead,
            }],
            timeout_ms: 1000,
            is_idempotent: true,
        })?);
        predecessor = Some(read.task_id.clone());
        reads.push(read);
    }
    let workflow = Workflow::new(
        workflow_id,
        objective.id.clone(),
        objective.created_at_ns,
        objective.epoch,
        tasks,
    )?;
    Ok(CompiledObjective { workflow, reads })
}

fn invalid(message: &'static str) -> EonError {
    EonError::ValidationError(message.into())
}

fn parse_read(words: &[&str]) -> EonResult<ReadRequest> {
    let [verb, rest @ ..] = words else {
        return Err(invalid("missing read step"));
    };
    if !verb.eq_ignore_ascii_case("read") {
        return Err(invalid("only explicit file reads are supported"));
    }
    let (path, offset, length) = match rest {
        [file, path] if file.eq_ignore_ascii_case("file") => (*path, 0, MAX_READ_BYTES),
        [count, bytes, from, path, tail @ ..]
            if bytes.eq_ignore_ascii_case("bytes") && from.eq_ignore_ascii_case("from") =>
        {
            let length = number(count)?;
            if !(1..=MAX_READ_BYTES as u64).contains(&length) {
                return Err(invalid("byte count exceeds the permitted range"));
            }
            let offset = match tail {
                [] => 0,
                [at, offset, n]
                    if at.eq_ignore_ascii_case("at") && offset.eq_ignore_ascii_case("offset") =>
                {
                    number(n)?
                }
                _ => return Err(invalid("expected an optional byte offset")),
            };
            (*path, offset, length as usize)
        }
        _ => return Err(invalid("unsupported or ambiguous read intent")),
    };
    // Keep the emitted relative path unambiguous on Unix and Windows. No dot
    // components, hidden files, device names, escape sequences or URI schemes.
    if path.len() > 240 || !path.split('/').all(valid_component) {
        return Err(invalid("expected a portable relative workspace file path"));
    }
    Ok(ReadRequest {
        task_id: TaskId::new(),
        path: path.into(),
        offset,
        length,
    })
}

fn valid_component(s: &str) -> bool {
    if !s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        || s.ends_with('.')
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return false;
    }
    let stem = s.split('.').next().unwrap_or_default().to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "NUL" | "AUX" | "PRN")
        && !matches!(
            stem.as_bytes(),
            [b'C', b'O', b'M', b'0'..=b'9'] | [b'L', b'P', b'T', b'0'..=b'9']
        )
}

fn number(s: &str) -> EonResult<u64> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("expected an unsigned decimal number"));
    }
    s.parse()
        .map_err(|_| invalid("invalid byte count or offset"))
}
