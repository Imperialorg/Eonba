# ROADMAP AND IMPLEMENTATION PLAN

> **MANDATORY EXECUTION DIRECTIVE FOR CODING AGENTS:**  
> When assigned to implement EON, find the first unchecked item `[ ]` in this document. Implement the specified classes, structs, traits, and functions, write and pass the corresponding unit tests, mark the checkbox `[x]`, and proceed to the next item immediately **without asking any questions or waiting for user approval**.  
> Follow the architecture defined in [docs/TECH_STACK_AND_TOOLCHAINS.md](./TECH_STACK_AND_TOOLCHAINS.md) (Rust Native Core + Kotlin Android Presence).

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
    - `crates/eon-vision`
    - `crates/eon-cognition`
    - `crates/eon-uniffi`
  - **Acceptance Criteria**: `cargo check --workspace` compiles with zero warnings across all crates. (Verified 2026-11-10)

- [x] **Task 0.2: Strongly Typed Identifiers (ULID)**
  - **Crate**: `eon-primitives`
  - **Files**: `src/id.rs`, `src/lib.rs`
  - **Requirements**: Enforce prefix conventions (`obj_<ULID>`, `task_<ULID>`, `wf_<ULID>`, `lease_<ULID>`, `id_<ULID>`, `exec_<ULID>`). Implement string parsing, regex validation, and monotonic sortability.
  - **Unit Tests**: `cargo test -p eon-primitives id_tests` verifying ULID generation, prefix validation, and byte ordering. (Verified 2026-11-10)

- [x] **Task 0.3: Monotonic Time & Result Types**
  - **Crate**: `eon-primitives`
  - **Files**: `src/time.rs`, `src/result.rs`, `src/error.rs`
  - **Requirements**: `EonResult<T>` with standard typed `EonError` enums. Nanosecond-resolution monotonic clock independent of wall-clock drift.
  - **Unit Tests**: `cargo test -p eon-primitives time_and_result_tests`. (Verified 2026-11-10)

---

## Phase 1: Foundation Services & Durable Journaling (Level 1)

- [x] **Task 1.1: Append-Only Event Journal**
  - **Crate**: `eon-foundation`
  - **Files**: `src/journal.rs`, `src/group_commit.rs`, `tests/journal_crash.rs`
  - **Requirements**: Append-only binary journal on disk with atomic sequence numbering (`u64`), CRC32 verification, and frame header `0x454F4E4A`. Crash-safe file sync (`flush` / `sync_all`).
  - **Verification**: `cargo test --locked -p eon-foundation` covers exclusive writing, full and checkpoint replay, CRC failure, torn tails, and acknowledged writes surviving a killed writer. Process termination is not a power-loss test.

- [x] **Task 1.2: Durable State Store**
  - **Crate**: `eon-foundation`
  - **Files**: `src/store.rs`, `tests/projection_recovery.rs`
  - **Requirements**: Embedded SQLite / redb store with WAL mode. Atomic transactions for snapshot creation and state commits.
  - **Verification**: `cargo test --locked -p eon-foundation` covers transactional checkpoint and snapshot commits, overlapping WAL reads, rollback on a failed write, upgrading a checkpoint-only database, and replay after reopening.

- [ ] **Task 1.3: Identity, Zero-Knowledge Credential Broker & Journal Scrubber**
  - **Crate**: `eon-foundation`
  - **Files**: `src/identity.rs`, `src/permissions.rs`, `src/credentials.rs`, `src/scrubber.rs`
  - **Requirements**: Ownership identities (System, User, EphemeralAgent). Permission bitflags (`READ_WORKSPACE`, `MUTATE_WORKSPACE`, `EXECUTE_PROCESS`, `CYBER_OPERATIONS`, `NETWORK_ACCESS`, `DEVICE_CONTROL`, `SPAWN_SUBAGENT`). Opaque credential handles (`secret://<vault_key>`) with late-binding memory resolution from hardware OS enclaves (Android Keystore, Windows DPAPI, Linux Keyring). In-flight `JournalScrubber` redacting tokens and high-entropy strings ($\mathcal{H} > 4.5$) before journal disk commitment.
  - **Journal verification**: `cargo test --locked -p eon-foundation` checks the stored frame and its replay for recognized credential fixtures, including unlabelled high-entropy hex. This does not guarantee detection of arbitrary secrets.
  - **Platform status**: Linux keyring and Android API 35 emulator tests passed. After force-stop, the original AndroidKeyStore key decrypted the saved inspection probe. The separate native vault passed its Rust broker and child-stdin binding test. Native Windows DPAPI still needs a Windows run; the emulator does not attest physical hardware backing.

---

## Phase 2: Execution Foundations & Resource Management (Level 2)

- [x] **Task 2.1: Finite State Machine Primitives**
  - **Crate**: `eon-kernel`
  - **Files**: `src/fsm.rs`, `src/events.rs`, `src/journal_sink.rs`
  - **Requirements**: Generic deterministic state machine engine validating allowed transitions (`TaskState`), handling `UncertainFailure`, and emitting transition events.
  - **Verification**: `cargo test --locked -p eon-kernel --lib -- fsm::tests:: events::tests:: journal_sink::tests::` checks all state/event pairs, record-before-transition, failure to persist, and durable replay of uncertain outcomes. This primitive does not implement the boot reconciler.

- [x] **Task 2.2: Resource Models & Atomic Leases**
  - **Crate**: `eon-kernel`
  - **Files**: `src/resource.rs`, `tests/resource_model.rs`
  - **Requirements**: Resource accounting for CPU permits, Memory MB, File Lock URIs, and Network slots. Two-phase atomic acquisition algorithm (all-or-nothing). Lease expiration timeouts.
  - **Verification**: `cargo test --locked -p eon-kernel --lib resource` and `cargo test --locked -p eon-kernel --test resource_model`. The scheduler must call `reap_expired` with a monotonic clock and dispatch the returned grants. No background timer is assumed.

---

## Phase 3: Vector DAG Execution Engine & Crash Recovery (Level 3)

- [x] **Task 3.1: Canonical Objective, Workflow & Task Models**
  - **Crate**: `eon-kernel`
  - **Files**: `src/model/objective.rs`, `src/model/workflow.rs`, `src/model/task.rs`
  - **Requirements**: Immutable Rust data models with Serde support. Topological acyclic DAG validation in `Workflow` constructor (Kahn's algorithm).
  - **Verification**: `cargo test --locked -p eon-kernel --test dag_validation` checks construction and deserialization, including missing dependencies, foreign workflow IDs and a cycle disconnected from a valid root. A validated definition projects into the existing execution graph without changing its task IDs or edges.

- [x] **Task 3.2: Resource-Aware Task Scheduler with Epoch Leases**
  - **Crate**: `eon-kernel`
  - **Files**: `src/scheduler.rs`, `src/epoch.rs`
  - **Requirements**: Analyzes DAG task dependencies; dispatches tasks with verified epoch tokens and resource leases.
  - **Unit Tests**: `cargo test -p eon-kernel scheduler_tests` testing parallel multi-branch execution and dependency gating.

- [x] **Task 3.3: Deterministic Task Dispatcher & Saga Reconciler**
  - **Crate**: `eon-kernel`
  - **Files**: `src/dispatcher.rs`, `src/saga.rs`, `src/engine.rs`
  - **Requirements**: Coordinates task dispatch. Emits `EPOCH_INVALIDATED` interrupts on user countermands, executing reverse-order compensating rollbacks.
  - **Unit Tests**: `cargo test -p eon-kernel saga_compensation_tests`.

- [x] **Task 3.4: Crash Recovery Reconciler (`LocalExecutionRecoveryWorker`)**
  - **Crate**: `eon-kernel`
  - **Files**: `src/recovery.rs`
  - **Requirements**: Scans event journal on boot. Identifies interrupted tasks. Releases stale leases. Re-queues idempotent tasks or transitions to `UncertainFailure` with safe rollback.
  - **Unit Tests**: `cargo test -p eon-kernel recovery_tests` simulating mid-execution termination and verifying reconciliation.

- [x] **Task 3.5: Dynamic DAG Expansion (`ExpandableTaskNode`)**
  - **Crate**: `eon-kernel`
  - **Files**: `src/model/continuation.rs`, `src/dynamic_dag.rs`
  - **Requirements**: Replaces static upfront DAGs with Dynamic Hierarchical DAGs. Implements `TaskType::CognitiveContinuation` checkpoints that take discovery outputs (`workspace.read`, `dev.lsp`) and call `Workflow::splice_subgraph` to dynamically splice mutation tasks (`workspace.patch`, `dev.test`) at runtime. Integrates mid-task context auto-compaction.
  - **Unit Tests**: `cargo test -p eon-kernel dynamic_dag_tests` testing runtime child task splicing and dependency rebinding.

---

## Phase 4: Capability Fabric & Sandboxed Tool Runtime (Level 4)

- [x] **Task 4.1: Capability Registry, Sandboxing & Cognitive Taint Enforcement**
  - **Crate**: `eon-capability`
  - **Files**: `src/capability.rs`, `src/registry.rs`, `src/sandbox.rs`, `src/taint.rs`
  - **Requirements**: Dynamic capability registration, schema verification, bounded memory output wrappers, execution timeouts. Information Flow Control (IFC) tagging `CognitiveTaint` (`TrustedOperator` vs `UntrustedExternal`) and Invariant 8 privilege escalation enforcement.
  - **Unit Tests**: `cargo test -p eon-capability registry_tests`, `taint_enforcement_tests`.
  - **Verified backend**: Registered import-free Wasm on Linux. Both dispatch APIs require durable redacted attempt and outcome records. The registry checks the descriptor, module digest, schema and taint; the worker limits fuel, memory, output and execution time. Its deadline includes compilation but does not cover host journal I/O, approval storage or masked review.

- [ ] **Task 4.2: Local Workspace, Process & Persistent PTY Multiplexer Providers**
  - **Crate**: `eon-capability`
  - **Files**: `src/providers/workspace.rs`, `src/providers/process.rs`, `src/providers/pty.rs`
  - **Requirements**: Range reads, atomic temp-file writes, SHA-256 digest validation, pre-write journal snapshots. Persistent bidirectional PTY session multiplexer (`portable-pty`) exposing `process.pty_open`, `process.pty_write`, `process.pty_read_ring`, and `process.pty_signal` with in-memory 10,000-line circular ring buffer and 300s idle watchdog.
  - **Unit Tests**: `cargo test -p eon-capability workspace_tests`, `process_tests`, `pty_tests`.
  - **Verified subset**: Linux has workflow-owned PTYs with bounded reads, writes, signals and idle cleanup. Host-approved operations can transform or count supplied bytes without exposing a shell, caller-controlled arguments or file operands. This is a devpts backend, not a cross-platform `portable-pty` provider. Workspace writes retain private file preimages, not directory snapshots.

- [x] **Task 4.3: Network & Web Research Providers**
  - **Crate**: `eon-capability`
  - **Files**: `src/providers/http.rs`, `src/providers/download.rs`, `src/providers/scraper.rs`
  - **Requirements**: Bounded HTTP downloads, readability text extraction, zero external cloud model dependence.
  - **Unit Tests**: `cargo test -p eon-capability network_tests`.

- [ ] **Task 4.4: LSP Client Bridge & AST Search/Replace Patcher**
  - **Crate**: `eon-capability`
  - **Files**: `src/providers/lsp.rs`, `src/patch/ast_patcher.rs`
  - **Requirements**: Embedded JSON-RPC LSP client multiplexer for `rust-analyzer`, `clangd`, `kotlin-language-server`, `pyright`, and `vtsls` providing sub-50ms diagnostic queries (`goto_definition`, `find_references`, `hover_type`, `diagnostics`). Symbol-scoped AST search/replace block patcher replacing fragile line-number diffs.
  - **Unit Tests**: `cargo test -p eon-capability lsp_and_patcher_tests`.
  - **Verified subset**: Confined clangd and rust-analyzer answered cross-file definitions, references, hover and fresh diagnostics on two-file projects. A task-capped rust-analyzer 1.99.1 worker took 122.8 ms from launch to its first diagnostic, 0.117 ms warm diagnostic p95 and 0.783 ms edit-to-diagnostic p95. It sees only supplied snapshots of a dependency-free library, not the workspace or Cargo build scripts.
  - **Remaining servers**: Pyright, Kotlin Language Server and vtsls have not passed this gate. Cold queries do not meet the 50 ms target.

---

## Phase 5: Low-Latency Duplex Voice Engine (STT & TTS)

- [x] **Task 5.1: Silero VAD & Audio Ring Buffer**
  - **Crate**: `eon-voice`
  - **Files**: `src/vad.rs`, `src/ring_buffer.rs`, `src/audio_device.rs`
  - **Requirements**: Native CPAL / AAudio / WASAPI audio driver loop. 30ms window ONNX evaluation, confidence scoring, 250ms dynamic end-of-utterance detection.
  - **Unit Tests**: `cargo test -p eon-voice vad_tests` verifying speech detection latency < 1ms per frame.
  - **Verified backend**: Linux CPAL capture on a private 48 kHz monitor. The adapter takes 30 ms device windows; the pinned Silero model takes 512 samples at 16 kHz (32 ms). All 501 measured live model frames took less than 1 ms. The 250 ms quiet threshold was observed at its next frame boundary, 256 ms. No physical microphone, Android AAudio or Windows WASAPI result.

- [x] **Task 5.2: Streaming Zipformer int8 ASR with Predictive Pre-Cognition**
  - **Crate**: `eon-voice`
  - **Files**: `src/asr.rs`, `src/pre_cognition.rs`
  - **Requirements**: Streams 16kHz audio chunks to `sherpa-onnx` C++ bindings, emits partial tokens incrementally, runs early-exit intent classifier during user speech.
  - **Unit Tests**: `cargo test -p eon-voice asr_tests`.
  - **Verified backend**: The optional native Sherpa online recognizer consumed 20 ms pushes of real 16 kHz PCM and produced 16 partials on an upstream recording. The first intent arrived at 2.08 seconds, before the 6.625-second EOF. Partial text and intent are retractable, display-only observations. The independent direct ONNX decoder matched the final reference transcript.

- [x] **Task 5.3: Kokoro-82M Streaming Neural TTS & Acoustic Commitment Gate**
  - **Crate**: `eon-voice`
  - **Files**: `src/tts.rs`, `src/chunker.rs`, `src/commitment_gate.rs`
  - **Requirements**: Emits acoustic bridges immediately; gates definitive semantic claims until lease acquisition. TTFA < 170ms, RTF < 0.15.
  - **Unit Tests**: `cargo test -p eon-voice tts_tests`, `commitment_gate_tests`.
  - **Verified backend**: On Linux, the explicitly selected v1.0 release INT8 graph with the pinned voice rendered a cached bridge and then an authorized clause into CPAL. First non-silent callback reference: 27.8 ms. Uncached clause RTF: 0.124. This is a warmed software/render test, not the physical TTFA gate in Task 10.2. The default profile graph could not be fetched; it was not silently replaced.

- [x] **Task 5.4: Acoustic Reflex & Full Duplex Barge-In**
  - **Crate**: `eon-voice`
  - **Files**: `src/barge_in.rs`, `src/coordinator.rs`
  - **Requirements**: Detects user voice while TTS is playing; aborts audio output driver in < 5ms; logs `INTERRUPTED_BY_USER`.
  - **Unit Tests**: `cargo test -p eon-voice barge_in_tests` verifying instantaneous buffer flush.
  - **Verified backend**: While committed Kokoro speech was playing, a held-out owner recording triggered `INTERRUPTED_BY_USER`. An independent 48 kHz Linux null-sink monitor matched the interrupted waveform and observed sustained silence within 2.48 ms of authorization in all three trials. Authorization itself took 3.300 seconds of recorded speech. Not an onset-to-stop or physical speaker latency claim.

- [x] **Task 5.5: AEC Loopback & Biometric Speaker Verification**
  - **Crate**: `eon-voice`
  - **Files**: `src/aec.rs`, `src/speaker_id.rs`
  - **Requirements**: `webrtc-audio-processing` reverse reference stream fed from the 24kHz TTS playback ring buffer to eliminate self-interruption on open speaker. WeSpeaker 256-dim cosine similarity check (< 2ms) to prevent crowd/TV false barge-in triggers.
  - **Unit Tests**: `cargo test -p eon-voice aec_and_speaker_id_tests`.
  - **Verified on recorded Linux duplex audio**: With a 24 kHz post-render reference and two simulated echo paths, neither echo alone nor independently voiced competing speech interrupted playback. Held-out owner speech did at 800 ms, with the original 0.35 threshold. The 256-value cosine check took 7.69 µs p95 of batch means; obtaining the embedding took 13.37 ms median. This is a controlled two-voice test, not a population-wide crowd/TV or physical output latency result.

---

## Phase 6: Ciel Cognitive Core & Thought Acceleration (Level 5)

- [ ] **Task 6.1: 3-Tier Cognitive Router (Thought Acceleration)**
  - **Crate**: `eon-cognition`
  - **Files**: `src/router.rs`, `src/reflex.rs`, `src/tactical_slm.rs`, `src/strategic_frontier.rs`
  - **Requirements**: Route latency < 20ms for Tier 0 reflex, < 120ms for Tier 1 local SLM. Dual-stream split: silent `<think>...</think>` scratchpad routed to HUD/CLI, post-`</think>` tokens to TTS.
  - **Unit Tests**: `cargo test -p eon-cognition router_tests`.
  - **Latency evidence**: On Linux, 2,000 complete reflex turns had a p95 of 621 ns. Checking the greedy winner against the grammar before filtering the whole vocabulary reduced warm Tier 1 p95 from 535–560 ms to 413–417 ms on the same host. All 120 optimized turns reached natural end of generation and returned the correct validated reading. The 120 ms target was not met.

- [x] **Task 6.2: Appraisal Engine & Hybrid Cognitive Memory**
  - **Crate**: `eon-cognition`
  - **Files**: `src/appraisal.rs`, `src/context_ledger.rs`, `src/graph_memory.rs`, `src/vector_store.rs`
  - **Requirements**: Working Context Ledger (bounded to 4k tokens with auto-compactor), Loro CRDT source-of-truth projected into local SQLite entity/relation tables, local vector embedding search (`bge-micro-v2` ONNX).
  - **Unit Tests**: `cargo test -p eon-cognition appraisal_tests`.
  - **Verified on Linux**: The opt-in `bge-memory` backend tokenizes and embeds visible Loro records using the pinned local BGE model. After reopening SQLite, a query retrieved a related note with no shared keywords. Selecting a current note compacted the bounded working ledger. A correction invalidated the old hit and its late embedding without writing either back into the paired document.

- [x] **Task 6.3: Objective Workflow Compiler & Speculative Shadow Sandbox**
  - **Crate**: `eon-cognition`
  - **Files**: `src/compiler.rs`, `src/shadow_sandbox.rs`
  - **Requirements**: Translates natural language intent into verified `Workflow` DAGs. Spawns CoW shadow worktrees for ambiguous goals with attention-gated 5-second NACK confirmation.
  - **Unit Tests**: `cargo test -p eon-cognition compiler_tests`.
  - **Verified backend**: The Linux compiler accepts bounded read intents and materializes ambiguous alternatives as private copy-on-write worktrees containing only the authorized file ranges. Presentation freezes them. Only an explicit selection with fresh attention in the original five-second window exports the chosen read-only directory interface and validated DAG. No shell, arbitrary goal execution or source writeback. The ext4 test exercised the copy-on-first-write fallback, not successful reflinking.

- [x] **Task 6.4: Ciel Persona & Verbal Output Formatter**
  - **Crate**: `eon-cognition`
  - **Files**: `src/persona.rs`, `src/prompts.rs`
  - **Requirements**: Strict enforcement of Ciel voice conventions (`Report:`, `Affirmative.`, `Notice:`, `Negative.`), zero pleasantries/fluff.
  - **Unit Tests**: `cargo test -p eon-cognition persona_tests`.

- [ ] **Task 6.5: Multimodal Vision Pipeline (`eon-vision`)**
  - **Crate**: `eon-vision`
  - **Files**: `src/omniparser.rs`, `src/keyframe_sampler.rs`, `src/ring_buffer.rs`, `src/lib.rs`
  - **Requirements**: OmniParser-v2 (< 30ms YOLO element detector + PaddleOCR ONNX) for UI grounding when accessibility trees are obfuscated. Adaptive 1–5 FPS video keyframe sampler and 30-second rolling visual memory in shared memory. Integration with `Qwen2.5-VL-3B-Instruct`.
  - **Unit Tests**: `cargo test -p eon-vision vision_tests`.
  - **Verified subset**: PaddleOCR reads text and OmniParser locates unlabeled icons on one selected, sealed JPEG without access to its history. The opt-in local Qwen2.5-VL-3B model independently transcribed five labels across two selections and returned `NONE` on a blank. The full CPU icon detector still missed its 30 ms gate (96.39 ms p95 on this runner).

---

## Phase 7: Program Analysis and Defensive Security

The security work in this roadmap is defensive: inspect authorized code, dependencies and local audit records, contain failures, and verify remediation. Earlier design sketches about autonomous exploitation, payload generation, or targeting systems are not implementation requirements, even in a lab.

- [ ] **Task 7.1: Polyglot AST Analysis, Extreme Code Synthesizer & DAP Bridge**
  - **Crate**: `eon-cyber`
  - **Files**: `src/ast/tree_sitter_engine.rs`, `src/ast/synthesizer.rs`, `src/ast/self_healing_compiler.rs`, `src/ast/repomap.rs`, `src/ast/dap.rs`
  - **Requirements**: Tree-sitter AST parsing across Rust, Kotlin, C/C++, Python, TypeScript. PageRank architectural RepoMap generator. Embedded Debug Adapter Protocol (DAP) client bridge (`dev.dap`) interfacing with `codelldb`, `debugpy`, and `jdwp` to inspect live stack frames, conditional breakpoints, and heap allocations. Automated compiler diagnostic capture and localized syntax tree repair.
  - **Unit Tests**: `cargo test -p eon-cyber ast_tests`, `self_healing_tests`, `dap_tests`.
  - **Verified subset**: Across four languages, 48 of 50 benign repair cases passed independent behavior checks after one edit, 49 within three revisions. Kotlin uses guarded token edits. The separate RepoMap parses all six languages and ranks lexical membership and unambiguous syntactic call matches. Its hand-checked fixture covers 19 declarations in eight files. This is not compiler name resolution or live debugger verification.

- [ ] **Task 7.2: Defensive source review, repair proposals and platform telemetry**
  - **Crate**: `eon-cyber`
  - **Files**: `src/defense/taint_auditor.rs`, `src/defense/auto_patch.rs`, `src/defense/telemetry.rs`
  - **Requirements**: Review authorized source for patterns associated with CWE-119, CWE-89, CWE-362 and observable timing differences. Produce reviewable constant-time and bounds-checking edits, without executing submitted code or patching a running process. Collect opt-in, metadata-only telemetry on supported Linux/Android eBPF, Windows ETW and stock Android backends. Distinguish observed evidence from unsupported rules and unavailable sensors.
  - **Verification**: Exercise both matching and nonmatching source, compile and test the reviewed repair, check that reports do not disclose the source, and test each claimed telemetry backend on its supported host. Local `/proc/self/io` counters do not establish eBPF, ETW or VPN coverage.
  - **Platform evidence**: An opted-in local action measured 334 ms of process-wide CPU time on an API 35 emulator. The app does not open a network connection, accept a target PID or persist a baseline. The Windows audit CLI cross-linked with a payload-free ETW marker. The Windows job checks delivery after opt-in and absence by default. No native Windows ETW, Android eBPF or VPN result.

- [x] **Task 7.3: Offline dependency and advisory assessment**
  - **Crate**: `eon-cyber`
  - **File**: `src/sbom.rs`
  - **Requirements**: Match local component inventories against offline OSV advisories. Distinguish exact affected versions from unresolved ranges, reject malformed input, and emit one verdict per component and advisory. The strongest verdict wins.
  - **Verification**: `cargo test --locked -p eon-cyber --no-default-features --lib sbom` using synthetic version lists, range-only and mixed records, malformed fields, and duplicate matches. No target execution or network access.

- [x] **Task 7.4: Verify sandbox and secret-containment boundaries**
  - **Crates**: `eon-capability`, `eon-foundation`
  - **Requirements**: Untrusted tools cannot read outside their workspace, open a network socket, or exceed their resource budgets. Verify that audit records contain no raw test credentials and that teardown releases resources even when a tool fails.
  - **Verification**: Denied access leaves a sentinel file unchanged; an over-budget tool terminates; replayed audit records and on-disk frames contain only redacted fixtures. Run real sandbox tests on a supported host. An unavailable backend is not evidence that isolation passed.
  - **Verified on Linux**: Registered Wasm tools cannot acquire filesystem or socket imports. The sentinel remained unchanged, failed guests left no child or host descriptor behind, and the next invocation started with clean memory. Raw journal frames and replay contained only redacted metadata, not the fixture names, arguments or outputs. No claim for unrestricted native processes or Firecracker/KVM.

---

## Phase 8: Distributed Networking Mesh (Engine Optimized for Networking)

- [x] **Task 8.1: Multiplexed P2P QUIC Transport & 15ms Spatial Election Protocol**
  - **Crate**: `eon-mesh`
  - **Files**: `src/transport/quic.rs`, `src/transport/discovery.rs`, `src/transport/security.rs`, `src/transport/spatial.rs`
  - **Requirements**: Zero-config mDNS peer discovery, mutual TLS 1.3 encryption, sub-10ms RPC latency across local LAN. 15ms Spatial Arbitration Protocol ("Single Mind, Multiple Bodies") broadcasting `SensoryClaim` over Class 0 QUIC datagrams, calculating deterministic multi-factor arbitration scores (SNR, screen unlock, HID recency, speaker ID confidence), and electing a single I/O leader while enforcing follower speaker suppression.
  - **Unit Tests**: `cargo test -p eon-mesh quic_tests`, `spatial_election_tests`.
  - **Verified on a two-node Linux virtual LAN**: Independently pinned peers discovered one another over mDNS, agreed on the leader and suppressed follower playback. Three consecutive runs passed the strict 100-RPC gate; maxima were 8.502, 0.566 and 0.363 ms. One earlier run missed at 10.062 ms. This measures an established connection on isolated veth interfaces, not connection setup, a physical Wi-Fi link or a hard real-time guarantee.

- [x] **Task 8.2: Split-Inference Coordinator**
  - **Crate**: `eon-mesh`
  - **Files**: `src/split_inference.rs`
  - **Requirements**: Dynamically streams speech tokens from mobile edge to workstation compute node, streaming verified results back to edge audio driver. Seamless offline fallback.
  - **Unit Tests**: `cargo test -p eon-mesh split_inference_tests`.
  - **Verified native edge readback**: With separate consent for one recording, the edge streamed actual Zipformer text-token IDs to a pinned worker, checked the returned caption and pronunciation against its local result, and queued only the verified final clause. Partition or corruption preserved local readback; partials granted no playback authority. A matching lease released 324,000 samples to a private 48 kHz Linux CPAL output. Both peer roles ran on Linux, not on physical mobile hardware. This is dictation readback, not general remote command execution.

- [x] **Task 8.3: CRDT State Synchronizer**
  - **Crate**: `eon-mesh`
  - **Files**: `src/crdt/sync.rs`, `src/crdt/ledger_crdt.rs`, `src/crdt/graph_crdt.rs`
  - **Requirements**: Conflict-free replication of working context and knowledge graph (via Loro) across paired devices without central database locks.
  - **Unit Tests**: `cargo test -p eon-mesh crdt_tests`.

- [ ] **Task 8.4: WAN Hole-Punching (`iroh`) & 4-Tier QUIC QoS**
  - **Crate**: `eon-mesh`
  - **Files**: `src/transport/iroh_node.rs`, `src/transport/qos_lanes.rs`
  - **Requirements**: `iroh` integration for Ed25519 node addressing, zero-config STUN hole-punching and DERP relay routing across 5G/Wi-Fi roaming. 4-class QUIC QoS prioritization using RFC 9221 datagrams for Class 0 voice/barge-in (< 5ms) and reliable streams for Class 1/2/3. Wire serialization via `proto/eon.proto`.
  - **Unit Tests**: `cargo test -p eon-mesh iroh_and_qos_tests`.
  - **Verified subset**: Mutually pinned peers reconnected through the original private relay after an outage, retained the bandwidth budget and exchanged all four traffic classes without replaying unacknowledged frames. Relay counters confirmed transit. This is not direct migration, STUN, external TLS or carrier roaming.

---

## Phase 9: Cross-Language Bridge (UniFFI) & Android Client Workspace

- [x] **Task 9.1: Mozilla UniFFI Bridge Definition & CodeGen**
  - **Crate**: `eon-uniffi`
  - **Files**: `src/eon_core.udl`, `src/lib.rs`, `build.rs`
  - **Requirements**: Exposes kernel primitives, test audio injection, and task execution endpoints to Kotlin and Swift without manual JNI.
  - **Unit Tests**: `cargo test -p eon-uniffi codegen_tests`.

- [x] **Task 9.2: Android Gradle Workspace & NDK Integration**
  - **Directory**: `engine/android/`
  - **Files**: `settings.gradle.kts`, `build.gradle.kts`, `app/build.gradle.kts`
  - **Requirements**: Cargo-ndk configuration bundling `libeon_uniffi.so` for `arm64-v8a`, `armeabi-v7a`, `x86_64`.
  - **Acceptance Criteria**: `./gradlew assembleDebug` compiles and packages native Rust binaries into APK.

- [ ] **Task 9.3: Android Accessibility Tree Inspector & Gestures**
  - **Directory**: `clients/android/accessibility/`
  - **Files**: `src/main/kotlin/com/eon/android/accessibility/EonAccessibilityService.kt`, `ScreenInspector.kt`
  - **Requirements**: Traverses active `AccessibilityNodeInfo` hierarchy, finds target buttons/inputs, executes clicks/scrolls, provides coordinate fallback.
  - **Unit Tests**: Android test harness with simulated UI hierarchies.
  - **Verified subset**: A private EON demo can review one counter click or list scroll, use a local hit test if the native action is absent, and check the resulting change. It never retries an attempted action or dispatches gestures outside the activity. Fifteen host tests and the clean API 35 denied scenario passed. The positive device scenario still needs an operator-selected assistant and enabled service and shortcut.

- [ ] **Task 9.4: Persistent Overlay HUD & Visualizer**
  - **Directory**: `clients/android/overlay/`
  - **Files**: `src/main/kotlin/com/eon/android/ui/EonOverlayManager.kt`, `AudioVisualizerView.kt`
  - **Requirements**: Persistent unobtrusive floating presence. Visualizes thought state, active listening wave, and task completion notices.
  - **Verified subset**: The collapsible assistant card can show progress and completion for a local model-profile check or analysis of an OS-installed notification tone. On a clean API 35 emulator the production decoder produced levels, cancellation stopped delivery, and the unattached view displayed and cleared the result. This was not an OS-invoked assistant session, live task feed or microphone waveform.

- [ ] **Task 9.5: Native Digital Assistant Role & OS Survival (`ROLE_ASSISTANT`)**
  - **Directory**: `clients/android/app/`
  - **Files**: `src/main/kotlin/com/eon/assistant/service/EonVoiceInteractionService.kt`, `EonVoiceInteractionSessionService.kt`, `res/xml/voice_interaction_service_config.xml`
  - **Requirements (stock Android)**: Support the user-selected assistant role and request hotword detection only where the OS and hardware permit it. Respect microphone permission and foreground restrictions. Consume only OS-supplied assist context while the role is active. Require fresh user consent for each screen-capture session; release resources on revocation. Test clean recovery after process death. A regular app cannot choose its OOM adjustment or promise immunity from OEM process management.
  - **Unit Tests**: Android instrumented test verifying assistant binding, assist structure extraction, and background audio streaming.

---

## Phase 10: Interactive Terminal, Live Canvas, System Verification & Benchmarks

- [x] **Task 10.1: Interactive Terminal Client (`ratatui`) & Live Canvas**
  - **Directory**: `clients/cli/` & `clients/canvas/`
  - **Files**: `src/main.rs`, `src/ui.rs`, `src/terminal.rs`, `clients/canvas/src/main.rs`
  - **Requirements**: Interactive console interface with welcome panel, status dock, live slash commands (`/menu`, `/capabilities`, `/cyber`, `/mesh`, `/vision`), Tab completion, and live audio visualizer. Terminal graphics via `ratatui-image` (Kitty/Sixel). Graphical canvas for live AST diffs, reverse engineering CFGs, and video scrubbing (`wgpu` / Compose).
  - **Unit Tests**: `cargo test -p eon-cli terminal_tests`.
  - **Verified subset**: Build `eon-cli` with `--features audio-input`, run `eon watch <journal-dir>`, open `/audio` and press F2. Pasted text cannot start capture. F2, `/`, leaving the view or quitting stops it. The meter keeps at most 48 peak readings and no PCM. A dedicated Linux null-sink monitor delivered a 50% peak; its recording stream disappeared on `/` before the process exited. This test did not use a physical microphone.
  - **Terminal graphics**: On Unix, `eon watch <journal-dir> --image <file>` accepts one PNG or JPEG. `/vision` displays that still, not a screen capture. Leaving releases it; re-entry does not reopen the file. Kitty and VT340 xterm both displayed the selected colors after the original file was unlinked and cleared them on exit. The path is visible to shell history and OS process inspection.
  - **Control-flow canvas**: The `cfg` example reads one bounded GCC successor export. Software Vulkan readback verified all seven labels and eight directed edges of a real function, including a back edge and a self-loop.
  - **Verified Linux workbench**: `/mesh` updates from opt-in completed paired syncs in the selected local journal, not a peer count. The live Rust/Python diff follows in-place edits to two selected files and hides stale results on parse failure. The timeline scrubs selected timestamped JPEGs. Neither client captures a screen.

- [ ] **Task 10.2: Sub-170ms Time-To-First-Audio (TTFA) Benchmark**
  - **Directory**: `tests/latency-benchmarks/`
  - **Requirements**: Automated end-to-end benchmark measuring audio input to speaker output. Asserts P50 < 170ms, P99 < 280ms.
  - **Measurement**: Use `scripts/measure_ttfa.py` on synchronized input and physical speaker recordings as described in Manual 06 §3.3. Generated PCM fixtures test the analyzer, not a device's latency.

- [x] **Task 10.3: Autonomous Cyber Defense & Patch Verification Test**
  - **Directory**: `tests/cyber-security-tests/`
  - **Requirements**: Injects an insecure code pattern; asserts EON detects taint, synthesizes constant-time patch, and passes unit test suite with zero user intervention.

- [x] **Task 10.4: Distributed Mesh Split-Inference & Partition Test**
  - **Directory**: `tests/mesh-network-tests/`
  - **Requirements**: Verifies peer-to-peer offloading between simulated edge phone and workstation; asserts graceful offline fallback upon simulated network partition.
  - **Verification**: `cargo test --locked -p eon-mesh --features native-zipformer --test split_zipformer` on two mutually pinned loopback endpoints. Both the worker-only run and independent local inference after closing the worker endpoint matched the upstream recording's reference transcript.

- [x] **Task 10.5: Full Process Crash Recovery & Reconciliation Test**
  - **Directory**: `tests/crash-recovery-tests/`
  - **Requirements**: Force-kills engine at Task 4 of 8. Reboots engine. Verifies state reconciliation, lease cleanup, and safe continuation.

- [ ] **Task 10.6: Deterministic Journal Replay & Benchmark Suite (`tests/eval-harness/`)**
  - **Directory**: `tests/eval-harness/`
  - **Files**: `src/replay_engine.rs`, `src/swe_bench.rs`, `src/defensive_regressions.rs`, `src/noisy_audio.rs`
  - **Requirements**:
    1. **Time-Travel Replay Engine**: Replays historical binary journal logs (`0x454F4E4A`) through the execution kernel; asserts that identical inputs reproduce identical DAG structures and verified outcomes.
    2. **SWE-Bench-Lite / Polyglot Repair Suite**: 50 automated bug repair scenarios across Rust, Kotlin, Python, and TypeScript; verifies autonomous AST repair with pass@1 rate > 80% using the self-healing compiler loop.
    3. **Defensive regression suite**: Use labeled source-code fixtures and patched counterparts to check detection, false positives, redaction, and remediation without constructing exploits or executing target code.
    4. **Noisy Duplex Audio Suite**: Synthetic audio benchmark with heavy acoustic background noise (coffee shop babble, street traffic, television dialog); asserts zero false barge-ins via WeSpeaker speaker verification and TTFA < 200ms.
  - **Acceptance Criteria**: `cargo test --test eval_harness` executes the complete evaluation suite with 100% pass rate.
  - **Repair result**: Manual 06 §3.6 records the toolchains, exact corpus and per-language outcomes. The 96% one-edit result clears the repair threshold but does not establish a noisy-scene TTFA below 200 ms or a passing combined evaluation.
  - **Noisy speech result**: The synthetic babble, traffic and TV scenes produced no false interruption in the non-owner intervals, but all three also missed the held-out owner. The clean and rumble controls detected the owner. This is a failing speaker evaluation, not a latency result (Manual 06 §3.7).
