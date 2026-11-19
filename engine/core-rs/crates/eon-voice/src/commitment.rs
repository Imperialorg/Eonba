//! Reply-scoped synthesis on the worker thread. Only fixed, cached bridges may
//! be heard before the kernel confirms a lease for this epoch.

use std::collections::VecDeque;

use eon_primitives::{EonResult, LeaseId};

use crate::barge_in::BargeIn;
use crate::tts::{KokoroTts, split_clauses};

#[derive(Clone, Copy)]
pub enum Bridge {
    Affirmative,
    Report,
    Notice,
    Negative,
    Proposal,
}

pub struct BridgeCache {
    pcm: [Vec<f32>; 5],
}

impl BridgeCache {
    /// Run once during initialization, not on the turn/latency measurement path.
    pub fn warm(tts: &mut KokoroTts, speed: f32) -> EonResult<Self> {
        let mut pcm = [const { Vec::new() }; 5];
        for (samples, text) in pcm
            .iter_mut()
            .zip(["Affirmative.", "Report:", "Notice:", "Negative.", "Proposal:"])
        {
            *samples = tts.synthesize(text, speed)?;
        }
        Ok(Self { pcm })
    }
}

enum State {
    Waiting,
    Committed(LeaseId),
    Aborted,
}

pub struct CommitmentGate {
    epoch: u64,
    state: State,
    pending: VecDeque<String>,
    playback: BargeIn,
}

impl CommitmentGate {
    pub fn new(epoch: u64, playback: BargeIn) -> Self {
        Self {
            epoch,
            state: State::Waiting,
            pending: VecDeque::new(),
            playback,
        }
    }

    pub fn bridge<'a>(&self, cache: &'a BridgeCache, bridge: Bridge) -> Option<&'a [f32]> {
        if self.playback.is_muted()
            || (matches!(self.state, State::Aborted) && !matches!(bridge, Bridge::Negative | Bridge::Notice))
        {
            return None;
        }
        Some(&cache.pcm[bridge as usize])
    }

    /// Free text is always a claim, even when it happens to start like a bridge.
    pub fn queue(&mut self, text: &str) {
        if !self.playback.is_muted() && !matches!(self.state, State::Aborted) {
            self.pending.extend(split_clauses(text));
        }
    }

    /// Called by the owner on a trusted kernel event, never from a model's text.
    pub fn lease_acquired(&mut self, epoch: u64, lease: LeaseId) {
        if epoch == self.epoch && matches!(self.state, State::Waiting) {
            self.state = State::Committed(lease);
        }
    }

    pub fn lease(&self) -> Option<&LeaseId> {
        match &self.state {
            State::Committed(lease) => Some(lease),
            _ => None,
        }
    }

    pub fn invalidate(&mut self, epoch: u64) {
        if epoch == self.epoch {
            self.state = State::Aborted;
            self.pending.clear();
        }
    }

    fn ready(&self) -> Option<&str> {
        if self.lease().is_none() || self.playback.is_muted() {
            return None;
        }
        self.pending.front().map(String::as_str)
    }

    /// Processes at most one clause, allowing lease/epoch events between calls.
    /// A model run already in flight at barge-in cannot publish its samples.
    pub fn synthesize_next(&mut self, tts: &mut KokoroTts, speed: f32) -> EonResult<Option<Vec<f32>>> {
        let Some(text) = self.ready() else {
            return Ok(None);
        };
        let samples = tts.synthesize(text, speed)?;
        self.pending.pop_front();
        Ok((!self.playback.is_muted()).then_some(samples))
    }
}

#[cfg(test)]
mod commitment_gate_tests {
    use super::*;

    #[test]
    fn a_claim_requires_the_current_lease_and_cannot_survive_invalidation() {
        let mut gate = CommitmentGate::new(4, BargeIn::new());
        gate.queue("Affirmative. I patched it.");
        assert!(gate.ready().is_none());
        gate.lease_acquired(3, LeaseId::new());
        assert!(gate.ready().is_none());
        gate.lease_acquired(4, LeaseId::new());
        assert_eq!(gate.ready(), Some("Affirmative."));
        gate.invalidate(4);
        gate.lease_acquired(4, LeaseId::new());
        assert!(gate.ready().is_none());
        assert!(gate.pending.is_empty());
    }

    #[test]
    fn interrupted_synthesis_cannot_dequeue_another_claim() {
        let signal = BargeIn::new();
        let mut gate = CommitmentGate::new(3, signal.clone());
        gate.queue("Deployed.");
        gate.lease_acquired(3, LeaseId::new());
        assert_eq!(gate.ready(), Some("Deployed."));
        signal.trigger();
        assert!(gate.ready().is_none());
    }
}
