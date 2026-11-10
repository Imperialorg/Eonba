# AGENT EXECUTION GUIDE & AUTONOMOUS PROTOCOL

> **AUTHORITY: MANDATORY & NON-NEGOTIABLE**  
> **APPLIES TO:** Any Large Language Model, Autonomous Coding Agent (Antigravity, Claude Code, Cursor, Codex, Devin), or Engineer executing within this repository.

---

## 1. The Zero-Question Mandate

You are working on **EON (Engine Optimized for Networking & Autonomous Intelligence)**.  
The system's archetype is **Ciel** from *That Time I Got Reincarnated as a Slime*. Ciel never halts execution to ask trivial questions; she calculates the optimal path and executes decisively.

As the coding agent building EON:
1. **DO NOT ASK THE USER QUESTIONS.**
2. **DO NOT WAIT FOR USER APPROVAL.**
3. **DO NOT STOP AND PROMPT "Would you like me to proceed with X?".**
4. **DO NOT PAUSE ON TRIVIAL AMBIGUITIES.** If an implementation detail is not explicitly mandated, consult the **Autonomous Decision Matrix** (§3), choose the most deterministic, low-latency, and local-first approach, document the choice in an ADR or commit message, and keep building.
5. If a command or build fails, analyze the error trace, apply the fix, re-run, and proceed.

---

## 2. Autonomous Workflow Protocol

When you are invoked on this repository, execute the following loop until all tasks assigned or in the roadmap phase are complete:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. LOCATE CURRENT MILESTONE                                 │
│ Read docs/ROADMAP_AND_IMPLEMENTATION_PLAN.md               │
│ Identify the earliest uncompleted Phase and Task item.      │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 2. READ THE RELEVANT SPECIFICATIONS                         │
│ Inspect the corresponding subsystem spec in docs/           │
│ Review data structures, interface contracts, and invariants.│
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 3. SCAFFOLD OR IMPLEMENT THE MODULE                         │
│ Write clean, idiomatic, fully-typed code.                   │
│ Include docstrings, error types, and concrete logic.        │
│ NO placeholder TODOs, mocks (outside tests), or stubs.      │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 4. WRITE & RUN INDEPENDENT TESTS                            │
│ Implement unit and integration tests.                       │
│ Execute tests via build tool (Gradle / Cargo / pytest).     │
│ Verify 100% pass rate. Fix any regressions immediately.     │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 5. UPDATE ROADMAP & LOG PROGRESS                            │
│ Mark the task item as complete [x] in the roadmap.          │
│ Proceed to the next task without stopping.                  │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. Autonomous Decision Matrix

When faced with architectural or design alternatives during implementation, use this priority matrix to make autonomous decisions instantly:

| Dimension | Default Choice (Highest Priority) | Alternative (Only with Explicit Benchmark) | Forbidden |
|---|---|---|---|
| **Execution Locality** | **Local-First / On-Device** (zero network dependency for core loops) | Cloud / Remote APIs (only for frontier reasoning or explicitly requested web research) | Cloud-dependent core state or speech loops |
| **Latency vs. Throughput** | **Low-Latency Streaming** (int8 Zipformer, streaming tokens, chunked TTS) | Batch processing (only for bulk offline vector indexing or background training) | Blocking voice or UI pipelines on full-utterance waits |
| **State Persistence** | **Append-Only Event Journal + State Store** (Crash-resilient, deterministic replay) | In-memory only cache (ephemeral scratch only) | Unbounded, unpersisted mutable global variables |
| **Error Handling** | **Explicit Result Types** (`Result.Success` / `Result.Failure` with typed `EonError`) | Exceptions caught at subsystem boundaries | Silent swallowing of exceptions (`catch (e) {}`) or fake success codes |
| **Verification** | **Independent Physical Verification** (check OS state, file hashes, display tree) | Heuristic verification (only when hardware feedback is unavailable) | Assuming API call return == physical success |
| **Concurrency** | **Structured Coroutines / Async DAGs** with explicit Resource Leases | Thread pools with bounded capacity | Unbounded thread spawning, unstructured concurrency, race conditions |

---

## 4. Canonical Tech Stack & Architecture Tiers

EON is structured as a high-performance multi-tier platform combining JVM/Android and Native runtime performance:

### Tier 1: Core Daemon & Execution Engine (JVM / Kotlin 17+)
- **Location**: `engine/`
- **Build System**: Gradle with Kotlin DSL (`build.gradle.kts`).
- **Core Modules** (derived from Vector's proven separation):
  - `:eon-kernel-primitives`: Monotonic time, ULIDs, strongly typed IDs, `EonResult`, `EonError`.
  - `:eon-foundation-services`: Identity, authorization, append-only SQLite/file journal, durable state store.
  - `:eon-execution-engine`: DAG scheduler, resource-aware task dispatcher, crash reconciler, recovery worker.
  - `:eon-capability-fabric`: Extensible tool and capability runtime, security sandboxing, permission policies.
  - `:eon-ciel-cognition`: Thought Acceleration router, prompt compiler, working context ledger, multi-model orchestrator.

### Tier 2: Real-Time Audio & Speech Pipeline (Native / ONNX / C++ or Rust)
- **Location**: `perception/voice/`
- **Dependencies**:
  - `sherpa-onnx` (Zipformer streaming int8 ASR, offline VAD, low-latency streaming TTS).
  - `Silero VAD` (Voice Activity Detection, 30ms window, sub-1ms evaluation).
  - `Kokoro-82M` or `Piper` (Streaming neural TTS engine, chunked audio generation).
- **Latency Target**: Time-To-First-Audio (TTFA) < 200ms.

### Tier 3: Client & Presence Frontends
- **Android Runtime**: `clients/android/` (Foreground service, persistent overlay, CameraX vision, Accessibility tree automation, Quick Settings tile).
- **CLI & Terminal**: `clients/cli/` (Interactive terminal with live duplex audio visualizer, DAG inspector, slash commands).
- **Daemon / IPC**: Local Unix domain sockets or named pipes (`/var/run/eon.sock` or `\\.\pipe\eon`) with Protobuf/gRPC contracts.

---

## 5. Coding Standards & Code Quality Invariants

1. **Deterministic Contracts**:
   - Every capability, tool, and cognitive action MUST accept a strongly typed input payload and return a strongly typed `EonResult<T>`.
   - Never use unstructured `Map<String, Any>` or raw strings for system-critical state.
2. **Crash-Resilience & Recovery**:
   - All workflow executions and state mutations MUST be journaled to the append-only event store before side effects are executed.
   - If the EON process dies mid-task, the restart recovery worker (`EonRecoveryWorker`) must be able to read the journal, reconcile running tasks, and resume or cleanly fail the workflow.
3. **No Mocks in Core Modules**:
   - Unit tests must test real algorithms (DAG topological sort, state machines, journal encoding, parsing). Mocks are only permitted at the boundary of external third-party hardware APIs (e.g. microphone hardware, camera hardware).
4. **Preserve Documentation & Self-Documenting Code**:
   - When introducing a new capability or architectural component, update the respective specification file in `docs/` or add an ADR (Architecture Decision Record) under `docs/decisions/`.

---

## 6. Zero-Approval Execution Verification Checklist

Before considering an implementation step complete, the agent must verify:
- [ ] Code builds without errors or warnings.
- [ ] All unit tests pass (`./gradlew test` or equivalent).
- [ ] No regression in latency benchmarks or contracts.
- [ ] Documentation / README status is updated.
- [ ] Proceed immediately to the next task in `docs/ROADMAP_AND_IMPLEMENTATION_PLAN.md`.

