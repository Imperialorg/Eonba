# ADR-0003: Adoption of Vector Deterministic DAG Execution Kernel

## Status
Accepted

## Context
When cognitive agents attempt multi-step operations without a deterministic execution engine, common failures occur:
- Race conditions on shared files or resources.
- Circular wait deadlocks when multiple sub-agents compete for locks.
- Abandoned state and orphaned tasks when the agent crashes or power is lost.
- Hallucinated success ("API returned 200, so the database must be updated") without physical verification.

Vector (Generation 2) developed a battle-tested deterministic execution architecture built around strongly-typed primitives, DAG scheduling, atomic resource leases, an append-only event journal, and a crash recovery reconciler.

## Decision
1. We adopt the Vector Generation 2 architectural foundation for EON's `engine/` subsystem.
2. We enforce strict downward dependency levels (Levels 0 through 5).
3. All multi-step goals are compiled into acyclic `Workflow` graphs before dispatch.
4. All resources (CPU cores, Memory MB, File Lock URIs, Network bandwidth slots) are acquired atomically via two-phase leases before task execution begins.
5. All state transitions are committed to an append-only event journal on disk before side effects occur.
6. A restart recovery worker (`LocalExecutionRecoveryWorker`) scans the journal on boot to reconcile interrupted tasks and safely roll back unverified mutations.
7. Verification is an independent, first-class requirement. A task cannot be marked `COMPLETED` without physical ground truth proof.

## Consequences
- EON can execute complex 20+ step workflows reliably across reboots without corrupting user state.
- Zero race conditions or deadlocks during parallel processing.
- Clean separation between cognition (which formulates the DAG) and the kernel (which deterministically executes it).

