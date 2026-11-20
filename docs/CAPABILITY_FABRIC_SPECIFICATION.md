# CAPABILITY FABRIC SPECIFICATION

> **Subsystem:** EON Capability & Tool Runtime  
> **Module:** `engine/core-rs/crates/eon-capability`  
> **Integrates:** Vector Governed Tools + CHRONOS Android Accessibility Controllers + Cyber Defense/Offense Core + Multimodal Vision Grounding

---

## 1. Overview & Capability Philosophy

The **Capability Fabric** is the operational surface through which EON interacts with the physical device, operating system, applications, file systems, network, and cyber substrates.

In adherence to Vector, CHRONOS, and Ciel principles:
1. **Contract-First**: Every capability declares a strongly-typed schema, semantic version, and explicit input/output structures. No arbitrary `Map<String, Any>` or raw unvalidated strings are passed across capability boundaries.
2. **Bounded Output Principle**: A capability never returns an unbounded memory blob into cognitive context. If reading a 500 MB file, it returns a bounded range, a summary, or a SHA-256 digest + URI pointer.
3. **Execution Sandboxing**: Capabilities execute within bounded workers with CPU/memory limits, timeouts, and cancellation hooks.
4. **Independent Verification Hook**: Every capability contract implements an asynchronous `verify()` method that inspects ground truth after an operation completes.

---

## 2. Capability Architecture & Interface Contracts

The capability runtime is authored in Rust (2024 Edition) under `engine/core-rs/crates/eon-capability`:

```rust
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use eon_primitives::{EonError, EonResult, IdentityId, MonotonicInstant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VerificationStatus {
    /// Physically verified against OS, hardware sensor, AST, or cryptographic digest
    SuccessVerified,
    /// Execution returned success, but hardware/environmental feedback is absent or non-deterministic
    ExecutedUnverified,
    /// Execution reported success, but independent verification proved failure
    VerificationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CognitiveTaint {
    /// Originates from authenticated human operator (voice utterance, direct terminal/keyboard input)
    TrustedOperator,
    /// Originates from external, potentially adversarial sources (web pages, third-party repos, decompiled APKs, external accessibility nodes)
    UntrustedExternal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityOutcome {
    pub status: VerificationStatus,
    pub payload: serde_json::Value,
    pub taint: CognitiveTaint,
    pub execution_duration_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub identifier: String,          // e.g. "workspace.read@v1", "dev.lsp@v1", "process.pty@v1"
    pub description: String,         // Semantic purpose
    pub operations: Vec<String>,     // Supported operations
    pub permissions: Vec<String>,    // Required security permissions
}

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub caller_id: IdentityId,
    pub cognitive_epoch: u64,
    pub timeout_ms: u64,
    pub is_shadow_sandbox: bool,
    pub sandbox_root: Option<String>,
}

#[async_trait]
pub trait Capability: Send + Sync {
    /// Returns the static metadata descriptor for this capability.
    fn descriptor(&self) -> CapabilityDescriptor;

    /// Executes an operation within a bounded asynchronous execution context.
    async fn execute(
        &self,
        operation: &str,
        parameters: serde_json::Value,
        context: &ExecutionContext,
    ) -> EonResult<serde_json::Value>;

    /// Independently inspects physical ground truth after execution.
    async fn verify(
        &self,
        operation: &str,
        parameters: &serde_json::Value,
        outcome: &serde_json::Value,
    ) -> EonResult<VerificationStatus>;
}
```

### Deterministic Mapping: `VerificationStatus` to `TaskState`
The Vector Execution Kernel (`eon-kernel`) consumes `VerificationStatus` and transitions task lifecycles deterministically:

| Capability `VerificationStatus` | Kernel `TaskState` | System Reaction |
|---|---|---|
| `SuccessVerified` | `TaskState::Completed` | State delta committed to journal; child tasks unblocked. |
| `ExecutedUnverified` | `TaskState::UncertainFailure` | Logged to journal; halts automatic chained execution of non-idempotent mutations. |
| `VerificationFailed` | `TaskState::Failed` | Triggers self-healing loop ($N \le 5$) or saga rollback of partial state. |

---

## 3. Core Capability Catalogs

EON comes out-of-the-box with nine comprehensive capability domains:

### 3.1 Workspace & Artifact Capabilities (`workspace.*`)
Designed for safe, bounded, high-speed inspection and mutation of local code and documents.
- `workspace.read`: Reads UTF-8 or binary files within bounded line ranges or byte offsets. Returns SHA-256 digest and base64/UTF-8 content.
- `workspace.write`: Performs atomic file writes using temp-file replacement. Verifies file digest on disk immediately.
- `workspace.patch`: **AST-Anchored Symbol-Scoped Search/Replace**. Targets specific function/class syntax nodes via tree-sitter span and fuzzy whitespace-insensitive matching, completely eliminating fragile line-number diff failures.
- `workspace.snapshot`: Creates an in-memory or journaled Git worktree snapshot of a directory before mutations begin.

### 3.2 Process & Development Capabilities (`dev.*` / `process.*`)
Automates developer tasks, builds, tests, persistent interactive debugging, and code navigation deterministically.

#### Process Execution & Stateful Interactive PTYs (`process.*`):
- `process.exec`: Spawns one-shot local processes with bounded stdout/stderr buffers, timeout kills, and exit code capture.
- `process.pty_open`: Spawns a stateful, persistent pseudo-terminal (PTY) session (e.g. `bash`, `zsh`, `powershell`, `adb shell`, `gdb`, Python REPL, ssh). Allocates a native master/slave PTY pair (`portable-pty` in Rust), configures environment variables, and returns a unique `session_id`.
- `process.pty_write`: Writes raw byte sequences or interactive keystrokes (ANSI escape codes, Ctrl+C, Ctrl+D, interactive prompts) to the running session.
- `process.pty_read_ring`: Reads from an in-memory 10,000-line circular ring buffer for the active PTY session, supporting cursor-based pagination and ANSI strip/parse.
- `process.pty_signal`: Delivers POSIX / Windows signals (`SIGINT`, `SIGTERM`, `SIGWINCH` for terminal resize).

#### Development & Live Debugging (`dev.*`):
- `dev.build`: Runs project build tools (`gradlew`, `cargo`, `npm`, `clang`) with automatic failure log extraction.
- `dev.test`: Runs unit or integration tests, parses test XML/reports, and returns pass/fail summaries.
- `dev.ast_inspect`: Parses source code into ASTs (Kotlin, Rust, C/C++, Python, TS) to locate symbols, classes, or function signatures.
- `dev.lsp`: **Language Server Protocol (LSP) Client Multiplexer**. Manages JSON-RPC channels to `rust-analyzer`, `clangd`, `kotlin-language-server`, `pyright`, and `vtsls`.
  - `lsp.goto_definition`: Resolves cross-file symbol definitions in < 30ms.
  - `lsp.find_references`: Locates all usages across workspace.
  - `lsp.hover_type`: Resolves exact inferred types without compiler build.
  - `lsp.workspace_symbols`: Fast symbol fuzzy matching.
  - `lsp.diagnostics`: Retrieves compiler-grade type errors and warnings on demand without running full build scripts.
- `dev.dap`: **Debug Adapter Protocol (DAP) Bridge**. Drives live interactive debuggers over DAP JSON-RPC (`codelldb` for Rust/C/C++, `debugpy` for Python, `jdwp` for Android Dalvik/JVM).
  - `dap.launch`: Launches target binary or test under DAP control with specified arguments and breakpoints.
  - `dap.attach`: Attaches to an active background process, daemon, or Android PID via JDWP.
  - `dap.set_breakpoint`: Sets conditional or hit-count breakpoints at `(file, line)` or raw memory address.
  - `dap.step`: Controls execution flow (`continue`, `step_in`, `step_over`, `step_out`).
  - `dap.inspect_stack`: Captures stack frames, instruction pointers, and thread statuses on crash/stop events.
  - `dap.evaluate_expression`: Evaluates runtime expressions or reads live registers and heap variables, feeding the exact runtime state into Ciel's self-healing compiler loop.

#### Provisioning the confined Rust LSP

The verified Rust profile is a dependency-free 2021 library rooted at `src/lib.rs`.
Only explicitly opened `.rs` snapshots are visible. It does not read the real
workspace, run Cargo/rustc, load proc macros, or discover project configuration.
It is not a full Cargo workspace or a cold sub-50-ms guarantee.

On x86-64 glibc Linux, configure `OperatorServer::new(ServerKind::RustAnalyzer, …)`
with the **native server** (not a rustup proxy), its individual shared libraries
and ELF loader, and a protocol deadline. Keep the installation disjoint from the
workspace and immutable to project authors. Call `.with_rust_analyzer_task_group(…)`
with the absolute path of a trusted cgroup v2 with `pids.max` set to 1–24.
The host must be able to enroll the child in `cgroup.procs`; do not move it or
raise the ceiling while the session is live. Unsupported confinement fails
closed. Do not grant a runtime directory or use a shell wrapper.

To reproduce both gates from a trusted checkout, use Bash with `jq`, a systemd
user manager with the pids controller delegated, and the trusted rustup component
(`rustup component add rust-analyzer` if missing). Build outside the capped group;
run each test binary in its own capped transient service. `ldd` here is **only**
for the operator-installed image, never a project executable.

```bash
set -euo pipefail
manifest=/home/runner/work/Eonba/Eonba/engine/core-rs/Cargo.toml
exe=$(rustup which rust-analyzer)
libs=$(ldd "$exe" | awk '$2=="=>"{print $3} $1~/^\//{print $1}' | paste -sd:)
gate() {
    local target=$1 test=$2 bin
    shift 2
    bin=$(cargo test --locked --manifest-path "$manifest" -p eon-capability "$@" \
        --no-run --message-format=json | jq -ser --arg name "$target" \
        'map(select(.target.name == $name and .executable != null)) | last.executable')
    systemd-run --user --pipe --quiet --wait --collect -p TasksMax=24 -p Delegate=pids \
        --setenv=EON_TEST_RUST_ANALYZER="$exe" --setenv=EON_TEST_RUST_ANALYZER_RUNTIME="$libs" \
        --setenv=EON_TEST_RUST_ANALYZER_CGROUP=self "$bin" --ignored --nocapture --exact "$test"
}
gate eon_capability sandbox::rust_analyzer::tests::lsp_and_patcher_tests_rust_worker_kernel_denials --lib
gate local_lsp_rust_analyzer lsp_and_patcher_tests_confined_rust_analyzer --test local_lsp_rust_analyzer
```

### 3.3 Cyber Defense & Hardening Capabilities (`cyber.defense.*`)
Automates host security, static taint tracking, and live patching.
- `cyber.defense.ast_taint`: Traces untrusted user inputs to dangerous execution sinks across source code.
- `cyber.defense.auto_patch`: Generates and applies constant-time cryptographic checks, bounds checks, and sanitized inputs.
- `cyber.defense.ebpf_monitor`: Queries kernel-level syscall telemetry (with ETW / VpnService fallbacks) to detect unauthorized child processes or executable heap pages.
- `cyber.defense.apk_harden`: Verifies APK signing structures (v2/v3/v4), removes debuggable flags, and secures exported IPC endpoints.

### 3.4 Cyber Offense & Vulnerability Research (`cyber.offense.*`)
Supports reverse engineering, disassembling, and red-team penetration testing in isolated sandboxes.
- `cyber.offense.disassemble`: Disassembles ELF, PE, Mach-O, and DEX binaries into annotated assembly with CFG recovery.
- `cyber.offense.decompile_dex`: Decompiles Android Dalvik bytecode into Smali and Java source trees.
- `cyber.offense.lift_ir`: Lifts native machine instructions (x86_64, ARM64, Dalvik) into Ghidra P-Code SSA Intermediate Representation.
- `cyber.offense.sym_exec`: Runs symbolic execution (Z3 engine) over P-Code to discover input constraints capable of triggering buffer overflows or logic bypasses.
- `cyber.offense.fuzz_concolic`: LibAFL-powered coverage-guided fuzzer with Z3 concolic constraint solving for magic bytes.
- `cyber.offense.packet_craft`: Crafts custom raw TCP/UDP/TLS network frames to evaluate protocol robustness and port state.

### 3.5 Distributed Mesh Capabilities (`mesh.*`)
Coordinates tasks across the user's phone, workstation, and server.
- `mesh.rpc_quic`: Dispatches low-latency RPC requests to paired peer nodes over encrypted QUIC streams using `iroh`.
- `mesh.sync_crdt`: Synchronizes working memory, knowledge triples, and task DAG state across devices without locks via Loro CRDT.
- `mesh.split_inference`: Dynamically offloads compute-heavy SLM or cyber tasks to a high-compute desktop node.

### 3.6 Android System & Accessibility Capabilities (`android.*`)
Inherited and refined from CHRONOS's native accessibility controller:
- `android.screen_inspect`: Dumps active `AccessibilityNodeInfo` hierarchies. Returns visible UI elements, text, and clickable bounds. Supports automatic fallback to Set-of-Mark (SoM) visual grounding when views are obfuscated.
- `android.node_click`: Dispatches an accessibility click or gesture to a specific node ID, Set-of-Mark `mark_id` (e.g. `[7]`), or coordinate fallback.
- `android.input_text`: Enters text into the currently focused accessibility input field.
- `android.system_controller`: Interacts with system bars, media sessions (Spotify, YouTube), volume controls, and notifications.
- `android.overlay`: Controls EON's persistent HUD overlay, audio visualizer, and contextual notification cards.

### 3.7 Multimodal Vision & UI Grounding (`vision.*`)
Provides screen comprehension and visual grounding when accessibility trees are obfuscated or unavailable.
- `vision.screen_grounding`: Runs **OmniParser-v2** (YOLOv8 icon/widget detector + PaddleOCR/Surya quantized ONNX, ~45 MB) on screen frames to locate clickable elements, icons, and text boxes. Implements **Set-of-Mark (SoM) Tagging**: renders numeric badges (`[1]`, `[2]`, `[3]`) directly over candidate widgets, enabling the VLM to emit discrete tag references (`Click [7]`) mapped back to exact pixel centroids with 100% precision.
- `vision.keyframe_sample`: Controls the adaptive 1–5 FPS video capture loop and retrieves recent temporal frames from the 30-second visual ring buffer.
- `vision.video_query`: Queries the 3-Tier Video Ingestion engine, retrieving sparse SigLIP frame embeddings from `sqlite-vec` and loading high-resolution cropped keyframes for fine-grained inspection.

### 3.8 Sub-Agent Orchestration Capabilities (`task.*`)
- `task.spawn`: Spawns an isolated, ephemeral sub-agent with its own dedicated 4,000-token working context ledger. Runs targeted exploratory sub-goals and returns a 3-line diagnostic digest to the parent workflow.

### 3.9 Network & Web Capabilities (`net.*`)
Enables internet research and data acquisition without cloud model dependencies:
- `net.http_get`: Performs bounded HTTP requests with redirect limits and SSL verification.
- `net.download`: Downloads files directly to local storage with resume support and SHA-256 integrity validation.
- `net.scrape_clean`: Fetches a web page and extracts readable markdown/text using readability algorithms, stripping ads and JS bloat.

### 3.10 Hardware & Device Capabilities (`device.*`)
Direct hardware sensor and actuator control:
- `device.battery`: Queries battery level, thermal status, and charging state.
- `device.flashlight`: Toggles device torch and verifies state via `CameraManager.TorchCallback`.
- `device.camera_capture`: Captures a single frame from CameraX without opening a full camera app.

### 3.11 Neuro-Symbolic & Formal Reasoning (`reasoning.*`)
Provides mathematical and constraint-satisfaction solving to eliminate logical hallucinations:
- `reasoning.smt_solve`: Formulates and executes first-order SMT-LIB2 constraint problems using the embedded Z3 theorem prover. Deterministically resolves task scheduling windows, dependency version matrices, firewall rule satisfiability, and permission algebra in $< 10\text{ms}$.
- `reasoning.rollout_eval`: Evaluates Best-of-$N$ candidate solution patches across parallel shadow sandboxes, scoring candidates on compilation status, AST linter diagnostics, and unit test pass rates.

---

## 4. Capability Sandboxing & Permission Guardrails

```mermaid
flowchart TD
    InvokedTask["Invoked Task Execution"] --> PermCheck{"Permission Check"}
    
    PermCheck -->|Authorized| RiskCheck{"Autonomy Tier"}
    PermCheck -->|Unauthorized| Deny["Reject: Permission Denied"]
    
    RiskCheck -->|Tier 1: Read-Only| Exec["Execute in Sandbox Worker"]
    RiskCheck -->|Tier 2: Reversible Mutation| Snapshot["Create Journal Snapshot"] --> Exec
    RiskCheck -->|Tier 3: External Non-Destructive| Exec
    RiskCheck -->|Tier 4: Destructive Action| Gate["Biometric / Security Challenge"]
    
    Gate -->|Passed| Exec
    Gate -->|Rejected / Timed Out| Abort["Cancel Task"]

    Exec --> Verify["Independent Physical Verification"]
    Verify -->|Verified| RecordSuccess["Journal: SUCCESS_VERIFIED"]
    Verify -->|Failed| Rollback["Auto-Rollback Snapshot & Report Failure"]
```

### Invariants:
1. **Never Bypass Security Controls**: If an operation touches external accounts or deletes directories, the system halts at Tier 4 for explicit authorization.
2. **Snapshot-Before-Write**: Any write operation on user workspaces or configuration files MUST record a snapshot in the journal before touching the file system.
3. **Auto-Rollback on Failure**: If a multi-step task mutates 3 files and the 4th task fails, the capability manager automatically rolls back the 3 mutated files to their pre-workflow state unless explicit partial retention was requested.
4. **Cognitive Taint Tracking (Invariant 8 Compliance)**: Capability outcomes carry a `CognitiveTaint` tag. Any outcome marked `UntrustedExternal` taints the calling workflow context. Downstream tasks requesting `MUTATE_WORKSPACE`, `EXECUTE_PROCESS`, or `DEVICE_CONTROL` are blocked from autonomous execution unless their parameters are sanitized through an isolated Dual-LLM filter or validated via an explicit user confirmation gate.
