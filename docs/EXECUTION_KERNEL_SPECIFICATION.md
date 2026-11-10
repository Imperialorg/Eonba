# EXECUTION KERNEL SPECIFICATION (VECTOR CORE)

> **Subsystem:** EON Deterministic Execution Engine  
> **Module:** `engine/execution-engine`  
> **Derived from:** Vector Generation 2 Deterministic Architecture + Ciel Autonomous Execution Engine

---

## 1. Core Principles & Separation

In EON, **cognition and execution are strictly decoupled**:
- **Cognition (Ciel)** answers: *What needs to be done?* It synthesizes human intent into a structured **Objective** and a validated **Workflow DAG**.
- **The Execution Kernel (Vector Engine)** answers: *How can this be executed with maximum efficiency, deterministic safety, resource accounting, and crash resilience?*

The Execution Kernel owns scheduling, task dispatch, atomic resource leases, event journaling, state persistence, error isolation, saga rollbacks, and crash recovery.

---

## 2. Canonical Dependency Levels (0 through 5)

Dependencies flow strictly downward. Lower levels never import or depend on types from higher levels:

```text
Level 5: Advanced Intelligence (Ciel Cognition)
         - ObjectiveWorkflowCompiler, WorkflowSynthesizer, WorkflowOptimizer
         ▲
Level 4: Capability Fabric
         - CapabilityRegistry, ToolProviders, SecuritySandboxes, DynamicPlugins
         ▲
Level 3: Execution Engine
         - DeterministicExecutionEngine, TaskScheduler, TaskDispatcher,
           LocalExecutionRecoveryWorker, ExecutionReconciler
         ▲
Level 2: Execution Foundations
         - ExecutionState, FiniteStateMachine, ResourcePrimitives, Leases, Events
         ▲
Level 1: Foundation Services
         - Identity, Append-Only Event Journal, Durable State Store, Security
         ▲
Level 0: Kernel Primitives
         - Strongly Typed Identifiers (ULID), Monotonic Time, EonResult<T>, EonError
```

---

## 3. Canonical Domain Primitives & Data Contracts

The execution kernel operates on a closed, strongly-typed set of domain primitives:

### 3.1 Objective
An objective represents the high-level intent declared by the user or synthesized by Ciel.
```kotlin
data class Objective(
    val id: ObjectiveId,             // Typed format: "obj_<ULID>"
    val ownerId: IdentityId,         // User or system identity
    val description: String,         // Human intent description
    val priority: Priority,          // CRITICAL, HIGH, NORMAL, BACKGROUND
    val createdAt: MonotonicInstant, // Monotonic timestamp
    val deadline: MonotonicInstant?  // Optional timeout boundary
)
```

### 3.2 Task
A task is an atomic, non-decomposable unit of work targeting a specific capability contract.
```kotlin
data class Task(
    val id: TaskId,                                  // "task_<ULID>"
    val workflowId: WorkflowId,                      // Parent workflow
    val capability: CapabilityRequirement,            // e.g. "workspace.read@v1"
    val operation: String,                           // e.g. "read_range"
    val parameters: TaskParameters,                  // Typed, validated payload
    val dependencies: Set<TaskId>,                   // Upstream dependencies (DAG)
    val resourceRequirements: List<ResourceRequirement>, // CPU, Memory, File Locks
    val timeoutMs: Long                              // Execution timeout
)
```

### 3.3 Workflow (DAG)
A workflow is a validated Directed Acyclic Graph (DAG) composed of tasks.
```kotlin
data class Workflow(
    val id: WorkflowId,              // "wf_<ULID>"
    val objectiveId: ObjectiveId,    // Linked objective
    val tasks: Map<TaskId, Task>,    // Task graph indexed by ID
    val createdAt: MonotonicInstant
) {
    init {
        // Enforce acyclic constraint via topological sort
        requireAcyclic()
    }
}
```

### 3.4 Execution State & Lifecycle
Every task and workflow transitions through deterministic states managed by an explicit Finite State Machine:

```mermaid
stateDiagram-v2
    [*] --> SUBMITTED: Objective Submitted
    SUBMITTED --> VALIDATED: Schema & DAG Validated
    VALIDATED --> SCHEDULED: Dependencies Met
    SCHEDULED --> LEASE_ACQUIRED: Resources Allocated
    LEASE_ACQUIRED --> DISPATCHED: Worker Assigned
    DISPATCHED --> RUNNING: Execution Started
    RUNNING --> VERIFYING: Capability Returned Outcome
    VERIFYING --> COMPLETED: Ground Truth Verified
    VERIFYING --> FAILED: Verification Mismatch
    RUNNING --> FAILED: Runtime Error / Timeout
    RUNNING --> CANCELLED: User Interruption
    FAILED --> [*]
    COMPLETED --> [*]
    CANCELLED --> [*]
```

---

## 4. Resource Accounting & Atomic Leases

To prevent deadlocks, out-of-memory crashes, or file lock contention during multi-step parallel processing, the execution engine enforces **Atomic Resource Leases**:

```kotlin
data class ResourceRequirement(
    val resourceType: ResourceType, // CPU_CORES, MEMORY_MB, FILE_LOCK, NETWORK_BANDWIDTH
    val resourceId: String,         // e.g. "file:///d:/repo/build.gradle"
    val amount: Long,               // e.g. 512 (MB)
    val mode: AccessMode            // SHARED_READ, EXCLUSIVE_WRITE
)

interface ResourceManager {
    fun acquireLeases(taskId: TaskId, requirements: List<ResourceRequirement>): EonResult<ResourceLease>
    fun releaseLeases(lease: ResourceLease): EonResult<Unit>
}
```

- **Two-Phase Acquisition**: If a task requires 3 resources, all 3 must be acquired atomically. If one is unavailable, all acquired locks are immediately released, preventing circular wait deadlocks.
- **Lease Expiration**: Every lease has a hardware-clock-backed expiration timeout to prevent abandoned locks after unhandled crashes.

---

## 5. Append-Only Event Journal & Crash Recovery

EON provides **100% durable execution guarantees**. No state change occurs without being committed to the journal first:

```text
Task Execution Request
       │
       ▼
[Append Event to Journal on Disk]  ──► (e.g. TaskDispatchedEvent)
       │
       ▼
[Execute Capability Worker]
       │
       ▼
[Append Outcome Event to Disk]     ──► (e.g. TaskCompletedEvent)
```

### Crash Recovery Reconciler (`LocalExecutionRecoveryWorker`)
If the system process is terminated unexpectedly (OS kill, battery death, crash):
1. Upon restart, `LocalExecutionRecoveryWorker` scans the journal from the last confirmed checkpoint.
2. Identifies tasks that were marked `RUNNING` or `DISPATCHED` but never completed.
3. If the task is marked idempotent, the worker re-dispatches it; if non-idempotent, it is marked `UNCERTAIN_FAILURE` and safely rolls back associated file snapshots.
4. All stale resource leases are purged.
5. The high-level workflow state is reconciled before new requests are accepted.

---

## 6. Multi-Step Execution Example: End-to-End Workflow

Consider the user instruction:  
*"Download the latest specification from https://internal.corp/spec.pdf, check if our security policy matches, update security.json, and run the validator."*

The Execution Engine processes this as a **4-Node DAG**:

```mermaid
graph LR
    Task1["Task 1: net.download<br/>(Download spec.pdf)"]
    Task2["Task 2: doc.extract_text<br/>(Extract sections)"]
    Task3["Task 3: security.compare<br/>(Compare with security.json)"]
    Task4["Task 4: workspace.mutate<br/>(Atomic update security.json)"]
    Task5["Task 5: dev.run_validator<br/>(Verify json schema & policy)"]

    Task1 --> Task2
    Task2 --> Task3
    Task3 --> Task4
    Task4 --> Task5
```

---

## 7. Epoch-Based Leases & Preemptive Saga Compensation

In real human-AI pairing, **the human operator is an asynchronous, unleased entity**. The user may issue a new command or countermand an active task while a 10-step background DAG is running.

### 7.1 Cognitive Epochs ($e_0, e_1, e_2$)
- Every new user voice or text input increments the **Cognitive Epoch ID**.
- Every submitted task carries the epoch ID under which it was authorized.
- If a new user utterance semantically conflicts with an active background DAG:
  1. The Cognitive Router dispatches an `EPOCH_INVALIDATED` event.
  2. The Task Dispatcher sends a `SIGINT` / cancellation coroutine signal to running capability workers.
  3. **Saga Compensation**: The engine executes compensating rollback actions in reverse order (e.g. restoring pre-task file snapshots, dropping temporary git branches).
  4. All held resource leases are released before the new request is scheduled.

---

## 8. Speculative Shadow Sandboxes (Copy-on-Write)

To satisfy the **Zero-Question Autonomous Mandate** when intent is underspecified, the kernel provides **Copy-on-Write (CoW) Shadow Sandboxes**:
- Ambiguous multi-step mutations do not run directly on the live working directory.
- The engine creates an instantaneous ephemeral Git worktree or overlayfs sandbox.
- The task DAG executes entirely within the shadow sandbox.
- Compilers, linters, and unit test suites validate the result in isolation.
- Once verified, the changes are presented with a **Negative Acknowledgment (NACK)** window: committed to live workspace after 5 seconds unless explicitly cancelled.

---

## 9. Temporal State Delta Oracles (Eventual Truth Verification)

Verification in dynamic environments (such as Android Accessibility or remote network services) cannot rely on static instantaneous assertions:
- A UI button click may take 300ms to trigger a screen transition.
- A network daemon may take 800ms to open a socket.

### 9.1 Temporal Predicate Verification
Verification is defined as a time-windowed predicate:
$$\mathcal{P}_{\text{success}}(S) = \exists t \in [t_{\text{start}}, t_{\text{start}} + \Delta t_{\text{max}}] \text{ such that } \text{Assert}(S_t) == \text{True}$$

### 9.2 Verification Tiers:
1. **Deterministic Oracles**: Cryptographic SHA-256 hashes, AST compiler exit codes, database ACID transactions (immediate check).
2. **Event-Driven Delta Oracles**: Android `TYPE_WINDOW_STATE_CHANGED` events, kernel socket state changes, file watcher events (checked across a bounded time window $\le 2000\text{ms}$).
3. **Multi-Modal Visual Oracles**: When interacting with arbitrary 3rd party web apps, a visual screenshot embedding delta verifies that the screen transitioned into the expected layout regime.
