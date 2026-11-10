# ROADMAP AND IMPLEMENTATION PLAN

> **MANDATORY EXECUTION DIRECTIVE FOR CODING AGENTS:**  
> When assigned to implement EON, find the first unchecked item `[ ]` in this document. Implement the specified classes and functions, write and pass the corresponding unit tests, mark the checkbox `[x]`, and proceed to the next item immediately **without asking any questions or waiting for user approval**.

---

## Phase 0: Repository Scaffolding & Kernel Primitives (Level 0)

- [ ] **Task 0.1: Gradle Multi-Module Project Setup**
  - **Directory**: `engine/`
  - **Files**: `settings.gradle.kts`, `build.gradle.kts`, `gradle.properties`, `gradlew.bat`, `gradlew`
  - **Modules**:
    - `:eon-kernel-primitives`
    - `:eon-foundation-services`
    - `:eon-execution-foundations`
    - `:eon-resource-management`
    - `:eon-execution-engine`
    - `:eon-capability-fabric`
    - `:eon-cyber-core`
    - `:eon-mesh-network`
    - `:eon-ciel-cognition`
    - `:eon-local-capability-providers`
  - **Acceptance Criteria**: `./gradlew projects` succeeds with all 10 modules recognized.

- [ ] **Task 0.2: Strongly Typed Identifiers (ULID)**
  - **Module**: `:eon-kernel-primitives`
  - **Files**: `eon/primitives/id/EonId.kt`, `ObjectiveId.kt`, `TaskId.kt`, `WorkflowId.kt`, `IdentityId.kt`, `LeaseId.kt`
  - **Requirements**: Enforce prefix conventions (`obj_<ULID>`, `task_<ULID>`, `wf_<ULID>`, `lease_<ULID>`). Implement string parsing, regex validation, and monotonic sortability.
  - **Unit Tests**: `EonIdTest.kt` verifying ULID generation, prefix validation, and byte ordering.

- [ ] **Task 0.3: Monotonic Time & Result Types**
  - **Module**: `:eon-kernel-primitives`
  - **Files**: `eon/primitives/time/MonotonicTime.kt`, `eon/primitives/result/EonResult.kt`, `eon/primitives/errors/EonError.kt`
  - **Requirements**: `EonResult<T>` with sealed subclasses `Success(val value: T)` and `Failure(val error: EonError)`. Monotonic nanosecond/millisecond clock independent of wall-clock drift.
  - **Unit Tests**: `EonResultTest.kt`, `MonotonicTimeTest.kt`.

---

## Phase 1: Foundation Services & Durable Journaling (Level 1)

- [ ] **Task 1.1: Append-Only Event Journal**
  - **Module**: `:eon-foundation-services`
  - **Files**: `eon/foundation/journal/EventJournal.kt`, `LocalFileEventJournal.kt`, `JournalRecordCodec.kt`
  - **Requirements**: Append-only binary or NDJSON journal on disk. High-speed concurrent writes with atomic sequence numbers (`Long`). Crash-safe file sync (`flush` / `fsync`).
  - **Unit Tests**: `LocalFileEventJournalTest.kt` verifying append, sequential reads, corruption detection, and crash simulation.

- [ ] **Task 1.2: Durable State Store**
  - **Module**: `:eon-foundation-services`
  - **Files**: `eon/foundation/state/DurableStateStore.kt`, `SqliteDurableStateStore.kt`
  - **Requirements**: Key-value state persistence using embedded SQLite with WAL (Write-Ahead Logging). Atomic transactions for snapshot creation and state commits.
  - **Unit Tests**: `SqliteDurableStateStoreTest.kt` testing concurrent reads, transactional writes, and rollback.

- [ ] **Task 1.3: Identity & Authorization Model**
  - **Module**: `:eon-foundation-services`
  - **Files**: `eon/foundation/identity/IdentityService.kt`, `PermissionPolicy.kt`
  - **Requirements**: Ownership identities (System, User, EphemeralAgent). Permission sets (`READ_WORKSPACE`, `MUTATE_WORKSPACE`, `EXECUTE_PROCESS`, `CYBER_AUDIT`, `NETWORK_ACCESS`, `DEVICE_CONTROL`).
  - **Unit Tests**: `IdentityServiceTest.kt`.

---

## Phase 2: Execution Foundations & Resource Management (Level 2)

- [ ] **Task 2.1: Finite State Machine Primitives**
  - **Module**: `:eon-execution-foundations`
  - **Files**: `eon/foundations/fsm/StateMachine.kt`, `StateTransition.kt`, `IllegalStateTransitionException.kt`
  - **Requirements**: Generic deterministic state machine engine validating allowed transitions and emitting transition events.
  - **Unit Tests**: `StateMachineTest.kt` verifying transition validation, rejection of illegal states, and listener triggers.

- [ ] **Task 2.2: Resource Models & Atomic Leases**
  - **Module**: `:eon-resource-management`
  - **Files**: `eon/resource/ResourceRequirement.kt`, `ResourceLease.kt`, `AtomicResourceManager.kt`
  - **Requirements**: Resource accounting for CPU permits, Memory MB, File Lock URIs, and Network slots. Two-phase atomic acquisition algorithm (all-or-nothing). Lease expiration timeouts.
  - **Unit Tests**: `AtomicResourceManagerTest.kt` verifying deadlock prevention, shared vs exclusive locks, and expiration reclamation.

---

## Phase 3: Vector DAG Execution Engine & Crash Recovery (Level 3)

- [ ] **Task 3.1: Canonical Objective, Workflow & Task Models**
  - **Module**: `:eon-execution-engine`
  - **Files**: `eon/execution/model/Objective.kt`, `Workflow.kt`, `Task.kt`, `TaskParameters.kt`
  - **Requirements**: Immutable data models. Topological acyclic DAG validation in `Workflow` constructor.
  - **Unit Tests**: `WorkflowDagValidationTest.kt` verifying cycle detection, orphan task detection, and topological ordering.

- [ ] **Task 3.2: Resource-Aware Task Scheduler with Epoch Leases**
  - **Module**: `:eon-execution-engine`
  - **Files**: `eon/execution/scheduling/TaskScheduler.kt`, `ResourceAwareTaskScheduler.kt`, `CognitiveEpochManager.kt`
  - **Requirements**: Analyzes DAG task dependencies; dispatches tasks with verified epoch tokens and resource leases.
  - **Unit Tests**: `TaskSchedulerTest.kt` testing parallel multi-branch execution and dependency gating.

- [ ] **Task 3.3: Deterministic Task Dispatcher & Saga Reconciler**
  - **Module**: `:eon-execution-engine`
  - **Files**: `eon/execution/engine/DeterministicExecutionEngine.kt`, `TaskDispatcher.kt`, `SagaCompensationWorker.kt`
  - **Requirements**: Coordinates task dispatch. Emits `EPOCH_INVALIDATED` interrupts on user countermands, executing reverse-order compensating rollbacks.
  - **Unit Tests**: `DeterministicExecutionEngineTest.kt`, `SagaCompensationTest.kt`.

- [ ] **Task 3.4: Crash Recovery Reconciler (`LocalExecutionRecoveryWorker`)**
  - **Module**: `:eon-execution-engine`
  - **Files**: `eon/execution/recovery/LocalExecutionRecoveryWorker.kt`, `ExecutionReconciler.kt`
  - **Requirements**: Scans event journal on boot. Identifies interrupted tasks. Releases stale leases. Re-queues idempotent tasks or rolls back unverified writes.
  - **Unit Tests**: `CrashRecoveryIntegrationTest.kt` simulating mid-execution JVM termination and verifying successful reconciliation on restart.

---

## Phase 4: Capability Fabric & Sandboxed Tool Runtime (Level 4)

- [ ] **Task 4.1: Capability Registry & Sandboxing**
  - **Module**: `:eon-capability-fabric`
  - **Files**: `eon/capability/Capability.kt`, `CapabilityRegistry.kt`, `SandboxedCapabilityExecutor.kt`
  - **Requirements**: Dynamic capability registration, schema verification, bounded memory output wrappers, and execution timeouts.
  - **Unit Tests**: `CapabilityRegistryTest.kt`.

- [ ] **Task 4.2: Local Workspace & Process Providers**
  - **Module**: `:eon-local-capability-providers`
  - **Files**: `eon/providers/workspace/WorkspaceReadCapability.kt`, `WorkspaceWriteCapability.kt`, `WorkspacePatchCapability.kt`, `ProcessExecCapability.kt`
  - **Requirements**: Range reads, atomic temp-file writes, SHA-256 digest validation, pre-write journal snapshots.
  - **Unit Tests**: `WorkspaceCapabilitiesTest.kt`, `ProcessExecCapabilityTest.kt`.

- [ ] **Task 4.3: Network & Web Research Providers**
  - **Module**: `:eon-local-capability-providers`
  - **Files**: `eon/providers/net/HttpGetCapability.kt`, `FileDownloadCapability.kt`, `WebScraperCapability.kt`
  - **Requirements**: Bounded HTTP downloads, readability text extraction, zero external cloud model dependence.
  - **Unit Tests**: `NetworkCapabilitiesTest.kt`.

---

## Phase 5: Low-Latency Duplex Voice Engine (STT & TTS)

- [ ] **Task 5.1: Silero VAD & Audio Ring Buffer**
  - **Directory**: `perception/voice-engine/`
  - **Files**: `eon/voice/vad/SileroVadEngine.kt`, `AudioRingBuffer.kt`
  - **Requirements**: 30ms window ONNX evaluation, confidence scoring, 250ms dynamic end-of-utterance detection.
  - **Unit Tests**: `SileroVadTest.kt` verifying speech detection latency < 1ms per frame.

- [ ] **Task 5.2: Streaming Zipformer int8 ASR with Predictive Pre-Cognition**
  - **Directory**: `perception/voice-engine/`
  - **Files**: `eon/voice/asr/StreamingAsrEngine.kt`, `PredictiveEarlyExitDecoder.kt`
  - **Requirements**: Streams 16kHz audio chunks to `sherpa-onnx`, emits partial tokens incrementally, runs early-exit intent classifier during user speech.
  - **Unit Tests**: `StreamingAsrEngineTest.kt`, `PredictiveEarlyExitTest.kt`.

- [ ] **Task 5.3: Kokoro-82M Streaming Neural TTS & Acoustic Commitment Gate**
  - **Directory**: `perception/voice-engine/`
  - **Files**: `eon/voice/tts/StreamingTtsEngine.kt`, `KokoroTtsProvider.kt`, `AcousticCommitmentGate.kt`, `ClauseChunker.kt`
  - **Requirements**: Emits acoustic bridges immediately; gates definitive semantic claims until lease acquisition. TTFA < 200ms, RTF < 0.15.
  - **Unit Tests**: `ClauseChunkerTest.kt`, `AcousticCommitmentGateTest.kt`, `StreamingTtsBenchmarkTest.kt`.

- [ ] **Task 5.4: Acoustic Reflex & Full Duplex Barge-In**
  - **Directory**: `perception/voice-engine/`
  - **Files**: `eon/voice/duplex/DuplexVoiceCoordinator.kt`, `BargeInController.kt`
  - **Requirements**: Detects user voice while TTS is playing; aborts audio output driver in < 5ms; logs `INTERRUPTED_BY_USER`.
  - **Unit Tests**: `BargeInControllerTest.kt` verifying instantaneous flush.

---

## Phase 6: Ciel Cognitive Core & Thought Acceleration (Level 5)

- [ ] **Task 6.1: 3-Tier Cognitive Router (Thought Acceleration)**
  - **Module**: `:eon-ciel-cognition`
  - **Files**: `eon/ciel/router/CognitiveRouter.kt`, `Tier0ReflexMatcher.kt`, `Tier1TacticalSlm.kt`, `Tier2StrategicFrontier.kt`
  - **Requirements**: Route latency < 20ms for Tier 0 reflex, < 120ms for Tier 1 local SLM. Non-blocking asynchronous Tier 2 invocation.
  - **Unit Tests**: `CognitiveRouterTest.kt` testing latency and route selection.

- [ ] **Task 6.2: Appraisal Engine & Hybrid Cognitive Memory**
  - **Module**: `:eon-ciel-cognition`
  - **Files**: `eon/ciel/appraisal/AppraisalEngine.kt`, `ContextLedger.kt`, `SqliteGraphMemory.kt`, `VectorRetrievalEngine.kt`
  - **Requirements**: Working Context Ledger (bounded to 4k tokens), SQLite entity/relation graph store, local vector embedding search (`all-MiniLM-L6-v2` ONNX).
  - **Unit Tests**: `AppraisalEngineTest.kt`, `SqliteGraphMemoryTest.kt`.

- [ ] **Task 6.3: Objective Workflow Compiler & Speculative Shadow Sandbox**
  - **Module**: `:eon-ciel-cognition`
  - **Files**: `eon/ciel/compiler/ObjectiveWorkflowCompiler.kt`, `ShadowSandboxManager.kt`, `WorkflowSynthesizer.kt`
  - **Requirements**: Translates natural language intent into verified `Workflow` DAGs. Spawns CoW shadow worktrees for ambiguous goals with NACK confirmation.
  - **Unit Tests**: `ObjectiveWorkflowCompilerTest.kt`, `ShadowSandboxManagerTest.kt`.

- [ ] **Task 6.4: Ciel Persona & Verbal Output Formatter**
  - **Module**: `:eon-ciel-cognition`
  - **Files**: `eon/ciel/persona/CielResponseFormatter.kt`, `CielPrompts.kt`
  - **Requirements**: Strict enforcement of Ciel voice conventions (`Report:`, `Affirmative.`, `Notice:`, `Negative.`), zero pleasantries/fluff.
  - **Unit Tests**: `CielResponseFormatterTest.kt`.

---

## Phase 7: Information Particle Manipulation & Cyber Warfare Core

- [ ] **Task 7.1: Polyglot AST Analysis & Extreme Code Synthesizer**
  - **Module**: `:eon-cyber-core`
  - **Files**: `eon/cyber/ast/PolyglotAstParser.kt`, `CodeSynthesizer.kt`, `SelfHealingCompilerLoop.kt`
  - **Requirements**: AST parsing and manipulation across Rust, Kotlin, C/C++, Python, TypeScript. Automated compiler diagnostic capture and repair.
  - **Unit Tests**: `PolyglotAstParserTest.kt`, `SelfHealingCompilerTest.kt`.

- [ ] **Task 7.2: Aegis Cyber Defense & Live Vulnerability Hotpatcher**
  - **Module**: `:eon-cyber-core`
  - **Files**: `eon/cyber/defense/StaticTaintAuditor.kt`, `AutoPatchEngine.kt`, `EbpfAnomalyMonitor.kt`
  - **Requirements**: Detection of CWE-119, CWE-89, CWE-362, timing attacks. Automated constant-time and bounds-checking patch synthesis.
  - **Unit Tests**: `StaticTaintAuditorTest.kt`, `AutoPatchEngineTest.kt`.

- [ ] **Task 7.3: Offensive Cyber & Binary Reverse Engineering**
  - **Module**: `:eon-cyber-core`
  - **Files**: `eon/cyber/offense/BinaryDisassembler.kt`, `DexDecompiler.kt`, `SymbolicExecutionEngine.kt`, `PacketCrafter.kt`
  - **Requirements**: Disassembly of ELF, PE, DEX binaries with CFG recovery. Z3 symbolic constraint solver integration. Raw TCP/UDP packet crafting in isolated sandboxes.
  - **Unit Tests**: `BinaryDisassemblerTest.kt`, `DexDecompilerTest.kt`, `SymbolicExecutionTest.kt`.

---

## Phase 8: Distributed Networking Mesh (Engine Optimized for Networking)

- [ ] **Task 8.1: Multiplexed P2P QUIC Transport**
  - **Module**: `:eon-mesh-network`
  - **Files**: `eon/mesh/transport/QuicMeshTransport.kt`, `PeerDiscoveryService.kt`, `TlsIdentityKeyStore.kt`
  - **Requirements**: Zero-config mDNS peer discovery, mutual TLS 1.3 encryption, sub-10ms RPC latency across local LAN.
  - **Unit Tests**: `QuicMeshTransportTest.kt` testing peer pairing, latency, and packet migration.

- [ ] **Task 8.2: Split-Inference Coordinator**
  - **Module**: `:eon-mesh-network`
  - **Files**: `eon/mesh/inference/SplitInferenceCoordinator.kt`, `InferenceOffloadRouter.kt`
  - **Requirements**: Dynamically streams speech tokens from mobile edge to workstation compute node, streaming verified results back to edge audio driver. Seamless offline fallback.
  - **Unit Tests**: `SplitInferenceTest.kt`.

- [ ] **Task 8.3: CRDT State Synchronizer**
  - **Module**: `:eon-mesh-network`
  - **Files**: `eon/mesh/crdt/CrdtStateSynchronizer.kt`, `LwwContextLedger.kt`, `GraphCrdt.kt`
  - **Requirements**: Conflict-free replication of working context and knowledge graph across paired devices without central database locks.
  - **Unit Tests**: `CrdtStateSynchronizerTest.kt` verifying concurrent multi-node edits and deterministic convergence.

---

## Phase 9: Android System & Accessibility Automation (CHRONOS Bridge)

- [ ] **Task 9.1: Android Accessibility Tree Inspector & Gestures**
  - **Directory**: `clients/android/`
  - **Files**: `com/eon/android/accessibility/EonAccessibilityService.kt`, `ScreenInspector.kt`, `GestureController.kt`
  - **Requirements**: Traverses active `AccessibilityNodeInfo` hierarchy, finds target buttons/inputs, executes clicks/scrolls, provides coordinate fallback.
  - **Unit Tests**: Simulated UI hierarchy traversal and node resolution tests.

- [ ] **Task 9.2: Persistent Overlay HUD & Visualizer**
  - **Directory**: `clients/android/`
  - **Files**: `com/eon/android/ui/EonOverlayManager.kt`, `AudioVisualizerView.kt`, `NotificationCard.kt`
  - **Requirements**: Persistent unobtrusive floating presence. Visualizes thought state, active listening wave, and task completion notices.
  - **Unit Tests**: Overlay lifecycle and window attachment tests.

---

## Phase 10: System Verification & Latency Benchmarks

- [ ] **Task 10.1: Sub-200ms Time-To-First-Audio (TTFA) Benchmark**
  - **Directory**: `tests/latency-benchmarks/`
  - **Requirements**: Automated end-to-end benchmark measuring audio input to speaker output. Asserts P50 < 170ms, P99 < 280ms.
- [ ] **Task 10.2: Autonomous Cyber Defense & Patch Verification Test**
  - **Directory**: `tests/cyber-security-tests/`
  - **Requirements**: Injects an insecure code pattern; asserts EON detects taint, synthesizes constant-time patch, and passes unit test suite with zero user intervention.
- [ ] **Task 10.3: Multi-Step Autonomous Workflow Integration Test**
  - **Directory**: `tests/end-to-end-workflows/`
  - **Requirements**: Executes a 10-step autonomous workflow (Read file -> Parse AST -> Edit file -> Run build -> Verify test -> Voice report). Zero user questions.
- [ ] **Task 10.4: Distributed Mesh Split-Inference & Partition Test**
  - **Directory**: `tests/mesh-network-tests/`
  - **Requirements**: Verifies peer-to-peer offloading between simulated edge phone and workstation; asserts graceful offline fallback upon simulated network partition.
- [ ] **Task 10.5: Full Process Crash Recovery & Reconciliation Test**
  - **Directory**: `tests/crash-recovery-tests/`
  - **Requirements**: Force-kills engine at Task 4 of 8. Reboots engine. Verifies state reconciliation, lease cleanup, and safe continuation.
