# CAPABILITY FABRIC SPECIFICATION

> **Subsystem:** EON Capability & Tool Runtime  
> **Module:** `engine/capability-fabric`  
> **Integrates:** Vector Governed Tools + CHRONOS Android Accessibility Controllers

---

## 1. Overview & Capability Philosophy

The **Capability Fabric** is the operational surface through which EON interacts with the physical device, operating system, applications, file systems, and network.

In adherence to Vector and CHRONOS principles:
1. **Contract-First**: Every capability declares a strongly-typed schema, semantic version, and explicit input/output structures. No arbitrary `Map<String, Any>` or raw unvalidated strings are passed across capability boundaries.
2. **Bounded Output Principle**: A capability never returns an unbounded memory blob into cognitive context. If reading a 500 MB file, it returns a bounded range, a summary, or a SHA-256 digest + URI pointer.
3. **Execution Sandboxing**: Capabilities execute within bounded workers with CPU/memory limits, timeouts, and cancellation hooks.
4. **Independent Verification Hook**: Every capability contract implements a `verify()` method that inspects ground truth after an operation completes.

---

## 2. Capability Architecture & Interface Contracts

```kotlin
package eon.capability

import eon.primitives.EonResult
import eon.primitives.MonotonicInstant

interface Capability {
    val identifier: CapabilityId         // e.g. "workspace.read@v1"
    val description: String              // Semantic purpose
    val operations: Set<String>          // Supported operations
    val permissions: Set<Permission>     // Required security permissions

    fun execute(
        operation: String,
        parameters: CapabilityParameters
    ): EonResult<CapabilityOutcome>

    fun verify(
        operation: String,
        parameters: CapabilityParameters,
        outcome: CapabilityOutcome
    ): EonResult<VerificationStatus>
}

enum class VerificationStatus {
    SUCCESS_VERIFIED,      // Physically verified against OS / hardware / digest
    EXECUTED_UNVERIFIED,   // Execution returned success, but hardware feedback is absent
    VERIFICATION_FAILED    // Execution reported success, but verification proved failure
}
```

---

## 3. Core Capability Catalogs

EON comes out-of-the-box with five core capability domains:

### 3.1 Workspace & Artifact Capabilities (`workspace.*`)
Designed for safe, bounded, high-speed inspection and mutation of local code and documents.
- `workspace.read`: Reads UTF-8 or binary files within bounded line ranges or byte offsets. Returns SHA-256 digest and base64/UTF-8 content.
- `workspace.write`: Performs atomic file writes using temp-file replacement. Verifies file digest on disk immediately.
- `workspace.patch`: Applies unified diffs or exact target-replacement strings. Fails if target string has drifted.
- `workspace.snapshot`: Creates an in-memory or journaled snapshot of a directory before mutations begin.

### 3.2 Process & Development Capabilities (`dev.*` / `process.*`)
Automates developer tasks, builds, and tests deterministically.
- `process.exec`: Spawns local processes with bounded stdout/stderr buffers, timeout kills, and exit code capture.
- `dev.build`: Runs project build tools (`gradlew`, `cargo`, `npm`) with automatic failure log extraction.
- `dev.test`: Runs unit or integration tests, parses test XML/reports, and returns pass/fail summaries.
- `dev.ast_inspect`: Parses source code into ASTs (Kotlin, Python, Rust, TS) to locate symbols, classes, or function signatures.

### 3.3 Android System & Accessibility Capabilities (`android.*`)
Inherited and refined from CHRONOS's native accessibility controller:
- `android.screen_inspect`: Dumps the active Android Accessibility node hierarchy (`AccessibilityNodeInfo`). Returns visible UI elements, text, resource IDs, and clickable bounds.
- `android.node_click`: Dispatches an accessibility click or gesture to a specific node ID or coordinate fallback.
- `android.input_text`: Enters text into the currently focused accessibility input field.
- `android.system_controller`: Interacts with system bars, media sessions (Spotify, YouTube), volume controls, and notifications.
- `android.overlay`: Controls EON's persistent HUD overlay, audio visualizer, and contextual notification cards.

### 3.4 Network & Web Capabilities (`net.*`)
Enables internet research and data acquisition without cloud model dependencies:
- `net.http_get`: Performs bounded HTTP requests with redirect limits and SSL verification.
- `net.download`: Downloads files directly to local storage with resume support and SHA-256 integrity validation.
- `net.scrape_clean`: Fetches a web page and extracts readable markdown/text using readability algorithms, stripping ads and JS bloat.

### 3.5 Hardware & Device Capabilities (`device.*`)
Direct hardware sensor and actuator control:
- `device.battery`: Queries battery level, thermal status, and charging state.
- `device.flashlight`: Toggles device torch and verifies state via `CameraManager.TorchCallback`.
- `device.camera_capture`: Captures a single frame from CameraX without opening a full camera app.

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
