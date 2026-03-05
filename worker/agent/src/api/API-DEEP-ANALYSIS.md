# Worker Agent API — Deep Analysis

Deep analysis of `worker/agent/src/api/` — the gRPC request handler layer for the worker agent.

---

## 1. Overview

### Purpose

The `api/` folder is the **gRPC entry point** for the worker agent. It implements the `WorkerAgent` service defined in `worker.proto` (package `automutate.worker`), translating incoming gRPC requests into calls to the worker's internal subsystems (execution engine, telemetry collectors, session management).

### Role in the Global Project

In the AutoMutate++ architecture, the worker agent runs on each Windows VM and serves two purposes:

1. **Execute artifacts** — receive cross-compiled PE binaries from the controller, run them under monitoring (RedEDR/ETW), and return execution outcomes + telemetry
2. **Maintain a real-time channel** — keep a bidirectional gRPC stream with the controller for commands, status, and telemetry

The `api/` folder is the **thin adapter layer** between the gRPC transport (tonic) and the worker's domain logic. It contains zero business logic itself — every handler validates the request, delegates to an internal module, and maps the result back to a protobuf response.

```
Controller (gRPC client)
    │
    │  worker.proto RPCs
    ▼
┌──────────────────────────────────┐
│         api/ (this folder)       │  ← gRPC handlers (thin adapters)
│  mod.rs  │ WorkerAgent trait impl│
│  run.rs  │ artifacts.rs          │
│  info.rs │ stream.rs             │
└──────┬───────────────────────────┘
       │ delegates to
       ▼
┌──────────────────────────────────┐
│     Internal Worker Modules      │
│  execution/  │ engine, state,    │
│              │ sink, monitor     │
│  telemetry/  │ rededr, trace     │
│  session/    │ stream_handler,   │
│              │ worker_state      │
│  capabilities│ infra/            │
└──────────────────────────────────┘
```

### Design Pattern: Thin Adapter

The module comment in `mod.rs` line 1 states the intent explicitly:

> `// API modules - gRPC handler implementations (thin adapters)`

Each file exports free functions (not methods on `WorkerAgentService`) that take `&WorkerAgentService` as the first parameter. The `mod.rs` file implements the tonic-generated `WorkerAgent` trait by dispatching each RPC to the corresponding function. This keeps the trait implementation clean and the handler logic testable in isolation.

---

## 2. File Inventory

| File | Lines | Functions | RPCs Handled | Domain |
|------|-------|-----------|-------------|--------|
| `mod.rs` | 71 | 0 (trait impl only) | All 7 | Dispatch hub |
| `run.rs` | 109 | 1 | `RunSample` | Artifact execution |
| `artifacts.rs` | 83 | 1 | `SendArtifact` | Binary transfer |
| `info.rs` | 210 | 4 | `Ping`, `HealthCheck`, `GetWorkerInfo`, `GetTelemetry` | Metadata & telemetry |
| `stream.rs` | 85 | 1 | `EstablishStream` | Bidirectional channel |
| **Total** | **558** | **7** | **7** | — |

---

## 3. Module Architecture

### 3.1 `mod.rs` — Dispatch Hub

**Purpose:** Implements the `WorkerAgent` tonic trait for `WorkerAgentService`, dispatching each RPC to its handler module.

**What it does:**
- Imports all generated protobuf types needed across the 7 RPCs
- Defines the `#[tonic::async_trait] impl WorkerAgent for WorkerAgentService` block
- Each method is a one-liner that delegates to the corresponding handler function
- Defines two associated stream types:
  - `GetTelemetryStream` — `Pin<Box<dyn Stream<Item = Result<TelemetryData, Status>> + Send>>`
  - `EstablishStreamStream` — `ReceiverStream<Result<WorkerMessage, Status>>`

**RPC dispatch map:**

| RPC | Handler Function | Module |
|-----|-----------------|--------|
| `Ping` | `info::ping()` | info.rs |
| `RunSample` | `run::run_sample()` | run.rs |
| `HealthCheck` | `info::health_check()` | info.rs |
| `SendArtifact` | `artifacts::send_artifact()` | artifacts.rs |
| `GetWorkerInfo` | `info::get_worker_info()` | info.rs |
| `GetTelemetry` | `info::get_telemetry()` | info.rs |
| `EstablishStream` | `stream::establish_stream()` | stream.rs |

**Key design decisions:**
- Free functions instead of methods avoids `self` borrowing issues with async tonic handlers
- All functions take `&WorkerAgentService` (immutable reference) — mutation happens through interior mutability (`Arc<Mutex<...>>`, `Arc<RwLock<...>>`)

---

### 3.2 `run.rs` — Artifact Execution Handler

**Purpose:** Handles the `RunSample` unary RPC — the Phase 1 execution path where the controller directly calls the worker to run an artifact.

**Function:** `run_sample(&WorkerAgentService, Request<SampleRequest>) -> Result<Response<SampleResponse>, Status>`

**Flow:**

```
SampleRequest arrives
    │
    ▼
1. Resolve run_id
   ├── Check stream_handler.worker_state.current_run_id (controller-assigned)
   └── Fallback: generate UUID v4
    │
    ▼
2. Build RunRequest + RunContext
   ├── RunRequest: job_id, artifact_id, timeout, run_id
   └── RunContext: artifact_path, telemetry_dir (derived from config)
    │
    ▼
3. Acquire execution lock
   ├── ExecutionState::Idle → Running { job_id, artifact, run_id }
   └── ExecutionState::Running → REJECT with resource_exhausted
    │
    ▼
4. Build ControlPlaneSink
   ├── If stream_handler exists → StreamSink (sends via bidirectional stream)
   └── If no stream → NullSink (no-op)
    │
    ▼
5. Execute via engine
   ├── req.is_dryrun=true  → engine::execute_dryrun() (no RedEDR, exit code only)
   └── req.is_dryrun=false → engine::execute_run()  (full RedEDR + telemetry)
    │
    ▼
6. Map RunOutcome → SampleResponse
   ├── format_output() for human-readable output
   ├── sample_response_ok() maps exit code → detected flag
   └── detection_verdict + last_checkpoint from classifier
    │
    ▼
7. Return SampleResponse (lock auto-released via ExecutionLockGuard RAII)
```

**Key behaviors:**

- **Single execution lock:** Only one artifact can execute at a time per worker. This prevents RedEDR telemetry cross-contamination between concurrent runs. A second `RunSample` call while busy returns `Status::resource_exhausted`.

- **Run ID resolution:** Prefers the controller-assigned `request_id` (set via the bidirectional stream's `RunSampleCommand`) over a locally generated UUID. This ensures the controller can correlate responses.

- **Dryrun support:** When `SampleRequest.is_dryrun = true`, the engine skips RedEDR setup and telemetry collection, only returning the process exit code. Used for the dryrun leg of the three-run differential protocol.

- **Sink pattern:** The execution engine sends status updates and telemetry through a `ControlPlaneSink` trait, not directly through the stream handler. This decouples execution from transport — if no stream exists, updates are silently dropped via `NullSink`.

**Dependencies:**
- `execution::engine` — `execute_run()`, `execute_dryrun()`
- `execution::state` — `ExecutionState`, `ExecutionLockGuard`
- `execution::types` — `RunRequest`, `RunContext`, `RunOutcome`, `format_output()`, `sample_response_ok()`
- `execution::sink` — `build_sink()`
- `session::stream_handler` — for reading `worker_state.current_run_id`

---

### 3.3 `artifacts.rs` — Binary Transfer Handler

**Purpose:** Handles the `SendArtifact` client-streaming RPC — receives an artifact binary from the controller as a stream of 4MB chunks.

**Function:** `send_artifact(&WorkerAgentService, Request<Streaming<ArtifactChunk>>) -> Result<Response<TransferAck>, Status>`

**Flow:**

```
Stream of ArtifactChunk messages arrives
    │
    ▼
1. Consume all chunks from stream
   ├── Extract artifact_id and expected_sha256 from first chunk
   └── Accumulate chunks in Vec
    │
    ▼
2. Validate
   ├── Empty stream → invalid_argument error
   └── Sort chunks by chunk_index (handles out-of-order delivery)
    │
    ▼
3. Reassemble binary
   └── Concatenate chunk.data in order
    │
    ▼
4. Verify integrity
   ├── Compute SHA-256 of reassembled binary
   └── Compare against expected_sha256 → data_loss error on mismatch
    │
    ▼
5. Write to disk
   ├── Create artifacts directory if needed
   └── Write to {artifacts_path}/{artifact_id}.exe
    │
    ▼
6. Return TransferAck
   └── received=true, chunks_received=N, storage_path=...
```

**Key behaviors:**

- **Integrity verification:** SHA-256 hash verification ensures no corruption during transfer. On mismatch, returns `Status::data_loss` — a retryable error.

- **Chunk reordering:** Chunks are sorted by `chunk_index` after collection, so out-of-order gRPC delivery is handled transparently.

- **Storage convention:** Artifacts are stored as `{artifact_id}.exe` in the configured `storage.artifacts_path` directory. The `run.rs` handler later locates them by the same convention via `RunContext::new()`.

- **No streaming backpressure:** All chunks are collected into memory before writing. This is acceptable because artifacts are typically small (tens of KB to a few MB), well within the 4MB-per-chunk limit.

**Dependencies:**
- `sha2::Sha256` — integrity verification
- `WorkerAgentService.config.storage.artifacts_path` — disk storage location

---

### 3.4 `info.rs` — Metadata, Health & Telemetry Handlers

**Purpose:** Handles four information-query RPCs: `Ping`, `HealthCheck`, `GetWorkerInfo`, and `GetTelemetry`. These are read-only operations that report worker state.

#### 3.4.1 `ping()`

**RPC:** `Ping(PingRequest) -> PingResponse`

Simple connectivity test. Returns `"pong: {message}"` with the current Unix timestamp and worker identity string `"worker-agent/{worker_id}"`.

#### 3.4.2 `health_check()`

**RPC:** `HealthCheck(HealthRequest) -> HealthResponse`

**What it reports:**
| Field | Source | Logic |
|-------|--------|-------|
| `worker_id` | `service.worker_id` | Static identity |
| `healthy` | CPU + memory check | `cpu < 95% AND memory < 95%` |
| `cpu_percent` | `sysinfo` crate | Refreshed on each call |
| `memory_percent` | `sysinfo` crate | Refreshed on each call |
| `active_jobs` | `ExecutionState` | 1 if `Running`, 0 if `Idle` |

**Key behavior:** Uses `service.system_info` (shared `Arc<Mutex<System>>`) to avoid re-creating the sysinfo object on every call. Refreshes only CPU and memory metrics (not disk, network, etc.) for efficiency.

#### 3.4.3 `get_worker_info()`

**RPC:** `GetWorkerInfo(WorkerInfoRequest) -> WorkerInfoResponse`

Returns the full worker capability profile. This is the Phase 1 pull model — the controller queries workers for their capabilities instead of workers pushing registration.

**What it reports:**
| Field | Source |
|-------|--------|
| `worker_id` | `service.worker_id` |
| `ip_address` | `config.worker.ip_address` |
| `os_version` | `config.worker.os_version` |
| `capabilities` | `service.capabilities` (cached at startup) |
| `metadata` | `service.capabilities` (cached at startup) |
| `tools` | `service.capabilities.to_tool_versions()` |
| `health` | Fresh `HealthMetrics` (CPU, memory, disk, active_jobs, uptime) |
| `current_job_id` | `ExecutionState` |

**Key behavior:** Capabilities are detected once at startup (expensive I/O: checking tool versions, OS features) and cached in `Arc<WorkerCapabilities>`. Only health metrics are collected fresh per call.

#### 3.4.4 `get_telemetry()`

**RPC:** `GetTelemetry(TelemetryRequest) -> stream TelemetryData`

Server-streaming RPC that returns telemetry events for a given job. This is the Phase 1 pull model — the controller calls this to fetch telemetry instead of the worker pushing via `StreamTelemetry`.

**Flow:**

```
TelemetryRequest { job_id, since_timestamp, max_events }
    │
    ▼
1. Create mpsc channel (buffer: 100)
    │
    ▼
2. Spawn background task
   ├── Create RedEdrCollector with config from service
   ├── Call collector.collect_all(job_id)
   ├── Apply max_events limit (if > 0)
   └── Send events through tx channel
    │
    ▼
3. Return ReceiverStream(rx) immediately
   └── Client receives events as they're sent through channel
```

**Key behavior:**
- Non-blocking: returns the stream immediately, collection happens asynchronously
- Uses `RedEdrCollector` from the telemetry subsystem to fetch events from RedEDR's HTTP API
- Supports pagination via `max_events` and time filtering via `since_timestamp` (though time filtering is not yet fully implemented — marked TODO)

**Dependencies:**
- `telemetry::collectors::rededr::RedEdrCollector` — RedEDR HTTP API client
- `sysinfo` crate — system metrics (health_check, get_worker_info)
- `infra::time::now_unix_secs()` — timestamp generation
- `infra::system::collect_system_metrics()` — CPU/memory collection

---

### 3.5 `stream.rs` — Bidirectional Stream Handler

**Purpose:** Handles the `EstablishStream` bidirectional RPC — the Phase 2 real-time communication channel between controller and worker.

**Function:** `establish_stream(&WorkerAgentService, Request<Streaming<ControllerMessage>>) -> Result<Response<ReceiverStream<Result<WorkerMessage, Status>>>, Status>`

**Flow:**

```
Controller opens bidirectional stream
    │
    ▼
1. Create WorkerState from cached capabilities
    │
    ▼
2. Create StreamHandler
   ├── Allocates mpsc channel (buffer: 100)
   ├── Returns (handler, rx) — handler sends, rx receives
   └── Stores worker_id, config, execution_lock (no Arc<WorkerAgentService>)
    │
    ▼
3. Cleanup previous session
   ├── Abort previous heartbeat task (prevents orphaned loops)
   └── Store new handler in service.stream_handler (RwLock)
    │
    ▼
4. Send registration
   └── handler.send_registration() → WorkerMessage::Registration
    │
    ▼
5. Spawn incoming message handler (background task)
   └── handler.handle_stream(stream) — processes ControllerMessage variants:
       ├── RunSample → execute artifact (calls engine directly)
       ├── HealthCheck → send status update
       ├── Heartbeat → update last_controller_heartbeat
       ├── Disconnect → update state, log reason
       ├── Ack → debug log
       └── ArtifactChunks → warn (not yet implemented)
    │
    ▼
6. Spawn heartbeat loop (background task)
   └── Every 30s: send StatusReport via stream
    │
    ▼
7. Return ReceiverStream(rx) as outgoing stream
```

**Key behaviors:**

- **No Arc cycle:** The `StreamHandler` does NOT hold `Arc<WorkerAgentService>`. Instead it receives individual fields (`worker_id`, `config`, `execution_lock`). This breaks what would otherwise be a reference cycle: `WorkerAgentService → stream_handler → WorkerAgentService`.

- **Reconnection handling:** When a new stream is established, the previous heartbeat task is aborted via `JoinHandle::abort()`. The new handler replaces the old one in `service.stream_handler`. This handles controller reconnections cleanly.

- **Dual execution paths:** Artifacts can be executed via two paths:
  1. **Unary RPC** (`RunSample` via `run.rs`) — controller calls worker directly
  2. **Stream command** (`RunSampleCommand` via stream) — controller sends command through bidirectional stream

  Both paths share the same `execution_lock` and call the same `engine::execute_run()` function.

- **Stream-based execution (in StreamHandler):** When `RunSampleCommand` arrives via stream, the handler spawns a task that: acquires the execution lock, runs the engine, sends the response back through the stream's tx channel. This is a parallel execution path to `run.rs` — same engine, different transport.

- **Handler stored for cross-RPC access:** The handler is stored in `service.stream_handler` so that `run.rs` can access the stream's tx channel to build a `ControlPlaneSink`. This allows unary RPC executions to still send status updates through the bidirectional stream.

**Dependencies:**
- `session::stream_handler::StreamHandler` — message processing and outgoing message sending
- `session::worker_state::WorkerState` — shared mutable state for the stream session
- `session::stream_handler::heartbeat_loop` — periodic status updates

---

## 4. Cross-Module Interactions

### 4.1 Shared State

All handlers access `WorkerAgentService` fields through interior mutability:

| State | Type | Accessed By | Purpose |
|-------|------|-------------|---------|
| `execution_lock` | `Arc<Mutex<ExecutionState>>` | run.rs, stream.rs | Single-execution guarantee |
| `stream_handler` | `Arc<RwLock<Option<Arc<StreamHandler>>>>` | run.rs, stream.rs | Cross-RPC stream access |
| `system_info` | `Arc<Mutex<System>>` | info.rs | Cached sysinfo object |
| `capabilities` | `Arc<WorkerCapabilities>` | info.rs, stream.rs | Immutable capability cache |
| `heartbeat_handle` | `Arc<RwLock<Option<JoinHandle<()>>>>` | stream.rs | Reconnect cleanup |
| `config` | `WorkerConfig` (Clone) | all modules | Configuration |

### 4.2 Data Flow Through the API Layer

```
                    ┌─────────────────────────────────────────────┐
                    │              Controller                      │
                    └──────────┬──────────────┬───────────────────┘
                               │              │
                    Unary RPCs │              │ Bidirectional Stream
                               │              │
              ┌────────────────┼──────────────┼────────────────────┐
              │  api/          │              │                     │
              │                ▼              ▼                     │
              │  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
              │  │ run.rs   │  │ info.rs  │  │ stream.rs        │  │
              │  │RunSample │  │Ping      │  │EstablishStream   │  │
              │  │          │  │Health    │  │                  │  │
              │  │          │  │Info      │  │  ┌────────────┐  │  │
              │  │          │  │Telemetry │  │  │StreamHndlr │  │  │
              │  └────┬─────┘  └────┬─────┘  │  │RunSample   │  │  │
              │       │             │         │  │HealthCheck │  │  │
              │       │             │         │  │ Heartbeat  │  │  │
              │       │             │         │  │ Disconnect │  │  │
              │       │             │         │  └─────┬──────┘  │  │
              └───────┼─────────────┼─────────┼───────┼──────────┘  │
                      │             │                 │              │
                      ▼             ▼                 ▼              │
              ┌──────────────────────────────────────────┐          │
              │         execution::engine                  │          │
              │    execute_run() / execute_dryrun()        │          │
              ├──────────────────────────────────────────┤          │
              │         execution::state                   │          │
              │    ExecutionState (Idle ↔ Running)         │          │
              ├──────────────────────────────────────────┤          │
              │         execution::sink                    │          │
              │    StreamSink / NullSink                   │          │
              └──────────────────────────────────────────┘          │
                                                                     │
              ┌──────────────────────────────────────────┐          │
              │   artifacts.rs                            │          │
              │   SendArtifact → disk write               │──────────┘
              │   {artifacts_path}/{id}.exe               │
              └──────────────────────────────────────────┘
```

### 4.3 Phase 1 vs Phase 2 Execution Paths

The API layer supports two distinct execution models that coexist:

| Aspect | Phase 1 (Unary RPCs) | Phase 2 (Bidirectional Stream) |
|--------|---------------------|-------------------------------|
| Entry point | `run.rs::run_sample()` | `stream.rs` → `StreamHandler::handle_run_sample()` |
| Transport | Individual gRPC calls | Multiplexed `ControllerMessage` envelope |
| Artifact transfer | `SendArtifact` (client-streaming) | `ArtifactChunkBatch` via stream (not yet implemented) |
| Telemetry return | `GetTelemetry` (server-streaming) | `TelemetryBatch` via stream |
| Status reporting | `ReportStatus` (unary to controller) | `StatusReport` via stream |
| Run ID source | `stream_handler.worker_state.current_run_id` or UUID | `RunSampleCommand.request_id` |
| Response delivery | Unary `SampleResponse` return | `WorkerMessage::SampleResponse` via stream |

Both paths converge at the execution engine — they call the same `engine::execute_run()` / `engine::execute_dryrun()` functions and share the same `execution_lock`.

---

## 5. Error Handling Strategy

| Error Type | Handler | gRPC Status | Behavior |
|-----------|---------|-------------|----------|
| Worker busy | run.rs | `resource_exhausted` | Caller retries later |
| No chunks received | artifacts.rs | `invalid_argument` | Request malformed |
| SHA-256 mismatch | artifacts.rs | `data_loss` | Integrity failure, retryable |
| Disk write failure | artifacts.rs | `internal` | Storage issue |
| Registration send failure | stream.rs | `internal` | Stream establishment fails |
| Execution engine error | run.rs | Mapped via `e.into_status()` | Engine errors converted to Status |
| Telemetry collection error | info.rs | `internal` | Sent as error on stream |
| Stream message processing error | stream.rs (via StreamHandler) | Warning logged | Continues processing other messages |

---

## 6. Summary Statistics

| Metric | Value |
|--------|-------|
| Files | 5 |
| Total lines | 558 |
| RPCs implemented | 7 (all of WorkerAgent service) |
| Unary RPCs | 4 (Ping, RunSample, HealthCheck, GetWorkerInfo) |
| Client-streaming RPCs | 1 (SendArtifact) |
| Server-streaming RPCs | 1 (GetTelemetry) |
| Bidirectional RPCs | 1 (EstablishStream) |
| Background tasks spawned | 3 (telemetry collector, stream handler, heartbeat loop) |
| External crate dependencies | sysinfo, sha2, uuid, tokio, tonic, tokio_stream |
| Internal module dependencies | execution (engine, state, sink, types), session (stream_handler, worker_state), telemetry (rededr), infra (time, system), capabilities |
