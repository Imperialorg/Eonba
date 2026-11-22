# STATE MACHINE AND ARCHITECTURAL INVARIANTS

> **Authority Level:** NON-NEGOTIABLE & SUPREME  
> **Applies to:** All modules, agents, algorithms, models, and code in EON.  
> **Rule of Conflict:** If any code, implementation shortcut, or convenience conflicts with an invariant in this document, **THE IMPLEMENTATION MUST CHANGE**.

---

## 1. The Canonical Cognitive State Machine

EON operates as a strictly governed, stateful cognitive presence. Global state transitions are owned exclusively by the `EonRuntimeStateManager` and are driven by verified events.

```mermaid
stateDiagram-v2
    [*] --> DORMANT
    
    DORMANT --> AWAKE: Authorized Wake Signal / Push Event
    AWAKE --> LISTENING: Audio Stream Active
    
    LISTENING --> APPRAISING: VAD End-of-Utterance Finalized
    LISTENING --> DORMANT: Silence Timeout / Cancel
    
    APPRAISING --> PLANNING: Complex Multi-Step Goal Detected
    APPRAISING --> EXECUTING: Reflex / Cached Macro Detected
    APPRAISING --> SPEAKING: Informational / Direct Verbal Reply
    
    PLANNING --> EXECUTING: Validated Workflow DAG Compiled
    PLANNING --> SPEAKING: Planning Failed / Clarification Needed
    
    EXECUTING --> VERIFYING: Capability Workers Return Outcomes
    EXECUTING --> SPEAKING: Partial Progress Update Stream
    
    VERIFYING --> SPEAKING: Ground Truth Verified
    VERIFYING --> PLANNING: Verification Failed (Auto-Repair Triggered)
    
    SPEAKING --> LISTENING: Barge-In Interrupt Detected
    SPEAKING --> RETURNING_TO_DORMANT: Speech Playback Finished
    
    RETURNING_TO_DORMANT --> DORMANT: Leases Released & Memory Bounded
    
    DORMANT --> [*]: System Shutdown
```

### Deterministic State Definitions

| State | Responsibility | Invariant / Boundary |
|---|---|---|
| `DORMANT` | Low-power passive awareness. Mic stream monitored only by VAD/wake-word. Zero CPU overhead. | No LLM inference. No disk writes. Memory pinned to baseline. |
| `AWAKE` | System actively energized. Perception subsystems initialized. | Transition authorized only by verified wake signal or direct UI touch. |
| `LISTENING` | Streaming audio PCM frames to Silero VAD and Zipformer ASR. | Max listening duration: 15s. Full duplex echo cancellation active. |
| `APPRAISING` | Thought Acceleration: Extracts intent, inspects screen/files, routes to Tier 0/1/2. | Latency budget: < 60ms. |
| `PLANNING` | ObjectiveWorkflowCompiler validates capability contracts, produces DAG. | Must verify all capability requirements exist before scheduling. |
| `EXECUTING` | TaskScheduler dispatches atomic tasks to worker threads with leases. | All state changes committed to append-only journal before side effects. |
| `VERIFYING` | Physical ground truth checks (file hash, OS callback, UI accessibility node). | No task marked `SUCCESS` without independent verification proof. |
| `SPEAKING` | Kokoro-82M streaming audio buffers to audio driver. | Must abort within < 5ms if barge-in interrupt is triggered. |
| `RETURNING_TO_DORMANT` | Releases all resource leases, evicts expired working memory, flushes logs. | Must leave system in clean, leak-free baseline state. |

---

## 2. Connecting Macro Operating Modes to the Per-Turn State Machine

The 9-state **Per-Turn Cognitive State Machine** (§1) executes *within* the context of the active **Macro Operating Mode** defined in [OPERATING_MODES_AND_PRESENCE.md](./OPERATING_MODES_AND_PRESENCE.md). The active Operating Mode enforces operational constraints that alter state transitions:

```mermaid
flowchart TD
    MacroMode["Active Macro Operating Mode (e.g. SILENT_COPILOT, FULL_DUPLEX_VOICE)"]
    MacroMode -->|Governs Sensory & Output Gates| PerTurnFSM["Per-Turn Cognitive State Machine (9 States)"]

    subgraph Alterations["Mode-Driven Transition Rules"]
        VoiceRule["In FULL_DUPLEX_VOICE:<br/>SPEAKING streams 24kHz audio via Kokoro TTS."]
        SilentRule["In SILENT_COPILOT:<br/>Audio output is muted. VERIFYING -> HUD Toast / Notification instead of SPEAKING."]
        CyberRule["In CYBER_SENTINEL:<br/>Voice loop is dormant. Syscall/packet events trigger APPRAISING -> EXECUTING headlessly."]
        DevRule["In PAIR_PROGRAMMING:<br/>Compiler/AST events trigger shadow workspace execution and IDE diff rendering."]
        DormantRule["In DORMANT_MODE:<br/>All execution frozen; only hardware wake-word or push notification can awaken."]
    end

    PerTurnFSM --- Alterations
```

### Specific Mode Constraints:
1. **Full-Duplex Voice (`FULL_DUPLEX_VOICE`)**:
   - The canonical 9-state machine executes in its full standard audio loop.
   - `LISTENING` uses continuous 16kHz microphone stream with AEC.
   - `SPEAKING` drives Kokoro-82M neural TTS through low-latency audio buffers (< 170ms TTFA).
2. **Silent Copilot (`SILENT_COPILOT`)**:
   - Speaker output is strictly **MUTED**.
   - The transition `VERIFYING` $\to$ `SPEAKING` is intercepted: the engine formats the output into a floating HUD notification card, toast pill, or terminal message and transitions directly to `RETURNING_TO_DORMANT`.
3. **Autonomous Cyber Sentinel (`CYBER_SENTINEL`)**:
   - Audio input and output remain dormant.
   - Transitions are triggered by background kernel/network events (eBPF / ETW alerts). The system enters `APPRAISING` $\to$ `PLANNING` $\to$ `EXECUTING` headlessly in isolated sandboxes, notifying the user only upon Tier 4 unresolvable threats.
4. **Deep Pair-Programming (`PAIR_PROGRAMMING`)**:
   - Voice is optional and opportunistic.
   - AST changes and build failures trigger `APPRAISING` $\to$ `PLANNING` in shadow sandboxes. Output is rendered visually via the live Artifact Canvas (`clients/canvas`) or terminal diffs (`ratatui`).
5. **Low-Power Dormant (`DORMANT_MODE`)**:
   - All state machine executions are inhibited. The system sits exclusively in `DORMANT` until an authorized wake signal occurs.

---

## 3. Non-Negotiable Invariants

### Invariant 1: Persistent Cognitive Operating System (Not a Launcher, Not a Chatbot)
EON is not an app drawer, a home screen replacement, or a traditional chatbot UI.  
EON is an **omnipresent cognitive presence** that lives above and within the operating environment (Android, desktop, server), observing context, executing tasks, and disappearing when idle.

### Invariant 2: Verification Over Assumption
Execution of an operation does not prove success in the physical world.
- An HTTP response code 200 does not prove a downloaded file is uncorrupted.
- A flashlight toggle API call does not prove the LED illuminated.
- An accessibility click on an app button does not prove the intended screen opened.  
**Rule**: EON must independently verify state changes against physical sensors, hardware callbacks, file digests, or updated UI accessibility trees before reporting completion.

### Invariant 3: Thought Acceleration & The < 200ms Latency Budget
Conversational fluency requires instant response.  
**Rule**: The primary audio and cognitive loop MUST deliver Time-to-First-Audio (TTFA) in **< 200ms** (P99 < 300ms). Heavy background computations, multi-document analysis, or code builds MUST run asynchronously in parallel workers without blocking the active voice loop.

### Invariant 4: 100% Durable Journaling & Crash Resilience
System failure, power loss, or OS process kills must never result in corrupted or abandoned state.  
**Rule**: All workflow submissions, task state transitions, and resource leases MUST be journaled to durable storage before external side effects occur. On restart, the `LocalExecutionRecoveryWorker` MUST reconcile active state, release orphaned leases, and safely roll back unverified partial file mutations.

### Invariant 5: Local Independence & Zero Data Exfiltration
EON is a **local-first** cognitive system.  
**Rule**: Core voice processing (VAD, ASR, TTS), reasoning, planning, memory storage, and device control MUST function 100% offline without requiring internet access or cloud API keys. Remote models or web searches may only be contacted for explicitly user-requested external research. Ambient audio and private user context must **never** be sent to unauthorized third-party telemetry.

### Invariant 6: Autonomous Initiative (The Ciel Mandate)
EON does not behave like a timid assistant that asks for permission to breathe.  
**Rule**: Read-only, analytical, optimization, and reversible operations execute automatically without user prompting. The system pauses for confirmation **only** for Tier 4 irreversible destructive operations (e.g. data wiping, financial transfers).

### Invariant 7: Honest Telemetry & Zero Fabrication
EON must report reality exactly as observed.  
**Rule**: Never fabricate model confidence, fake successful verification, simulate device hardware states, or hallucinate missing facts.
- When capability verification is physically confirmed, the task state is committed as `TaskState::Completed` (`VerificationStatus::SuccessVerified`).
- If an outcome is uncertain or hardware feedback is non-deterministic, it MUST be reported as `TaskState::UncertainFailure` (`VerificationStatus::ExecutedUnverified`).
- If verification demonstrates failure, it MUST be reported as `TaskState::Failed` (`VerificationStatus::VerificationFailed`).

### Invariant 8: Untrusted-to-Privileged Escalation Gate (Cognitive Taint Tracking)
EON ingests massive volumes of external, untrusted content (web scrapes, third-party git repos, decompiled binaries, untrusted app accessibility trees) while maintaining radical autonomy to write code and execute processes. This creates a lethal indirect prompt injection vulnerability unless strictly gated at the information-flow level.  
**Rule**: All data entering the cognitive context is strictly partitioned into Information Flow Control (IFC) trust tiers:
1. `TRUSTED_OPERATOR`: Directly uttered voice instructions from authenticated user biometrics, authenticated local terminal inputs, and system-level configuration files.
2. `UNTRUSTED_EXTERNAL`: Web scrapes (`net.scrape_clean`), third-party repository files (`workspace.read`), decompiled bytecode/strings (`cyber.offense.decompile_dex`), and accessibility nodes from external applications (`android.screen_inspect`).

**Deterministic Kernel Enforcement**:
- If an active workflow or context ledger contains `UNTRUSTED_EXTERNAL` data spans, the `eon-kernel` `TaskDispatcher` evaluates all downstream tasks against the **Privileged Escalation Boundary**.
- Any task requesting **Privileged Permissions** (`Permissions::MUTATE_WORKSPACE`, `Permissions::EXECUTE_PROCESS`, `Permissions::DEVICE_CONTROL`, `Permissions::INVOKE_NETWORK_EGRESS`) whose parameters (file paths, shell commands, script code, destination URLs) derive from untrusted spans is **STRICTLY BLOCKED** from autonomous execution.
- To execute, the task must either:
  1. Pass through an isolated **Dual-LLM Deterministic Privilege Filter** (a separate sandboxed verification model verifying zero instruction drift or prompt injection payloads), OR
  2. Drop directly to a **NACK User Confirmation Gate** requiring explicit operator approval.
- Radical Autonomy (Invariant 6) is subordinate to Invariant 8: untrusted external data can never autonomously trigger privileged host mutations.
