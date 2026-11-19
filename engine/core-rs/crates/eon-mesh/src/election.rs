//! Clock-skew-immune spatial election (docs/implementation/05, section 2).
//!
//! Devices announce a claim with a score. The election uses only the local monotonic time at which each
//! claim arrives, never a timestamp the sender supplies, so no sender clock can win or lose the election.
//! The window is 15 ms from the first claim. The incumbent keeps the role unless a challenger beats its score
//! by a margin, which stops two near-equal devices from trading the role back and forth.

use std::collections::BTreeMap;

pub const WINDOW_NS: u64 = 15_000_000;
/// Challenger must beat the incumbent by this many score points to take over.
pub const HYSTERESIS_MARGIN: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Winner {
    pub node_id: String,
    pub score: u32,
}

#[derive(Debug, Default)]
pub struct SpatialElection {
    window_start_ns: Option<u64>,
    claims: BTreeMap<String, u32>,
    incumbent: Option<Winner>,
}

impl SpatialElection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a claim that arrived at local monotonic time `arrived_ns`.
    /// Claims after the window closes are ignored; `poll` must run before they are counted in the next round.
    pub fn on_claim(&mut self, node_id: &str, score: u32, arrived_ns: u64) {
        match self.window_start_ns {
            None => {
                self.window_start_ns = Some(arrived_ns);
                self.claims.insert(node_id.to_string(), score);
            }
            Some(start) if arrived_ns.saturating_sub(start) < WINDOW_NS => {
                let entry = self.claims.entry(node_id.to_string()).or_insert(score);
                *entry = (*entry).max(score);
            }
            Some(_) => {}
        }
    }

    /// Closes the window once `now_ns` is at least 15 ms past its start, then decides the winner.
    pub fn poll(&mut self, now_ns: u64) -> Option<Winner> {
        let start = self.window_start_ns?;
        if now_ns.saturating_sub(start) < WINDOW_NS {
            return None;
        }
        let best = self
            .claims
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(id, score)| Winner {
                node_id: id.clone(),
                score: *score,
            });
        self.window_start_ns = None;
        self.claims.clear();
        let decided = match (&self.incumbent, best) {
            (_, None) => None,
            (Some(current), Some(challenger)) if current.node_id != challenger.node_id => {
                if challenger.score >= current.score.saturating_add(HYSTERESIS_MARGIN) {
                    Some(challenger)
                } else {
                    Some(current.clone())
                }
            }
            (_, Some(winner)) => Some(winner),
        };
        self.incumbent = decided.clone();
        decided
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn highest_score_inside_the_window_wins() {
        let mut e = SpatialElection::new();
        e.on_claim("phone", 40, 0);
        e.on_claim("desktop", 90, 5 * MS);
        e.on_claim("laptop", 60, 9 * MS);
        assert_eq!(e.poll(15 * MS).unwrap().node_id, "desktop");
    }

    #[test]
    fn claims_after_the_window_are_ignored() {
        let mut e = SpatialElection::new();
        e.on_claim("phone", 40, 0);
        e.on_claim("desktop", 90, 15 * MS);
        assert_eq!(e.poll(15 * MS).unwrap().node_id, "phone");
    }

    #[test]
    fn no_decision_before_the_window_closes() {
        let mut e = SpatialElection::new();
        e.on_claim("phone", 40, 0);
        assert!(e.poll(14 * MS).is_none());
    }

    #[test]
    fn ties_break_on_node_id_deterministically() {
        let mut e = SpatialElection::new();
        e.on_claim("zeta", 50, 0);
        e.on_claim("alpha", 50, MS);
        assert_eq!(e.poll(15 * MS).unwrap().node_id, "alpha");
    }

    #[test]
    fn incumbent_is_kept_against_a_near_equal_challenger() {
        let mut e = SpatialElection::new();
        e.on_claim("desktop", 80, 0);
        e.poll(15 * MS);
        e.on_claim("laptop", 85, 20 * MS);
        assert_eq!(e.poll(35 * MS).unwrap().node_id, "desktop", "only +5 is below the margin");
    }

    #[test]
    fn a_clear_challenger_takes_over() {
        let mut e = SpatialElection::new();
        e.on_claim("desktop", 80, 0);
        e.poll(15 * MS);
        e.on_claim("laptop", 95, 20 * MS);
        assert_eq!(e.poll(35 * MS).unwrap().node_id, "laptop");
    }

    #[test]
    fn an_empty_round_leaves_the_incumbent_in_place() {
        let mut e = SpatialElection::new();
        e.on_claim("desktop", 80, 0);
        e.poll(15 * MS);
        assert!(e.poll(100 * MS).is_none());
        e.on_claim("laptop", 70, 200 * MS);
        assert_eq!(e.poll(215 * MS).unwrap().node_id, "desktop");
    }
}
