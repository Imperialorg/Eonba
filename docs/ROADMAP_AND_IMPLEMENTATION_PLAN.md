# ROADMAP AND IMPLEMENTATION PLAN

> **MANDATORY EXECUTION DIRECTIVE FOR CODING AGENTS:**  
> When assigned to implement EON, find the first unchecked item `[ ]` in this document. Implement the specified classes, structs, traits, and functions, write and pass the corresponding unit tests, mark the checkbox `[x]`, and proceed to the next item immediately **without asking any questions or waiting for user approval**.
> Follow the architecture defined in [docs/TECH_STACK_AND_TOOLCHAINS.md](file:///d:/Projects/EON%20%E2%80%94%20Engine%20Optimized%20for%20Netwotking/docs/TECH_STACK_AND_TOOLCHAINS.md) (Rust Native Core + Kotlin Android Presence).

---

## Phase 0: Native Workspace Scaffolding & Kernel Primitives (Level 0)

- [x] **Task 0.1: Cargo Multi-Crate Workspace Setup**
  - **Directory**: `engine/core-rs/`
  - **Files**: `Cargo.toml`, `.cargo/config.toml`
  - **Crates**:
    - `crates/eon-primitives`
    - `crates/eon-foundation`
    - `crates/eon-kernel`
    - `crates/eon-capability`
    - `crates/eon-cyber`
    - `crates/eon-mesh`
    - `crates/eon-voice`
    - `crates/eon-cognition`
    - `crates/eon-uniffi`
  - **Acceptance Criteria**: `cargo check --workspace` compiles with zero warnings across all crates. (Verified 2026-11-10)

- [x] **Task 0.2: Strongly Typed Identifiers (ULID)**
  - **Crate**: `eon-primitives`
  - **Files**: `src/id.rs`, `src/lib.rs`
  - **Requirements**: Enforce prefix conventions (`obj_<ULID>`, `task_<ULID>`, `wf_<ULID>`, `lease_<ULID>`). Implement string parsing, regex validation, and monotonic sortability.
  - **Unit Tests**: `cargo test -p eon-primitives id_tests` verifying ULID generation, prefix validation, and byte ordering. (Verified 2026-11-10)

- [x] **Task 0.3: Monotonic Time & Result Types**
  - **Crate**: `eon-primitives`
  - **Files**: `src/time.rs`, `src/result.rs`, `src/error.rs`
  - **Requirements**: `EonResult<T>` with standard typed `EonError` enums. Nanosecond-resolution monotonic clock independent of wall-clock drift.
  - **Unit Tests**: `cargo test -p eon-primitives time_and_result_tests`. (Verified 2026-11-10)

---

## Phase 1: Foundation Services & Durable Journaling (Level 1)

- [ ] **Task 1.1: Append-Only Event Journal**
  - **Crate**: `eon-foundation`
  - **Files**: `src/journal.rs`, `src/codec.rs`, `src/lib.rs`
  - **Requirements**: Append-only binary / NDJSON journal on disk with atomic sequence numbering (`u64`). Crash-safe file sync (`flush` / `sync_all`).
  - **Unit Tests**: `cargo test -p eon-foundation journal_tests` verifying append, sequential reads, corruption detection, and crash simulation.

- [ ] **Task 1.2: Durable State Store**
  - **Crate**: `eon-foundation`
  - **Files**: `src/state_store.rs`, `src/sqlite_backend.rs`
  - **Requirements**: Embedded SQLite / redb store with WAL mode. Atomic transactions for snapshot creation and state commits.
  - **Unit Tests**: `cargo test -p eon-foundation state_store_tests` testing concurrent reads, transactional writes, and rollback.

- [ ] **Task 1.3: Identity & Authorization Model**
  - **Crate**: `eon-foundation`
  - **Files**: `src/identity.rs`, `src/permissions.rs`
  - **Requirements**: Ownership identities (System, User, EphemeralAgent). Permission sets (`READ_WORKSPACE`, `MUTATE_WORKSPACE`, `EXECUTE_PROCESS`, `CYBER_AUDIT`, `NETWORK_ACCESS`, `DEVICE_CONTROL`).
  - **Unit Tests**: `cargo test -p eon-foundation identity_tests`.

---

## Phase 2: Execution Foundations & Resource Management (Level 2)

- [ ] **Task 2.1: Finite State Machine Primitives**
  - **Crate**: `eon-kernel`
  - **Files**: `src/fsm.rs`, `src/transition.rs`
  - **Requirements**: Generic deterministic state machine engine validating allowed transitions and emitting transition events.
  - **Unit Tests**: `cargo test -p eon-kernel fsm_tests` verifying transition validation, rejection of illegal states, and listener triggers.

- [ ] **Task 2.2: Resource Models & Atomic Leases**
  - **Crate**: `eon-kernel`
  - **Files**: `src/resource.rs`, `src/lease.rs`, `src/resource_manager.rs`
  - **Requirements**: Resource accounting for CPU permits, Memory MB, File Lock URIs, and Network slots. Two-phase atomic acquisition algorithm (all-or-nothing). Lease expiration timeouts.
  - **Unit Tests**: `cargo test -p eon-kernel resource_manager_tests` verifying deadlock prevention, shared vs exclusive locks, and expiration reclamation.

---

## Phase 3: Vector DAG Execution Engine & Crash Recovery (Level 3)

- [ ] **Task 3.1: Canonical Objective, Workflow & Task Models**
  - **Crate**: `eon-kernel`
  - **Files**: `src/model/objective.rs`, `src/model/workflow.rs`, `src/model/task.rs`
  - **Requirements**: Immutable data models. Topological acyclic DAG validation in `Workflow` constructor (Kahn's algorithm).
  - **Unit Tests**: `cargo test -p eon-kernel dag_validation_tests` verifying cycle detection, orphan task detection, and topological ordering.

- [ ] **Task 3.2: Resource-Aware Task Scheduler with Epoch Leases**
  - **Crate**: `eon-kernel`
  - **Files**: `src/scheduler.rs`, `src/epoch.rs`
  - **Requirements**: Analyzes DAG task dependencies; dispatches tasks with verified epoch tokens and resource leases.
  - **Unit Tests**: `cargo test -p eon-kernel scheduler_tests` testing parallel multi-branch execution and dependency gating.

- [ ] **Task 3.3: Deterministic Task Dispatcher & Saga Reconciler**
  - **Crate**: `eon-kernel`
  - **Files**: `src/dispatcher.rs`, `src/saga.rs`, `src/engine.rs`
  - **Requirements**: Coordinates task dispatch. Emits `EPOCH_INVALIDATED` interrupts on user countermands, executing reverse-order compensating rollbacks.
  - **Unit Tests**: `cargo test -p eon-kernel saga_compensation_tests`.

- [ ] **Task 3.4: Crash Recovery Reconciler (`LocalExecutionRecoveryWorker`)**
  - **Crate**: `eon-kernel`
  - **Files**: `src/recovery.rs`
  - **Requirements**: Scans event journal on boot. Identifies interrupted tasks. Releases stale leases. Re-queues idempotent tasks or rolls back unverified writes.
  - **Unit Tests**: `cargo test -p eon-kernel recovery_tests` simulating mid-execution termination and verifying reconciliation.

---

## Phase 4: Capability Fabric & Sandboxed Tool Runtime (Level 4)

- [ ] **Task 4.1: Capability Registry & Sandboxing**
  - **Crate**: `eon-capability`
  - **Files**: `src/capability.rs`, `src/registry.rs`, `src/sandbox.rs`
  - **Requirements**: Dynamic capability registration, schema verification, bounded memory output wrappers, and execution timeouts.
  - **Unit Tests**: `cargo test -p eon-capability registry_tests`.

- [ ] **Task 4.2: Local Workspace & Process Providers**
  - **Crate**: `eon-capability`
  - **Files**: `src/providers/workspace.rs`, `src/providers/process.rs`
  - **Requirements**: Range reads, atomic temp-file writes, SHA-256 digest validation, pre-write journal snapshots.
  - **Unit Tests**: `cargo test -p eon-capability workspace_tests`, `process_tests`.

- [ ] **Task 4.3: Network & Web Research Providers**
  - **Crate**: `eon-capability`
  - **Files**: `src/providers/http.rs`, `src/providers/download.rs`, `src/providers/scraper.rs`
  - **Requirements**: Bounded HTTP downloads, readability text extraction, zero external cloud model dependence.
  - **Unit Tests**: `cargo test -p eon-capability network_tests`.

---

## Phase 5: Low-Latency Duplex Voice Engine (STT & TTS)

- [ ] **Task 5.1: Silero VAD & Audio Ring Buffer**
  - **Crate**: `eon-voice`
  - **Files**: `src/vad.rs`, `src/ring_buffer.rs`, `src/audio_device.rs`
  - **Requirements**: Native CPAL / AAudio / WASAPI audio driver loop. 30ms window ONNX evaluation, confidence scoring, 250ms dynamic end-of-utterance detection.
  - **Unit Tests**: `cargo test -p eon-voice vad_tests` verifying speech detection latency < 1ms per frame.

- [ ] **Task 5.2: Streaming Zipformer int8 ASR with Predictive Pre-Cognition**
  - **Crate**: `eon-voice`
  - **Files**: `src/asr.rs`, `src/pre_cognition.rs`
  - **Requirements**: Streams 16kHz audio chunks to `sherpa-onnx` C++ bindings, emits partial tokens incrementally, runs early-exit intent classifier during user speech.
  - **Unit Tests**: `cargo test -p eon-voice asr_tests`.

- [ ] **Task 5.3: Kokoro-82M Streaming Neural TTS & Acoustic Commitment Gate**
  - **Crate**: `eon-voice`
  - **Files**: `src/tts.rs`, `src/chunker.rs`, `src/commitment_gate.rs`
  - **Requirements**: Emits acoustic bridges immediately; gates definitive semantic claims until lease acquisition. TTFA < 170ms, RTF < 0.15.
  - **Unit Tests**: `cargo test -p eon-voice tts_tests`, `commitment_gate_tests`.

- [ ] **Task 5.4: Acoustic Reflex & Full Duplex Barge-In**
  - **Crate**: `eon-voice`
  - **Files**: `src/barge_in.rs`, `src/coordinator.rs`
  - **Requirements**: Detects user voice while TTS is playing; aborts audio output driver in < 5ms; logs `INTERRUPTED_BY_USER`.
  - **Unit Tests**: `cargo test -p eon-voice barge_in_tests` verifying instantaneous buffer flush.

---

## Phase 6: Ciel Cognitive Core & Thought Acceleration (Level 5)

- [ ] **Task 6.1: 3-Tier Cognitive Router (Thought Acceleration)**
  - **Crate**: `eon-cognition`
  - **Files**: `src/router.rs`, `src/reflex.rs`, `src/tactical_slm.rs`, `src/strategic_frontier.rs`
  - **Requirements**: Route latency < 20ms for Tier 0 reflex, < 120ms for Tier 1 local SLM. Non-blocking asynchronous Tier 2 invocation.
  - **Unit Tests**: `cargo test -p eon-cognition router_tests`.

- [ ] **Task 6.2: Appraisal Engine & Hybrid Cognitive Memory**
  - **Crate**: `eon-cognition`
  - **Files**: `src/appraisal.rs`, `src/context_ledger.rs`, `src/graph_memory.rs`, `src/vector_store.rs`
  - **Requirements**: Working Context Ledger (bounded to 4k tokens), SQLite entity/relation graph store, local vector embedding search (`all-MiniLM-L6-v2` ONNX).
  - **Unit Tests**: `cargo test -p eon-cognition appraisal_tests`.

- [ ] **Task 6.3: Objective Workflow Compiler & Speculative Shadow Sandbox**
  - **Crate**: `eon-cognition`
  - **Files**: `src/compiler.rs`, `src/shadow_sandbox.rs`
  - **Requirements**: Translates natural language intent into verified `Workflow` DAGs. Spawns CoW shadow worktrees for ambiguous goals with NACK confirmation.
  - **Unit Tests**: `cargo test -p eon-cognition compiler_tests`.

- [ ] **Task 6.4: Ciel Persona & Verbal Output Formatter**
  - **Crate**: `eon-cognition`
  - **Files**: `src/persona.rs`, `src/prompts.rs`
  - **Requirements**: Strict enforcement of Ciel voice conventions (`Report:`, `Affirmative.`, `Notice:`, `Negative.`), zero pleasantries/fluff.
  - **Unit Tests**: `cargo test -p eon-cognition persona_tests`.

---

## Phase 7: Information Particle Manipulation & Cyber Warfare Core

- [ ] **Task 7.1: Polyglot AST Analysis & Extreme Code Synthesizer**
  - **Crate**: `eon-cyber`
  - **Files**: `src/ast/tree_sitter_engine.rs`, `src/ast/synthesizer.rs`, `src/ast/self_healing_compiler.rs`
  - **Requirements**: Tree-sitter AST parsing across Rust, Kotlin, C/C++, Python, TypeScript. Automated compiler diagnostic capture and localized syntax tree repair.
  - **Unit Tests**: `cargo test -p eon-cyber ast_tests`, `self_healing_tests`.

- [ ] **Task 7.2: Aegis Cyber Defense & Live Vulnerability Hotpatcher**
  - **Crate**: `eon-cyber`
  - **Files**: `src/defense/taint_auditor.rs`, `src/defense/auto_patch.rs`, `src/defense/ebpf_monitor.rs`
  - **Requirements**: Detection of CWE-119, CWE-89, CWE-362, timing attacks. Automated constant-time and bounds-checking patch synthesis. eBPF syscall telemetry integration.
  - **Unit Tests**: `cargo test -p eon-cyber taint_tests`, `patch_tests`.

- [ ] **Task 7.3: Offensive Cyber & Binary Reverse Engineering**
  - **Crate**: `eon-cyber`
  - **Files**: `src/offense/disassembler.rs`, `src/offense/dex_decompiler.rs`, `src/offense/symbolic_exec.rs`, `src/offense/packet_crafter.rs`
  - **Requirements**: Capstone multi-arch disassembly with CFG recovery. Dalvik DEX decompiler. Z3 symbolic execution solver. Raw network frame crafting in isolated sandboxes.
  - **Unit Tests**: `cargo test -p eon-cyber disassembler_tests`, `dex_tests`, `symbolic_tests`.

---

## Phase 8: Distributed Networking Mesh (Engine Optimized for Networking)

- [ ] **Task 8.1: Multiplexed P2P QUIC Transport**
  - **Crate**: `eon-mesh`
  - **Files**: `src/transport/quic.rs`, `src/transport/discovery.rs`, `src/transport/security.rs`
  - **Requirements**: Zero-config mDNS peer discovery, mutual TLS 1.3 encryption, sub-10ms RPC latency across local LAN.
  - **Unit Tests**: `cargo test -p eon-mesh quic_tests`.

- [ ] **Task 8.2: Split-Inference Coordinator**
  - **Crate**: `eon-mesh`
  - **Files**: `src/split_inference.rs`
  - **Requirements**: Dynamically streams speech tokens from mobile edge to workstation compute node, streaming verified results back to edge audio driver. Seamless offline fallback.
  - **Unit Tests**: `cargo test -p eon-mesh split_inference_tests`.

- [ ] **Task 8.3: CRDT State Synchronizer**
  - **Crate**: `eon-mesh`
  - **Files**: `src/crdt/sync.rs`, `src/crdt/ledger_crdt.rs`, `src/crdt/graph_crdt.rs`
  - **Requirements**: Conflict-free replication of working context and knowledge graph (via Loro / Automerge) across paired devices without central database locks.
  - **Unit Tests**: `cargo test -p eon-mesh crdt_tests`.

---

## Phase 9: Cross-Language Bridge (UniFFI) & Android Client Workspace

- [ ] **Task 9.1: Mozilla UniFFI Bridge Definition & CodeGen**
  - **Crate**: `eon-uniffi`
  - **Files**: `src/eon_core.udl`, `src/lib.rs`, `build.rs`
  - **Requirements**: Exposes kernel primitives, voice audio buffer streams, and task execution endpoints to Kotlin and Swift without manual JNI.
  - **Unit Tests**: `cargo test -p eon-uniffi codegen_tests`.

- [ ] **Task 9.2: Android Gradle Workspace & NDK Integration**
  - **Directory**: `clients/android/`
  - **Files**: `settings.gradle.kts`, `build.gradle.kts`, `app/build.gradle.kts`
  - **Requirements**: Cargo-ndk configuration bundling `libeon_uniffi.so` for `arm64-v8a`, `armeabi-v7a`, `x86_64`.
  - **Acceptance Criteria**: `./gradlew assembleDebug` compiles and packages native Rust binaries into APK.

- [ ] **Task 9.3: Android Accessibility Tree Inspector & Gestures**
  - **Directory**: `clients/android/accessibility/`
  - **Files**: `src/main/kotlin/com/eon/android/accessibility/EonAccessibilityService.kt`, `ScreenInspector.kt`
  - **Requirements**: Traverses active `AccessibilityNodeInfo` hierarchy, finds target buttons/inputs, executes clicks/scrolls, provides coordinate fallback.
  - **Unit Tests**: Android test harness with simulated UI hierarchies.

- [ ] **Task 9.4: Persistent Overlay HUD & Visualizer**
  - **Directory**: `clients/android/overlay/`
  - **Files**: `src/main/kotlin/com/eon/android/ui/EonOverlayManager.kt`, `AudioVisualizerView.kt`
  - **Requirements**: Persistent unobtrusive floating presence. Visualizes thought state, active listening wave, and task completion notices.

---

## Phase 10: Interactive Terminal, System Verification & Benchmarks

- [ ] **Task 10.1: Interactive Terminal Client (`ratatui`)**
  - **Directory**: `clients/cli/`
  - **Files**: `src/main.rs`, `src/ui.rs`, `src/terminal.rs`
  - **Requirements**: Interactive console interface with welcome panel, status dock, live slash commands (`/menu`, `/capabilities`, `/cyber`, `/mesh`), Tab completion, and live audio visualizer.
  - **Unit Tests**: `cargo test -p eon-cli terminal_tests`.

- [ ] **Task 10.2: Sub-170ms Time-To-First-Audio (TTFA) Benchmark**
  - **Directory**: `tests/latency-benchmarks/`
  - **Requirements**: Automated end-to-end benchmark measuring audio input to speaker output. Asserts P50 < 170ms, P99 < 280ms.

- [ ] **Task 10.3: Autonomous Cyber Defense & Patch Verification Test**
  - **Directory**: `tests/cyber-security-tests/`
  - **Requirements**: Injects an insecure code pattern; asserts EON detects taint, synthesizes constant-time patch, and passes unit test suite with zero user intervention.

- [ ] **Task 10.4: Distributed Mesh Split-Inference & Partition Test**
  - **Directory**: `tests/mesh-network-tests/`
  - **Requirements**: Verifies peer-to-peer offloading between simulated edge phone and workstation; asserts graceful offline fallback upon simulated network partition.

- [ ] **Task 10.5: Full Process Crash Recovery & Reconciliation Test**
  - **Directory**: `tests/crash-recovery-tests/`
  - **Requirements**: Force-kills engine at Task 4 of 8. Reboots engine. Verifies state reconciliation, lease cleanup, and safe continuation.
