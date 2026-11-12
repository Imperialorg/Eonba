# EON System Architecture Specification

> **Status:** Definitive Architectural Specification  
> **System:** EON (Engine Optimized for Networking & Autonomous Intelligence)  
> **Archetype:** Manas: Ciel (The Sovereign Intelligence, Cyber Core & Distributed Mesh)

---

## 1. Architectural Philosophy: The Ciel Model

EON is designed around four fundamental truths:
1. **Separation of Perception, Cognition, Execution, and Substrate Manipulation**:
   - *Perception* converts analog physical signals (audio, screen pixels, camera frames, network packets) into structured digital events.
   - *Cognition (Ciel)* synthesizes intent, appraises context, formulates goals, and constructs dependency-checked execution graphs (DAGs).
   - *Information Particle Manipulation (Cyber Core)* provides master-level deconstruction, synthesis, and hardening of software code, ASTs, binaries, and network frames.
   - *Execution (Vector Engine)* allocates physical resources (CPU, threads, memory, I/O leases), coordinates workers, executes capabilities, and verifies outcomes against ground truth.
2. **Sub-200ms Latency Budget (Thought Acceleration)**:
   - Voice and reflex loops cannot wait for large multi-gigabyte models or multi-turn cloud handshakes.
   - EON utilizes **Predictive Pre-Cognition** during streaming ASR and a **Tiered Inference Pipeline**:
     - **Tier 0 (Reflex)**: < 20ms deterministic heuristic / local embedding matcher + Acoustic Bridge tokens.
     - **Tier 1 (Tactical)**: < 120ms local quantized SLM (e.g., Gemma-2B int4 / Qwen-2.5-3B) running on-device via ONNX Runtime / NPU.
     - **Tier 2 (Strategic / Mesh)**: Asynchronous high-compute frontier models or workstation-class coding SLMs (Qwen-2.5-32B / Claude 3.5 Sonnet) connected via low-latency QUIC mesh.
3. **Execution Does Not Equal Success**:
   - An API returning `200 OK` or a function returning `void` is **not** proof of completion.
   - EON implements **Temporal State Delta Oracles** (hardware state queries, UI accessibility tree re-inspection, cryptographic hash checks, visual diffs) before reporting completion.
4. **Engine Optimized for Networking (Distributed Cognitive Mesh)**:
   - EON is not confined to one machine. It forms an encrypted, peer-to-peer cognitive mesh spanning mobile edge devices (Android), high-compute workstations, and persistent servers using P2P QUIC and CRDT memory synchronization.

---

## 2. High-Level System Architecture Diagram

```mermaid
flowchart TD
    subgraph Human["Human Operator (Rimuru)"]
        UserVoice["Voice Input (Mic)"]
        UserScreen["Device Screen / App State"]
        UserSpeaker["Audio Response (Speaker)"]
    end

    subgraph PerceptionEngine["Perception Engine (< 100ms)"]
        VAD["Silero VAD (30ms chunks)"]
        ASR["Streaming Zipformer ASR (int8)"]
        PreCog["Predictive Pre-Cognition (Early-Exit Decoder)"]
        ScreenReader["Accessibility UI Tree / OmniParser-v2"]
        VideoSampler["Adaptive Keyframe Sampler (1-5 FPS)"]
        BargeIn["Acoustic Echo Canceller (AEC) & Barge-In Detector"]
    end

    subgraph CielCognition["Ciel Cognitive Core (Thought Acceleration)"]
        Router["Cognitive Router (Tier 0 / Tier 1 / Tier 2)"]
        Appraisal["Appraisal Engine (Context & AST Analyzer)"]
        MemoryLedger["CRDT Context Ledger + Graph Memory + SQLite Vector"]
        WorkflowCompiler["Objective Workflow Compiler (DAG Synthesizer)"]
    end

    subgraph CyberSubstrate["Information Particle Manipulation (Cyber Core)"]
        PolyglotSynthesizer["Extreme Polyglot Code Synthesizer (Rust/Kotlin/C/TS)"]
        CyberDefense["Aegis Cyber Defense (Taint Analysis, eBPF, Auto-Patch)"]
        CyberOffense["Vulnerability Research (Decompilation, Fuzzing, Symbolic Exec)"]
    end

    subgraph ExecutionKernel["Vector Execution Kernel (Deterministic Execution)"]
        ObjMgr["Objective Manager"]
        Scheduler["Resource-Aware DAG Task Scheduler (Epoch Leases)"]
        ResourceManager["Atomic Resource Leases (CPU / Memory / I/O)"]
        Dispatcher["Ephemeral Task Dispatcher (Saga Compensation)"]
        Journal["Append-Only Event Journal (Durable State)"]
        Reconciler["Crash Recovery Reconciler"]
    end

    subgraph DistributedMesh["Distributed Networking Mesh (EON Network Engine)"]
        QuicMesh["Multiplexed P2P QUIC Transport (< 10ms intra-LAN)"]
        SplitInfer["Split-Inference Coordinator (Phone <-> Workstation)"]
        CrdtSync["CRDT State Synchronizer"]
    end

    subgraph CapabilityFabric["Capability Fabric (The Real World)"]
        OSCap["Local OS / Shell / File Capabilities"]
        AndroidCap["Android Accessibility & System Controller"]
        NetCap["Network / HTTP / Web Scraper Capabilities"]
        CyberCap["Binary & Cyber Capabilities (ELF/DEX/PacketCraft)"]
    end

    subgraph VoiceSynthesis["Voice Pipeline (< 150ms TTFA)"]
        CommitmentGate["Acoustic Commitment Gate"]
        TTSChunker["Token-to-Audio Streaming Buffer"]
        NeuralTTS["Kokoro-82M / Piper Streaming TTS"]
        AudioOut["Audio Track Output Stream"]
    end

    %% Wiring
    UserVoice --> VAD
    VAD --> ASR
    ASR --> PreCog
    PreCog --> Router
    ASR -->|Streaming Text Tokens| Router
    UserScreen --> ScreenReader
    ScreenReader -->|UI Tree State| Appraisal

    Router -->|Reflex Intent| WorkflowCompiler
    Router -->|Context Query| MemoryLedger
    MemoryLedger --> Appraisal
    Appraisal --> PolyglotSynthesizer
    Appraisal --> CyberDefense
    Appraisal --> CyberOffense
    CyberSubstrate --> WorkflowCompiler

    WorkflowCompiler -->|Validated Workflow DAG| ObjMgr
    ObjMgr --> Scheduler
    Scheduler --> ResourceManager
    ResourceManager --> Dispatcher
    Dispatcher --> OSCap
    Dispatcher --> AndroidCap
    Dispatcher --> NetCap
    Dispatcher --> CyberCap
    Dispatcher <==> QuicMesh
    QuicMesh <==> SplitInfer
    QuicMesh <==> CrdtSync
    CrdtSync <==> MemoryLedger

    OSCap & AndroidCap & NetCap & CyberCap -->|Task Outcomes| Journal
    Journal --> Reconciler
    Journal -->|Verified Event| CielCognition

    CielCognition --> CommitmentGate
    CommitmentGate -->|Streaming Verbal Response| TTSChunker
    TTSChunker --> NeuralTTS
    NeuralTTS --> AudioOut
    AudioOut --> UserSpeaker

    UserVoice -.->|Interrupt Signal| BargeIn
    BargeIn -.->|Instant Mute / Abort| AudioOut
```

---

## 3. End-to-End Latency Budget & Sequence Flow

The benchmark for human conversational comfort is **< 300ms** from end of speech to start of response. EON achieves sub-200ms using **Predictive Pre-Cognition** and **Streaming Clause Synthesis**:

| Stage | Subsystem | Target Latency | P99 Latency | Implementation Technique |
|---|---|---|---|---|
| 1 | VAD (End-of-Utterance) | 30ms | 50ms | Silero VAD (ONNX), 30ms window sliding evaluation |
| 2 | Streaming ASR Token Finalization | 40ms | 80ms | Sherpa-onnx Zipformer int8 streaming decoder |
| 3 | Predictive Pre-Cognition | Concurrently | Concurrently | Evaluates partial tokens *during* speech; pre-warms KV-cache |
| 4 | Cognitive Routing & Acoustic Bridge | 20ms | 40ms | Tier 0 emits acoustic bridge ("Affirmative.") immediately |
| 5 | Token Streaming Generation | 30ms | 60ms | Local quantized SLM (Gemma-2B/Qwen-2.5-3B) with first token in < 30ms |
| 6 | Streaming Neural TTS Synthesis | 40ms | 80ms | Kokoro-82M ONNX chunked synthesis (first 2-3 words) |
| 7 | Audio Driver Output Buffer | 10ms | 20ms | Low-latency audio stream buffer (AAudio / Oboe / WASAPI) |
| **TOTAL** | **Full Speech-to-Audio (TTFA)** | **170ms** | **280ms** | **Exceeds human baseline conversational speed** |

### Execution Sequence Diagram

```mermaid
sequenceDiagram
    autonumber
    actor User as Rimuru (User)
    participant Audio as Voice Engine (VAD/ASR)
    participant PreCog as Predictive Pre-Cognition
    participant Ciel as Ciel Cognitive Router
    participant Cyber as Cyber Substrate (IPM)
    participant Kernel as Vector DAG Kernel
    participant TTS as Neural Streaming TTS

    User->>Audio: "Audit the auth module for timing attacks and patch it."
    Audio->>PreCog: Streaming partial tokens: ["Audit", "auth", "module"] (T+40ms)
    PreCog->>Ciel: Pre-warm AST inspection templates (T+45ms)
    Audio->>Ciel: Final Transcript Validated (T+70ms)
    
    activate Ciel
    Ciel->>TTS: Stream Acoustic Bridge (T+90ms): "Affirmative."
    TTS-->>User: Audio Out (T+170ms)
    
    Ciel->>Cyber: Execute Static Taint & AST Analysis (T+110ms)
    Cyber->>Cyber: Pinpoint string comparison in AuthService.kt
    Cyber->>Cyber: Synthesize constant-time MessageDigest patch
    Cyber->>Kernel: Submit Verified Workflow DAG (T+160ms)
    deactivate Ciel

    activate Kernel
    Kernel->>Kernel: Acquire Epoch Lease & Dispatch Shadow Sandbox
    Kernel->>Kernel: Apply patch & compile & run test suite (T+350ms)
    Kernel->>Ciel: Task Verified (All tests pass) (T+370ms)
    deactivate Kernel

    activate Ciel
    Ciel->>TTS: Stream Result: "Report: Timing attack patched in AuthService.kt using constant-time comparison. Tests verified." (T+390ms)
    TTS-->>User: Audio Output Stream (T+470ms)
    deactivate Ciel
```

---

## 4. Repository & Module Directory Layout

The codebase in `./` is organized across dedicated, decoupled native Rust crates, platform clients, and definitive specifications:

```text
EON/
├── README.md                              # System Overview, Vision & Ciel Pillars
├── AGENT_EXECUTION_GUIDE.md               # Mandatory Agent Zero-Question Protocol
├── proto/                                 # Wire Serialization Contracts
│   └── eon.proto                          # P2P QUIC Mesh RPC & Desktop Daemon IPC
├── docs/                                  # Definitive Specifications
│   ├── ARCHITECTURE.md                    # This document
│   ├── CIEL_CORE_SPECIFICATION.md         # Cognitive Soul & Thought Acceleration
│   ├── CYBER_AND_PROGRAMMING_MASTERY.md   # Information Particle Manipulation & Cyber
│   ├── DISTRIBUTED_NETWORKING_MESH.md     # Engine Optimized for Networking Spec
│   ├── VOICE_PIPELINE_SPECIFICATION.md    # Low-latency duplex STT/TTS & AEC loopback
│   ├── MULTIMODAL_VISION_AND_RENDERING.md # Vision, OmniParser, Keyframes & Artifact Canvas
│   ├── FOUNDATION_AND_PRIMITIVES_SPECIFICATION.md # Level 0 & 1 Primitives, Journal & Identity
│   ├── EXECUTION_KERNEL_SPECIFICATION.md  # Vector DAG Execution Kernel
│   ├── CAPABILITY_FABRIC_SPECIFICATION.md # Capability & Tool Runtime (LSP, Sandboxing)
│   ├── MEMORY_AND_APPRAISAL_SPECIFICATION.md # Hybrid memory (CRDT source-of-truth + SQLite projection)
│   ├── STATE_MACHINE_AND_INVARIANTS.md    # State machine & safety rules
│   ├── ROADMAP_AND_IMPLEMENTATION_PLAN.md # Phased implementation backlog
│   ├── SYSTEM_PROMPTS_AND_PERSONA.md      # Ciel Prompts & Output Schemas
│   ├── MODELS_AND_WEIGHTS_SETUP.md        # Model weights, profiles & checksum downloader
│   ├── OPERATING_MODES_AND_PRESENCE.md    # 5 Canonical Operating Modes
│   ├── ANDROID_INTEGRATION_SPECIFICATION.md # Android presence, accessibility & native audio
│   ├── TECH_STACK_AND_TOOLCHAINS.md       # Toolchains, Cargo workspace & UniFFI bridge
│   └── decisions/                         # Architectural Decision Records (ADRs)
│
├── engine/                                # Native Core Engine Workspace (Rust 2024 Edition)
│   └── core-rs/
│       ├── Cargo.toml                     # Cargo workspace root manifest
│       └── crates/
│           ├── eon-primitives/            # Level 0: ULIDs, MonotonicTime, EonResult, EonError
│           ├── eon-foundation/            # Level 1: Append-only event journal, Identity, State Store
│           ├── eon-kernel/                # Level 2 & 3: DAG scheduler, Epoch leases, Saga reconciler
│           ├── eon-capability/            # Level 4: Tool registry, LSP client, Sandboxes, Process exec
│           ├── eon-cyber/                 # Information Particle Manipulation (AST, LibAFL, Z3, P-Code)
│           ├── eon-mesh/                  # P2P QUIC transport (iroh), Loro CRDT synchronizer
│           ├── eon-voice/                 # Audio loop (CPAL/AAudio), VAD, ASR, Kokoro TTS, AEC loopback
│           ├── eon-vision/                # OmniParser UI grounding, 1-5 FPS adaptive keyframe sampler
│           ├── eon-cognition/             # Level 5: Thought Acceleration router, Ciel persona
│           └── eon-uniffi/                # UniFFI interface definitions for Kotlin & Swift
│
├── perception/                            # Perception & Speech Pipeline
│   └── models/                            # Quantized model weights & ONNX graphs (Silero, Kokoro, OmniParser)
│
├── clients/                               # Interactive Frontends
│   ├── cli/                               # Interactive terminal interface (ratatui + ratatui-image)
│   ├── android/                           # Android persistent presence, accessibility & overlay (Kotlin)
│   ├── canvas/                            # Live interactive Artifact Canvas (wgpu / Compose Desktop)
│   └── daemon/                            # System background service & IPC
│
└── tests/                                 # Full-System & Integration Test Suites
    ├── latency-benchmarks/                # Sub-200ms TTFA verification
    ├── cyber-security-tests/              # Vulnerability detection & fuzzing tests
    ├── mesh-network-tests/                # Split-inference & CRDT sync tests
    └── crash-recovery-tests/              # Mid-task kill and reconciliation tests
```
