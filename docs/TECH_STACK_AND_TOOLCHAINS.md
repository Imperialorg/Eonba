# TECH STACK & TOOLCHAINS SPECIFICATION

> **Subsystem:** EON Engineering & Build Infrastructure  
> **Architecture:** Native High-Performance Core (Rust) + Mobile Presence (Kotlin/Android)  
> **Authority:** CANONICAL REFERENCE FOR ALL IMPLEMENTING AGENTS

---

## 1. Executive Summary & Design Rationale

To achieve **Manas: Ciel's** performance targets—sub-200ms Time-to-First-Audio (TTFA), deterministic DAG scheduling, microsecond packet inspection, binary reverse engineering, and peer-to-peer QUIC mesh—EON eliminates runtime garbage-collection jitter on hot paths.

EON deploys a **Dual-Engine Architecture**:
1. **The Native Core Engine (`engine/core-rs`)**: Authored in **Rust (2024 Edition)**. Owns the audio loop, neural ONNX runtimes, the Vector DAG execution kernel, the Cyber and Information Particle manipulation engine, and the P2P QUIC mesh.
2. **The Mobile Platform Presence (`clients/android`)**: Authored in **Kotlin 2.0 / Android SDK 35+**. Owns system-level accessibility node traversal, the persistent HUD floating overlay, camera frames, and Android system controllers.
3. **The Inter-Language Bridge**: Powered by **Mozilla UniFFI**, generating zero-overhead, memory-safe foreign function interfaces between the Rust core and Kotlin/Android without manual, error-prone JNI boilerplate.

---

## 2. Language, Runtime & Compiler Matrix

| Layer | Language / Tool | Minimum Version | Target Triple / Runtime | Purpose |
|---|---|---|---|---|
| **Core Engine** | Rust | `1.82.0+` (Edition 2024) | `x86_64-pc-windows-msvc`<br/>`aarch64-linux-android`<br/>`x86_64-unknown-linux-gnu` | Kernel, Audio, DAG, Cyber, Mesh |
| **Android Presence** | Kotlin | `2.0.20+` | Android SDK 35 (minSdk 28) | Accessibility, Overlay HUD, System Controller |
| **Build System (Android)**| Gradle | `8.10+` with AGP `8.7+` | JVM 17/21 | Android packaging & compilation |
| **Build System (Native)** | Cargo | `1.82.0+` | Cargo Workspace with `cargo-ndk` | Native core compilation & cross-targets |
| **Inference Runtime** | ONNX Runtime | `1.20.0+` | C++ API / Native shared libraries | Silero VAD, Kokoro TTS, Zipformer ASR |
| **Local SLM Engine** | `llama.cpp` | Latest stable | GGUF Q4_K_M / Q8 quantization | Tactical on-device SLM (Gemma-2B, Qwen-2.5-3B) |
| **CLI / Terminal UI** | Rust | `1.82.0+` | Native CLI binary | Interactive terminal (`ratatui`) |

---

## 3. Rust Native Core Workspace Architecture (`engine/core-rs`)

The native engine is structured as a modular Cargo workspace with strictly enforced dependency boundaries:

```text
engine/core-rs/
├── Cargo.toml                         # Workspace root manifest
├── crates/
│   ├── eon-primitives/                # Level 0: ULIDs, MonotonicTime, EonResult, EonError
│   ├── eon-foundation/                # Level 1: Append-only journal (redb/SQLite), Identity
│   ├── eon-kernel/                    # Level 2 & 3: DAG scheduler, Epoch leases, Saga reconciler
│   ├── eon-capability/                # Level 4: Capability traits, process exec, workspace fs
│   ├── eon-cyber/                     # Information Particle Manipulation:
│   │                                  # - tree-sitter (AST parsing & self-healing compiler)
│   │                                  # - capstone-rs (multi-arch disassembly)
│   │                                  # - z3 (symbolic execution & constraint solver)
│   │                                  # - pnet / socket2 (raw network frame crafting)
│   │                                  # - aya-rs (eBPF telemetry for Linux/Android)
│   ├── eon-mesh/                      # Engine Optimized for Networking:
│   │                                  # - s2n-quic or quiche (P2P QUIC transport, mTLS 1.3)
│   │                                  # - loro or automerge (Delta-state CRDT synchronization)
│   ├── eon-voice/                     # Real-Time Perception:
│   │                                  # - cpal / AAudio / WASAPI (low-latency audio loop)
│   │                                  # - Silero VAD v5 (ONNX Runtime)
│   │                                  # - sherpa-onnx (int8 Zipformer streaming ASR)
│   │                                  # - Kokoro-82M (ONNX streaming neural TTS)
│   │                                  # - Acoustic Commitment Gate & Barge-in controller
│   ├── eon-cognition/                 # Level 5: Thought Acceleration router, Ciel persona
│   └── eon-uniffi/                    # UniFFI interface definitions for Kotlin & Swift
```

### High-Performance Cargo Profile Configuration (`Cargo.toml`)
```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
overflow-checks = false

# Dedicated profile for low-latency audio & network benchmarks
[profile.bench]
opt-level = 3
debug = true
lto = "thin"
```

---

## 4. UniFFI Interface Definition & Cross-Language Bridge

EON uses Mozilla UniFFI to generate type-safe Kotlin classes directly from Rust interface definition files (`.udl` or proc-macros).

### 4.1 Interface Contract (`eon_core.udl`)
```rust
namespace eon_core {
    string generate_ulid(string prefix);
    i64 get_monotonic_time_nanos();
};

dictionary TaskResultDto {
    string task_id;
    string status;
    string outcome_json;
    string? error_code;
};

interface EonKernelBridge {
    constructor(string storage_path, string identity_id);
    
    [Throws=EonBridgeError]
    void submit_objective(string objective_id, string description);
    
    [Throws=EonBridgeError]
    TaskResultDto execute_task_sync(string task_id, string capability, string operation, string parameters_json);
    
    void interrupt_current_epoch(string reason);
};

interface EonVoiceBridge {
    constructor();
    void start_listening();
    void stop_listening();
    void feed_audio_pcm(sequence<i16> samples);
    void register_barge_in_callback(BargeInCallback callback);
};

callback interface BargeInCallback {
    void on_barge_in_detected(i64 timestamp_ms);
};

[Error]
enum EonBridgeError {
    "InitializationFailed",
    "InvalidPayload",
    "TaskExecutionFailed",
    "SecurityViolation",
};
```

### 4.2 Cross-Compilation for Android with `cargo-ndk`
To build native `.so` shared libraries for all Android hardware architectures:
```powershell
# From engine/core-rs/
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o ../../clients/android/app/src/main/jniLibs build --release -p eon-uniffi
```

---

## 5. Android Client & UI Workspace (`clients/android`)

The Android application acts as EON's mobile eyes, ears, and hands:
- **Language**: Kotlin 2.0 with Jetpack Compose.
- **Root Directory**: `clients/android/`
- **Modules**:
  - `app`: Main Android application packaging JNI libraries, settings, and permissions.
  - `accessibility`: Implements `EonAccessibilityService` to inspect `AccessibilityNodeInfo` hierarchies, execute taps, text entry, and window change listeners.
  - `overlay`: Implements `EonOverlayService` managing the floating visualizer HUD and thought bubbles via Android `WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY`.
  - `system`: Quick Settings Tile (`EonQuickActionTile`), MediaSession listeners, and WorkManager background reconciliation.

---

## 6. Desktop & CLI Workspace (`clients/cli` & `clients/daemon`)

For desktop workstations and servers:
- **Interactive Terminal**: Authored in Rust using `ratatui` + `crossterm`. Displays live audio wave visualizers, active DAG task graphs, and interactive slash commands (`/menu`, `/capabilities`, `/cyber`, `/mesh`).
- **Headless Daemon**: Background service using local Named Pipes on Windows (`\\.\pipe\eon`) and Unix Domain Sockets on Linux (`/tmp/eon.sock`).
