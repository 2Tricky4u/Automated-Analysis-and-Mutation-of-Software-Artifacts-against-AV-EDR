# Worker Agent — Module & Struct Reference

Complete module tree, struct field descriptions, enum variants, trait definitions, and function signatures for `worker/agent/`. Generated from `cargo modules structure` output and `struct_printer.py` field extraction, annotated with usage context from the submodule deep analyses.

---

## 1. Module Tree

```
crate worker_agent
│
├── struct WorkerAgentService                          [lib.rs]
│
├── mod api                                            [api/]
│   ├── mod artifacts
│   ├── mod info
│   ├── mod run
│   └── mod stream
│
├── mod automutate                                     [lib.rs — generated proto]
│   ├── mod common
│   ├── mod controller
│   └── mod worker
│
├── mod capabilities                                   [capabilities.rs]
│
├── mod constants                                      [constants.rs]
│
├── mod execution                                      [execution/]
│   ├── mod classifier
│   ├── mod engine
│   ├── mod guards
│   ├── mod monitor
│   ├── mod sink
│   ├── mod state
│   └── mod types
│
├── mod infra                                          [infra/]
│   ├── mod process
│   ├── mod system
│   └── mod time
│
├── mod session                                        [session/]
│   ├── mod stream_handler
│   └── mod worker_state
│
└── mod telemetry                                      [telemetry/]
    ├── mod collectors
    │   ├── mod rededr
    │   └── mod trace
    ├── mod pipeline
    └── mod trace_compressor
```

---

## 2. Root — `lib.rs`

### 2.1 `WorkerAgentService`

The central service struct. Cloneable (required by tonic) — all mutable state is behind `Arc<Mutex/RwLock>`.

```rust
#[derive(Clone)]
pub struct WorkerAgentService {
    worker_id:        String,
    config:           automutate_config::WorkerConfig,
    system_info:      Arc<Mutex<sysinfo::System>>,
    execution_lock:   Arc<Mutex<ExecutionState>>,
    stream_handler:   Arc<RwLock<Option<Arc<StreamHandler>>>>,
    heartbeat_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    capabilities:     Arc<WorkerCapabilities>,
}
```

| Field | Type | Mutability | Description |
|-------|------|------------|-------------|
| `worker_id` | `String` | Immutable | Unique worker identity (e.g., `"win10-worker-01"`). Set at startup from config. |
| `config` | `WorkerConfig` | Immutable (Clone) | TOML-loaded configuration: paths, ports, logging, storage. Cloned to each tonic connection. |
| `system_info` | `Arc<Mutex<sysinfo::System>>` | Interior mutable | Shared sysinfo object. Refreshed on demand by `api/info.rs` for CPU/memory metrics. Avoids recreating the heavyweight `System` object per call. |
| `execution_lock` | `Arc<Mutex<ExecutionState>>` | Interior mutable | Single-execution guarantee. `Idle` or `Running{job_id, artifact, run_id}`. Shared between `api/run.rs`, `api/stream.rs`, and `session/stream_handler.rs`. Prevents RedEDR telemetry cross-contamination. |
| `stream_handler` | `Arc<RwLock<Option<Arc<StreamHandler>>>>` | Interior mutable | Active bidirectional stream session, or `None` if no stream is established. Written by `api/stream.rs` on connect; read by `api/run.rs` to get the tx channel for `ControlPlaneSink`. |
| `heartbeat_handle` | `Arc<RwLock<Option<JoinHandle<()>>>>` | Interior mutable | Background heartbeat task handle. Aborted via `handle.abort()` on reconnection to prevent orphaned heartbeat loops. |
| `capabilities` | `Arc<WorkerCapabilities>` | Immutable | Detected tools, OS metadata, hardware specs. Expensive I/O done once at startup, cached for the process lifetime. |

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `fn new(worker_id: String, config: WorkerConfig, capabilities: WorkerCapabilities) -> Self` | Constructor. Initializes `execution_lock` to `Idle`, `system_info` to `System::new_all()`, stream/heartbeat to `None`. |
| `get_execution_state()` | `async fn get_execution_state(&self) -> ExecutionState` | Returns a clone of the current execution state. Used by `api/info.rs` for health check reporting. |
| `truncate_middle_output()` | `fn truncate_middle_output(stdout_output: &str) -> String` | If > 1000 chars, keeps first 400 + last 400 with truncation notice. Prevents gRPC message bloat from verbose artifact output. |

---

## 3. `mod api` — gRPC Handlers

Thin adapter layer. Zero business logic. Each handler is a free function taking `&WorkerAgentService`.

### 3.1 `mod api::artifacts`

| Function | Signature | RPC | Description |
|----------|-----------|-----|-------------|
| `send_artifact()` | `async fn(service, Request<Streaming<ArtifactChunk>>) -> Result<Response<TransferAck>>` | `SendArtifact` (client-streaming) | Receives chunked PE binary, sorts by index, reassembles, verifies SHA-256, writes to `{artifacts_path}/{artifact_id}.exe`. Returns `TransferAck` with chunk count and storage path. |

### 3.2 `mod api::info`

| Function | Signature | RPC | Description |
|----------|-----------|-----|-------------|
| `ping()` | `async fn(service, Request<PingRequest>) -> Result<Response<PingResponse>>` | `Ping` | Returns `"pong: {message}"` with timestamp and `"worker-agent/{worker_id}"` identity. |
| `health_check()` | `async fn(service, Request<HealthRequest>) -> Result<Response<HealthResponse>>` | `HealthCheck` | Refreshes CPU/memory via `sysinfo`, returns `healthy = cpu<95% AND mem<95%`, active job count. |
| `get_worker_info()` | `async fn(service, Request<WorkerInfoRequest>) -> Result<Response<WorkerInfoResponse>>` | `GetWorkerInfo` | Full capability profile: identity, IP, OS, capabilities, tools, metadata, live health metrics, current job. |
| `get_telemetry()` | `async fn(service, Request<TelemetryRequest>) -> Result<Response<GetTelemetryStream>>` | `GetTelemetry` (server-streaming) | Spawns `RedEdrCollector.collect_all()`, sends events through mpsc channel as server-stream. Supports `max_events` limit. |

### 3.3 `mod api::run`

| Function | Signature | RPC | Description |
|----------|-----------|-----|-------------|
| `run_sample()` | `async fn(service, Request<SampleRequest>) -> Result<Response<SampleResponse>>` | `RunSample` | Phase 1 execution path. Resolves run_id, acquires execution lock (or rejects `resource_exhausted`), builds sink from stream_handler, calls `engine::execute_run/dryrun`, maps `RunOutcome` → `SampleResponse`. |

### 3.4 `mod api::stream`

| Function | Signature | RPC | Description |
|----------|-----------|-----|-------------|
| `establish_stream()` | `async fn(service, Request<Streaming<ControllerMessage>>) -> Result<Response<EstablishStreamStream>>` | `EstablishStream` (bidirectional) | Creates `WorkerState` + `StreamHandler`, aborts old heartbeat, stores new handler, sends registration, spawns incoming handler + heartbeat loop, returns `ReceiverStream(rx)`. |

---

## 4. `mod capabilities` — Startup Detection

### 4.1 `WorkerCapabilities`

```rust
pub struct WorkerCapabilities {
    capabilities: Vec<String>,
    tools:        HashMap<String, String>,
    metadata:     HashMap<String, String>,
}
```

| Field | Type | Content |
|-------|------|---------|
| `capabilities` | `Vec<String>` | Feature tags detected at startup: `"rededr"`, `"mde"`, `"cortex"`. Extended by `config.worker.extra_capabilities` (e.g., `"dryrun"`). |
| `tools` | `HashMap<String, String>` | Tool version strings. Keys: `rededr_version`, `defender_version`, `etw_version`, `llvm_version`. |
| `metadata` | `HashMap<String, String>` | System info. Keys: `hostname`, `cpu_cores`, `ram_gb`, `os_key` (e.g., `"win11-build-22631"`), `os_build`. |

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `to_tool_versions()` | `fn to_tool_versions(&self) -> ToolVersions` | Converts `tools` HashMap to protobuf `ToolVersions` message. Missing keys default to empty string. |

### 4.2 `WindowsVersionInfo`

```rust
pub struct WindowsVersionInfo {
    product_name:    Option<String>,
    edition_id:      Option<String>,
    display_version: Option<String>,
    release_id:      Option<String>,
    build:           Option<u32>,
    ubr:             Option<u32>,
    is_windows_11:   Option<bool>,
}
```

| Field | Type | Source | Description |
|-------|------|--------|-------------|
| `product_name` | `Option<String>` | Registry `ProductName` | e.g., `"Windows 10 Pro"` |
| `edition_id` | `Option<String>` | Registry `EditionID` | e.g., `"Professional"` |
| `display_version` | `Option<String>` | Registry `DisplayVersion` | e.g., `"23H2"` |
| `release_id` | `Option<String>` | Registry `ReleaseId` | Legacy release ID |
| `build` | `Option<u32>` | Registry `CurrentBuildNumber` | e.g., `22631` |
| `ubr` | `Option<u32>` | Registry `UBR` (DWORD) | Update Build Revision |
| `is_windows_11` | `Option<bool>` | Derived: `build >= 22000` | Practical Win11 detection heuristic |

### 4.3 Functions

| Function | Visibility | Signature | Description |
|----------|-----------|-----------|-------------|
| `detect_capabilities()` | `pub` | `async fn() -> Result<WorkerCapabilities>` | Main entry point. Probes RedEDR HTTP, Defender service, MDE registry, Cortex XDR, OS version, hardware. Returns combined result. |
| `check_rededr_available()` | `pub(self)` | `async fn(client: &reqwest::Client) -> bool` | GET `localhost:8081/api/stats` → 200 OK = present. |
| `get_rededr_version()` | `pub(self)` | `async fn(client: &reqwest::Client) -> Option<String>` | GET `localhost:8081/api/logs/agent` → regex `RedEdr\s+(\d+\.\d+)`. |
| `check_defender_available()` | `pub(self)` | `fn() -> bool` | `sc query WinDefend` → contains `RUNNING`. Windows-only. |
| `get_defender_version()` | `pub(self)` | `fn() -> Option<String>` | PowerShell `(Get-MpComputerStatus).AMProductVersion`. Windows-only. |
| `is_mde_onboarded()` | `pub(self)` | `fn() -> bool` | Registry `HKLM\...\Windows Advanced Threat Protection\OnboardedInfo` non-empty. `#[cfg(windows)]`. |
| `is_cortex_xdr_present()` | `pub(self)` | `fn() -> bool` | Checks service registry key OR `C:\ProgramData\Cyvera` exists. `#[cfg(windows)]`. |
| `is_cortex_xdr_installed()` | `pub(self)` | `fn() -> bool` | Registry `HKLM\...\Services\CyveraService`. `#[cfg(windows)]`. |
| `is_cortex_xdr_footprint_present()` | `pub(self)` | `fn() -> bool` | `C:\ProgramData\Cyvera` or `C:\Program Files\Palo Alto Networks\Traps` exists. `#[cfg(windows)]`. |
| `get_windows_version_info()` | `pub` | `fn() -> WindowsVersionInfo` | Registry `HKLM\...\Windows NT\CurrentVersion`. `#[cfg(windows)]`. Non-Windows returns all `None`. |
| `get_hostname()` | `pub(self)` | `fn() -> String` | `COMPUTERNAME` env var, fallback `HOSTNAME`, fallback `"unknown"`. |
| `get_cpu_cores()` | `pub(self)` | `fn() -> usize` | `std::thread::available_parallelism()`, fallback 1. |
| `get_total_ram_gb()` | `pub(self)` | `fn() -> u64` | `sysinfo::System::total_memory() / 1GB`. |

---

## 5. `mod constants` — Tuning Parameters

```rust
pub const CLEANUP_TIMEOUT_SECS: u64       = 10;
pub const MONITOR_POLL_INTERVAL_SECS: u64 = 3;
pub const CPU_IDLE_THRESHOLD: i32         = 5;
pub const IDLE_COUNT_THRESHOLD: i32       = 3;
pub const TIMEOUT_APPROACH_SECS: i32      = 5;
pub const MAX_SERIALIZED_PAYLOAD: usize   = 3_500_000;
```

| Constant | Value | Used By | Description |
|----------|-------|---------|-------------|
| `CLEANUP_TIMEOUT_SECS` | 10s | `MonitorGuard::stop()` | Max wait for monitor graceful shutdown before force-abort. |
| `MONITOR_POLL_INTERVAL_SECS` | 3s | `ExecutionMonitor::start()` | Interval between process-alive checks + RedEDR stats queries. |
| `CPU_IDLE_THRESHOLD` | 5% | `ExecutionMonitor` | CPU usage below this = process is idle (not computing). |
| `IDLE_COUNT_THRESHOLD` | 3 | `ExecutionMonitor` | Consecutive idle polls (both CPU idle AND no new events) before `telemetry_idle`. |
| `TIMEOUT_APPROACH_SECS` | 5s | `ExecutionMonitor` | Seconds before timeout to emit `approaching_timeout` status. |
| `MAX_SERIALIZED_PAYLOAD` | 3.5MB | `pipeline::package_trace_log()` | Max JSON payload size for gRPC (4MB default limit minus overhead). |

---

## 6. `mod execution` — Core Execution Engine

### 6.1 `mod execution::engine`

#### `enum RunError`

```rust
pub enum RunError {
    ArtifactNotFound(String),
    RedEdrSetupFailed(String),
    EnvironmentSetupFailed(String),
    ProcessSpawnFailed(String),
    FailedPrecondition(String),
}
```

| Variant | gRPC Status | Cause |
|---------|-------------|-------|
| `ArtifactNotFound` | `not_found` | `{artifact_path}` does not exist on disk. Forgot `SendArtifact`? |
| `RedEdrSetupFailed` | `internal` | RedEDR HTTP API call failed during Phase 2 setup. |
| `EnvironmentSetupFailed` | `internal` | Telemetry directory creation failed in Phase 3. |
| `ProcessSpawnFailed` | `internal` | `tokio::process::Command::spawn()` failed in Phase 4. |
| `FailedPrecondition` | `failed_precondition` | RedEDR contaminated (>1 stale events) in strict mode. |

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `into_status()` | `fn into_status(self) -> tonic::Status` | Maps each variant to the appropriate gRPC status code. |

#### Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `execute_run()` | `async fn(req: RunRequest, ctx: RunContext, sink: Arc<dyn ControlPlaneSink>) -> Result<RunOutcome, RunError>` | Full 10-phase execution pipeline. Sets up RedEDR, spawns process, monitors, collects telemetry from 5 sources, classifies, streams results. |
| `execute_dryrun()` | `async fn(req: RunRequest, ctx: RunContext) -> Result<RunOutcome, RunError>` | Lightweight path: validate → spawn → wait → classify (empty telemetry) → cleanup. No RedEDR, no trace pipe, no monitor. |

---

### 6.2 `mod execution::classifier`

#### `ClassificationEvidence` (private)

```rust
struct ClassificationEvidence {
    exit_code:    i32,
    timed_out:    bool,
    has_launched: bool,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `exit_code` | `i32` | Process exit code or synthetic code (`-1` to `-4`). |
| `timed_out` | `bool` | Whether the execution timeout was reached. |
| `has_launched` | `bool` | Whether the artifact reached the `Launching` checkpoint (payload execution started). Extracted from `CheckpointEvent` telemetry. |

#### Functions

| Function | Visibility | Signature | Description |
|----------|-----------|-----------|-------------|
| `classify_run()` | `pub` | `fn(exit_code: i32, timed_out: bool, events: &[TelemetryData]) -> (DetectionVerdict, Option<String>)` | Main entry. Extracts evidence from checkpoint events, runs decision tree, returns `(verdict, last_checkpoint)`. |
| `extract_evidence()` | `pub(self)` | `fn(events: &[TelemetryData]) -> ClassificationEvidence` | Scans events for `CheckpointEvent` typed events. Checks `has_launched()` helper and tracks last checkpoint name. |
| `classify_outcome()` | `pub(self)` | `fn(evidence: &ClassificationEvidence) -> DetectionVerdict` | Decision tree: exit_code → timed_out → has_launched → NTSTATUS → crash codes → carrier codes → fallback. |

**Detection verdicts** (from `automutate_common::DetectionVerdict`):

| Verdict | `is_detected()` | Exit code signals |
|---------|-----------------|-------------------|
| `Evasion` | `false` | Exit 0, or timeout while active (has_launched) |
| `Detected` | `true` | EXIT_NO_CODE (-2), or NTSTATUS 0xC0000906/07 |
| `Ambiguous` | `true` | Crashes (0xC0000005, etc.), carrier errors (30-39), other nonzero |
| `Stalled` | `false` | Timeout without has_launched |
| `InfraError` | `false` | EXIT_INFRA (-4), EXIT_WAIT (-1), guardrails (10-19) |
| `MutationFailed` | `false` | Controller-side only |
| `Anomaly` | `false` | Controller-side only |

---

### 6.3 `mod execution::guards`

#### `RedEdrGuard`

```rust
pub struct RedEdrGuard {
    collector:     RedEdrCollector,
    reset_on_drop: bool,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `collector` | `RedEdrCollector` | Owned RedEDR HTTP client. Accessible via `collector()` for API calls during the run. |
| `reset_on_drop` | `bool` | Safety flag. If `true` when `Drop` runs, spawns fire-and-forget `POST /api/trace/reset`. Disarmed by `reset_now()`. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `fn new(collector: RedEdrCollector) -> Self` | Creates guard with `reset_on_drop = true`. |
| `collector()` | `fn collector(&self) -> &RedEdrCollector` | Borrows the inner collector for Phase 2-7 operations. |
| `reset_now()` | `async fn reset_now(&self)` | Explicit reset (Phase 9). Calls `collector.reset()`, sets `reset_on_drop = false`. |

**Drop behavior:** Uses `tokio::runtime::Handle::try_current()` to get the async runtime and spawns a cleanup task. Best-effort safety net — the normal path is explicit `reset_now()`.

#### `ProcessGuard`

```rust
pub struct ProcessGuard {
    child:       Option<tokio::process::Child>,
    should_kill: bool,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `child` | `Option<Child>` | The child process. `None` after `disarm()` takes ownership. |
| `should_kill` | `bool` | If `true` when `Drop` runs, calls `child.start_kill()` (synchronous signal send). |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `fn new(child: Child) -> Self` | Creates guard with `should_kill = true`. |
| `child_mut()` | `fn child_mut(&mut self) -> &mut Child` | Borrows the child for `wait()`, `stdout.take()`, etc. |
| `disarm()` | `fn disarm(&mut self) -> Child` | Takes ownership of the child, sets `should_kill = false`. Called after normal exit in Phase 6. |

#### `MonitorGuard`

```rust
pub struct MonitorGuard {
    stop_tx:        Option<watch::Sender<bool>>,
    handle:         Option<JoinHandle<()>>,
    event_consumer: Option<JoinHandle<()>>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `stop_tx` | `Option<watch::Sender<bool>>` | Stop signal channel. Sending `true` tells the monitor to exit its poll loop. |
| `handle` | `Option<JoinHandle<()>>` | Monitor task handle. Awaited during graceful `stop()`. |
| `event_consumer` | `Option<JoinHandle<()>>` | Local event logging task. Aborted BEFORE awaiting monitor (prevents channel deadlock). |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `fn new(stop_tx, handle, event_consumer) -> Self` | Creates guard with all three handles. |
| `stop()` | `async fn stop(self)` | Graceful shutdown: send stop → abort consumer → await monitor with `CLEANUP_TIMEOUT_SECS`. |

**Drop behavior:** Sends stop signal + aborts consumer (no awaiting — `Drop` is sync).

---

### 6.4 `mod execution::monitor`

#### `MonitorConfig`

```rust
pub struct MonitorConfig {
    run_id:           String,
    job_id:           String,
    worker_id:        String,
    worker_ip:        String,
    artifact_name:    String,
    pid:              u32,
    rededr_base_url:  String,
    timeout_seconds:  i32,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `run_id` | `String` | Run identity for status reports. |
| `job_id` | `String` | Job identity for status reports. |
| `worker_id` | `String` | Worker identity included in `ExecutionStatusReport`. |
| `worker_ip` | `String` | Worker IP included in status reports. |
| `artifact_name` | `String` | Name of the artifact being monitored (e.g., `"abc123.exe"`). |
| `pid` | `u32` | Process ID to monitor via `is_process_alive()`. |
| `rededr_base_url` | `String` | RedEDR API endpoint for event count queries (e.g., `"http://localhost:8081"`). |
| `timeout_seconds` | `i32` | Execution timeout. Used to calculate `approaching_timeout` threshold. |

#### `ExecutionMonitor`

```rust
pub struct ExecutionMonitor {
    config:     MonitorConfig,
    sink:       Arc<dyn ControlPlaneSink>,
    start_time: Instant,
    client:     reqwest::Client,
    sys:        Arc<Mutex<sysinfo::System>>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `config` | `MonitorConfig` | Run parameters and identity. |
| `sink` | `Arc<dyn ControlPlaneSink>` | Transport abstraction for sending `ExecutionStatusReport` to controller. |
| `start_time` | `Instant` | Monotonic start time for elapsed/remaining calculations. |
| `client` | `reqwest::Client` | HTTP client for `GET /api/stats` RedEDR event count queries. |
| `sys` | `Arc<Mutex<sysinfo::System>>` | Per-PID sysinfo for CPU/memory metrics of the monitored process. |

| Method | Visibility | Signature | Description |
|--------|-----------|-----------|-------------|
| `new()` | `pub` | `fn new(config: MonitorConfig, sink: Arc<dyn ControlPlaneSink>) -> Self` | Creates monitor with fresh `Instant::now()` and `System::new()`. |
| `start()` | `pub` | `async fn start(self, stop_rx, event_tx)` | Main poll loop. Every 3s: check alive, get metrics, query RedEDR stats, classify event type, send to controller + local channel. Stops on `stop_rx` signal or process termination. |
| `collect_status()` | `pub(self)` | `async fn collect_status(&self, ...) -> ExecutionStatusReport` | Builds a single status report from current metrics, elapsed time, event counts, idle detection. |
| `get_process_metrics()` | `pub(self)` | `async fn get_process_metrics(&self) -> (i32, i32)` | Per-PID CPU/memory via `sysinfo`. Refreshes process-specific data only. |
| `send_status_to_controller()` | `pub(self)` | `async fn send_status_to_controller(&self, status)` | Sends via sink with 1s timeout. Logs warning on failure but does not abort the monitor. |

---

### 6.5 `mod execution::sink`

#### `trait ControlPlaneSink`

```rust
#[tonic::async_trait]
pub trait ControlPlaneSink: Send + Sync {
    async fn send_status(&self, status: ExecutionStatusReport) -> Result<()>;
    async fn send_telemetry(&self, batch: TelemetryBatch) -> Result<()>;
    async fn send_ack(&self, request_id: &str, success: bool, error: &str) -> Result<()>;
}
```

Transport abstraction. Decouples execution engine from gRPC. Two implementations:

#### `StreamSink`

```rust
pub struct StreamSink {
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `tx` | `mpsc::Sender<Result<WorkerMessage, Status>>` | Outgoing message channel of the bidirectional stream. Wraps each payload in the appropriate `WorkerMessage` envelope variant. |

Active when a bidirectional stream is established. Holds only the `tx` channel (not `Arc<StreamHandler>`) to prevent Arc cycles.

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `fn new(tx: mpsc::Sender<...>) -> Self` | Constructor. |
| `send_status()` | `async fn send_status(&self, status) -> Result<()>` | Wraps as `WorkerMessage::ExecutionStatus(status)`, sends via `tx`. |
| `send_telemetry()` | `async fn send_telemetry(&self, batch) -> Result<()>` | Wraps as `WorkerMessage::Telemetry(batch)`, sends via `tx`. |
| `send_ack()` | `async fn send_ack(&self, request_id, success, error) -> Result<()>` | Wraps as `WorkerMessage::Ack(Ack{...})`, sends via `tx`. |

#### `NullSink`

```rust
pub struct NullSink;  // no fields
```

No-op implementation. Used when no bidirectional stream exists (standalone/unary-only mode). All methods return `Ok(())` with debug logging.

#### `build_sink()`

```rust
pub fn build_sink(
    tx: Option<&mpsc::Sender<Result<WorkerMessage, Status>>>
) -> Arc<dyn ControlPlaneSink>
```

Factory. Returns `StreamSink` if `tx` is `Some`, `NullSink` if `None`.

---

### 6.6 `mod execution::state`

#### `enum ExecutionState`

```rust
pub enum ExecutionState {
    Idle,
    Running {
        job_id:   String,
        artifact: String,
        run_id:   String,
    },
}
```

State machine ensuring only one artifact executes at a time. Enum prevents inconsistent state (impossible to be `busy=true` with `job_id=None`).

| Method | Signature | Description |
|--------|-----------|-------------|
| `acquire()` | `fn acquire(&mut self, job_id, artifact, run_id) -> Result<(), ExecutionBusyError>` | Transition `Idle → Running`. Returns error if already `Running`. |
| `release()` | `fn release(&mut self) -> (String, String)` | Transition `Running → Idle`. Returns `(job_id, artifact)`. If already `Idle`, returns `("unknown", "unknown")`. |
| `is_busy()` | `fn is_busy(&self) -> bool` | `true` if `Running`. |
| `current_job_id()` | `fn current_job_id(&self) -> Option<&str>` | Returns `Some(job_id)` if `Running`, `None` if `Idle`. |
| `current_artifact()` | `fn current_artifact(&self) -> Option<&str>` | Returns `Some(artifact)` if `Running`, `None` if `Idle`. |

#### `ExecutionLockGuard`

```rust
pub struct ExecutionLockGuard {
    lock: Arc<Mutex<ExecutionState>>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `lock` | `Arc<Mutex<ExecutionState>>` | Reference to the shared execution lock. On `Drop`, spawns a tokio task to acquire + `release()`. |

RAII guard. Guarantees the lock returns to `Idle` even on panic or early return.

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `fn new(lock: Arc<Mutex<ExecutionState>>) -> Self` | Creates guard. |

**Drop:** Spawns `tokio::spawn(async { lock.lock().await.release(); })`.

#### `ExecutionBusyError`

```rust
pub struct ExecutionBusyError {
    current_job_id:   String,
    current_artifact: String,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `current_job_id` | `String` | The job that currently holds the lock. |
| `current_artifact` | `String` | The artifact being executed. |

Returned by `ExecutionState::acquire()`. Mapped to `Status::resource_exhausted` at the API layer.

---

### 6.7 `mod execution::types`

#### `RunRequest`

```rust
pub struct RunRequest {
    job_id:          String,
    artifact_id:     String,
    timeout_seconds: u32,
    run_id:          String,
}
```

| Field | Type | Source | Description |
|-------|------|--------|-------------|
| `job_id` | `String` | `SampleRequest.job_id` | Parent job identity. |
| `artifact_id` | `String` | `SampleRequest.artifact_id` | Artifact identity (maps to `{id}.exe` filename). |
| `timeout_seconds` | `u32` | `SampleRequest.timeout_seconds` | Execution timeout. 0 = default from config. |
| `run_id` | `String` | Controller-assigned or `uuid::Uuid::new_v4()` | Unique run identity for telemetry correlation. |

#### `RunContext`

```rust
pub struct RunContext {
    worker_id:     String,
    config:        WorkerConfig,
    telemetry_dir: PathBuf,
    artifact_path: PathBuf,
    artifact_name: String,
}
```

| Field | Type | Derivation | Description |
|-------|------|------------|-------------|
| `worker_id` | `String` | From service | Worker identity for status reports. |
| `config` | `WorkerConfig` | From service | Full configuration (RedEDR URL, timeouts, paths). |
| `telemetry_dir` | `PathBuf` | `{artifacts_path}/telemetry_{artifact_id}` | Directory where the artifact writes trace.log, coverage.bin, checkpoints.log. Becomes the process CWD. |
| `artifact_path` | `PathBuf` | `{artifacts_path}/{artifact_id}.exe` | Full path to the PE binary on disk. |
| `artifact_name` | `String` | `{artifact_id}.exe` | Process name for RedEDR trace targeting. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `fn new(worker_id, config, artifact_id) -> Self` | Derives all paths from `config.storage.artifacts_path` and `artifact_id`. |

#### `RunOutcome`

```rust
pub struct RunOutcome {
    exit_code:          i32,
    timed_out:          bool,
    stdout:             String,
    stderr:             String,
    telemetry_events:   Vec<TelemetryData>,
    elapsed:            Duration,
    phase_timings:      RunPhaseTimings,
    detection_verdict:  String,
    last_checkpoint:    String,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `exit_code` | `i32` | Process exit code or synthetic: `-1` (wait failed), `-2` (no code/killed), `-3` (timeout), `-4` (infra error). |
| `timed_out` | `bool` | Whether execution timeout was reached. |
| `stdout` | `String` | Captured process stdout (truncated to 400+400 chars). |
| `stderr` | `String` | Captured process stderr. |
| `telemetry_events` | `Vec<TelemetryData>` | All collected telemetry from 5 sources. |
| `elapsed` | `Duration` | Wall-clock execution time (spawn to completion/timeout). |
| `phase_timings` | `RunPhaseTimings` | Per-phase breakdown for performance analysis. |
| `detection_verdict` | `String` | Classifier result as string (e.g., `"evasion"`, `"detected"`, `"ambiguous"`). |
| `last_checkpoint` | `String` | Last checkpoint event name before exit (e.g., `"Launching"`, `"Decoding"`). |

#### `RunPhaseTimings`

```rust
pub struct RunPhaseTimings {
    rededr_setup_ms:      u64,
    process_spawn_ms:     u64,
    process_wait_ms:      u64,
    telemetry_collect_ms: u64,
    rededr_reset_ms:      u64,
}
```

| Field | Type | Phase | Description |
|-------|------|-------|-------------|
| `rededr_setup_ms` | `u64` | Phase 2 | RedEDR sanity check + start tracing. |
| `process_spawn_ms` | `u64` | Phase 4 | `spawn_artifact()` latency. |
| `process_wait_ms` | `u64` | Phase 6 | Time waiting for process exit or timeout. |
| `telemetry_collect_ms` | `u64` | Phase 7 | Collecting from all 5 telemetry sources. |
| `rededr_reset_ms` | `u64` | Phase 9 | `RedEdrGuard::reset_now()` latency. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `to_metadata()` | `fn to_metadata(&self) -> HashMap<String, String>` | Converts to string key-value pairs for inclusion as telemetry metadata. |

#### Synthetic Exit Codes

```rust
pub const EXIT_WAIT_FAILED: i32 = -1;  // OS error on child.wait()
pub const EXIT_NO_CODE: i32     = -2;  // Externally terminated (no exit code)
pub const EXIT_TIMEOUT: i32     = -3;  // Timeout expired, process killed
pub const EXIT_INFRA: i32       = -4;  // Setup failure, never executed
```

#### Free Functions

| Function | Visibility | Signature | Description |
|----------|-----------|-----------|-------------|
| `resolve_run_id()` | `pub` | `fn(requested: Option<&str>) -> String` | Returns `requested` if non-empty, else `uuid::Uuid::new_v4()`. |
| `format_output()` | `pub` | `fn(exit_code, stdout, stderr, verdict, last_checkpoint) -> String` | Human-readable output summary with exit code description. |
| `sample_response_ok()` | `pub` | `fn(outcome: &RunOutcome, output: String) -> SampleResponse` | Maps `RunOutcome` to protobuf `SampleResponse`. Uses classifier verdict for `detected` flag. |
| `sample_response_error()` | `pub` | `fn(error_message: &str) -> SampleResponse` | Builds error response: `exit_code = EXIT_INFRA`, `detection_verdict = "infra_error"`. |
| `describe_exit()` | `pub(self)` | `fn(code: i32) -> String` | Human-readable exit code description. Handles synthetic codes, guardrails (10-19), carrier errors (30-39), NTSTATUS. |
| `looks_like_ntstatus()` | `pub(self)` | `fn(code: u32) -> bool` | `code >= 0x80000000`. `#[cfg(target_os = "windows")]`. |
| `ntstatus_to_message()` | `pub(self)` | `fn(code: u32) -> Option<String>` | `RtlNtStatusToDosError` + `FormatMessageW`. `#[cfg(target_os = "windows")]`. |

---

## 7. `mod infra` — OS Boundary

### 7.1 `mod infra::process`

| Function | Signature | Platform | Description |
|----------|-----------|----------|-------------|
| `spawn_artifact()` | `fn(artifact_path: &Path, working_dir: &Path) -> io::Result<Child>` | Cross-platform | Spawns PE as child process. `stdin=null`, `stdout/stderr=piped`, `current_dir=working_dir`. |
| `kill_process_tree()` | `async fn(child: &mut Child, pid: Option<u32>)` | Windows: `taskkill /F /T` + `child.kill()`. Non-Windows: `child.kill()` only. | Two-stage termination. Kills entire process tree on Windows. 500ms sleep after kill for OS resource reclaim. |
| `is_process_alive()` | `fn(pid: u32) -> bool` | `#[cfg(windows)]`: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`. Non-Windows: `false`. | Minimum-privilege liveness check. |
| `capture_stream()` | `fn<R: AsyncRead>(stream: Option<R>) -> JoinHandle<String>` | Cross-platform | Spawns async task to read entire stream to string. Prevents pipe buffer deadlock. |

### 7.2 `mod infra::system`

| Function | Signature | Description |
|----------|-----------|-------------|
| `collect_system_metrics()` | `fn(sys: &sysinfo::System) -> (i32, i32)` | Returns `(cpu_percent, memory_percent)`. Caller must refresh `System` first. Div-by-zero guard on total_memory. |
| `prepare_telemetry_dir()` | `fn(dir: &Path) -> io::Result<()>` | Remove-then-create. Guarantees clean slate for each run. |
| `cleanup_run_artifacts()` | `fn(artifact_path: &Path, telemetry_dir: &Path)` | Deletes artifact file + telemetry directory. Non-fatal (warns on failure). |

### 7.3 `mod infra::time`

| Function | Signature | Description |
|----------|-----------|-------------|
| `now_unix_secs()` | `fn() -> i64` | `chrono::Utc::now().timestamp()`. Centralized timestamp source. |

---

## 8. `mod session` — Bidirectional Stream

### 8.1 `mod session::stream_handler`

#### `StreamHandler`

```rust
pub struct StreamHandler {
    worker_state:   Arc<RwLock<WorkerState>>,
    tx:             mpsc::Sender<Result<WorkerMessage, Status>>,
    worker_id:      String,
    config:         WorkerConfig,
    execution_lock: Arc<Mutex<ExecutionState>>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `worker_state` | `Arc<RwLock<WorkerState>>` | Shared runtime state. `pub` — read by `api/run.rs` for `current_run_id`. Written by message handlers. |
| `tx` | `mpsc::Sender<Result<WorkerMessage, Status>>` | Outgoing message channel (buffer: 100). Paired with `rx` returned by `new()`, wrapped in `ReceiverStream` for gRPC. |
| `worker_id` | `String` | Cloned from `WorkerAgentService` (not `Arc` — breaks reference cycle). |
| `config` | `WorkerConfig` | Cloned from `WorkerAgentService` (same cycle-breaking reason). |
| `execution_lock` | `Arc<Mutex<ExecutionState>>` | Same `Arc` as `WorkerAgentService.execution_lock`. Shared, not cloned. |

| Method | Visibility | Signature | Description |
|--------|-----------|-----------|-------------|
| `new()` | `pub` | `fn(worker_state, worker_id, config, execution_lock) -> (Self, Receiver<...>)` | Creates handler + channel pair. Buffer: 100 messages. |
| `sender()` | `pub` | `fn(&self) -> &mpsc::Sender<...>` | Borrows the tx channel for `build_sink()`. |
| `handle_stream()` | `pub` | `async fn(&self, stream: Streaming<ControllerMessage>) -> Result<(), Status>` | Main receive loop. Processes messages until stream closes. Errors on individual messages are logged, not fatal. |
| `process_message()` | `pub(self)` | `async fn(&self, msg: ControllerMessage) -> Result<()>` | Routes message variant to handler. |
| `handle_run_sample()` | `pub(self)` | `async fn(&self, cmd: RunSampleCommand) -> Result<()>` | Sends immediate Ack, spawns execution task (acquires lock, runs engine, sends SampleResponse). |
| `handle_health_check()` | `pub(self)` | `async fn(&self, req: HealthCheckRequest) -> Result<()>` | Calls `send_status_update("health_check")`. |
| `handle_heartbeat()` | `pub(self)` | `async fn(&self, hb: Heartbeat) -> Result<()>` | Updates `worker_state.last_controller_heartbeat`. |
| `handle_disconnect()` | `pub(self)` | `async fn(&self, notice: DisconnectNotice) -> Result<()>` | Sets `controller_disconnected`, `disconnect_reason`, `reconnect_allowed` in state. |
| `send_registration()` | `pub` | `async fn(&self) -> Result<()>` | Sends `WorkerMessage::Registration` with identity, capabilities, tools, metadata. Called once on stream establishment. |
| `send_status_update()` | `pub` | `async fn(&self, event_type: &str) -> Result<()>` | Sends `WorkerMessage::Status(StatusReport)` with current health, job ID, event type. |
| `send_telemetry()` | `pub` | `async fn(&self, batch: TelemetryBatch) -> Result<()>` | Sends `WorkerMessage::Telemetry(batch)`. |
| `send_ack()` | `pub(self)` | `async fn(&self, request_id, success, error) -> Result<()>` | Sends `WorkerMessage::Ack`. Immediate response to commands. |

#### `heartbeat_loop()`

```rust
pub async fn heartbeat_loop(handler: Arc<StreamHandler>, interval_secs: u64)
```

Free function. Background task sending `StatusReport` every `interval_secs` (default 30). Never stops — continues retrying after failures. Detects reconnection when a send succeeds after disconnect. Adaptive logging: `debug` for expected failures, `warn` for unexpected.

### 8.2 `mod session::worker_state`

#### `WorkerState`

```rust
pub struct WorkerState {
    worker_id:                 String,
    capabilities:              Vec<String>,
    metadata:                  HashMap<String, String>,
    tools:                     Option<ToolVersions>,
    health:                    HealthMetrics,
    current_job_id:            Option<String>,
    current_run_id:            Option<String>,
    last_controller_heartbeat: Option<i64>,
    controller_disconnected:   bool,
    disconnect_reason:         Option<String>,
    reconnect_allowed:         bool,
}
```

| Field | Type | Group | Updated By | Description |
|-------|------|-------|------------|-------------|
| `worker_id` | `String` | Identity | Set once | Worker identity. |
| `capabilities` | `Vec<String>` | Identity | Set once | Feature tags from `WorkerCapabilities`. |
| `metadata` | `HashMap<String, String>` | Identity | Set once | System info from `WorkerCapabilities`. |
| `tools` | `Option<ToolVersions>` | Identity | Set once | Tool versions as protobuf message. |
| `health` | `HealthMetrics` | Health | `update_health()` | Live CPU/memory/disk metrics. |
| `current_job_id` | `Option<String>` | Job tracking | `handle_run_sample()` | Active job or `None`. |
| `current_run_id` | `Option<String>` | Job tracking | `handle_run_sample()` | Active run or `None`. Read by `api/run.rs` to resolve run_id. |
| `last_controller_heartbeat` | `Option<i64>` | Connection | `handle_heartbeat()` | Unix timestamp of last controller heartbeat. |
| `controller_disconnected` | `bool` | Connection | `handle_disconnect()`, `heartbeat_loop()` | Whether controller explicitly disconnected. |
| `disconnect_reason` | `Option<String>` | Connection | `handle_disconnect()` | e.g., `"controller_shutdown"`. |
| `reconnect_allowed` | `bool` | Connection | `handle_disconnect()` | From `DisconnectNotice.reconnect_allowed`. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `fn new(worker_id: String, caps: WorkerCapabilities) -> Self` | Initializes identity from capabilities, all mutable fields to defaults. |
| `update_health()` | `fn update_health(&mut self)` | Creates fresh `sysinfo::System`, calls `collect_system_metrics()`. |

#### `HealthMetrics`

```rust
#[derive(Default)]
pub struct HealthMetrics {
    cpu_percent:    i32,
    memory_percent: i32,
    disk_percent:   i32,
    active_jobs:    i32,
    uptime_seconds: i64,
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cpu_percent` | `i32` | 0 | System-wide CPU usage percentage. |
| `memory_percent` | `i32` | 0 | System-wide memory usage percentage. |
| `disk_percent` | `i32` | 0 | Disk usage percentage (not yet populated). |
| `active_jobs` | `i32` | 0 | Number of active executions (0 or 1). |
| `uptime_seconds` | `i64` | 0 | Agent uptime (not yet populated). |

---

## 9. `mod telemetry` — Data Collection

### 9.1 `mod telemetry::collectors::rededr`

#### `RedEdrCollectorConfig`

```rust
pub struct RedEdrCollectorConfig {
    base_url:          String,
    flush_interval_ms: u64,
    job_id:            String,
    run_id:            String,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `base_url` | `String` | RedEDR HTTP API base URL (e.g., `"http://localhost:8081"`). |
| `flush_interval_ms` | `u64` | Polling interval for `start()` loop (default 1000ms). |
| `job_id` | `String` | Job identity tagged on every `TelemetryData`. |
| `run_id` | `String` | Run identity for traceability. |

#### `RedEdrCollector`

```rust
pub struct RedEdrCollector {
    config:         RedEdrCollectorConfig,
    client:         reqwest::Client,
    seen_trace_ids: HashSet<u64>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `config` | `RedEdrCollectorConfig` | Connection parameters and identity. |
| `client` | `reqwest::Client` | HTTP client with 5s timeout. |
| `seen_trace_ids` | `HashSet<u64>` | Deduplication set. Events with already-seen `trace_id` are filtered during `start()` polling. |

| Method | Visibility | Signature | Description |
|--------|-----------|-----------|-------------|
| `new()` | `pub` | `fn new(config: RedEdrCollectorConfig) -> Self` | Creates client with 5s timeout. |
| `config()` | `pub` | `fn config(&self) -> &RedEdrCollectorConfig` | Config accessor (used by `RedEdrGuard` Drop for base_url). |
| `start()` | `pub` | `async fn start(mut self, tx: Sender<TelemetryData>) -> Result<()>` | **Consumes self.** Infinite poll loop: fetch → dedup by trace_id → transform → `tx.try_send()` → sleep. |
| `fetch_events()` | `pub(self)` | `async fn fetch_events(&self) -> Result<Vec<RedEdrEvent>>` | GET `{base_url}/api/logs/rededr`. Handles empty responses and detailed JSON parse errors. |
| `start_trace()` | `pub` | `async fn start_trace(&self, targets: Vec<String>) -> Result<()>` | POST `{base_url}/api/trace/start` with `{"trace": targets}`. |
| `collect_all()` | `pub` | `async fn collect_all(&self, job_id: &str) -> Result<Vec<TelemetryData>>` | One-shot batch collection. No dedup filtering (collects everything). |
| `reset()` | `pub` | `async fn reset(&self) -> Result<()>` | POST `{base_url}/api/trace/reset` with 30s timeout. Clears RedEDR state for next run. |
| `acquire_lock()` | `pub` | `async fn acquire_lock(&self) -> Result<()>` | POST `{base_url}/api/lock/acquire`. Exclusive access for ETW tracing. |
| `release_lock()` | `pub` | `async fn release_lock(&self) -> Result<()>` | POST `{base_url}/api/lock/release`. |
| `transform_event()` | `pub(self)` | `fn transform_event(&self, event: &RedEdrEvent) -> TelemetryData` | Delegates to `transform_event_with_job` with `self.config.job_id`. |
| `transform_event_with_job()` | `pub(self)` | `fn transform_event_with_job(&self, job_id: &str, event: &RedEdrEvent) -> TelemetryData` | Full event JSON in `payload`, key fields extracted to `metadata` map. |

#### `RedEdrEvent`

```rust
#[derive(Serialize, Deserialize)]
pub struct RedEdrEvent {
    date:        Option<String>,
    r#type:      Option<String>,
    trace_id:    Option<u64>,
    target:      Option<String>,
    func:        Option<String>,
    pid:         Option<u32>,
    tid:         Option<u32>,
    provider:    Option<String>,
    event_id:    Option<u32>,
    callstack:   Option<serde_json::Value>,
    stack_trace: Option<Vec<StackTraceEntry>>,
    targets:     Option<Vec<String>>,
    #[serde(flatten)]
    extra:       serde_json::Map<String, serde_json::Value>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `date` | `Option<String>` | Event timestamp string (e.g., `"2025-11-02-15-30-00"`). |
| `type` | `Option<String>` | Event category: `"etw"`, `"dll"`, `"kernel"`, etc. |
| `trace_id` | `Option<u64>` | Unique event ID for deduplication. |
| `target` | `Option<String>` | Target process name. |
| `func` | `Option<String>` | API function name (e.g., `"NtAllocateVirtualMemory"`). |
| `pid` | `Option<u32>` | Process ID. |
| `tid` | `Option<u32>` | Thread ID. |
| `provider` | `Option<String>` | ETW provider name (e.g., `"Microsoft-Windows-Kernel-Process"`). |
| `event_id` | `Option<u32>` | ETW event ID. |
| `callstack` | `Option<Value>` | Flexible: `Vec<String>` or `Vec<Object>`. |
| `stack_trace` | `Option<Vec<StackTraceEntry>>` | Structured stack frames. |
| `targets` | `Option<Vec<String>>` | Multiple target processes. |
| `extra` | `Map<String, Value>` | `#[serde(flatten)]` catch-all for unknown fields. |

All fields `Option` because RedEDR events are heterogeneous — different event types have different fields.

#### `StackTraceEntry`

```rust
pub struct StackTraceEntry {
    addr:      Option<u64>,
    addr_info: Option<String>,
    idx:       Option<u32>,
}
```

### 9.2 `mod telemetry::collectors::trace`

#### `TraceEvent`

```rust
#[derive(Serialize, Deserialize)]
pub struct TraceEvent {
    seq:       u32,
    thread_id: u32,
    file:      String,
    line:      u32,
    func:      String,
    ts_us:     u64,
}
```

| Field | Type | Binary Protocol | Base64 Protocol | Description |
|-------|------|----------------|-----------------|-------------|
| `seq` | `u32` | From header `seq_no` | `AtomicU32` counter | Execution order sequence number. |
| `thread_id` | `u32` | From header `thread_id` | Always `0` | OS thread ID. |
| `file` | `String` | Payload `file:line:func` | Decoded `line:file:N:func` | Source file name. |
| `line` | `u32` | Payload parse | Decoded parse | Source line number. |
| `func` | `String` | Payload parse | Decoded parse (optional) | Function name. |
| `ts_us` | `u64` | From header `ts_us` | `SystemTime::now()` | Timestamp in microseconds. |

#### `InstRecordHeader` (private, Windows-only)

```rust
#[repr(C, packed)]
struct InstRecordHeader {
    magic:       u32,   // 0x49535452 ('ISTR')
    version:     u16,
    event_type:  u16,   // 1=line_trace
    thread_id:   u32,
    seq_no:      u64,
    ts_us:       u64,
    payload_len: u32,
}
```

32 bytes total. Matches C runtime `#pragma pack(1)`. All field reads use `ptr::read_unaligned`.

#### `TraceCollector`

```rust
pub struct TraceCollector {
    pipe_name:        String,
    event_tx:         mpsc::Sender<TraceEvent>,
    sequence_counter: Arc<AtomicU32>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `pipe_name` | `String` | `"\\.\pipe\rededr_trace"`. Convention shared with C instrumentation runtime. |
| `event_tx` | `mpsc::Sender<TraceEvent>` | Output channel. Events sent here are consumed by the engine's streaming writer → `trace_events.jsonl`. |
| `sequence_counter` | `Arc<AtomicU32>` | Monotonic counter for Base64 protocol (which has no seq in wire format). |

| Method | Visibility | Signature | Platform | Description |
|--------|-----------|-----------|----------|-------------|
| `new()` | `pub` | `fn new(event_tx: Sender<TraceEvent>) -> Self` | All | Creates collector with default pipe name and counter at 0. |
| `start_server()` | `pub` | `async fn start_server(&self) -> Result<()>` | `#[cfg(windows)]` | Named pipe server. 1MB buffers, retry logic (5 attempts). Auto-detects binary vs Base64 by peeking first 4 bytes. Non-Windows: returns error. |
| `read_binary_stream()` | `pub(self)` | `async fn read_binary_stream(&self, stream, first_bytes) -> Result<()>` | `#[cfg(windows)]` | Reads binary protocol records in a loop until disconnect or bad magic. |
| `read_text_stream()` | `pub(self)` | `async fn read_text_stream(&self, stream, first_bytes) -> Result<()>` | `#[cfg(windows)]` | Reads Base64 text lines until EOF. Handles `b64line:` (IR) and `YjY0` (AST) prefixes. |
| `parse_on_event_type()` | `pub(self)` | `fn parse_on_event_type(&self, hdr, event_type, payload)` | `#[cfg(windows)]` | Routes: type 1 → `handle_binary_line_trace`, type 2-4 → warn (wrong pipe), other → debug. |
| `handle_binary_line_trace()` | `pub(self)` | `fn handle_binary_line_trace(&self, hdr, payload) -> Result<()>` | `#[cfg(windows)]` | Parses `"file:line:func"` payload, combines with header fields, sends via `event_tx.try_send()`. |

### 9.3 `mod telemetry::pipeline`

| Function | Visibility | Signature | Description |
|----------|-----------|-----------|-------------|
| `deduplicate_trace_jsonl()` | `pub(self)` | `fn(raw: &str) -> (String, usize, usize)` | Collapses `(file, line, func)` duplicates, keeps highest `seq`, adds `count: N`. Returns `(deduped_jsonl, raw_count, unique_count)`. |
| `package_trace_log()` | `pub` | `fn(trace_events_file: &Path, job_id: &str, events: &mut Vec<TelemetryData>)` | Reads JSONL, deduplicates, progressive tail-truncation if > 3.5MB, creates single `trace_log` event. |
| `collect_trace_log_binary()` | `pub` | `fn(trace_log_path: &Path, job_id: &str, events: &mut Vec<TelemetryData>)` | Parses binary `trace.log` (32-byte headers + payloads). Creates one `trace_line` event per record. |
| `collect_bb_coverage()` | `pub` | `async fn(bitmap_path: &Path, metadata_path: &Path, job_id: &str) -> Result<TelemetryData>` | Reads `coverage_bbs.txt` (`BB_ID HIT_COUNT` lines). Returns typed `CoverageEvent`. |
| `collect_api_checkpoints()` | `pub` | `async fn(checkpoints_path: &Path, job_id: &str) -> Result<Vec<TelemetryData>>` | Reads `checkpoints.log` JSONL. Creates typed `CheckpointEvent` per line. Handles `success`/`failure` types. |

### 9.4 `mod telemetry::trace_compressor` (NOT INTEGRATED)

**Status:** `#[allow(dead_code)]`. Tests pass but 3 blockers prevent integration.

#### `CompressedTrace`

```rust
pub struct CompressedTrace {
    original_size:     usize,
    compressed_size:   usize,
    content:           String,
    compression_ratio: f64,
    statistics:        CompressionStatistics,
}
```

#### `CompressionStatistics`

```rust
#[derive(Default, Serialize)]
pub struct CompressionStatistics {
    original_events:           usize,
    unique_files:              usize,
    unique_functions:          usize,
    patterns_found:            usize,
    max_pattern_length:        usize,
    total_pattern_occurrences: usize,
    grammar_rules:             usize,
}
```

#### Private Types

| Type | Description |
|------|-------------|
| `TraceEvent` | Deserialized JSONL trace event (seq, thread_id, file, line, func, ts_us). |
| `ColumnarTrace` | CLP-inspired columnar decomposition: dense `Vec<u32>` line_sequence + string dictionaries. |
| `MatrixProfile` | Contains `Vec<Motif>` found by sliding-window pattern matching. |
| `Motif` | `{ start_index, length, occurrences: Vec<usize>, distance: f64 }`. |
| `Grammar` | `{ rules: Vec<GrammarRule>, start_rule: Vec<Symbol> }`. |
| `GrammarRule` | `{ id, expansion: Vec<Symbol>, usage_count }`. |
| `Symbol` | `Terminal(u32)` (line number) or `NonTerminal(usize)` (rule reference). |

#### Public Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `compress_trace_log()` | `fn(content: &str, min_loop_iterations: usize) -> CompressedTrace` | Three-stage pipeline: CLP columnar → Matrix Profile motifs → Sequitur grammar. Early exit for < 10 lines. |
| `gzip_compress()` | `fn(data: &[u8]) -> Result<Vec<u8>>` | `flate2::GzEncoder` wrapper. Not called by any current code path. |

---

## 10. Summary

| Category | Count |
|----------|-------|
| **Modules** | 16 (7 top-level + 9 nested) |
| **Public structs** | 27 |
| **Private structs** | 7 (classifier, trace_compressor internals) |
| **Enums** | 3 (`RunError`, `ExecutionState`, `Symbol`) |
| **Traits** | 1 (`ControlPlaneSink`) |
| **Public functions** | 48 |
| **Private functions** | 22 |
| **Constants** | 10 (6 tuning + 4 exit codes) |
| **`#[cfg(windows)]` items** | 12 |
| **Total source lines** | ~5,926 |
