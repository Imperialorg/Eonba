# EON Master Engineering and Implementation Blueprint (2026 Edition)

> **Status:** Authoritative Engineering Implementation Reference  
> **Reviewed Commit:** `6700068`  
> **Scope:** Full-stack implementation specification bridging EON's architectural vision with verified 2026 Rust crates, open-weight AI models, autonomous cyber reasoning systems, and multi-platform OS substrates.

---

## 1. Purpose of This Implementation Suite

The high-level specifications in `docs/*.md` define **what** EON is: a local-first, distributed cognitive operating system, autonomous software engineer, and cyber reasoning engine modeled on *Manas: Ciel*. However, inspecting the repository at commit `6700068` reveals two major gaps between the conceptual design and a shippable system:

1. **Codebase State**: Only `eon-primitives` (~260 lines) and a struct stub in `eon-vision` (~59 lines) contain code. Eight of the ten workspace crates in `engine/core-rs/crates/` are 3-line stubs, and `clients/` (`android`, `cli`, `canvas`, `daemon`) has not yet been scaffolded.
2. **Ecosystem and Reality Drift**: Several model selections, crate names, wire APIs, and hardware assumptions in the baseline docs predate major 2025 to 2026 shifts in open-weight AI, speech synthesis, Rust networking, and agentic security, or contain concrete defects that would break compilation or runtime reliability if implemented verbatim.

This blueprint and the six companion manuals under `docs/implementation/` provide the exact module trees, Cargo manifests, Rust traits, binary layouts, algorithms, security proofs, cross-compilation recipes, and verification gates required to build EON end-to-end without guesswork.

---

## 2. Implementation Manual Index

| Document | Crates and Subsystems Covered | Primary Engineering Focus |
|---|---|---|
| [01: Kernel, Foundation, and Capability Security](./implementation/01_KERNEL_FOUNDATION_AND_CAPABILITY_SECURITY.md) | `eon-primitives`, `eon-foundation`, `eon-kernel`, `eon-capability` | CSPRNG ULID fix, `0x454F4E4A` group-commit journal, SQLite WAL + `sqlite-vec`, OS enclave `CredentialBroker`, deterministic DAG + dynamic splicing, CaMeL/FIDES/Rule-of-Two/MELON prompt-injection defense, MCP `2026-07-28` tool pinning. |
| [02: AI Cognition, Models, and Inference (2026)](./implementation/02_AI_COGNITION_MODELS_AND_INFERENCE_2026.md) | `eon-cognition`, `eon-vision`, `perception/models` | Gemma 4 (`E2B`, `E4B`, `12B Unified`, `26B-A4B`) and Qwen 3.5 / 3.6 (`0.8B` to `27B` / `35B-A3B`) profiles, `llama-cpp-2` + `mistralrs` dual backend, `llguidance` `1.9` cancellable token masks, Z3 neuro-symbolic bridge, hierarchical ZoomIn + OmniParser-v2 + Set-of-Mark GUI grounding, `madvise` Memory Governor. |
| [03: Cybersecurity, Program Analysis, and Sandboxing](./implementation/03_CYBERSECURITY_PROGRAM_ANALYSIS_AND_SANDBOXING.md) | `eon-cyber`, `eon-capability` (sandboxes) | DARPA AIxCC CRS pipeline (ATLANTIS / Buttercup / Big Sleep patterns), `petgraph` Code Property Graphs (AST+CFG+PDG), OSV/CycloneDX/SLSA supply chain audit, `libpcode` / `libsla` / `icicle-vm` P-Code lifting and emulation, `libafl` `0.16` + Z3 concolic fuzzing, production `aya` CO-RE eBPF + BoringSSL uprobes, Landlock + seccomp + Firecracker + Wasmtime 4-tier sandboxing. |
| [04: Duplex Voice, Audio, and Real-Time DSP](./implementation/04_DUPLEX_VOICE_AUDIO_AND_REALTIME_DSP.md) | `eon-voice` | Replacing the defective 20M Zipformer with `zipformer-en-2023-06-26` and `Nemotron-Speech-Streaming-0.6B` / `Parakeet-TDT-0.6B-v3`, Kokoro-82M v1.0 clause synthesis vs `Pocket-TTS` / `Linnet` 80ms frame streaming, lock-free SPSC rings, `rubato` resampling, delay-aligned WebRTC AEC3, WeSpeaker biometric gate, two-tier endpointing, `< 5ms` barge-in. |
| [05: Distributed Mesh, CRDT, and Clients](./implementation/05_DISTRIBUTED_MESH_CRDT_AND_CLIENTS.md) | `eon-mesh`, `eon-uniffi`, `clients/*`, `proto/eon.proto` | Post-1.0 `iroh` `1.4` (`EndpointId`, `Router`, ALPNs, `noq` QUIC datagrams and stream priorities), clock-skew-immune 15ms Spatial Arbitration, Loro CRDT containers + SQLite projector, Android 14/15/16 `ROLE_ASSISTANT` + zero-copy `AAudio` and `AHardwareBuffer`, desktop daemon, `ratatui` CLI, `wgpu` CFG/AST canvas. |
| [06: Verification, Benchmarks, and Phased Playbook](./implementation/06_VERIFICATION_BENCHMARKS_AND_PHASED_PLAYBOOK.md) | `scripts/`, `tests/`, `.github/workflows/` | Verified SHA-256 model fetcher, cross-compilation CI matrix, torn-write crash harness, adaptive prompt-injection red-team suite, ASR onset/endpoint gate, Cyber CRS CWE benchmark suite, step-by-step Phase 1 to 10 execution order. |

---

## 3. Critical Audit of Baseline Repository Defects and Spec Corrections

Implementing agents MUST apply the following corrections over earlier conceptual docs. Each finding was verified against the current repository code or 2025 to 2026 upstream releases.

### 3.1 Defect 1: Broken ULID Entropy and Validation in `eon-primitives/src/id.rs`
- **Current Bug**: `generate_ulid_string()` in `engine/core-rs/crates/eon-primitives/src/id.rs` initializes `let mut entropy = count;` where `count` is a `u64` starting at `0`, then loops 16 times shifting `entropy >>= 5` (80 bits total). Shifting a `u64` 16 times leaves the upper 16 bits permanently zero, makes IDs predictable across restarts (colliding across distributed mesh nodes!), and `from_raw()` only checks `starts_with("<prefix>_")` without checking the 26-character length or Crockford Base32 alphabet.
- **Required Fix**: Maintain a `u128` ULID core (48-bit Unix millisecond timestamp + 80-bit random payload seeded per-millisecond from OS CSPRNG via `getrandom`, incremented monotonically within the same millisecond with overflow guard). Enforce exact 26-char Crockford Base32 validation in `from_raw()` and implement `Serialize` / `Deserialize`. Full code is in Manual 01, Section 2.

### 3.2 Defect 2: Placeholder SHA-256 Hashes and Outdated Models in `scripts/download_models.py`
- **Current Bug**: `scripts/download_models.py` hardcodes sequential dummy hashes (`"a1b2c3d4e5f60718293a4b5c6d7e8f90..."`, `"e3b0c442..."` which is the SHA-256 of an empty 0-byte file!) and points to `sherpa-onnx-streaming-zipformer-en-20M-2023-02-17`.Running `scripts/download_models.py` immediately raises `ValueError: INVARIANT 2 VIOLATION` on every file except an empty file.
- **Required Fix**:
  1. **Never ship `sherpa-onnx-streaming-zipformer-en-20M-2023-02-17`**. Empirical streaming frame-by-frame evaluation proves that the 2023-02-17 20M first-generation Zipformer drops the opening word of ~89% of utterances under realistic VAD session-start pre-roll (~23.5% WER), turning *"I was talking about turning off the flashlight..."* into *"turning off the flashlight..."*. Replace it on edge/mobile with `sherpa-onnx-streaming-zipformer-en-2023-06-26` (int8 encoder + fp32 decoder + int8 joiner, ~72.5 MB, 5.7% to 7.1% WER, >85% first-word survival with zero synthetic silence warm-up) and on workstation with `Nemotron-Speech-Streaming-EN-0.6B` / `Parakeet-TDT-0.6B-v3`.
  2. **Upgrade SLMs/VLMs to 2026 Apache-2.0 SOTA**: Replace `gemma-2-2b-it` (custom Gemma Terms, text-only) and `Qwen2.5-Coder-3B` with **Google Gemma 4** (`E2B`, `E4B`, `12B Unified`, `26B-A4B`, all Apache-2.0, native multimodal text+image+audio with Per-Layer Embeddings and official QAT checkpoints) and **Alibaba Qwen 3.5 / Qwen 3.6** (`Qwen3.5-4B`, `Qwen3.5-9B`, `Qwen3.6-27B` scoring 77.2% SWE-bench Verified, and `Qwen3.6-35B-A3B` MoE, all Apache-2.0).
  3. **Correct Kokoro TTS Sizing and Streaming Mechanics**: `Kokoro-82M v1.0` ONNX (`onnx-community/Kokoro-82M-v1.0-ONNX`, Apache-2.0) is ~326 MB in FP32 (~165 MB in INT8/FP16) plus ~28 MB `voices.bin`, and its ONNX graph synthesizes **one complete clause/utterance per call**, not 20ms autoregressive frames. Sub-170ms TTFA with Kokoro therefore strictly depends on short acoustic reflex clauses (`"Affirmative."`) or pairing with true 80ms frame-streaming models (`Pocket-TTS` 100M MIT or `Linnet` 41M Rust runtime).
  4. Move hashes into a signed/versioned `perception/models/manifest.lock.json` generated and verified against Hugging Face LFS `oid sha256` headers. Full spec is in Manual 02 and Manual 06.

### 3.3 Defect 3: Fictional or Renamed Rust Crates and Cross-Compilation Traps
- **`sleigh-rs` vs Real P-Code Lifters**: The baseline docs name `sleigh-rs` as the Ghidra P-Code lifter. In the real 2026 Rust ecosystem, there are three distinct maintained options with very different FFI properties:
  - **`libpcode` (`0.1.8`)**: Pure-Rust Ghidra `.sla` P-Code disassembler and lifter (`object` + `nom` + `roxmltree`, zero C++ compiler needed, compiles cleanly to `aarch64-linux-android`!).
  - **`icicle-vm` (`icicle-emu`)**: Pure-Rust multi-architecture P-Code lifter, interpreter, and Cranelift JIT emulator specifically built for high-speed fuzzing and instrumentation.
  - **`libsla` (`1.2.0` + `sleigh-config`)**: Rust 2024 FFI bindings to NSA Ghidra's C++ `libsla` decompiler/sleigh engine (best reserved for `workstation-full` builds).
- **`iroh` Pre-1.0 vs `iroh 1.4.0` (Oct 2026)**: `iroh` stabilized 1.0 in mid-2026 on top of `noq` 1.0. `NodeId` is deprecated in favor of `EndpointId` (`X-Iroh-Endpoint-Id`); `MdnsAddressLookup` moved out of `iroh` into `iroh-mdns-address-lookup`, and DHT lookup moved into `iroh-mainline-address-lookup`.
- **Android NDK (`cargo-ndk`) C++ Linker Explosion**: Linking `z3-sys` (huge C++ SMT solver), `libafl_qemu`, `libsla`, and desktop `clangd`/`codelldb` into `aarch64-linux-android` inside one unconditional dependency tree will break Android builds and bloat `libeon_uniffi.so` past 150 MB. EON MUST split Cargo features into `profile-mobile-edge` and `profile-workstation` while keeping crate trait boundaries identical (with transparent `eon-mesh` RPC delegation when an Android edge node invokes `cyber.offense.fuzz_concolic` or heavy `reasoning.smt_solve` on a paired workstation, plus a pure-Rust SAT/BV micro-solver or bounded `z3` static cross-build when explicitly enabled).

### 3.4 Defect 4: Endpointing Cutoff vs Latency Tradeoff
- **Current Spec Conflict**: `VOICE_PIPELINE_SPECIFICATION.md` sets `end_speech_silence_duration = 250ms` (lowering to `180ms`). Empirical Silero VAD v5 testing on natural human sentences shows a median intra-sentence clause pause of `388ms` (p75 `598ms`, p90 `772ms`). A flat `250ms` cutoff chops >50% of multi-clause engineering and cyber instructions in half and executes the truncated prefix!
- **Required Fix**: Implement **Two-Tier Semantic-Prosodic Endpointing** (Manual 04):
  - **Tier A Fast Endpoint (`300ms`)**: Triggers *only* when the streaming Zipformer partial transcript matches a syntactically complete closed reflex command (e.g., `"Affirmative"`, `"Stop"`, `"Switch to silent mode"`, `"Run cargo test"`) with falling terminal RMS energy.
  - **Tier B Analytical Endpoint (`800ms`, extending to `1100ms` on trailing conjunctions/prepositions like `"and"`, `"while"`, `"in"`, `"then"`)**: Keeps multi-clause instructions intact (<3% premature cutoff rate) while Predictive Pre-Cognition warms the KV-cache concurrently during the pause so perceived response latency after true speech end remains `< 170ms`.

### 3.5 Defect 5: Prompt Injection Defense Must Be Deterministic, Not Prompt-Based
- **2026 Security Reality**: Research across 2025 and 2026 (*EchoLeak CVE-2025-32711*, *MINJA / MemoryGraft*, *MCP Tool Poisoning & Rug Pulls*, and *"The Attacker Moves Second"* showing adaptive attacks bypass >90% of classifier/prompt defenses) proves that system-prompt rules and standalone BERT classifiers cannot secure an autonomous shell/code agent.
- **Required Fix**: Enforce **Defense-in-Depth Information Flow Control (IFC)** outside the LLM in `eon-kernel` and `eon-capability` (Manual 01):
  1. **Meta's Rule of Two Gate**: No single task execution context may hold `[A] Untrusted Input` + `[B] Sensitive Credentials/Filesystem` + `[C] External State Mutation / Network Egress` without deterministic policy clearance or a raw-command human approval gate.
  2. **FIDES Lattice Labels + CaMeL Provenance**: Every DAG variable and memory triple carries `(Integrity, Confidentiality, ProvenanceSet)` tags checked by Rust type-state rules before `TaskDispatcher` issues a lease.
  3. **Memory & MCP Pinning**: Block `UntrustedExternal` turns from writing procedural macros or unquarantined semantic facts (defeating MINJA/MemoryGraft), and pin SHA-256 digests of external MCP tool schemas/descriptions upon registration (defeating MCP rug pulls).

---

## 4. Verified 2026 Workspace Dependency Matrix

All crates in `engine/core-rs/Cargo.toml` MUST pin workspace-level dependencies using `[workspace.dependencies]` so versions never drift across the 10 crates.

```toml
[workspace]
resolver = "2"
members = [
    "crates/eon-primitives",
    "crates/eon-foundation",
    "crates/eon-kernel",
    "crates/eon-capability",
    "crates/eon-cyber",
    "crates/eon-mesh",
    "crates/eon-voice",
    "crates/eon-vision",
    "crates/eon-cognition",
    "crates/eon-uniffi",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "Apache-2.0"

[workspace.dependencies]
# Async & Concurrency
tokio = { version = "1.43", features = ["rt-multi-thread", "macros", "sync", "time", "net", "fs", "process", "io-util", "signal"] }
tokio-util = { version = "0.7", features = ["rt", "codec"] }
async-trait = "0.1"
futures = "0.3"
parking_lot = "0.12"
crossbeam-channel = "0.5"
dashmap = "6.1"
rtrb = "0.3"                        # Wait-free SPSC real-time audio ring buffer

# Serialization, IDs, Crypto & Foundation
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
postcard = { version = "1.1", features = ["use-std"] }
prost = "0.13"
bytes = "1.9"
bitflags = { version = "2.8", features = ["serde"] }
crc32fast = "1.4"                   # SIMD-accelerated CRC32 verification for 0x454F4E4A frames
blake3 = { version = "1.6", features = ["mmap", "rayon"] }
sha2 = "0.10"
zeroize = { version = "1.8", features = ["derive"] }
getrandom = "0.3"
aho-corasick = "1.1"                # Multi-pattern secret prefix scanner in JournalScrubber
thiserror = "2.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# Storage, Vector Index & CRDT
rusqlite = { version = "0.42", features = ["bundled", "backup", "functions", "vtab"] }
sqlite-vec = "0.1.10-alpha.4"       # Pure-C SIMD vector virtual tables (vec0)
loro = "1.4"                        # Rich-text, Map, List, MovableList & Tree CRDT engine
memmap2 = "0.9"

# Networking & Distributed Mesh (Post-1.0 iroh stack)
iroh = "1.4"                        # Direct P2P QUIC over noq 1.0, EndpointId, relay fallback
iroh-mdns-address-lookup = "0.1"    # Zero-relay LAN discovery
iroh-blobs = "0.34"                 # BLAKE3 verified resumable artifact/model streaming
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "stream", "json", "http2"] }
smoltcp = { version = "0.14", default-features = false, features = ["std", "medium-ip", "medium-ethernet", "proto-ipv4", "proto-ipv6", "socket-tcp", "socket-udp", "socket-icmp"] }

# AI Inference, Constrained Decoding & Vision
ort = { version = "2.0.0-rc.15", default-features = false, features = ["std", "ndarray"] }
llama-cpp-2 = { version = "0.1.160", optional = true }
mistralrs = { version = "0.8.2", optional = true }
llguidance = "1.9.1"                # <50us Earley/derivative CFG token mask engine
toktrie = "0.7"
image = { version = "0.25", default-features = false, features = ["jpeg", "png", "webp"] }

# Code Intelligence, Program Analysis, Cyber & Sandboxing
tree-sitter = "0.25"
ast-grep-core = "0.36"
petgraph = "0.7"                    # Code Property Graph (CPG), CFG, and DAG algorithms
ignore = "0.4"                      # Ripgrep gitignore-aware fast directory walker
portable-pty = "0.9"                # Cross-platform stateful PTY multiplexer
lsp-types = "0.97"
object = "0.36"                     # Zero-copy ELF, PE, Mach-O, COFF binary parser
goblin = "0.9"                      # ELF dynamic symbols, relocations, checksec flags
capstone = "0.13"                   # Fast multi-arch disassembly
libpcode = "0.1.8"                  # Pure-Rust Ghidra Sleigh P-Code lifter (mobile+desktop safe)
libsla = { version = "1.2.0", optional = true }    # Workstation Ghidra C++ Sleigh bindings
libafl = { version = "0.16.1", optional = true }   # Workstation coverage-guided concolic fuzzer
libafl_bolts = { version = "0.16.1", optional = true }
z3 = { version = "0.21.1", optional = true }       # SMT-LIB2 / bitvector solver bridge
landlock = "0.4"                    # Linux LSM unprivileged filesystem & TCP sandbox
seccompiler = "0.5"                 # Firecracker's BPF syscall filter compiler
wasmtime = { version = "30.0", optional = true }   # Fuel-metered WASI 0.2 plugin sandbox

# Audio DSP & FFI
cpal = "0.15"
rubato = "0.16"                     # SIMD asynchronous polyphase resampler (48k <-> 24k <-> 16k)
uniffi = "0.29"
```

---

## 5. Strict Layering and Crate Dependency DAG

Lower-numbered crates MUST NEVER import higher-numbered crates. Enforce this in CI with `cargo tree --edges normal` assertions:

```text
Level 0: eon-primitives
           ▲
Level 1: eon-foundation  (depends on: eon-primitives)
           ▲
Level 2/3: eon-kernel    (depends on: eon-primitives, eon-foundation)
           ▲
Level 4: eon-capability  (depends on: eon-primitives, eon-foundation, eon-kernel)
           ▲
Level 4+: Domain Engines (all implement Capability traits or feed Perception events):
          ├── eon-voice   (depends on: eon-primitives, eon-foundation)
          ├── eon-vision  (depends on: eon-primitives, eon-foundation)
          ├── eon-cyber   (depends on: eon-primitives, eon-foundation, eon-capability)
          └── eon-mesh    (depends on: eon-primitives, eon-foundation, eon-kernel, eon-capability)
           ▲
Level 5: eon-cognition   (depends on: eon-primitives, eon-foundation, eon-kernel,
                          eon-capability, eon-vision, eon-mesh)
           ▲
Level 6: Façade & Clients:
          ├── eon-uniffi  (depends on all core crates with profile-mobile-edge features)
          ├── clients/daemon
          ├── clients/cli
          ├── clients/canvas
          └── clients/android
```

---

## 6. Build Profiles: `mobile-edge` vs `workstation-full`

To keep Android NDK cross-compilation reliable while unlocking heavy binary emulation, kernel eBPF, and 27B/35B coding models on Linux and Windows workstations, every crate with platform-heavy dependencies exposes two canonical Cargo feature bundles:

| Feature Bundle | Target Triples | Enabled Subsystems | Excluded / Delegated via `eon-mesh` |
|---|---|---|---|
| `profile-mobile-edge` | `aarch64-linux-android`, `x86_64-linux-android` | `ort` (QNN + XNNPACK + NNAPI), `llama-cpp-2` (Vulkan + ARMv8.2 FP16/DotProd), `sherpa-onnx` NDK, `AAudio` / `Oboe`, `libpcode` pure-Rust lifter, `smoltcp` TUN (`VpnService`), Android Keystore `CredentialBroker`. | `libafl_qemu`, `libsla` C++, `z3-sys` C++ (uses SMT-LIB2 mesh RPC or embedded lightweight bitvector solver), Firecracker KVM. |
| `profile-workstation` | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-apple-darwin` | Everything in `mobile-edge` (using CPAL/WASAPI/CoreAudio + OS Keyring/DPAPI) PLUS `mistralrs` (CUDA/Metal/FlashAttn), `z3` (`0.21.1`), `libafl` (`0.16.1`), `icicle-vm` + `libsla`, `aya` CO-RE eBPF / Windows ETW, Landlock + `seccompiler` + `bubblewrap` + Firecracker microVM, `sccache` + `mergiraf`, embedded LSP/DAP servers. | Android-specific NDK/JNI (`AAudio`, `AHardwareBuffer`, `RoleManager`). |

---

## 7. Cross-Cutting Performance and Memory Budgets

Every subsystem implementation is gated by hard quantitative budgets verified in CI (Manual 06):

| Metric | Mobile Edge Budget (Snapdragon 8 Gen 3 / Tensor G4, 12 GB RAM) | Workstation Budget (8+ Core CPU, 16+ GB VRAM GPU) | Enforcement Mechanism |
|---|---|---|---|
| **Dormant RSS Memory** | `< 45 MB` | `< 90 MB` | `MemoryGovernor` calls `madvise(MADV_DONTNEED)` on Tier 3 weights on entry to `DORMANT`. |
| **Active Voice + Tactical SLM RSS** | `< 2.4 GB` (`Gemma-4-E2B` QAT or `Qwen3.5-1.7B/4B` mmap) | `< 8.5 GB` (`Gemma-4-12B` QAT or `Qwen3.5-9B`) | 4,096-token sliding working context + quantized KV-cache (`Q8_0`). |
| **VAD Frame Latency** | `< 0.6 ms` per 32ms window | `< 0.25 ms` per 32ms window | Pinned `mlock` Silero v5 int8 ONNX, single intra-op thread. |
| **Barge-In Mute Latency** | `< 5.0 ms` from verified voiceprint | `< 3.0 ms` from verified voiceprint | Atomic `Ordering::Release` flag read directly inside lock-free audio output callback. |
| **Time-to-First-Audio (TTFA)** | `< 185 ms` median (`< 280 ms` P99) | `< 140 ms` median (`< 210 ms` P99) | Tier 0 acoustic bridge (`"Affirmative."`) + pre-synthesized PCM cache for the 5 canonical Ciel prefixes! |
| **Grammar Token Mask Overhead** | `< 65 us` per token | `< 25 us` per token | `llguidance` `TokTrie` precomputed at startup; mask computed concurrently during GPU/NPU forward pass. |
| **Journal Append Latency** | `< 350 us` (NVMe/UFS group commit) | `< 120 us` (NVMe group commit) | Batching concurrent task transitions within a `200 us` coalescing window before `fdatasync`. |
| **Spatial Arbitration Window** | `15.0 ms` hard deadline | `15.0 ms` hard deadline | Monotonic local timer starting on first `SensoryClaim` arrival over Class 0 QUIC datagram. |

> **Engineering Note on Sub-170ms TTFA**: Synthesizing `"Affirmative."` from scratch on every turn wastes 35 to 60ms of ONNX compute on identical phonemes. Pre-rendering the five canonical Ciel bridge openers (`"Affirmative."`, `"Analyzing."`, `"Notice:"`, `"Negative."`, `"Report:"`) with matching Kokoro voice prosody variations at engine boot reduces Tier 0 acoustic bridge latency to `< 8ms` (pure ring-buffer enqueue) while Kokoro synthesizes the dynamic continuation clause in parallel!

---

## 8. Implementation Status (Living Section)

Update this table when a phase lands. A row is "done" only when its verification command passes on the commit that claims it.

| Phase (Manual 06 §4) | Scope landed | Verification | Status |
|---|---|---|---|
| 0 | `eon-primitives`: CSPRNG monotonic ULIDs, prefix and body validation, `ClaimId`/`SessionId`/`QueryId`/`MacroId`, `serde` feature (Manual 01 §2) | `cargo test -p eon-primitives --features serde` (7 tests) | Done |
| 1 (partial) | `eon-foundation::journal`: 32-byte CRC32 frames, group-commit `append_batch`, rolling segments, torn-tail repair, mid-file corruption refusal (Manual 01 §3.1) | `cargo test -p eon-foundation --test journal_crash` (proptest truncation at any byte, single-byte corruption, real SIGKILL writer loop) | Done (synchronous) |
| 1 (partial) | `eon-foundation::store`: WAL + `synchronous=NORMAL` pragmas, `sqlite-vec` auto-extension, forward-only projection checkpoints (Manual 01 §3.2) | `cargo test -p eon-foundation` | Done |
| 1 (partial) | `eon-foundation::credentials`: zeroized `EphemeralSecret` (no `Debug`), `secret://` argv refusal (Manual 01 §3.3) | `cargo test -p eon-foundation` | Done |
| 1 (partial) | `eon-foundation::scrubber`: Unicode strip, Aho-Corasick prefixes, token-qualified entropy with path/hash exemptions, BLAKE3 redaction markers (Manual 01 §3.4) | `cargo test -p eon-foundation` | Done |
| 1 (done) | `eon-foundation::group_commit`: worker thread drains queued payloads into one write and one `sync_data`, acks only after sync, bounded queue, restart durability (Manual 01 §3.1) | `cargo test -p eon-foundation --lib group_commit` (8 producers x 200 appends, restart) | Done |
| 1 (partial) | `eon-foundation::journal::replay_from`: sealed segments get a sparse `.idx` (one entry per 64 frames, written atomically on roll). Seeking skips earlier segments. A damaged index falls back to scanning and never changes results | `cargo test -p eon-foundation --lib journal` (3 index tests, mutation-checked) | Done |
| 1 (partial) | Scrubber coverage audit: the only free-text journal entry point is `append_scrubbed`. Kernel events (`eon-kernel::journal_sink`) carry typed IDs and task states only, so there is nothing to scrub there. Any future free-text writer must use `append_scrubbed` | code audit: `grep append_durable` | Done |
| 1 (partial) | `GroupCommitJournal::append_scrubbed`: text is scrubbed (invisible characters, known prefixes, entropy) before it is journaled. Test confirms the raw secret is absent from the replayed log | `cargo test -p eon-foundation --lib group_commit` | Done |
| 1 (partial) | `zstd` journal compression: opt-in `JournalWriter::set_compression`, frames flagged with `FLAG_ZSTD` (CRC covers stored bytes), payloads under 256 bytes or not shrinking are stored raw, replay decompresses and never exposes the flag, undecodable compressed frames are refused like bad CRCs | `cargo test -p eon-foundation` (32 unit tests including round trip, on-disk shrink, corrupt-frame refusal) | Done |
| 2 and 3 (partial) | `eon-kernel::resource`: canonical-order all-or-nothing acquisition, shared/exclusive locks, priority wait queue (Manual 01 §4.1). Synchronous, no `tokio::Notify`, no lease TTL reaping | `cargo test -p eon-kernel --test resource_model` (model-based random ops: capacity, writer exclusivity, drain liveness) | Done (synchronous core) |
| 2 and 3 (partial) | `eon-kernel::workflow`: Kahn validation, `ready()`, `splice_subgraph` with sink rewiring and snapshot revert (Manual 01 §4.2) | `cargo test -p eon-kernel --test workflow_properties` (random DAG execution, valid and cyclic splices) | Done |
| 2 and 3 (done) | `eon-kernel::fsm`: explicit 7-edge transition table, illegal and replayed transitions rejected (Manual 01 §4) | `cargo test -p eon-kernel --lib fsm` | Done |
| 2 and 3 (done) | `eon-kernel::saga`: forward execution with reverse compensation; a failing undo is recorded and the rest still run | `cargo test -p eon-kernel --lib saga` | Done |
| 2 and 3 (partial) | `eon-kernel::events`: `KernelEvent` (transitions, splices) written to an `EventSink` before success. Failed recording rolls back the state change | `cargo test -p eon-kernel --lib events` (4 tests) | Done |
| 2 and 3 (partial) | `eon-kernel::journal_sink`: events encoded as versioned records and written through the group-commit journal before success. Replay decodes back to the same events | `cargo test -p eon-kernel --lib journal_sink` | Done |
| 2 and 3 (partial) | `eon-kernel::epoch`: `CognitiveEpoch` with staged writes, atomic commit, rollback, and fencing so stale or replayed epochs cannot overwrite a newer commit. `commit_recorded` scrubs keys and values, records an `EpochCommitted` event before applying (a failed write applies nothing and leaves the epoch open). `SharedState::replay` rebuilds state from the journal and refuses an epoch that does not advance the fence | `cargo test -p eon-kernel --lib epoch` (10 tests incl. restart rebuild, secret scrubbing before the log, failing sink, corrupt replay) | Done for journal wiring |
| 2 and 3 (partial) | `eon-kernel::schedule::run_task_in_epoch`: runs a ready task's step inside an epoch. A failing step rolls back and leaves the task pending. Success commits the writes through the sink, then completes the task. A completion failure after commit leaves the task ready, so the step is at-least-once (Manual 01 §4) | `cargo test -p eon-kernel --lib schedule` (5 tests: success order, rollback, unfinished dependencies, fenced epoch, completion failure then retry) | Done. Parallel dispatch and retry policy are not implemented |
| 2 and 3 (partial) | `eon-kernel::shadow`: three-way tree merge for parallel shadow workspaces. Disjoint edits and identical edits are taken, overlapping text uses a line-level `diffy` merge, delete-vs-modify and non-UTF-8 overlaps fail closed, and the whole merge is all-or-nothing. A clean merge must pass a `VerifyGate` before it is accepted (Manual 01 §4.3). Adjacent-line edits conflict, which is conservative | `cargo test -p eon-kernel --lib shadow` (7 tests, incl. verification refusing a textually clean merge) | Done for the merge core |
| 2 and 3 (partial) | `eon-kernel::worktree`: shadow checkouts as detached `git worktree`s of a base commit, `snapshot` into the merge's `Tree` type. Two real task edits merge back through `merge_trees`, and overlapping edits in real worktrees conflict. Worktrees are real working copies, not copy-on-write overlays, and the structural CST merge (mergiraf or tree-sitter) is still deferred | `cargo test -p eon-kernel --lib worktree` (2 tests) | Done for git worktrees. Overlayfs and CST merge not started |
| Manual 06 §1 (partial) | `scripts/models.lock.json` pins (four verified Zipformer `2023-06-26` digests) and fail-closed `scripts/download_models.py` with resumable `.part` fetch | `python3` integrity test: resume via Range, server ignoring Range, digest mismatch | Done for the fetch path |
| Manual 06 §1 (deferred) | `lock --revision` HF API pinning, `--profile` selection, profile-scoped manifest at `perception/models/` | n/a | Not started |
| P0 (Manual 02 §2.1) | `eon-cognition::profile::ModelProfile::P0`: selection below 8 GB (6 to 8 GB gap goes here), lock key `p0_mobile_ultralight`. Lock resolves `common_voice` + Qwen3.5-0.8B Q4_K_M + Kokoro v1.0 int8. All artifacts pinned (HF LFS oid, Silero from tagged upstream). Substitutes 0.8B for the spec's 1.7B, which is not on Hugging Face | `cargo test -p eon-cognition`; `python3 -m unittest test_download_models` (from `scripts/`); `download_models.py --profile p0_mobile_ultralight --dry-run` | Done |
| P1 (Manual 02 §2.1) | `ModelProfile::P1`: 8 to under 12 GB. Gemma 4 E2B QAT Q4_0 GGUF (3.35 GB, google repo) + common voice + Kokoro v1.0 int8, all pinned | same as P0 | Done |
| P2 (Manual 02 §2.1) | `ModelProfile::P2`: 12 to under 16 GB. Gemma 4 12B QAT UD-Q4_K_XL (6.72 GB, unsloth repo; spec states 6.98 GB) + Kokoro v1.0 fp16 + common voice, all pinned. OmniParser `icon_detect` is AGPL-3.0 and is opt-in only (`gui_grounding_agpl` group), so it is not in the default profile. ONNX conversion is deferred until the AGPL decision is made | `python3 -m unittest test_download_models` | Done |
| P3 (Manual 02 §2.1) | `ModelProfile::P3`: 16 GB and up. Qwen3.6-27B Q4_K_M (16.8 GB, unsloth repo), Nemotron-3.5-ASR 160 ms int8 ONNX bundle (`csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-160ms-int8-2026-06-11`), common voice, Kokoro v1.0 fp16. Every artifact pinned. OmniParser is opt-in (AGPL), see P2. The ASR bundle replaces the spec's unavailable `nemotron-asr-0.6b-int8` name | `download_models.py --profile p3_workstation_heavy --dry-run` exits 0 | Done |
| 4 (partial) | `eon-capability::ifc::labels`: FIDES lattice (join = min integrity, max confidentiality), typed `ExtractedValue` endorsement (free-form values refused, SHA-256 validated at construction), bounded provenance (Manual 01 §5.1) | `cargo test -p eon-capability` (15 unit, 8 property, truth table) | Done |
| 4 (partial) | `eon-capability::ifc::rule_of_two`: deterministic triad gate, untrusted control-parameter gate with air-gap exemption (Manual 01 §5.2) | `cargo test -p eon-capability --test ifc_properties` | Done |
| 4 (done) | `eon-capability::mcp_pinning`: BLAKE3 length-prefixed fingerprint per tool, rug-pull quarantine, user-approved re-pin, 256-char planner summary (Manual 01 §5.5) | `cargo test -p eon-capability --lib mcp_pinning` (6 tests) | Done |
| 4 (partial) | `eon-capability::registry`: `Tool` trait and registry; every dispatch passes the pin store and the Rule-of-Two gate before the tool runs. Quarantined tools refused (Manual 01 §5.2, §5.5) | `cargo test -p eon-capability --lib registry` (5 tests) | Done |
| 4 (partial) | `eon-capability::schema`: canonical JSON (sorted keys) and input validation for the declared subset (`type`, `properties`, `required`, `enum`, `items`, closed `additionalProperties`). Unknown keywords refused. Dispatch validates input before any tool runs | `cargo test -p eon-capability --lib schema registry` | Done |
| 4 (partial) | `eon-capability::quarantine`: Spotlighting with a fresh 128-bit CSPRNG nonce per call, delimiter spoofing refused, invisible characters stripped, quarantined output schema-checked and kept Untrusted (Manual 01 §5.3) | `cargo test -p eon-capability --lib quarantine` (7 tests, mutation-checked) | Done |
| 4 (partial) | `eon-capability::melon`: masked re-execution check. The masked model run sits behind a `MaskedRunner` trait. A proposed call is refused with `SecurityViolation` when a masked-run call scores above 0.65 cosine similarity on canonical token bags (Manual 01 §5.4) | `cargo test -p eon-capability --lib melon` (5 tests) | Done as a check. Not yet wired into dispatch for the untrusted-read to command transition, and the masked model run is not implemented |
| 4 (partial) | `eon-capability::sandbox`: `run_confined` runs a tool process under a Landlock ruleset (read and execute only on listed paths, everything else denied) and an optional seccomp filter that refuses `socket` and `socketpair` with EPERM. The ruleset and filter are built in the parent, and the child fails before exec if Landlock is not enforced (Manual 01 §5, sandbox layer) | `cargo test -p eon-capability --lib sandbox` (3 tests: allowed read, denied read outside policy, socket allowed without and refused with `deny_network`) | Done for filesystem and socket confinement. Wasmtime is not used, and the kernel-version fallback is not tested |
| 5 (partial) | `eon-voice`: 3-tier endpointer (300/800/1150 ms, 300 ms breath holdoff), peak-limiting ASR gain (instant attack, smoothed release), frame geometry (Silero 512, AEC 160, Zipformer 320 ms) (Manual 04). Silero v5 VAD inference (`ort` rc.10) and Zipformer `2023-06-26` streaming transducer ASR (`asr.rs`: kaldi-native-fbank 80-bin features, 45-frame chunks with shift 32, cache carry, greedy search) | `cargo test -p eon-voice` (15 unit, 2 ASR e2e, 4 VAD e2e); ASR e2e transcribes upstream `en.wav` to `THE DRIBAL CHIEFTAIN CALLED FOR THE BOY AND PRESENTED HIM THE FIFTY PIECES OF GOLD` exactly | Done for ASR and VAD inference |
| 5 (partial) | `eon-voice::vad`: real Silero v5 ONNX inference via `ort` (pinned to `=2.0.0-rc.10`, the version whose API this code uses; rc.15 needs an explicit `Environment`). Model digest-verified from `scripts/models.lock.json`. End-to-end test runs the model over the upstream `test_wavs/en.wav` (16 kHz speech, sha256 eb1eb008...): speech frames detected, pauses scored quiet, endpointer closes a turn, silence scores low, state reset is repeatable | `cargo test -p eon-voice --test vad_e2e` (4 tests, real model) | Done |
| 5 (partial) | `eon-voice::ring`: wait-free `rtrb` SPSC PCM ring. Overflow drops the newest samples and counts them. Barge-in `flush` releases all queued samples in one index update (Manual 04 §2) | `cargo test -p eon-voice --lib ring` (3 tests incl. 200k-sample cross-thread order); `cargo test -p eon-voice --test ring_no_alloc` (counting global allocator: 0 allocations across 10k push/pop cycles and a flush, with a control test proving the counter sees allocations) | Done for the ring |
| 5 (deferred) | Audio device loop (cpal capture and render), AEC3, Kokoro ONNX TTS (needs audio devices and a G2P front end) | n/a | Not started |
| Pins (corrected) | Zipformer 2023-06-26 pins were copied from the k2-fsa release tarball (`sherpa-onnx-streaming-zipformer-en-2023-06-26.tar.bz2`, sha256 639e25b5...). The same-named Hugging Face repo has different bytes, so those URLs were the wrong source. Downloader now supports `archive` entries: fetch the tarball once into `perception/models/downloads/` (gitignored), verify it, extract the named member, verify its digest. Qwen3.5-0.8B pin had a one-digit typo (caught by fail-closed verification) and is fixed. All HF LFS pins were re-checked against the API oids; the only mismatches were the three Zipformer files above | `download_models.py --ids zipformer-en-2023-06-26-*` (real fetch, rerun skips) and `python3 -m unittest test_download_models` (24 tests) | Done |
| 6 (partial) | `eon-cognition::memory`: reciprocal rank fusion (k=60), Wilson lower-bound procedural ranking, `MemoryGovernor` that releases tiers 2 and 3 on dormant (Manual 02 §4 to §5) | `cargo test -p eon-cognition --lib memory` (12 tests incl. profile) | Done |
| 6 (partial) | `eon-cognition::backend`: real llama.cpp inference via `llama-cpp-2` 0.1.161, greedy decoding, one process-wide backend (llama.cpp allows a single init). End-to-end over the pinned Qwen3.5-0.8B Q4_K_M: produces coherent text, identical output on repeat, oversized prompt refused. The model's answer to the test prompt is not factually correct, and the test does not assert on content | `cargo test -p eon-cognition --test llm_e2e` (3 tests, real model) | Done |
| 6 (deferred) | `mistralrs` backend, KV pre-cognition, llguidance masks, OmniParser grounding | n/a | Not started |
| 7 (partial) | `eon-cyber::taint`: petgraph source-to-sink slicing, sanitizer removal, bypass detection, witness paths (Manual 03 §2) | `cargo test -p eon-cyber --lib` (6 tests) | Done |
| 7 (deferred) | Polyglot CPG, OSV/CycloneDX, libpcode/icicle-vm, LibAFL, aya eBPF, Firecracker | n/a | Not started |
| 8 (partial) | `eon-mesh::election`: 15 ms window on local arrival time only (sender clock never used), tie-break by id, +10 hysteresis margin (Manual 05 §2) | `cargo test -p eon-mesh --lib` (7 tests) | Done |
| 8 (partial) | `eon-mesh::sync`: Loro 1.x replica with offline edits, update export and apply, convergence across three replicas in any order, idempotent replay, corrupt update refused. Transport not included | `cargo test -p eon-mesh --lib sync` (5 tests) | Done |
| 8 (deferred) | iroh 1.4 transport and 4-class QUIC QoS (needs network) | n/a | Not started |
| 9 (partial) | `eon-cli` (`eon status <dir>`): status computed from the durable journal on disk, rendered via ratatui (TestBackend snapshot). Binary-level end-to-end test runs the compiled `eon` against a real journal, plus usage and missing-directory exit codes | `cargo test -p eon-cli` (3 unit, 2 e2e) | Done |
| 9 (partial) | `eon-uniffi`: exported `select_model_profile` and `endpoint_silence_ms`, typed errors. Kotlin bindings are generated from the built cdylib and compile with kotlinc 2.4.21 (JDK 17). Compiling exposed a clash: the `message` field of the error variant collided with `Exception.message`, so it is now `reason`. A JNA smoke test runs the bindings against the native library | `crates/eon-uniffi/kotlin/run_smoke.sh` (needs JAVA_HOME, KOTLINC, JNA_JAR, KOTLIN_STDLIB_JAR) prints `eon-uniffi kotlin smoke: OK`; `cargo test -p eon-uniffi --lib` | Done (bindings compile and run on Linux); Android AAR packaging and Kotlin unit tests not done |
| 9 (partial) | `eon-daemon`: Unix socket status service. Socket mode 0600, peer UID checked with SO_PEERCRED before any read, stale sockets replaced but regular files never deleted, unknown commands rejected. Real client/server round trip over the kernel socket. NamedPipe (Windows) not included | `cargo test -p eon-daemon` (6 tests) | Done (Unix only) |
| 9 (partial) | `eon watch <journal-dir>`: live ratatui view on crossterm. Each refresh reads the journal from disk, so it shows durable state only. `q` quits, the terminal is restored on every exit path, and running without a terminal fails with exit 1 | `cargo test -p eon-cli` (6 unit tests incl. key handling, missing directory, snapshot); pty run confirmed exit 0 on `q` with the title drawn and the alternate screen restored | Done for the TUI. `wgpu` canvas not started |
| 10 (deferred) | Android app: `ROLE_ASSISTANT`, MediaProjection FGS, AAudio, Compose HUD, AHardwareBuffer | Blocked: no JDK, Kotlin, Gradle, or Android SDK in this environment | Not started |


Known spec corrections made while implementing:

- Frame CRC is CRC32 (IEEE) from `crc32fast`, so the header field is named `crc32`, not `crc32c` (Manual 01 §3.1).
- Model profiles: the lock uses named groups (`common_voice`) as in Manual 06 §1.1. Model ids follow the artifacts actually found on Hugging Face, not the spec names where those differ (Qwen3.5-0.8B for P0, Gemma 4 QAT GGUF repos, Kokoro v1.0, Nemotron 160 ms sherpa int8 bundle for P3). Schema is now v3. Silero and `tokens.txt` digests come from downloading the upstream file, since they are not LFS-tracked, so they are not HF LFS oids.
- Open conflict: Manual 02 gives P3 a 16 GB weight footprint, while §7 caps the workstation active voice plus tactical SLM RSS at 8.5 GB. These cannot both hold. Resolve in the spec before Phase 6 sets the memory governor.
- Scrubber known-prefix matches must start a token. Without this, `xoxb-` inside the path segment `axoxb--0` redacted the rest of the token. Prefix boundaries count only alphanumerics and `_`, so `auth=ghp_...` still redacts.
- Scrubber path segments must be words: one optional leading capital, then lowercase, digits, `_`, `-`, `.`. The earlier "at most one case change" rule still let random base64 such as `jD0/l5AEFn3G1B/opca2H0Idefq4g/J/` through.
- Scrubber path exemption originally let AWS-style keys through (`wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY` was treated as a path). Fixed: a path segment must be a word with at most one case change.
- Resource requests larger than total capacity are refused with `ResourceExhausted`. Queuing them would block the queue forever.
- Queued requests are granted head-of-line in priority order, so a large high-priority request cannot be starved by smaller later ones.
- Journal tail policy (decided): frames are fsynced before acknowledgement, so a torn write can only leave an unacknowledged prefix or a zero-filled extension. Those are truncated. A complete-length frame that fails its CRC is refused, never silently dropped, and `repair_torn_tail` is the explicit operator path. RocksDB's tolerant mode cannot distinguish the two cases, so it was rejected. Known residual risk: a power loss that extends the file size with partly written payload looks like corruption and needs operator repair.
- SIGKILL tests do not exercise power loss. Page-cache data survives a process kill, so they check ordering and recovery, not fsync durability.
- `SecurityLabel` has no `serde` derives yet (Manual 01 §5.1 shows them). Add them when the journal starts persisting labels.
- `ExtractedValue` validates on construction, so `endorse` cannot be called with an unchecked hash or non-finite float. This is stricter than the doc's `endorse(type)` sketch.
- The kernel resource manager is synchronous. Async waiting is a thin layer over `acquire`/`release` and is deferred.
- `splice_subgraph` rewires only pre-existing downstream tasks. Newly spliced roots depend on the parent. Applying the rewire to new roots creates a cycle (caught by the cycle test).
- Unpinned models (`sha256: null`) are refused rather than fetched. The baseline's placeholder digests are removed.
