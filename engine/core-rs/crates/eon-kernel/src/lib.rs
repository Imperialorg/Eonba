//! EON Execution Kernel: DAG scheduling, resource leases, task state machine, and saga rollback.
pub mod epoch;
pub mod events;
pub mod fsm;
pub mod journal_sink;
pub mod overlay;
pub mod resource;
pub mod saga;
pub mod schedule;
pub mod shadow;
pub mod structural;
pub mod worktree;
pub mod workflow;

pub use eon_primitives::*;
