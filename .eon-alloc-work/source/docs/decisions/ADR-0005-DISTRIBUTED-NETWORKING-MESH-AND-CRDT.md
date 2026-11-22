# ADR-0005: Distributed Cognitive Mesh via P2P QUIC & CRDT State Synchronization

## Status
Accepted

## Context
EON is named **Engine Optimized for Networking**. An AI personal assistant constrained to a single device suffers from fundamental physical trade-offs:
- Mobile devices have battery and thermal ceilings that prevent running 32B+ parameter models or heavy compilation toolchains locally.
- Workstations have powerful compute and GPUs, but lack ubiquitous microphone proximity and mobile accessibility hooks.
- Cloud-centric systems sacrifice privacy, introduce network jitter (> 500ms latency), and fail completely when offline.

We require an architecture that operates as a single unified cognitive presence distributed across the user's personal hardware fleet.

## Decision
1. **Multiplexed P2P QUIC Mesh**:
   - Deploy QUIC (RFC 9000) over UDP with Mutual TLS 1.3 encryption using local ed25519 identity keys.
   - Sub-10ms RPC latency across local Wi-Fi / LAN with zero head-of-line blocking and seamless connection migration.
   - Automatic local discovery via mDNS and Bluetooth Low Energy (BLE).
2. **Split-Inference Execution**:
   - Edge Node (Android phone): Handles voice capture, Silero VAD, streaming Zipformer ASR, and UI actions (< 60ms).
   - High-Compute Node (Workstation): Receives streaming tokens, executes high-parameter SLMs (Qwen-2.5-Coder 32B), deep AST refactoring, binary disassembly, and compiler passes.
   - Streams audio/result tokens back to the phone speaker in real-time.
   - Graceful offline fallback: Automatically drops to on-device quantized models if disconnected.
3. **CRDT Cognitive State Synchronization**:
   - Working context ledger, knowledge graph triples, and task execution journals are modeled as Conflict-free Replicated Data Types (CRDTs).
   - Simultaneous user interactions across phone and desktop merge deterministically without central database locks.

## Consequences
- EON achieves the compute capacity of a workstation cluster with the physical convenience and presence of a smartphone.
- Zero cloud dependency for personal hardware clustering.
- Sub-200ms voice conversational speed maintained even during heavy compile and cyber tasks.

