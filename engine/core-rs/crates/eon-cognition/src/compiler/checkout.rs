//! Bounded Linux checkouts, not working directories: all writes target private
//! pages of a sealed capture. There is no source descriptor, path lookup, file
//! creation, subprocess, or merge operation on a branch.
use std::ops::Range;

use eon_kernel::model::Objective;
use eon_primitives::{EonResult, TaskId};
use memmap2::MmapMut;

use super::candidates::candidate_plans;
use super::snapshot::{Extent, Sealed, capture};
use super::{
    Attention, CandidateDecision, CompiledObjective, Confirmation, ReadRequest, ReadWorkspace,
    invalid,
};

/// Metadata for the trusted presentation. `id` names this variant, not the
/// underlying read task (two variants can share one task).
pub struct CheckoutChoice {
    pub id: TaskId,
    pub request: ReadRequest,
    /// The proposed fixed-size overwrite in original file coordinates, if any.
    pub overwrite: Option<Range<u64>>,
}

struct Branch {
    choice: CheckoutChoice,
    extent: Extent,
    pages: MmapMut,
}

/// Two independently writable alternatives. Only the trusted host supplies
/// edits; neither the objective nor captured file contents are edit instructions.
pub struct PrivateCheckouts([Branch; 2]);

/// Opaque frozen results. No backing mappings survive into the consent window,
/// even if the UI never responds.
pub struct CheckoutPresentation([SelectedCheckout; 2]);

/// Bytes from the selected private branch, **not** evidence of the original
/// file and not a workflow, a dispatch approval or a patch to apply to the host.
pub struct SelectedCheckout {
    pub choice: CheckoutChoice,
    pub file_len: usize,
    pub bytes: Vec<u8>,
}

/// Opt in to staging private variants of the two existing `read file A or B`
/// candidates. Both paths must pass the same capture checks as read previews.
pub fn checkout_candidates(
    objective: &Objective,
    workspace: &ReadWorkspace,
) -> EonResult<PrivateCheckouts> {
    let [left, right] = candidate_plans(objective)?;
    let [left] = left.single_read()?;
    let [right] = right.single_read()?;
    let (base, extents) = capture(workspace, [&left, &right].into_iter())?;
    let [a, b]: [_; 2] = extents
        .try_into()
        .map_err(|_| invalid("expected two captured alternatives"))?;
    PrivateCheckouts::new(base, [(left, a), (right, b)])
}

impl CompiledObjective {
    pub(super) fn single_read(self) -> EonResult<[ReadRequest; 1]> {
        self.reads
            .try_into()
            .map_err(|_| invalid("expected a single read for a private checkout"))
    }
}

impl PrivateCheckouts {
    pub(super) fn new(base: Sealed, pair: [(ReadRequest, Extent); 2]) -> EonResult<Self> {
        let branch = |(request, extent)| -> EonResult<Branch> {
            Ok(Branch {
                choice: CheckoutChoice {
                    id: TaskId::new(),
                    request,
                    overwrite: None,
                },
                extent,
                pages: base.fork()?,
            })
        };
        let [left, right] = pair;
        Ok(Self([branch(left)?, branch(right)?]))
    }

    pub fn choices(&self) -> impl Iterator<Item = &CheckoutChoice> {
        self.0.iter().map(|branch| &branch.choice)
    }

    /// Stage at most one nonempty literal overwrite per alternative. Offsets
    /// are absolute within the original file, bounded by the bytes actually
    /// captured (including short reads at EOF), never by the full file length.
    pub fn overwrite(&mut self, id: &TaskId, offset: u64, bytes: &[u8]) -> EonResult<()> {
        let branch = self
            .0
            .iter_mut()
            .find(|b| &b.choice.id == id)
            .ok_or_else(|| invalid("unknown checkout choice"))?;
        if bytes.is_empty() || branch.choice.overwrite.is_some() {
            return Err(invalid("expected one nonempty overwrite per checkout"));
        }
        let extent = &mut branch.pages[branch.extent.packed.clone()];
        let start = offset
            .checked_sub(branch.choice.request.offset)
            .and_then(|n| usize::try_from(n).ok());
        let end = offset.checked_add(bytes.len() as u64);
        let target = start
            .and_then(|n| extent.get_mut(n..))
            .and_then(|tail| tail.get_mut(..bytes.len()));
        let (Some(target), Some(end)) = (target, end) else {
            return Err(invalid("overwrite outside captured range"));
        };
        target.copy_from_slice(bytes);
        branch.choice.overwrite = Some(offset..end);
        Ok(())
    }

    /// Freeze both choices. Neither can be edited, re-presented, or inspected
    /// through the window. Drop or cancel it on countermand or attention loss.
    pub fn present(self, attention: Attention) -> EonResult<Confirmation<CheckoutPresentation>> {
        Confirmation::start_with(attention, || {
            CheckoutPresentation(self.0.map(|branch| SelectedCheckout {
                choice: branch.choice,
                bytes: branch.pages[branch.extent.packed].to_vec(),
                file_len: branch.extent.file_len,
            }))
        })
    }
}

impl Confirmation<CheckoutPresentation> {
    /// Consume both alternatives. Only the matching choice can leave the window.
    pub fn select_checkout(
        self,
        decision: CandidateDecision,
        attention: Attention,
    ) -> EonResult<SelectedCheckout> {
        let (pair, id) = self.approve_candidate(decision, attention)?;
        pair.0
            .into_iter()
            .find(|b| b.choice.id == id)
            .ok_or_else(|| invalid("selection not in this presentation"))
    }
}
