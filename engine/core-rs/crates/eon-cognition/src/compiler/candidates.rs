use eon_kernel::model::Objective;
use eon_primitives::{EonError, EonResult, TaskId};

use super::{
    Attention, Confirmation, ConfirmedPreview, Decision, ReadRequest, ReadWorkspace, StagedPreview,
    compile_words, intent_words, invalid, snapshot,
};

/// Two separately validated single-read DAGs, never an implicit best guess.
pub struct CandidatePreview([StagedPreview; 2]);

/// Only `read file A or B` with two distinct, independently approved paths.
/// Ordinary `compile` still rejects ambiguous input. Both branches must succeed
/// against private CoW views of one sealed capture, or neither is presented.
pub fn preview_candidates(
    objective: &Objective,
    workspace: &ReadWorkspace,
) -> EonResult<CandidatePreview> {
    let words = intent_words(&objective.description)?;
    let [verb, file, a, or, b] = words.as_slice() else {
        return Err(invalid("expected exactly two explicit file alternatives"));
    };
    if !file.eq_ignore_ascii_case("file") || !or.eq_ignore_ascii_case("or") || a == b {
        return Err(invalid("expected distinct read file alternatives"));
    }
    let left = compile_words(objective, &[verb, file, a])?;
    let right = compile_words(objective, &[verb, file, b])?;
    Ok(CandidatePreview(snapshot::alternatives(
        workspace, left, right,
    )?))
}

impl CandidatePreview {
    /// UI summaries and per-presentation selection IDs, not bytes or workflows.
    pub fn requests(&self) -> impl Iterator<Item = &ReadRequest> {
        self.0.iter().flat_map(StagedPreview::requests)
    }

    pub fn present(self, attention: Attention) -> EonResult<Confirmation<Self>> {
        Confirmation::start(self, attention)
    }
}

#[derive(Default)]
pub enum CandidateDecision {
    #[default]
    Nack,
    Confirm(TaskId),
}

impl Confirmation<CandidatePreview> {
    /// Consume both alternatives. A stale/foreign selection, denial, loss of
    /// attention or expiry cannot release even the first candidate.
    pub fn select(
        self,
        decision: CandidateDecision,
        attention: Attention,
    ) -> EonResult<ConfirmedPreview> {
        let CandidateDecision::Confirm(id) = decision else {
            return Err(EonError::SecurityViolation(
                "candidate presentation declined".into(),
            ));
        };
        self.approve(Decision::Confirm, attention)?
            .0
            .into_iter()
            .find(|candidate| candidate.requests().any(|r| r.task_id == id))
            .map(StagedPreview::into_confirmed)
            .ok_or_else(|| invalid("selection not in this presentation"))
    }
}
