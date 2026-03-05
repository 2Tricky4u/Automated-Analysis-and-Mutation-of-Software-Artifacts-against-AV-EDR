# Session Module — Deep Analysis

Deep analysis of `worker/agent/src/session/` — the bidirectional stream session layer and worker runtime state management.

---

## 1. Overview

### Purpose

The `session/` folder manages the **persistent, real-time communication channel** between a worker agent and the controller. It owns the bidirectional gRPC stream lifecycle: receiving commands from the controller, dispatching them to the execution engine, and sending back registration, status updates, telemetry, and execution results — all multiplexed over a single TCP connection.

It also owns `WorkerState`, the mutable runtime snapshot of the worker (current job, health metrics, connection status) that is shared across the stream handler, heartbeat loop, and api layer.

### Role in the Global Project

In the AutoMutate++ architecture, the session module implements the **Phase 2 communication model** where a single bidirectional gRPC stream (`EstablishStream` RPC from `worker.proto`) replaces the collection of individual unary RPCs from Phase 1. This provides:

1. **Multiplexed communication** — commands, telemetry, status, and results share one connection instead of requiring separate RPC calls
2. **Real-time liveness** — periodic heartbeats in both directions detect connection loss within seconds
3. **Controller-initiated execution** — the controller pushes `RunSampleCommand` through the stream rather than calling the worker's `RunSample` unary RPC
4. **Automatic registration** — the worker announces its identity and capabilities as soon as the stream opens

```
Controller                                    Worker Agent
    │                                              │
    │  ─── EstablishStream (bidi gRPC) ──────────► │
    │                                              │
    │  ◄── WorkerMessage::Registration ─────────── │  (once, on connect)
    │                                              │
    │  ─── ControllerMessage::RunSample ─────────► │  (command)
    │  ◄── WorkerMessage::Ack ──────────────────── │  (immediate ack)
    │  ◄── WorkerMessage::ExecutionStatus ──────── │  (during execution)
    │  ◄── WorkerMessage::Telemetry ────────────── │  (after execution)
    │  ◄── WorkerMessage::SampleResponse ───────── │  (result)
    │                                              │
    │  ─── ControllerMessage::Heartbeat ─────────► │  (periodic)
    │  ◄── WorkerMessage::Status ───────────────── │  (periodic, every 30s)
    │                                              │
    │  ─── ControllerMessage::Disconnect ────────► │  (graceful shutdown)
    │                                              │
```

### Module Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     session/ (this folder)                    │
│                                                               │
│  ┌─────────────────────────────────────────────────────┐     │
│  │              stream_handler.rs                        │     │
│  │                                                       │     │
│  │  StreamHandler                                        │     │
│  │  ├── handle_stream() ── incoming message loop         │     │
│  │  │   ├── RunSample → spawn execution task             │     │
│  │  │   ├── HealthCheck → send status update             │     │
│  │  │   ├── Heartbeat → update state timestamp           │     │
│  │  │   ├── Disconnect → mark disconnected               │     │
│  │  │   ├── Ack → debug log                              │     │
│  │  │   └── ArtifactChunks → not yet implemented         │     │
│  │  ├── send_registration() ── on connect                │     │
│  │  ├── send_status_update() ── periodic heartbeat       │     │
│  │  ├── send_telemetry() ── after execution              │     │
│  │  └── send_ack() ── immediate command acknowledgement  │     │
│  │                                                       │     │
│  │  heartbeat_loop() ── background task (30s interval)   │     │
│  └───────────────────────────┬───────────────────────────┘     │
│                              │ reads/writes                     │
│  ┌───────────────────────────▼───────────────────────────┐     │
│  │              worker_state.rs                           │     │
│  │                                                       │     │
│  │  WorkerState                                          │     │
│  │  ├── identity: worker_id, capabilities, tools         │     │
│  │  ├── runtime: current_job_id, current_run_id          │     │
│  │  ├── health: cpu%, memory%, disk%, active_jobs        │     │
│  │  └── connection: disconnected, reason, reconnect      │     │
│  │                                                       │     │
│  │  HealthMetrics                                        │     │
│  └───────────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────────┘
```

---

## 2. File Inventory

| File | Lines | Purpose | Key Exports |
|------|-------|---------|-------------|
| `mod.rs` | 3 | Module declarations | — |
| `stream_handler.rs` | 424 | Bidirectional stream message processing + heartbeat loop | `StreamHandler`, `heartbeat_loop()` |
| `worker_state.rs` | 68 | Mutable runtime state shared across session | `WorkerState`, `HealthMetrics` |
| **Total** | **495** | — | — |

---

## 3. Per-Module Deep Analysis

### 3.1 `worker_state.rs` — Worker Runtime State (68 lines)

#### 3.1.1 Purpose

Holds the mutable snapshot of the worker's runtime identity, current job, health metrics, and connection status. This state is shared (via `Arc<RwLock<WorkerState>>`) between the `StreamHandler`, `heartbeat_loop`, and `api/` modules to ensure a consistent view of the worker across all communication paths.

#### 3.1.2 `WorkerState` Struct

| Field | Type | Purpose | Updated By |
|-------|------|---------|------------|
| `worker_id` | `String` | Worker identity (e.g., `"win10-worker-01"`) | Set once on creation |
| `capabilities` | `Vec<String>` | Available features (e.g., `["rededr", "mde"]`) | Set once from `WorkerCapabilities` |
| `metadata` | `HashMap<String, String>` | System info (hostname, cpu_cores, ram_gb, os_key) | Set once from `WorkerCapabilities` |
| `tools` | `Option<ToolVersions>` | Installed tool versions (RedEDR, Defender, etc.) | Set once from `WorkerCapabilities` |
| `health` | `HealthMetrics` | Live CPU/memory/disk metrics | `update_health()` |
| `current_job_id` | `Option<String>` | Active job or `None` | `stream_handler::handle_run_sample()` |
| `current_run_id` | `Option<String>` | Active run or `None` | `stream_handler::handle_run_sample()` |
| `last_controller_heartbeat` | `Option<i64>` | Timestamp of last controller heartbeat | `stream_handler::handle_heartbeat()` |
| `controller_disconnected` | `bool` | Whether controller explicitly disconnected | `stream_handler::handle_disconnect()` |
| `disconnect_reason` | `Option<String>` | Reason for disconnect (e.g., `"controller_shutdown"`) | `stream_handler::handle_disconnect()` |
| `reconnect_allowed` | `bool` | Whether reconnection is permitted | `stream_handler::handle_disconnect()` |

**Field groups:**

- **Identity (immutable):** `worker_id`, `capabilities`, `metadata`, `tools` — set once at construction from `WorkerCapabilities` detected at startup. Never changes during the session.
- **Job tracking (mutable):** `current_job_id`, `current_run_id` — set to `Some(...)` when execution starts, cleared to `None` when it completes. Used by `send_status_update()` to report the active job and by `api/run.rs` to resolve the run ID.
- **Health (mutable):** `health` — refreshed via `update_health()` which calls `sysinfo` and `infra::system::collect_system_metrics()`.
- **Connection (mutable):** `controller_disconnected`, `disconnect_reason`, `reconnect_allowed`, `last_controller_heartbeat` — track the controller's connection status. Used by `heartbeat_loop()` to adjust log levels and detect reconnection.

#### 3.1.3 `WorkerState::new()`

```rust
pub fn new(worker_id: String, capabilities: WorkerCapabilities) -> Self
```

Constructs a fresh state from the worker ID and capabilities detected at startup. Initializes all mutable fields to their default (no job, no heartbeat, not disconnected, reconnect allowed).

The `WorkerCapabilities` → `WorkerState` conversion:
- `capabilities.capabilities` → `state.capabilities` (Vec clone)
- `capabilities.metadata` → `state.metadata` (HashMap clone)
- `capabilities.to_tool_versions()` → `state.tools` (proto message)

#### 3.1.4 `WorkerState::update_health()`

Refreshes CPU and memory metrics using a fresh `sysinfo::System` instance. Delegates to `infra::system::collect_system_metrics()` for the actual computation.

**Note:** Creates a new `System::new_all()` on every call. This is acceptable because `update_health()` is called infrequently (the heartbeat loop currently doesn't call it — it reads stale values from `state.health`). The `api/info.rs` handlers use their own `System` instance directly rather than going through `WorkerState`.

#### 3.1.5 `HealthMetrics` Struct

| Field | Type | Default |
|-------|------|---------|
| `cpu_percent` | i32 | 0 |
| `memory_percent` | i32 | 0 |
| `disk_percent` | i32 | 0 |
| `active_jobs` | i32 | 0 |
| `uptime_seconds` | i64 | 0 |

Derives `Default` so all fields start at zero. Updated by `update_health()`.

---

### 3.2 `stream_handler.rs` — Bidirectional Stream Handler (424 lines)

The core of the session module. Manages the full-duplex gRPC stream, processing incoming controller commands and sending outgoing worker messages.

#### 3.2.1 `StreamHandler` Struct

```rust
pub struct StreamHandler {
    pub worker_state: Arc<RwLock<WorkerState>>,
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,
    worker_id: String,
    config: WorkerConfig,
    execution_lock: Arc<Mutex<ExecutionState>>,
}
```

| Field | Type | Purpose |
|-------|------|---------|
| `worker_state` | `Arc<RwLock<WorkerState>>` | Shared mutable state (pub for `api/run.rs` to read `current_run_id`) |
| `tx` | `mpsc::Sender<Result<WorkerMessage, Status>>` | Outgoing message channel (buffer: 100) |
| `worker_id` | `String` | Cloned from service (avoids Arc cycle) |
| `config` | `WorkerConfig` | Cloned from service (avoids Arc cycle) |
| `execution_lock` | `Arc<Mutex<ExecutionState>>` | Shared with service and api layer |

**Arc-Cycle Prevention Design:**

The `StreamHandler` does NOT hold `Arc<WorkerAgentService>`. If it did:
```
WorkerAgentService ──Arc──► stream_handler ──Arc──► WorkerAgentService
                           (reference cycle, memory leak)
```

Instead, individual fields are cloned/shared:
```
WorkerAgentService ──Arc──► stream_handler
    │                           │
    ├── worker_id (cloned)      ├── worker_id (owned copy)
    ├── config (cloned)         ├── config (owned copy)
    └── execution_lock (Arc) ───┴── execution_lock (same Arc)
```

This breaks the cycle while preserving shared access to the execution lock.

#### 3.2.2 `StreamHandler::new()`

```rust
pub fn new(
    worker_state: Arc<RwLock<WorkerState>>,
    worker_id: String,
    config: WorkerConfig,
    execution_lock: Arc<Mutex<ExecutionState>>,
) -> (Self, mpsc::Receiver<Result<WorkerMessage, Status>>)
```

Creates a `(handler, rx)` pair. The handler holds the `tx` end of the channel; the `rx` is returned to the caller (`api/stream.rs`) which wraps it in a `ReceiverStream` and returns it as the outgoing gRPC stream.

**Channel buffer size:** 100 messages. This provides backpressure — if the controller stops reading, the worker's sends will eventually block (or fail) rather than consuming unbounded memory.

#### 3.2.3 Incoming Message Processing

**`handle_stream()`** — Main receive loop:

```rust
pub async fn handle_stream(&self, mut stream: Streaming<ControllerMessage>) -> Result<(), Status>
```

Runs until the stream closes (controller disconnects or network error). Each message is processed by `process_message()`. Errors on individual messages are logged but do NOT terminate the loop — only stream closure or gRPC transport errors stop it.

**`process_message()`** — Message router:

| `ControllerMessage` Variant | Handler | Behavior |
|-----------------------------|---------|----------|
| `RunSample(cmd)` | `handle_run_sample()` | Spawn execution task |
| `HealthCheck(req)` | `handle_health_check()` | Send status update |
| `Heartbeat(hb)` | `handle_heartbeat()` | Update timestamp in state |
| `Disconnect(notice)` | `handle_disconnect()` | Mark disconnected in state |
| `Ack(ack)` | (inline) | Debug log |
| `ArtifactChunks(_)` | (inline) | Warning: not yet implemented |
| `None` | (inline) | Warning: empty message |

#### 3.2.4 `handle_run_sample()` — Stream-Based Execution

The most complex handler. Receives a `RunSampleCommand`, sends an immediate acknowledgement, then spawns a background task that runs the full execution pipeline and sends the result back through the stream.

**Flow:**

```
RunSampleCommand arrives
    │
    ▼
1. Extract request_id and SampleRequest
   └── Missing request → error
    │
    ▼
2. Send immediate Ack
   └── Ack { request_id, success: true }
    │
    ▼
3. Clone all needed fields (tx, worker_state, config, execution_lock)
    │
    ▼
4. tokio::spawn(async move {
    │
    │  4a. Set worker_state.current_job_id/run_id
    │
    │  4b. Acquire execution lock
    │      ├── Ok → ExecutionLockGuard (RAII)
    │      └── Err → send error SampleResponse, clear state, return
    │
    │  4c. Build RunRequest + RunContext
    │
    │  4d. Build ControlPlaneSink from tx channel
    │      └── build_sink(Some(&tx)) → StreamSink
    │
    │  4e. Execute via engine
    │      ├── is_dryrun → engine::execute_dryrun()
    │      └── !is_dryrun → engine::execute_run(request, context, sink)
    │
    │  4f. Map result → SampleResponse
    │      ├── Ok(outcome) → sample_response_ok()
    │      └── Err(e) → sample_response_error()
    │
    │  4g. Clear worker_state.current_job_id/run_id
    │
    │  4h. Send SampleResponse via tx channel
    │      └── WorkerMessage::SampleResponse(result)
    │
    │  [ExecutionLockGuard drops → release lock]
   })
```

**Key design decisions:**

- **Spawned task:** Execution happens in a separate tokio task so the stream handler is not blocked. It can continue processing heartbeats, health checks, and other commands while the artifact runs.
- **Immediate ack:** The controller receives confirmation that the command was received before execution begins. This prevents timeouts on the controller side for long-running artifacts.
- **Run ID from request_id:** The `RunSampleCommand.request_id` becomes the `run_id`. This allows the controller to correlate the eventual `SampleResponse` with the original command.
- **Sink from tx:** The `ControlPlaneSink` is built from the raw `tx` channel, not from `Arc<StreamHandler>`. This avoids needing `Arc<Self>` inside the spawned task.
- **State bookkeeping:** `worker_state.current_job_id`/`current_run_id` are set before execution and cleared after, regardless of success or failure. This ensures `send_status_update()` reports the correct active job.

#### 3.2.5 `handle_heartbeat()` — Controller Liveness

```rust
async fn handle_heartbeat(&self, hb: Heartbeat) -> Result<()>
```

Updates `worker_state.last_controller_heartbeat` with the controller's timestamp. This is a passive operation — the worker does not send a heartbeat response. Instead, the worker sends its own heartbeats via `heartbeat_loop()`.

The `last_controller_heartbeat` field can be used for stale-connection detection: if the worker hasn't received a controller heartbeat in N seconds, the connection may be dead even if the gRPC stream hasn't errored yet.

#### 3.2.6 `handle_disconnect()` — Graceful Shutdown

```rust
async fn handle_disconnect(&self, notice: DisconnectNotice) -> Result<()>
```

Processes a graceful disconnect notification from the controller. Updates three state fields:

| Field | Value | Effect |
|-------|-------|--------|
| `controller_disconnected` | `true` | `heartbeat_loop()` uses debug-level logging instead of warn |
| `disconnect_reason` | e.g., `"controller_shutdown"` | Diagnostic information |
| `reconnect_allowed` | from `notice.reconnect_allowed` | Future use: reconnection logic |

**Log level differentiation:** Reconnect-allowed disconnects are logged at `info` level; reconnect-forbidden disconnects at `warn`. This helps operators distinguish routine shutdowns from permanent disconnections.

#### 3.2.7 `handle_health_check()` — On-Demand Status

```rust
async fn handle_health_check(&self, req: HealthCheckRequest) -> Result<()>
```

Responds to a health check by sending a status update via `send_status_update("health_check")`. This uses the same mechanism as the periodic heartbeat but with event_type `"health_check"` for differentiation.

#### 3.2.8 Outgoing Message Senders

Five methods send `WorkerMessage` variants through the `tx` channel:

| Method | Visibility | `WorkerMessage` Variant | When Called |
|--------|-----------|------------------------|------------|
| `send_registration()` | pub | `Registration(WorkerRegistration)` | Once, on stream establishment |
| `send_status_update()` | pub | `Status(StatusReport)` | Periodic heartbeat + on health check |
| `send_telemetry()` | pub | `Telemetry(TelemetryBatch)` | After execution (via ControlPlaneSink) |
| `send_ack()` | private | `Ack(Ack)` | Immediately after RunSample command |
| (inline in handle_run_sample) | — | `SampleResponse(SampleResponse)` | After execution completes |

#### 3.2.9 `send_registration()`

Sends the worker's identity to the controller immediately after stream establishment. Builds a `WorkerRegistration` message from:

| Field | Source |
|-------|--------|
| `worker_id` | `worker_state.worker_id` |
| `ip_address` | `config.worker.ip_address:config.worker.listen_port` |
| `os_version` | `config.worker.os_version` |
| `capabilities` | `worker_state.capabilities` |
| `metadata` | `worker_state.metadata` |
| `tools` | `worker_state.tools` |

This is the Phase 2 replacement for the Phase 1 `GetWorkerInfo` pull model — the worker pushes its identity proactively.

#### 3.2.10 `send_status_update()`

Builds a `StatusReport` from current `WorkerState` and sends it through the stream. Fields:

| Field | Source |
|-------|--------|
| `worker_id` | `state.worker_id` |
| `worker_ip` | `state.metadata["ip_address"]` (or empty) |
| `cpu_percent` | `state.health.cpu_percent` |
| `memory_mb` | `state.health.memory_percent` (note: field name mismatch — sends percent as mb) |
| `active_jobs` | 1 if `current_job_id.is_some()`, else 0 |
| `event_type` | parameter (e.g., `"heartbeat"`, `"health_check"`) |
| `current_job_id` | `state.current_job_id` (or empty) |

#### 3.2.11 `heartbeat_loop()` — Background Heartbeat Task

```rust
pub async fn heartbeat_loop(handler: Arc<StreamHandler>, interval_secs: u64)
```

Free function (not a method on `StreamHandler`) that runs as a background tokio task. Sends periodic status updates to the controller.

**Loop behavior:**

```
Every {interval_secs} seconds (default: 30):
    │
    ├── Read worker_state.controller_disconnected
    │
    ├── send_status_update("heartbeat")
    │   ├── Ok:
    │   │   ├── If was disconnected → log "reconnected", clear disconnect state
    │   │   └── Otherwise → debug log
    │   └── Err:
    │       ├── If disconnected → debug log (expected failure)
    │       └── Otherwise → warn log (unexpected failure)
    │
    └── Continue (never stops — retries forever)
```

**Key behaviors:**

- **Never stops:** The loop continues even after send failures. This ensures the worker keeps trying to send heartbeats if the connection is temporarily interrupted.
- **Reconnection detection:** If a heartbeat succeeds after the controller was marked as disconnected, the worker clears the disconnect state and logs at `info` level. This provides automatic reconnection awareness.
- **Adaptive logging:** Expected failures (controller explicitly disconnected) use `debug` level; unexpected failures use `warn`. This prevents log spam during graceful shutdowns.
- **Externally abortable:** The loop runs in a `tokio::spawn` task whose `JoinHandle` is stored in `service.heartbeat_handle`. On reconnection, `api/stream.rs` calls `handle.abort()` before creating a new heartbeat loop.

---

## 4. Cross-Module Interactions

### 4.1 Shared State Access Patterns

`WorkerState` is wrapped in `Arc<RwLock<WorkerState>>` and accessed by multiple components:

| Component | Access | Fields Read | Fields Written |
|-----------|--------|-------------|----------------|
| `stream_handler::handle_run_sample()` | Write | — | `current_job_id`, `current_run_id` |
| `stream_handler::handle_heartbeat()` | Write | — | `last_controller_heartbeat` |
| `stream_handler::handle_disconnect()` | Write | — | `controller_disconnected`, `disconnect_reason`, `reconnect_allowed` |
| `stream_handler::send_registration()` | Read | `worker_id`, `capabilities`, `metadata`, `tools` | — |
| `stream_handler::send_status_update()` | Read | `worker_id`, `metadata`, `health`, `current_job_id` | — |
| `heartbeat_loop()` | Read+Write | `controller_disconnected` | `controller_disconnected`, `disconnect_reason` |
| `api/run.rs` | Read | `current_run_id` (via `handler.worker_state`) | — |
| `api/stream.rs` | — | — | Creates `WorkerState` from capabilities |

### 4.2 Integration with Other Worker Modules

```
                    ┌──────────────┐
                    │ api/stream.rs│ creates StreamHandler,
                    │              │ stores in service.stream_handler
                    └──────┬───────┘
                           │
              ┌────────────▼─────────────────────────┐
              │         session/stream_handler.rs      │
              │                                        │
              │  incoming:                             │
              │    ControllerMessage → process_message │
              │                                        │
              │  dispatches to:                        │
              │  ┌────────────────────────────┐       │
              │  │ execution/engine.rs         │       │
              │  │ execute_run()               │       │
              │  │ execute_dryrun()            │       │
              │  └────────────────────────────┘       │
              │                                        │
              │  uses:                                 │
              │  ┌────────────────────────────┐       │
              │  │ execution/state.rs          │       │
              │  │ ExecutionState (lock)       │       │
              │  │ ExecutionLockGuard          │       │
              │  └────────────────────────────┘       │
              │  ┌────────────────────────────┐       │
              │  │ execution/sink.rs           │       │
              │  │ build_sink(Some(&tx))       │       │
              │  └────────────────────────────┘       │
              │  ┌────────────────────────────┐       │
              │  │ execution/types.rs          │       │
              │  │ RunRequest, RunContext      │       │
              │  │ sample_response_ok/error()  │       │
              │  │ format_output()             │       │
              │  └────────────────────────────┘       │
              │                                        │
              │  outgoing:                             │
              │    tx channel → ReceiverStream → gRPC  │
              └────────────────────────────────────────┘
                           │
              ┌────────────▼─────────────────────────┐
              │      session/worker_state.rs           │
              │                                        │
              │  populated from:                       │
              │  ┌────────────────────────────┐       │
              │  │ capabilities.rs             │       │
              │  │ WorkerCapabilities          │       │
              │  │ (detected at startup)       │       │
              │  └────────────────────────────┘       │
              │                                        │
              │  health metrics from:                  │
              │  ┌────────────────────────────┐       │
              │  │ infra/system.rs             │       │
              │  │ collect_system_metrics()    │       │
              │  └────────────────────────────┘       │
              └────────────────────────────────────────┘
```

### 4.3 Dual Execution Paths

The session module provides the **Phase 2 execution path** that coexists with the Phase 1 path:

| Aspect | Phase 1 (`api/run.rs`) | Phase 2 (`session/stream_handler.rs`) |
|--------|----------------------|--------------------------------------|
| Entry point | `RunSample` unary RPC | `RunSampleCommand` in stream |
| Request unwrap | `Request<SampleRequest>` | `RunSampleCommand.request` |
| Run ID source | `worker_state.current_run_id` or UUID | `cmd.request_id` |
| Execution lock | `service.execution_lock` | `self.execution_lock` (same Arc) |
| Engine call | `engine::execute_run()` | `engine::execute_run()` (identical) |
| Sink | `build_sink(handler.sender())` | `build_sink(Some(&tx))` |
| Response | Unary `Response<SampleResponse>` | `WorkerMessage::SampleResponse` via stream |
| Blocking | Blocks the RPC until done | Returns immediately (spawns task) |
| Ack | None | Immediate `Ack` before execution |

Both paths share the same `execution_lock`, preventing concurrent execution regardless of which path initiated it.

---

## 5. Connection Lifecycle

### 5.1 Stream Establishment

```
1. Controller calls EstablishStream RPC
    │
    ▼
2. api/stream.rs::establish_stream()
    ├── Create WorkerState from cached capabilities
    ├── Create StreamHandler (handler, rx)
    ├── Abort previous heartbeat task (handle.abort())
    ├── Store handler in service.stream_handler
    ├── Send registration → WorkerMessage::Registration
    ├── Spawn incoming message handler task
    ├── Spawn heartbeat loop task (30s interval)
    └── Return ReceiverStream(rx) as outgoing stream
```

### 5.2 Active Session

```
                          ┌─────────┐
                          │  Active  │
                          └────┬────┘
                               │
            ┌──────────────────┼──────────────────┐
            ▼                  ▼                  ▼
    Incoming messages    Outgoing heartbeats   Executions
    (handle_stream)      (heartbeat_loop)     (handle_run_sample)
            │                  │                  │
            │  processes:      │  sends:          │  spawns:
            │  RunSample       │  StatusReport    │  engine::execute_run()
            │  HealthCheck     │  every 30s       │  sends via tx:
            │  Heartbeat       │                  │  - Ack
            │  Disconnect      │                  │  - ExecutionStatus
            │  Ack             │                  │  - TelemetryBatch
            │                  │                  │  - SampleResponse
            └──────────────────┴──────────────────┘
```

### 5.3 Disconnection

**Graceful (DisconnectNotice received):**
```
DisconnectNotice → handle_disconnect()
    │
    ├── state.controller_disconnected = true
    ├── state.disconnect_reason = notice.reason
    └── state.reconnect_allowed = notice.reconnect_allowed
    │
    ▼
heartbeat_loop continues (debug-level log on failure)
    │
    ▼  (if reconnection happens)
heartbeat succeeds → clear disconnect state → log "reconnected"
```

**Ungraceful (stream closes/errors):**
```
stream.message() returns None or Err
    │
    ▼
handle_stream() exits normally
    │
    ▼
heartbeat_loop send failures → warn-level log
    │
    ▼  (on reconnection)
api/stream.rs aborts old heartbeat → creates new StreamHandler
```

### 5.4 Reconnection

```
Controller opens new EstablishStream
    │
    ▼
api/stream.rs::establish_stream()
    ├── Abort old heartbeat task
    ├── Create new StreamHandler (new tx/rx channels)
    ├── Replace service.stream_handler
    ├── Send fresh registration
    ├── Spawn new incoming handler
    └── Spawn new heartbeat loop
```

All previous stream state (old tx channel, old worker_state instance) is dropped. The new stream starts with a fresh `WorkerState`.

---

## 6. Summary Statistics

| Metric | Value |
|--------|-------|
| Files | 3 |
| Total lines | 495 |
| Structs | 3 (`StreamHandler`, `WorkerState`, `HealthMetrics`) |
| Public functions | 6 (`new`, `sender`, `handle_stream`, `send_registration`, `send_status_update`, `send_telemetry`) + `heartbeat_loop` |
| Private functions | 5 (`process_message`, `handle_run_sample`, `handle_health_check`, `handle_heartbeat`, `handle_disconnect`, `send_ack`) |
| Message variants handled | 6 (RunSample, HealthCheck, Heartbeat, Disconnect, Ack, ArtifactChunks) |
| Message variants sent | 5 (Registration, Status, Telemetry, Ack, SampleResponse) |
| Shared state fields | 11 (in WorkerState) |
| Background tasks spawned | 2 (incoming handler, heartbeat loop) + 1 per execution |
| Channel buffer size | 100 messages |
| Heartbeat interval | 30 seconds (configurable via parameter) |
