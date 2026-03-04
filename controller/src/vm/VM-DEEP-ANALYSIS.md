# VM Module Deep Analysis

## Overview

The `controller/src/vm/` module is the **physical connection layer** of the AutoMutate++ controller. It manages the lifecycle of remote Windows VM targets — discovery, gRPC connection establishment, bidirectional streaming, state tracking, artifact transport, and reconnection. Every VM interaction in the system flows through this module.

While `dispatch/` decides *what* to run and *when*, and `api/` exposes the control plane, the `vm/` module handles *how* the controller talks to real Windows machines.

```
 controller.toml / automation/generated/*.toml
       |
       | discover_and_register_targets()
       v
 +================+                          +==================+
 | TargetManager  |  <-- gRPC bidi stream -> | Worker Agent     |
 | (DashMap of    |                          | (remote Windows  |
 |  Targets)      |  establish_stream()      |  VM process)     |
 +================+                          +==================+
       |                                            ^
       | spawns per-VM:                             |
       |   stream_handler (incoming messages)       |
       |   VMExecutor (run dispatch)                |
       |   heartbeat (keepalive)                    |
       |                                            |
       | send_artifact() ---- chunked gRPC -------->|
       | send_command()  ---- ControllerMessage --->|
       |                                            |
       |<---- WorkerMessage (Registration, ---------|
       |       SampleResponse, Telemetry,           |
       |       Status, Ack)                         |
```

---

## File Inventory

| File | Lines | Role |
|------|------:|------|
| `mod.rs` | 15 | Module declaration + re-exports |
| `manager.rs` | 1066 | TargetManager: registration, state, connections, streams, artifacts, reconnection |

**Total: ~1,081 lines** (including ~50 lines of tests)

---

## 1. `mod.rs` — Module Root (15 lines)

Declares `pub mod manager` and re-exports the five public types:

```rust
pub use manager::{RegistrationType, Target, TargetEvent, TargetManager, TargetStatus};
```

These are consumed by `api/`, `dispatch/`, and `main.rs`.

---

## 2. `manager.rs` — The Entire VM Layer (1066 lines)

This single file contains all VM management logic. It breaks down into seven functional areas.

---

### 2.1 Type Definitions (lines 38–159)

#### TargetStatus — State Machine

```
    Offline ──mark_connected()──> Available
    Available ──reserve()──────> Busy
    Busy ──release()───────────> Available
    Any ──mark_offline()───────> Offline
    Any ──stream closed────────> Offline
    Any ──send failed──────────> Offline
```

Three states, one-directional transitions enforced by the methods:
- `reserve()` fails if not `Available`
- `release()` fails if `Offline`
- `mark_connected()` only transitions from `Offline` → `Available`
- `mark_offline()` warns if target was `Busy` (may have an in-flight run)

#### RegistrationType

```rust
enum RegistrationType {
    Static,   // Loaded from TOML at startup
    Dynamic,  // Registered via gRPC Registration message
}
```

Static targets are discovered from `automation/generated/` TOML files. Dynamic targets register themselves during stream establishment.

#### TargetEvent

Events emitted to the Orchestrator via `events_tx`:

| Variant | When | Carried Data |
|---------|------|-------------|
| `Connected` | Registration message received on stream | target_id, os_version, capabilities |
| `Disconnected` | Stream closed or send failed | target_id, reason |
| `Message` | Any WorkerMessage received | target_id, full WorkerMessage |

All messages (including Registration/SampleResponse) are forwarded as `Message` events to the Orchestrator. The `Connected` and `Disconnected` events are additional lifecycle signals.

#### TargetConfig

Minimal registration input: `{ id: TargetId, address: String, enabled: bool }`.

#### Target — Per-VM State

```rust
struct Target {
    // Identity
    id: TargetId,
    address: String,                        // "ip:port"

    // Metadata (populated by Registration or query_all_info)
    os_version: String,                     // "win10", "win11"
    capabilities: Vec<String>,              // ["rededr", "mde", "defender"]
    metadata: HashMap<String, String>,      // arbitrary key-value from worker
    tools: HashMap<String, String>,         // tool versions (rededr, defender, etw, llvm)

    // State
    status: TargetStatus,
    enabled: bool,
    registration_type: RegistrationType,
    current_job: Option<JobId>,
    last_seen: SystemTime,                  // updated on every message
    connected_at: Option<SystemTime>,

    // Connection handles (private)
    channel: Option<Channel>,               // tonic gRPC channel
    stream_tx: Option<mpsc::Sender<ControllerMessage>>, // outgoing bidi stream
}
```

The `channel` and `stream_tx` fields are private. `channel` is a tonic `Channel` for unary RPCs (like `send_artifact`). `stream_tx` is the sender half of the bidirectional stream, used for commands and heartbeats.

#### Target Discovery from TOML

Worker TOML files in `automation/generated/` follow the pattern `win*-worker-*.toml`:

```toml
[worker]
worker_id = "win10-worker-01"
ip_address = "192.168.1.100"
listen_port = 50052   # default: 50052
```

`load_target_config()` parses this into `(worker_id, "ip:port")`.

---

### 2.2 TargetManager — Core Struct (lines 165–185)

```rust
struct TargetManager {
    targets:    DashMap<TargetId, Target>,       // lock-free concurrent map
    events_tx:  mpsc::Sender<TargetEvent>,       // to Orchestrator
    rpc_timeout: Duration,                       // for unary RPCs
    run_pool:   Arc<RunPool>,                    // for VMExecutor creation
}
```

Uses `DashMap` for lock-free read access (queries from API handlers, VMExecutors, etc.) with fine-grained write locking (per-entry) for state mutations.

---

### 2.3 Registration (lines 196–240)

Two registration paths:

#### `register(config)` — Static Registration

Creates a `Target` with `RegistrationType::Static`, `status = Offline` (or `Offline` if `!enabled`), and inserts into the DashMap. Called during startup from `discover_and_register_targets()`.

#### `register_with_metadata(id, address, os_version, capabilities, metadata, tools)` — Dynamic Registration

Called by the Orchestrator when a worker sends a `Registration` message. If the target already exists (e.g. from static config), updates its metadata in-place. Otherwise creates a new entry with `RegistrationType::Dynamic`.

---

### 2.4 Queries — Public API (lines 244–297)

Read-only accessors used by `api/worker.rs` and `dispatch/orchestrator.rs`:

| Method | Returns | Usage |
|--------|---------|-------|
| `get(id)` | `Option<Target>` | Single target lookup |
| `list_ids()` | `Vec<TargetId>` | All registered target IDs |
| `list_all()` | `Vec<Target>` | All targets (cloned) |
| `count()` | `usize` | Total registered |
| `get_available()` | `Vec<TargetId>` | Available + enabled targets |
| `get_available_by_os_and_capabilities(caps)` | `HashMap<String, Vec<TargetId>>` | Available targets grouped by OS, filtered by required capabilities (case-insensitive) |

`get_available_by_os_and_capabilities()` is the primary query used by the API's `GetAvailableWorkers` RPC for capability-aware VM selection.

---

### 2.5 State Management (lines 301–379)

Five atomic state transitions, each operating on a single DashMap entry:

| Method | Transition | Side Effects |
|--------|-----------|-------------|
| `reserve(id)` | Available → Busy | Fails if not Available |
| `release(id)` | Busy → Available | Clears `current_job` |
| `mark_connected(id)` | Offline → Available | Sets `connected_at`, updates `last_seen` |
| `mark_offline(id)` | Any → Offline | Warns if Busy, clears `stream_tx` + `channel` + `current_job` |
| `update_health(id)` | (no transition) | Updates `last_seen` timestamp |

These methods are called from:
- `VMExecutor` — `reserve()` before dispatch, `release()` after result
- `stream_handler` — `mark_connected()` on registration, implicit offline on stream close
- Orchestrator — `mark_connected()` / `mark_offline()` on target events
- API — `update_health()` on status reports, `ping_worker` reconciliation

---

### 2.6 Connection Management — The Core Complexity (lines 383–661)

This section contains the most intricate logic in the module.

#### `get_channel(id)` — Lazy Channel Creation (lines 385–415)

Returns a cached `tonic::Channel` for unary RPCs. If not cached, creates one with:
- 10s request timeout
- 5s connect timeout
- 30s TCP keepalive

Stored in `target.channel` for reuse. Used by `send_artifact()` and `get_worker_info()`.

**Important:** This channel is NOT used for the bidirectional stream — the stream gets its own dedicated channel without a request timeout (otherwise the stream would die after 10s of idle time).

#### `establish_stream(id)` — The Main Connection Flow (lines 418–526)

The most important function in the module. Establishes a bidirectional gRPC stream with a VM and spawns three concurrent tasks:

**Step 1: Create dedicated stream channel**
```
Endpoint (no request timeout) → connect() → WorkerAgentClient
```

**Step 2: Set up outgoing message channel**
```rust
let (stream_tx, stream_rx) = mpsc::channel::<ControllerMessage>(128);
let outgoing = ReceiverStream::new(stream_rx);
```

**Step 3: Establish bidirectional stream**
```rust
let response = client.establish_stream(Request::new(outgoing)).await?;
let incoming = response.into_inner(); // tonic::Streaming<WorkerMessage>
```

**Step 4: Store stream_tx in Target** for `send_command()` use.

**Step 5: Create result channel** for VMExecutor to receive run completions:
```rust
let (result_tx, result_rx) = mpsc::channel::<RemoteRunResult>(128);
```

**Step 6: Create registration oneshot** — the stream_handler will send `VMInfo` through this once the worker's Registration message arrives:
```rust
let (reg_tx, reg_rx) = oneshot::channel::<VMInfo>();
```

**Step 7: Spawn stream_handler** — receives all incoming `WorkerMessage`s from the VM.

**Step 8: Spawn deferred VMExecutor** — waits up to 15s for registration via `reg_rx`, then starts the executor:
```rust
tokio::spawn(async move {
    match timeout(15s, reg_rx).await {
        Ok(Ok(vm_info)) => VMExecutor::new(...).run().await,
        Ok(Err(_))      => warn("stream closed before registration"),
        Err(_)           => warn("registration timeout"),
    }
});
```

**Step 9: Spawn heartbeat** — sends `Heartbeat` messages every 30s.

This deferred pattern ensures the VMExecutor knows the VM's actual OS and capabilities (from the Registration message) before it starts taking runs from the pool.

#### `stream_handler(...)` — Incoming Message Processor (lines 528–626)

Long-lived async task per VM. Reads from `tonic::Streaming<WorkerMessage>` in a loop:

1. **Every message** — Updates `target.last_seen` via `touch()`.
2. **Registration** (first message only):
   - Stores capabilities and OS version in Target
   - Sends `VMInfo` via `reg_tx` oneshot to unblock VMExecutor spawn
   - Emits `TargetEvent::Connected` to Orchestrator
3. **SampleResponse** — Converts to `RemoteRunResult`, sends to VMExecutor via `result_tx`:
   ```rust
   RemoteRunResult {
       run_id, detected, exit_code, success,
       error (None if empty), elapsed_ms,
       detection_verdict, last_checkpoint,
   }
   ```
4. **All messages** — Forwarded as `TargetEvent::Message` to Orchestrator (for telemetry indexing, status handling, etc.)
5. **Stream close/error** — Cleans up Target state (clears `stream_tx`, `channel`, sets `Offline`), emits `TargetEvent::Disconnected`.

#### `spawn_heartbeat(id, tx)` — Keepalive (lines 628–661)

Sends `Heartbeat { timestamp }` every 30 seconds through the bidirectional stream. Stops when:
- Target goes Offline (checked before each send)
- Channel send fails (stream closed)

Purpose: TCP keepalive prevents idle connection reaping by intermediate proxies/firewalls, and gives the worker a regular liveness signal from the controller.

#### `establish_all_streams()` — Batch Connect (lines 663–671)

Iterates all registered targets and calls `establish_stream()` sequentially. Returns a map of results. Called once at startup from `main.rs`.

#### `spawn_reconnect_loop(interval_secs)` — Auto-Recovery (lines 674–709)

Background task that periodically:
1. Finds all `Offline + enabled` targets
2. Attempts `establish_stream()` for each
3. Logs success/failure

Skips the first tick (startup already ran `establish_all_streams`). Disabled when `interval_secs == 0`.

This provides automatic recovery from transient network failures without manual intervention.

---

### 2.7 Command Sending & Disconnect (lines 711–812)

#### `send_command(id, msg)` — Send via Bidi Stream (lines 711–737)

Sends a `ControllerMessage` to a VM through the stored `stream_tx`. On send failure:
- Clears `stream_tx`
- Marks target `Offline`
- Emits `TargetEvent::Disconnected`
- Returns error

Used by:
- `VMExecutor::dispatch()` — sends `RunSampleCommand`
- `api/worker.rs::ping_worker()` — sends `HealthCheckRequest`
- `disconnect_one()` / `disconnect_all()` — sends `DisconnectNotice`

#### `broadcast(msg)` — Send to All (lines 739–748)

Sends to every registered target. Returns count of successful sends. Used by `disconnect_all()`.

#### `disconnect_all(reason, reconnect_allowed)` — Mass Disconnect (lines 750–776)

1. Signals RunPool shutdown (cancels all VMExecutors)
2. Waits 100ms for executors to finish current runs
3. Broadcasts `DisconnectNotice` to all VMs
4. Waits 100ms for delivery
5. Force-marks all targets Offline (clears `stream_tx`, `channel`)

#### `disconnect_one(id, reason)` — Single Disconnect (lines 779–812)

Best-effort `DisconnectNotice` send, then `mark_offline()`. Warns if target is `Busy` (in-flight run will fail).

---

### 2.8 Artifact Operations (lines 818–843)

#### `send_artifact(id, artifact_id, path)` — Chunked Upload

1. Reads artifact from disk (async)
2. Gets unary RPC channel via `get_channel(id)` (NOT the bidi stream)
3. Chunks data into 4 MB `ArtifactChunk` messages via `chunk_artifact()`
4. Streams via `client.send_artifact(stream::iter(chunks))`

Note: This uses a separate unary channel, not the bidirectional stream. The bidi stream carries commands and results; artifact upload is a one-shot client-streaming RPC.

#### `TargetArtifactSender` — Trait Impl (lines 981–1002)

Wraps `TargetManager::send_artifact()` to satisfy the `dispatch::ArtifactSender` trait:

```rust
struct TargetArtifactSender { manager: Arc<TargetManager> }

impl ArtifactSender for TargetArtifactSender {
    fn send_artifact(&self, vm_id, artifact_id, path) -> Pin<Box<dyn Future<...>>> {
        Box::pin(self.manager.send_artifact(vm_id, artifact_id, path))
    }
}
```

Created inside `establish_stream()` and passed to each VMExecutor.

---

### 2.9 Info Queries (lines 850–887)

#### `get_worker_info(id)` — Unary RPC

Sends `WorkerInfoRequest` to the VM via unary gRPC, returns `WorkerInfoResponse` (os_version, capabilities). Uses `rpc_timeout` guard.

#### `query_all_info()` — Batch Query

Queries every registered target, updates `target.os_version` and `target.capabilities` with the response. Called at startup to populate metadata before stream establishment.

---

### 2.10 Target Discovery (lines 889–974)

#### `discover_and_register_targets()`

Scans `automation/generated/` for TOML files matching `win*-worker-*.toml`:

1. Parses each file with `load_target_config()`
2. **Deduplicates by IP** — skips targets with IPs already registered, warns about conflicts
3. Registers each unique target via `register(TargetConfig { ... })`
4. Reports total count and duplicate count

---

## Concurrency Model

### Per-VM Task Ensemble

Each connected VM has **three concurrent tokio tasks**:

| Task | Lifetime | Purpose |
|------|----------|---------|
| `stream_handler` | Stream open → stream close | Reads incoming WorkerMessages, dispatches to events + result channels |
| `VMExecutor::run()` | Registration received → pool shutdown | Takes runs from pool, dispatches to VM, routes results back |
| `heartbeat` | Stream open → target offline | Sends 30s keepalive Heartbeats |

All three share the same `stream_tx` for outgoing messages and reference the same `DashMap` entry.

### Shared State Access Pattern

```
                  DashMap<TargetId, Target>
                 /          |           \
            API queries   VMExecutor    stream_handler
            (read)        reserve/      touch/
                          release       status/
                          (write)       stream_tx
                                        (write)
```

DashMap provides lock-free reads and per-shard write locks. No global mutex contention.

### Channel Layout Per VM

```
stream_tx (128) ──> ReceiverStream ──> gRPC bidi outgoing ──> Worker Agent
                                                                    |
Worker Agent ──> gRPC bidi incoming ──> stream_handler              |
                    |         |                                     |
                    |         +──> result_tx (128) ──> VMExecutor   |
                    |                                               |
                    +──> events_tx ──> Orchestrator                 |
                                                                    |
reg_tx (oneshot) ──> deferred VMExecutor spawn                      |
```

---

## Relationship to the Global AutoMutate++ Project

The VM module is the **physical transport layer**. It bridges the logical dispatch system to real Windows sandboxes:

| Global Project Need | VM Module Responsibility |
|--------------------|------------------------|
| Execute artifacts in sandboxed VMs | Connection management, command dispatch via bidi stream |
| Collect ETW telemetry | Stream `WorkerMessage::Telemetry` → Orchestrator → ES |
| Upload built artifacts | `send_artifact()` chunked gRPC streaming (4 MB chunks) |
| Track VM availability | `TargetStatus` state machine (Offline/Available/Busy) |
| Filter VMs by OS + capabilities | `get_available_by_os_and_capabilities()` |
| Recover from network failures | `spawn_reconnect_loop()` automatic retry |
| Worker registration and metadata | `register_with_metadata()` + `query_all_info()` |
| Admin control (ping, disconnect) | `send_command()` + `disconnect_one/all()` |
| Spawn execution engine per VM | Deferred `VMExecutor` creation after registration |

The VM module does **not** contain:
- Run scheduling logic (that's `dispatch/job_worker.rs`)
- Run queuing/routing (that's `dispatch/run_pool.rs`)
- Detection analysis / differential protocol (that's `dispatch/types/round.rs`)
- ElasticSearch indexing (that's `storage/`)
- gRPC API handlers (that's `api/`)
- Artifact compilation (that's the `build` crate)

It is the **infrastructure layer** that makes remote VM execution possible, sitting below the dispatch engine and above the physical network.

---

## Design Decisions

### 1. Two Separate gRPC Channels Per VM

- **Unary channel** (`target.channel`): 10s request timeout, used for `send_artifact()` and `get_worker_info()`. Created lazily by `get_channel()`.
- **Bidi stream channel**: No request timeout (would kill idle streams), used for `establish_stream()`. Created explicitly in `establish_stream()`.

This separation prevents the long-lived bidirectional stream from being killed by the unary timeout policy.

### 2. Deferred VMExecutor Spawn

The VMExecutor is NOT started immediately when the stream opens. Instead, a deferred task waits up to 15 seconds for the worker's `Registration` message, which carries the actual OS and capability information. Only then is the VMExecutor created with accurate `VMInfo`.

This avoids a race condition where the executor might take runs before knowing its capabilities, leading to mismatched dispatches.

### 3. DashMap Over RwLock

`DashMap` was chosen over `RwLock<HashMap>` because:
- API handlers make frequent read queries (list/get workers) that shouldn't block
- State transitions (reserve/release) are per-target and shouldn't block other targets
- DashMap's per-shard locking provides good concurrency for both patterns

### 4. Stream Handler Forwards Everything

The stream handler forwards ALL messages to the Orchestrator as `TargetEvent::Message`, in addition to handling `Registration` and `SampleResponse` specifically. This ensures the Orchestrator can handle telemetry indexing, status updates, and any future message types without changes to the VM module.

### 5. Reconnection Loop

Automatic reconnection runs as a background task with configurable interval. It only targets `Offline + enabled` targets, so disabled targets or actively connected ones are never disturbed. This provides hands-off recovery for lab environments where VMs may reboot or temporarily lose network connectivity.
