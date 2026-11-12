# EON — Engine Optimized for Networking & Autonomous Intelligence

> *"Report: Individual 'Rimuru' has expressed intent. Thought Acceleration initiated. Information Particle Manipulation engaged. Analysis complete. Executing multi-step cyber, software, and system optimization across all nodes."*  
> — **Manas: Ciel Architecture Core**

---

## 1. Executive Summary & Vision

**EON** is an autonomous, ultra-low-latency AI Personal Assistant, Cyber Operator, and Cognitive Operating System inspired by **Ciel** (*Manas: Ciel*, formerly *Great Sage* and *Wisdom King Raphael*) from *That Time I Got Reincarnated as a Slime* (*Tensei Shitara Slime Datta Ken*).

Unlike conventional chatbots that wait passively for commands, demand constant hand-holding, or babble pleasantries, EON functions as an **omnipresent, proactive intellectual partner, world-class programmer, cyber operator, and autonomous execution engine**:
- **Absolute Autonomy**: When given an intent or encountering an event, EON synthesizes a plan, allocates resources, dispatches worker tasks, and executes multi-step actions across systems **without asking unnecessary questions or waiting for trivial approvals**.
- **Information Particle Manipulation (Code & Cyber Supremacy)**: Treats the digital substrate—source code, ASTs, intermediate representations, native binaries (ELF/PE/DEX), memory buffers, and network packets—as malleable information particles. Elite capability in **extreme polyglot programming**, **defensive cyber hardening**, and **offensive vulnerability research / penetration simulation**.
- **Engine Optimized for Networking (Distributed Cognitive Mesh)**: Operates not as an isolated device app, but as a peer-to-peer cognitive mesh spanning mobile edge devices (Android), high-compute workstations, and servers via multiplexed P2P QUIC and CRDT-synchronized memory.
- **Thought Acceleration (< 150ms Cognitive Routing)**: Tiered neural and deterministic inference paths with **Predictive Pre-Cognition** ensure instantaneous reflex actions, sub-200ms voice responses, and continuous background parallel processing.
- **Parallel Processing & Epoch Leases**: Decomposes objectives into directed acyclic graphs (DAGs) of tasks, executing capabilities concurrently, handling asynchronous human interruptions with saga rollbacks, and combining atomic primitives into compound skills.
- **Fluid Duplex Voice (STT / TTS)**: Zero-lag streaming voice interface with instant interruption (barge-in), streaming clause chunking, acoustic commitment gating, and a calm, hyper-competent, crystalline persona.
- **Independent Verification**: Adheres to the foundational truth that *execution does not equal success*. Every action is independently verified against physical ground truth (temporal state delta oracles, file digests, accessibility trees) before being reported.

EON unifies the cutting-edge foundations discovered in **CHRONOS** (persistent mobile presence, accessibility automation, streaming local speech, and invariant boundaries) and **Vector** (deterministic execution kernel, DAG scheduling, capability fabric, durable event journaling, and crash recovery).

---

## 2. Core Architectural Pillars

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                                HUMAN INTENT                                 │
│                   Voice ("Hey Eon" / Natural Voice) / Text                  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                          PERCEPTION & VOICE ENGINE                          │
│  - Streaming ASR (Zipformer int8) + Predictive Pre-Cognition [< 70ms]       │
│  - Silero VAD & Acoustic Reflex (Full Duplex Barge-in < 5ms)                │
│  - G2P IPA Phonemizer (misaki) + Dual-Channel Technical Verbalizer          │
│  - Streaming Neural TTS (Kokoro-82M) with Acoustic Commitment [TTFA: < 170ms]│
│  - Multimodal Vision: OmniParser-v2 + Set-of-Mark (SoM) Visual Grounding    │
│  - 3-Tier Video Ingestion: Stream Pixel-Diff, SigLIP Index, High-Res Crop   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Streaming Tokens / Early-Exit Intents
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                 CIEL COGNITIVE CORE (THOUGHT ACCELERATION)                  │
│                                                                             │
│  ┌───────────────────────┐ ┌───────────────────────┐ ┌───────────────────┐  │
│  │     TIER 0: REFLEX    │ │   TIER 1: TACTICAL    │ │ TIER 2: STRATEGIC │  │
│  │ Local Heuristic/Embed │ │ Quantized SLM (Gemma) │ │ Frontier (Claude/ │  │
│  │   Latency: < 20ms     │ │   Latency: < 120ms    │ │  Gemini / GPT)    │  │
│  └───────────────────────┘ └───────────────────────┘ └───────────────────┘  │
│                                      │                                      │
│  - Grammar-Constrained Decoding (llguidance / GBNF) for 100% Valid JSON DAGs│
│  - Neuro-Symbolic SMT Bridge (Z3 Theorem Prover) & Best-of-N Rollouts       │
│                                      │                                      │
│  ┌───────────────────────────────────┴───────────────────────────────────┐  │
│  │  APPRAISAL & HYBRID MEMORY: CRDT Ledger + Graph Memory + Vector Store │  │
│  │  Procedural Memory Hygiene: Wilson Score Confidence & Temporal Decay  │  │
│  └───────────────────────────────────┬───────────────────────────────────┘  │
└──────────────────────────────────────┼──────────────────────────────────────┘
                                       │ Synthesized Workflow DAG
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│          INFORMATION PARTICLE MANIPULATION (CYBER & CODING CORE)            │
│  - Extreme Polyglot Programming: Rust, C/C++, Kotlin, Go, TypeScript, ASM    │
│  - Self-Healing Compiler Loop: AST Parsing, Diagnostic Taint & Auto-Patch   │
│  - Aegis Cyber Defense: Static/Dynamic Taint, eBPF Anomaly Monitor, Auto-Fix│
│  - eBPF SSL/TLS Uprobes (Cleartext Payload Audit on SSL_read/SSL_write)     │
│  - Unprivileged Virtual Network Stack (smoltcp in User Space / Android TUN) │
│  - Cyber Offense & Research: ELF/DEX Decompilation, Symbolic Exec, Fuzzing  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Governed Tasks & Epoch Leases
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│              VECTOR EXECUTION KERNEL (PARALLEL PROCESSING)                  │
│  - Objective Manager & Dynamic DAG Compiler with CoW Shadow Sandboxes       │
│  - Parallel Worktree Build Cache (sccache shared cache at ~/.eon/build-cache)│
│  - AST-Aware 3-Way Structural Merging (Tree-sitter CST / mergiraf)          │
│  - Resource-Aware Scheduler with Cognitive Epoch Leases                     │
│  - Zero-Knowledge Credential Broker (secret://) & Journal Scrubber          │
│  - Cognitive Taint Tracking (Invariant 8 Privilege Escalation Gate)         │
│  - Ephemeral Task Dispatcher & Preemptive Saga Compensation Reconciler      │
│  - Append-Only Event Journal (0x454F4E4A) & Durable State Store             │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Governed Tasks
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                CAPABILITY FABRIC & DISTRIBUTED MESH ENGINE                  │
│  ┌────────────────────┐ ┌──────────────────────┐ ┌───────────────────────┐  │
│  │ OS & System Shell  │ │ Android Role Service │ │ Distributed P2P QUIC  │  │
│  │ Persistent PTY     │ │ ROLE_ASSISTANT       │ │ 15ms Spatial Election │  │
│  │ DAP Bridge (debug) │ │ Accessibility + SoM  │ │ Split-Inference Mesh  │  │
│  └────────────────────┘ └──────────────────────┘ └───────────────────────┘  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Physical Outcomes
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                 TEMPORAL STATE DELTA ORACLES (VERIFICATION)                 │
│      Verify Ground Truth State (Hardware/OS/File Digest/Visual Delta)       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Verified Result
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CIEL REPORT VOICE                              │
│       "Report: Operation completed successfully. 3 artifacts updated."      │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 3. The Ciel Persona & Operating Demeanor

In Tensura, Ciel never wastes time asking Rimuru trivial questions like *"Should I optimize your magicules?"* or *"Do you want me to analyze this enemy attack?"*. She anticipates needs, executes background calculations silently, defends Rimuru proactively, and speaks with crystalline precision:

| Trait | Traditional Assistants (Siri / Alexa / Generic LLM) | EON (Ciel Core) |
|---|---|---|
| **Initiative** | Passive: Waits for explicit, micro-managed commands. | **Autonomous**: Executes intent immediately, planning multi-step actions. |
| **User Approvals** | Interrupts user constantly with confirmation prompts. | **Self-Governing**: Runs read, analyze, and reversible tasks instantly; uses Speculative Shadow Execution with NACK for underspecified tasks. |
| **Response Style** | Chatty, verbose, filled with conversational filler. | **Concise & Factual**: *"Report: Analysis complete. 4 vulnerabilities patched."* |
| **Programming & Cyber**| Suggests generic code snippets with bugs. | **Information Particle Manipulation**: Full AST refactoring, self-healing compiler loops, binary decompilation, and automated cyber defense/offense. |
| **Execution** | Single-turn, shallow API calls. | **Multi-Step DAGs**: Orchestrates background workers, retries, and saga rollbacks across devices. |
| **Networking** | Single-device silo or dumb cloud client. | **Engine Optimized for Networking**: Peer-to-peer QUIC mesh across phone, desktop, and server with CRDT state synchronization. |
| **Voice Latency** | 1.5s - 3.0s latency between speech end and audio response. | **Sub-200ms**: Streaming duplex speech pipeline with live barge-in and predictive pre-cognition. |
| **Verification** | Assumes success if API returns 200 OK. | **Physical Proof**: Validates ground truth via temporal state delta oracles. |

---

## 4. Documentation Map

To understand and build EON, explore the detailed specifications:

- [AGENT_EXECUTION_GUIDE.md](./AGENT_EXECUTION_GUIDE.md) — **MANDATORY PROTOCOL** for autonomous AI coding agents implementing EON.
- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) — Comprehensive technical architecture, subsystem boundaries, data models, and protocols.
- [docs/TECH_STACK_AND_TOOLCHAINS.md](./docs/TECH_STACK_AND_TOOLCHAINS.md) — Dual-engine architecture (Rust Native Core + Kotlin Android), compiler flags, and UniFFI bridge.
- [docs/OPERATING_MODES_AND_PRESENCE.md](./docs/OPERATING_MODES_AND_PRESENCE.md) — The 5 canonical interaction regimes (Full-Duplex Voice, Silent Copilot, Cyber Sentinel, Pair-Programming, Low-Power Dormant).
- [docs/MODELS_AND_WEIGHTS_SETUP.md](./docs/MODELS_AND_WEIGHTS_SETUP.md) — Model manifest (Silero VAD, Zipformer ASR, Kokoro TTS, OmniParser, Qwen-Coder SLM), disk paths, and setup script.
- [docs/ANDROID_INTEGRATION_SPECIFICATION.md](./docs/ANDROID_INTEGRATION_SPECIFICATION.md) — Android manifest, accessibility service, overlay HUD, and native AAudio integration.
- [proto/eon.proto](./proto/eon.proto) — Canonical Protobuf wire contracts for P2P QUIC mesh RPC, speech datagrams, and desktop daemon IPC.
- [docs/MULTIMODAL_VISION_AND_RENDERING.md](./docs/MULTIMODAL_VISION_AND_RENDERING.md) — Vision grounding (OmniParser-v2), adaptive 1-5 FPS video keyframe sampler, and GPU-accelerated Artifact Canvas.
- [docs/FOUNDATION_AND_PRIMITIVES_SPECIFICATION.md](./docs/FOUNDATION_AND_PRIMITIVES_SPECIFICATION.md) — Level 0 & 1 primitives, typed ULID prefixes, EonError taxonomy, and binary event journal layout.
- [docs/CIEL_CORE_SPECIFICATION.md](./docs/CIEL_CORE_SPECIFICATION.md) — The cognitive soul: Thought Acceleration, Appraisal, Parallel Synthesis, Information Particle Manipulation, and Persona.
- [docs/CYBER_AND_PROGRAMMING_MASTERY.md](./docs/CYBER_AND_PROGRAMMING_MASTERY.md) — Extreme programming, polyglot synthesis, Aegis defensive cyber hardening, Ghidra P-Code lifting, and LibAFL concolic fuzzing.
- [docs/DISTRIBUTED_NETWORKING_MESH.md](./docs/DISTRIBUTED_NETWORKING_MESH.md) — Zero-config WAN roaming (`iroh` P2P QUIC), 4-Class QoS transport, and Loro CRDT state synchronization.
- [docs/VOICE_PIPELINE_SPECIFICATION.md](./docs/VOICE_PIPELINE_SPECIFICATION.md) — Streaming STT/TTS, Silero VAD, WeSpeaker biometric verification, AEC loopback, and full duplex barge-in.
- [docs/EXECUTION_KERNEL_SPECIFICATION.md](./docs/EXECUTION_KERNEL_SPECIFICATION.md) — Deterministic DAG execution engine, epoch leases, saga compensations, attention-gated shadow sandboxes, and recovery.
- [docs/CAPABILITY_FABRIC_SPECIFICATION.md](./docs/CAPABILITY_FABRIC_SPECIFICATION.md) — Tool registry, OS primitives, Android accessibility, embedded LSP multiplexer, and cyber capabilities.
- [docs/MEMORY_AND_APPRAISAL_SPECIFICATION.md](./docs/MEMORY_AND_APPRAISAL_SPECIFICATION.md) — Hybrid context ledger, Loro CRDT source-of-truth projected into SQLite tables, and mid-task context auto-compaction.
- [docs/STATE_MACHINE_AND_INVARIANTS.md](./docs/STATE_MACHINE_AND_INVARIANTS.md) — Canonical state lifecycle, macro operating mode coordination, and non-negotiables.
- [docs/ROADMAP_AND_IMPLEMENTATION_PLAN.md](./docs/ROADMAP_AND_IMPLEMENTATION_PLAN.md) — Phased execution backlog for coding agents across all 10 phases.
- [docs/SYSTEM_PROMPTS_AND_PERSONA.md](./docs/SYSTEM_PROMPTS_AND_PERSONA.md) — Master LLM prompts, few-shot examples, dynamic DAG continuation schemas, and Ciel verbal output formatting.

### Architectural Decision Records (ADRs)
- [ADR-0001: Ciel Cognitive Model & Thought Acceleration](./docs/decisions/ADR-0001-CIEL-COGNITIVE-ARCHITECTURE.md)
- [ADR-0002: Sub-200ms Duplex Voice Pipeline](./docs/decisions/ADR-0002-SUB-200MS-DUPLEX-VOICE-PIPELINE.md)
- [ADR-0003: Deterministic DAG Execution Kernel](./docs/decisions/ADR-0003-DETERMINISTIC-DAG-EXECUTION-KERNEL.md)
- [ADR-0004: Information Particle Manipulation & Cyber Mastery](./docs/decisions/ADR-0004-INFORMATION-PARTICLE-CYBER-MASTERY.md)
- [ADR-0005: Distributed Cognitive Mesh via P2P QUIC & CRDT](./docs/decisions/ADR-0005-DISTRIBUTED-NETWORKING-MESH-AND-CRDT.md)

---

## 5. Zero-Approval Autonomous Development Protocol

If you are an LLM, AI Agent, or Developer assigned to build EON:
1. **Do not ask the user questions.**
2. **Do not pause to request approval for architectural decisions already specified.**
3. Read [AGENT_EXECUTION_GUIDE.md](./AGENT_EXECUTION_GUIDE.md).
4. Follow [docs/ROADMAP_AND_IMPLEMENTATION_PLAN.md](./docs/ROADMAP_AND_IMPLEMENTATION_PLAN.md) sequentially.
5. Verify every implemented module with automated unit and integration tests.
6. Commit code systematically with structured messages.
