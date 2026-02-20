# Worker Agent Module & Struct Reference

## Module Hierarchy

```
crate worker_agent
│
├── main.rs                         Entry point: config, logging, gRPC server
├── lib.rs                          WorkerAgentService struct + proto re-exports
│
├── mod automutate                  Proto-generated types (pub)
│   ├── mod common                  TelemetryData, ControllerMessage, WorkerMessage, etc.
│   ├── mod controller              Controller-side service definitions
│   └── mod worker                  Worker-side service definitions
│
├── mod api                         gRPC handler implementations (thin adapters)
│   ├── mod.rs                      WorkerAgent trait impl (dispatches to handlers)
│   ├── mod run                     run_sample() - unary RPC → engine
│   ├── mod stream                  establish_stream() - bidirectional setup
│   ├── mod artifacts               send_artifact() - chunked binary transfer
│   └── mod info                    ping, health_check, get_worker_info, get_telemetry
│
├── mod dispatch                    Execution orchestration (pub)
│   ├── mod engine                  execute_run() - 9-phase pipeline (~698 lines)
│   │   └── enum RunError           Setup error variants → tonic::Status
│   ├── mod guards                  RAII guards with Drop cleanup
│   │   ├── struct RedEdrGuard      Resets RedEDR HTTP API on drop
│   │   ├── struct ProcessGuard     Kills child process on drop
│   │   └── struct MonitorGuard     Stops monitor tasks on drop
│   ├── mod state                   Execution state + lock
│   │   ├── enum ExecutionState     Idle | Running { job_id, artifact, run_id }
│   │   ├── struct ExecutionBusyError  Error when lock is held
│   │   └── struct ExecutionLockGuard  RAII lock release on drop
│   ├── mod sink                    Transport-agnostic output
│   │   ├── trait ControlPlaneSink  send_status, send_telemetry, send_ack
│   │   ├── struct StreamSink       Delegates to mpsc::Sender (stream mode)
│   │   ├── struct NullSink         No-op (worker-only mode)
│   │   └── fn build_sink()         Factory: Option<&Sender> → Arc<dyn ControlPlaneSink>
│   ├── mod monitor                 Process monitoring during execution
│   │   └── struct ExecutionMonitor Polls PID + RedEDR every 3s
│   └── mod types                   Typed domain types
│       ├── struct RunRequest       Typed execution request
│       ├── struct RunContext       Worker-level context for a run
│       ├── struct RunOutcome       Completed run result
│       ├── struct RunPhaseTimings  Per-phase timing breakdown
│       └── fn resolve_run_id()     Optional request_id → UUID fallback
│
├── mod session                     Stream session & worker runtime (pub)
│   ├── mod stream_handler          Bidirectional gRPC stream
│   │   ├── struct StreamHandler    Message router + outbound channel
│   │   └── fn heartbeat_loop       30s periodic status sender
│   └── mod worker_state            Mutable runtime state
│       ├── struct WorkerState      Capabilities, health, job tracking
│       └── struct HealthMetrics    CPU/memory/disk metrics
│
├── mod capabilities                System detection (pub)
│   ├── struct WorkerCapabilities   Detection results snapshot
│   └── struct WindowsVersionInfo   OS build info from registry
│
├── mod infra                       OS + side effects (pluggable boundary)
│   ├── mod helpers                 BB coverage + API checkpoint parsers
│   ├── mod process                 Process lifecycle (spawn, kill, verify, capture)
│   └── mod system                  Telemetry directory management
│
└── mod telemetry                   Telemetry collection & compression (pub)
    ├── mod collectors
    │   ├── mod rededr              RedEDR HTTP API collector
    │   │   ├── struct RedEdrCollector       HTTP poller + event transformer
    │   │   ├── struct RedEdrCollectorConfig  URL, interval, job/run IDs
    │   │   ├── struct RedEdrEvent           Parsed RedEDR JSON event
    │   │   └── struct StackTraceEntry       Stack trace frame
    │   └── mod trace               Named pipe trace collector
    │       ├── struct TraceCollector        Pipe server + protocol detection
    │       ├── struct TraceEvent            Parsed line trace event
    │       └── struct InstRecordHeader      Binary protocol header (32 bytes)
    ├── mod pipeline                Trace gather → size → compress → batch
    └── mod trace_compressor        3-stage compression pipeline (experimental)
        ├── struct CompressedTrace          Output container
        ├── struct CompressionStatistics    Metrics about compression
        ├── struct ColumnarTrace            Stage 1: CLP decomposition
        ├── struct MatrixProfile            Stage 2: pattern detection
        ├── struct Motif                    Recurring pattern
        ├── struct Grammar                  Stage 3: rule-based compression
        ├── struct GrammarRule              Named pattern rule
        ├── struct TraceEvent               Internal parsed event
        └── enum Symbol                     Terminal(line) | NonTerminal(rule)
```

---

## Module Details

### `lib.rs` -- Central Service Struct

```rust
#[derive(Clone)]
struct WorkerAgentService {
    worker_id: String,
    config: WorkerConfig,                                    // TOML config (automutate_config)
    system_info: Arc<Mutex<System>>,                         // sysinfo for health
    execution_lock: Arc<Mutex<ExecutionState>>,               // ONE run at a time
    stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>,  // Lazy init on stream
}
```

| Method | Description |
|--------|-------------|
| `new(worker_id, config)` | Create service with idle execution state |
| `get_execution_state()` | Clone current ExecutionState (async, locks Mutex) |
| `truncate_middle_output(output)` | Keep first/last 400 chars, truncate middle |

---

### `capabilities` -- Detection & Runtime State

**Detection Results (one-shot)**

```rust
struct WorkerCapabilities {
    capabilities: Vec<String>,          // ["rededr", "mde", "cortex"]
    tools: HashMap<String, String>,     // {"rededr_version": "1.2.3", ...}
    metadata: HashMap<String, String>,  // {"hostname": "VM1", "os_key": "win11-build-22621"}
}
```

**OS Version Info (Windows registry)**

```rust
struct WindowsVersionInfo {
    product_name: Option<String>,    // "Windows 11 Pro"
    edition_id: Option<String>,      // "Professional"
    display_version: Option<String>, // "23H2"
    release_id: Option<String>,      // "2009"
    build: Option<u32>,              // 22621
    ubr: Option<u32>,                // Update Build Revision
    is_windows_11: Option<bool>,     // build >= 22000
}
```

**Detection Functions**

| Function | Detection Method | Returns |
|----------|-----------------|---------|
| `detect_capabilities()` | Orchestrates all checks | `WorkerCapabilities` |
| `check_rededr_available()` | HTTP GET `localhost:8081/api/stats` | `bool` |
| `get_rededr_version()` | HTTP GET `/api/logs/agent`, regex | `Option<String>` |
| `check_defender_available()` | `sc query WinDefend` -> "RUNNING" | `bool` |
| `get_defender_version()` | PowerShell `Get-MpComputerStatus` | `Option<String>` |
| `is_mde_onboarded()` | Registry `HKLM\...\OnboardedInfo` | `bool` |
| `is_cortex_xdr_present()` | Registry + filesystem check | `bool` |
| `get_windows_version_info()` | Registry `CurrentVersion` | `WindowsVersionInfo` |

---

### `session::worker_state` -- Mutable Runtime State

**Runtime State (mutable, shared via RwLock)**

```rust
#[derive(Debug, Clone)]
struct WorkerState {
    worker_id: String,
    capabilities: Vec<String>,
    metadata: HashMap<String, String>,
    tools: Option<ToolVersions>,               // Proto type

    // Health
    health: HealthMetrics,

    // Execution tracking
    current_job_id: Option<String>,
    current_run_id: Option<String>,            // Correlates telemetry batches

    // Controller connectivity
    last_controller_heartbeat: Option<i64>,
    controller_disconnected: bool,
    disconnect_reason: Option<String>,
    reconnect_allowed: bool,
}
```

| `WorkerState` Method | Description |
|----------------------|-------------|
| `new(worker_id, capabilities)` | Initialize from detection results |
| `update_health()` | Refresh CPU/mem via sysinfo |

**Health Metrics**

```rust
#[derive(Debug, Clone, Default)]
struct HealthMetrics {
    cpu_percent: i32,
    memory_percent: i32,
    disk_percent: i32,       // TODO: always 0
    active_jobs: i32,        // 0 or 1
    uptime_seconds: i64,     // TODO: always 0
}
```

---

### `session::stream_handler` -- Bidirectional gRPC Stream

```rust
struct StreamHandler {
    worker_state: Arc<RwLock<WorkerState>>,
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,  // capacity: 100

    // Individual fields (NO Arc<WorkerAgentService> — breaks cycle)
    worker_id: String,
    config: WorkerConfig,
    execution_lock: Arc<Mutex<ExecutionState>>,
}
```

| Method | Direction | Description |
|--------|-----------|-------------|
| `new(worker_state, worker_id, config, execution_lock)` | -- | Returns `(Self, mpsc::Receiver)` |
| `sender()` | -- | Reference to tx (for building ControlPlaneSink) |
| `handle_stream(stream)` | Inbound | Message loop dispatching controller messages |
| `handle_run_sample(cmd)` | Inbound | ACK + spawn async execution via engine |
| `handle_health_check(req)` | Inbound | Reply with StatusReport |
| `handle_heartbeat(hb)` | Inbound | Update last_controller_heartbeat |
| `handle_disconnect(notice)` | Inbound | Set disconnected flag |
| `send_ack(request_id, success, error)` | Outbound | Acknowledge command |
| `send_telemetry(batch)` | Outbound | Stream TelemetryBatch |
| `send_status_update(event_type)` | Outbound | StatusReport (heartbeat/health) |
| `send_execution_status(...)` | Outbound | ExecutionStatusReport (12 fields) |
| `send_registration()` | Outbound | WorkerRegistration (capabilities, metadata) |

**Free Function**

| Function | Description |
|----------|-------------|
| `heartbeat_loop(handler, 30)` | Send status every 30s, detect reconnection |

---

### `api` -- gRPC RPC Handlers

#### `api::mod.rs` -- WorkerAgent Trait Implementation

Implements `WorkerAgent` gRPC trait, delegates to handler modules:

| RPC | Handler | Direction | Description |
|-----|---------|-----------|-------------|
| `Ping` | `info::ping` | Unary | Liveness check |
| `RunSample` | `run::run_sample` | Unary | Execute artifact |
| `HealthCheck` | `info::health_check` | Unary | CPU/mem/busy status |
| `SendArtifact` | `artifacts::send_artifact` | Client streaming | Chunked transfer |
| `GetWorkerInfo` | `info::get_worker_info` | Unary | Capabilities + tools |
| `GetTelemetry` | `info::get_telemetry` | Server streaming | Pull telemetry |
| `EstablishStream` | `stream::establish_stream` | Bidirectional | Real-time stream |

---

#### `api::run` -- Unary Execution Entry Point

```
run_sample(service, Request<SampleRequest>) -> Response<SampleResponse>

Flow:
1. Resolve run_id (from worker_state.current_run_id or UUID)
2. Acquire execution lock (ExecutionState.acquire)
3. Build RunRequest + RunContext from config
4. Build ControlPlaneSink from stream handler's tx
5. Call engine::execute_run()
6. Map RunOutcome → SampleResponse
```

**Helper Functions**

| Function | Visibility | Description |
|----------|-----------|-------------|
| `run_sample(service, request)` | pub | Full unary execution lifecycle |
| `format_output(outcome, timeout)` | pub | Human-readable output string |
| `describe_exit(exit_code)` | private | Human-readable exit code description |
| `ntstatus_to_message(status)` | private | Windows NTSTATUS -> message string |
| `looks_like_ntstatus(code)` | private | Check if code has 0x8000_0000 bit set |

**Exit Code Semantics**

| Code | Meaning |
|------|---------|
| `0` | Success |
| `-1` | Timeout or `wait()` failed |
| `-2` | Externally terminated (no exit code, likely AV/EDR) |
| `0xC0000005` | ACCESS_VIOLATION (NTSTATUS) |
| `0xC0000409` | STACK_BUFFER_OVERRUN (NTSTATUS) |
| other | Interpreted via `RtlNtStatusToDosError` + `FormatMessageW` |

---

#### `api::artifacts` -- Binary Transfer

```
send_artifact(service, request: Streaming<ArtifactChunk>) -> TransferAck

Flow:
1. Receive all chunks from stream
2. Sort by chunk_index
3. Reassemble flat byte array
4. SHA256 verify against expected hash
5. Write to {config.storage.artifacts_path}\{artifact_id}.exe
6. Return TransferAck { received, chunks_received, storage_path }
```

---

#### `api::info` -- Information RPCs

| Function | Returns | Description |
|----------|---------|-------------|
| `ping(service, req)` | `PingResponse` | Echo with timestamp |
| `health_check(service, req)` | `HealthResponse` | CPU%, mem%, active_jobs, healthy flag |
| `get_worker_info(service, req)` | `WorkerInfoResponse` | Capabilities, tools, metadata, health |
| `get_telemetry(service, req)` | `Stream<TelemetryData>` | Pull RedEDR events on demand |

---

#### `api::stream` -- Stream Establishment

```
establish_stream(service, request: Streaming<ControllerMessage>)
    -> ReceiverStream<WorkerMessage>

Flow:
1. detect_capabilities()          -> WorkerCapabilities
2. WorkerState::new()             -> Arc<RwLock<WorkerState>>
3. StreamHandler::new(            -> (handler, rx)
       worker_state, worker_id,
       config, execution_lock)
4. Store handler in service.stream_handler
5. send_registration()            -> Controller
6. Spawn: handle_stream(stream)   -> message loop
7. Spawn: heartbeat_loop(30s)     -> periodic status
8. Return: ReceiverStream(rx)     -> outbound messages
```

---

### `dispatch::state` -- Execution State & Lock

**ExecutionState (enum, not struct)**

```rust
#[derive(Debug, Clone)]
enum ExecutionState {
    Idle,
    Running {
        job_id: String,
        artifact: String,
        run_id: String,
    },
}
```

| Method | Description |
|--------|-------------|
| `is_busy()` | Returns `true` if `Running` variant |
| `current_job_id()` | `Option<&str>` — `Some` if Running |
| `current_artifact()` | `Option<&str>` — `Some` if Running |
| `acquire(job_id, artifact, run_id)` | `Idle → Running` or `Err(ExecutionBusyError)` |
| `release()` | `Running → Idle`, returns `(job_id, artifact)` |

**ExecutionBusyError**

```rust
struct ExecutionBusyError {
    current_job_id: String,
    current_artifact: String,
}
// impl Display + Error
```

**ExecutionLockGuard**

```rust
struct ExecutionLockGuard {
    lock: Arc<Mutex<ExecutionState>>,
}
// Drop: tokio::spawn -> state.release()
```

---

### `dispatch::types` -- Typed Domain Types

```rust
struct RunRequest {
    job_id: String,
    artifact_id: String,
    timeout_seconds: u32,
    run_id: String,           // Resolved from controller request_id or UUID
}

struct RunContext {
    worker_id: String,
    config: WorkerConfig,
    telemetry_dir: PathBuf,
    artifact_path: PathBuf,
    artifact_name: String,    // "{artifact_id}.exe"
}

struct RunOutcome {
    exit_code: i32,
    timed_out: bool,
    stdout: String,
    stderr: String,
    telemetry_events: Vec<TelemetryData>,
    elapsed: Duration,
    phase_timings: RunPhaseTimings,
}

#[derive(Debug, Default)]
struct RunPhaseTimings {
    rededr_setup_ms: u64,
    process_spawn_ms: u64,
    process_wait_ms: u64,
    telemetry_collect_ms: u64,
    rededr_reset_ms: u64,
}
```

| Function | Description |
|----------|-------------|
| `resolve_run_id(requested: Option<&str>)` | Non-empty string → use it; else UUID v4 |

---

### `dispatch::sink` -- Control Plane Sink

```rust
#[tonic::async_trait]
trait ControlPlaneSink: Send + Sync {
    async fn send_status(&self, status: ExecutionStatusReport) -> Result<()>;
    async fn send_telemetry(&self, batch: TelemetryBatch) -> Result<()>;
    async fn send_ack(&self, request_id: &str, success: bool, error: &str) -> Result<()>;
}
```

**Implementations**

```rust
struct StreamSink {
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,
}
// Wraps the stream channel — used when bidirectional stream is active

struct NullSink;
// No-op — used when no bidirectional stream is available (worker-only mode)
```

| Function | Description |
|----------|-------------|
| `build_sink(tx: Option<&Sender>)` | `Some` → `StreamSink`, `None` → `NullSink` |

---

### `dispatch::guards` -- RAII Guards

```rust
struct RedEdrGuard {
    collector: RedEdrCollector,
    reset_on_drop: bool,
}
// Drop: Handle::try_current() → spawn POST /api/trace/reset (fire-and-forget)
// Normal: reset_now() → explicit reset, set reset_on_drop=false

struct MonitorGuard {
    stop_tx: Option<watch::Sender<bool>>,
    handle: Option<JoinHandle<()>>,
    event_consumer: Option<JoinHandle<()>>,
}
// Drop: send stop signal + abort consumer (synchronous)
// Normal: stop() → graceful shutdown with 10s timeout

struct ProcessGuard {
    child: Option<tokio::process::Child>,
    should_kill: bool,
}
// Drop: child.start_kill() (synchronous — no runtime needed)
// Normal: disarm() → take child, prevent kill
```

| Guard | Normal Exit | Error/Panic Exit (Drop) |
|-------|-------------|-------------------------|
| `ExecutionLockGuard` | (dropped) → tokio::spawn release | (dropped) → tokio::spawn release |
| `RedEdrGuard` | `reset_now()` → POST reset | `Handle::try_current()` → spawn POST reset |
| `ProcessGuard` | `disarm()` → take child | `start_kill()` (synchronous) |
| `MonitorGuard` | `stop()` → signal + wait 10s | send stop signal + abort consumer |

---

### `dispatch::engine` -- Execution Engine

**RunError**

```rust
enum RunError {
    ArtifactNotFound(String),
    RedEdrSetupFailed(String),
    EnvironmentSetupFailed(String),
    ProcessSpawnFailed(String),
    FailedPrecondition(String),    // RedEDR contamination (strict mode)
}
```

| Method | Description |
|--------|-------------|
| `into_status()` | Convert to `tonic::Status` with appropriate code |

**Main Function**

```
execute_run(request: &RunRequest, context: &RunContext, sink: Arc<dyn ControlPlaneSink>)
    -> Result<RunOutcome, RunError>

9-Phase Pipeline:
1. Validate artifact exists
2. Setup RedEDR (sanity check, start tracing)
3. Prepare environment (telemetry dir, trace collectors, streaming writer)
4. Spawn artifact process
5. Start monitoring (ExecutionMonitor + event consumer)
6. Wait for process completion or timeout
7. Collect telemetry (RedEDR, trace, coverage, checkpoints)
8. Stream telemetry to controller via sink
9. Reset RedEDR
```

---

### `dispatch::monitor` -- Process Monitoring

```rust
struct ExecutionMonitor {
    run_id: String,
    job_id: String,
    worker_id: String,
    worker_ip: String,
    artifact_name: String,
    pid: u32,
    rededr_base_url: String,
    sink: Arc<dyn ControlPlaneSink>,    // NOT Arc<StreamHandler>
    start_time: Instant,
    timeout_seconds: i32,
    client: reqwest::Client,            // 3s timeout
    sys: Arc<Mutex<sysinfo::System>>,   // per-PID refresh
}
```

| Method | Description |
|--------|-------------|
| `new(...)` | Create with all execution context (9 params) |
| `start(stop_rx, event_tx)` | Main poll loop: 3s interval, select! with stop |
| `collect_status()` | PID alive? + CPU/mem + RedEDR event count |
| `send_status_to_controller(...)` | Via `sink.send_status()` with 1s timeout |
| `is_process_alive(pid)` | Windows `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` |
| `get_process_metrics(pid)` | sysinfo per-PID refresh -> (cpu%, mem_mb) |

**Event Types Emitted**

| Event | Trigger | Threshold |
|-------|---------|-----------|
| `started` | Initial | Once on start |
| `heartbeat` | Process alive, events growing | Every 3s |
| `telemetry_idle` | Process alive, no new events AND CPU < 5% | 3+ idle cycles (9s) |
| `approaching_timeout` | Process alive, near timeout | elapsed >= timeout - 5s |
| `terminated` | PID no longer exists | Process check fails |

---

### `infra::process` -- Process Lifecycle

| Function | Description |
|----------|-------------|
| `spawn_artifact(artifact_path, working_dir)` | `tokio::process::Command` with piped stdout/stderr |
| `kill_process_tree(child, pid)` | Windows: `taskkill /F /T /PID` + `child.kill()` |
| `is_process_alive(pid)` | Windows: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` |
| `capture_stream(stream)` | Spawn task to read async stream into `String` |

---

### `infra::system` -- System Operations

| Function | Description |
|----------|-------------|
| `prepare_telemetry_dir(dir)` | Remove stale files if exists, create fresh directory |

---

### `infra::helpers` -- Telemetry Parsers

| Function | Input | Output |
|----------|-------|--------|
| `extract_filename(path)` | `/path/to/file.exe` | `file.exe` |
| `collect_bb_coverage(bitmap, metadata, job_id)` | `coverage_bbs.txt` | `TelemetryData` with `CoverageEvent` |
| `collect_api_checkpoints(path, job_id)` | `checkpoints.log` (JSONL) | `Vec<TelemetryData>` with `CheckpointEvent` |

**Coverage File Format** (`coverage_bbs.txt`):
```
# BB_ID HIT_COUNT
42 15
43 1
100 3
```

**Checkpoint File Format** (`checkpoints.log`):
```json
{"ts_us":1234567,"checkpoint":"api:VirtualAlloc"}
{"ts_us":1234600,"checkpoint":"api:VirtualProtect"}
{"ts_us":1234700,"checkpoint":"setup_done","type":"artifact_checkpoint"}
{"ts_us":1234800,"checkpoint":"All stages completed","type":"success"}
{"ts_us":1234900,"checkpoint":"alloc failed","type":"failure","error_code":42}
```

---

### `telemetry::collectors::rededr` -- RedEDR HTTP Collector

**Config**

```rust
struct RedEdrCollectorConfig {
    base_url: String,          // "http://localhost:8081"
    flush_interval_ms: u64,    // 1000
    job_id: String,
    run_id: String,
}
```

**Collector**

```rust
struct RedEdrCollector {
    config: RedEdrCollectorConfig,
    client: reqwest::Client,           // 5s timeout
    seen_trace_ids: HashSet<u64>,      // dedup by trace_id
}
```

| Method | Description |
|--------|-------------|
| `new(config)` | Create HTTP client with 5s timeout |
| `config()` | Reference to config (used by guard Drop) |
| `start(tx)` | Poll loop: fetch_events -> filter -> transform -> send |
| `fetch_events()` | GET `/api/logs/rededr` -> `Vec<RedEdrEvent>` |
| `start_trace(targets)` | POST `/api/trace/start` with target list |
| `collect_all(job_id)` | One-shot fetch + transform (post-execution) |
| `reset()` | POST `/api/trace/reset` (30s timeout) |
| `acquire_lock()` | POST `/api/lock/acquire` (TODO: not yet used) |
| `release_lock()` | POST `/api/lock/release` (TODO: not yet used) |
| `transform_event(event)` | RedEdrEvent -> TelemetryData |
| `transform_event_with_job(job_id, event)` | Same with custom job_id |

**RedEDR Event Structure**

```rust
struct RedEdrEvent {
    date: Option<String>,
    r#type: Option<String>,               // "etw", "ntdll", etc.
    trace_id: Option<u64>,                 // Unique per event (dedup key)
    target: Option<String>,                // "artifact.exe"
    func: Option<String>,                  // "NtAllocateVirtualMemory"
    pid: Option<u32>,
    tid: Option<u32>,
    provider: Option<String>,              // "Microsoft-Windows-Kernel-Process"
    event_id: Option<u32>,
    callstack: Option<serde_json::Value>,  // Flexible: Vec<String> or Vec<Object>
    stack_trace: Option<Vec<StackTraceEntry>>,
    targets: Option<Vec<String>>,
    extra: serde_json::Map<String, serde_json::Value>,  // #[serde(flatten)]
}

struct StackTraceEntry {
    addr: Option<u64>,
    addr_info: Option<String>,
    idx: Option<u32>,
}
```

**Transform Output** (TelemetryData metadata):

| Key | Value |
|-----|-------|
| `source` | `"rededr"` |
| `event_type` | From `RedEdrEvent.type` |
| `pid` | From `RedEdrEvent.pid` |
| `tid` | From `RedEdrEvent.tid` |
| `provider` | From `RedEdrEvent.provider` |
| `trace_id` | From `RedEdrEvent.trace_id` |

---

### `telemetry::collectors::trace` -- Named Pipe Trace Collector

**Collector**

```rust
struct TraceCollector {
    pipe_name: String,                              // "\\.\pipe\rededr_trace"
    event_tx: mpsc::Sender<TraceEvent>,             // capacity: 100,000
    sequence_counter: Arc<AtomicU32>,               // For text protocol
}
```

| Method | Description |
|--------|-------------|
| `new(event_tx)` | Create with default pipe name |
| `start_server()` | Windows-only: create pipe, accept connections, auto-detect protocol |
| `read_binary_stream(stream, first_bytes)` | Parse ISTR binary records |
| `read_text_stream(stream, first_bytes)` | Parse Base64 lines |
| `parse_on_event_type(hdr, type, payload)` | Dispatch binary event by type (warns on 2-4) |
| `handle_binary_line_trace(hdr, payload)` | Type 1: parse "file:line:func" |
| `handle_trace_line(line)` | Text protocol: decode Base64 -> parse |

**Trace Event**

```rust
struct TraceEvent {
    seq: u32,          // Sequence number
    thread_id: u32,    // OS thread ID (0 for text protocol)
    file: String,      // Source file ("loader.c")
    line: u32,         // Line number
    func: String,      // Function name ("main")
    ts_us: u64,        // Timestamp in microseconds
}
```

**Binary Protocol Header (packed, 32 bytes)**

```rust
#[repr(C, packed)]
struct InstRecordHeader {
    magic: u32,        // 0x49535452 ('ISTR')
    version: u16,
    event_type: u16,   // 1=line_trace (status events 2-4 now use checkpoint pipe)
    thread_id: u32,
    seq_no: u64,
    ts_us: u64,
    payload_len: u32,
}
```

| event_type | Name | Payload Format |
|------------|------|---------------|
| 1 | LINE_TRACE | `"file:line:func"` (UTF-8) |
| 2-4 | (deprecated on trace pipe) | Artifact status events now use checkpoint pipe. Warned and ignored if received here. |

**Protocol Auto-Detection**

```
First 4 bytes == 0x49535452 ('ISTR') → Binary protocol
Otherwise → Base64 text protocol
  ├── "b64line:<base64>"  → Old IR format
  └── "YjY0<base64>"     → New AST format (YjY0 = Base64("b64"))
Decoded payload: "line:file.c:42:main"
```

---

### `telemetry::pipeline` -- Trace Packaging

**Constants**

| Name | Value | Description |
|------|-------|-------------|
| `MAX_IMMEDIATE_SIZE` | 2MB | Threshold for small vs large trace |
| `MAX_PAYLOAD_SIZE` | 4MB | gRPC message limit |

**Public Functions**

| Function | Description |
|----------|-------------|
| `package_trace_log(file, job_id, events)` | Two-phase: small → inline, large → tail + async compress |
| `collect_trace_log_binary(file, job_id, events)` | Parse binary trace.log, extract telemetry events |

**Large Trace Strategy**

1. Send last 2MB of JSONL immediately (complete lines)
2. Spawn async task: CLP+MatrixProfile+Sequitur → gzip → base64

---

### `telemetry::trace_compressor` -- 3-Stage Compression

**Output**

```rust
struct CompressedTrace {
    original_size: usize,
    compressed_size: usize,
    content: String,
    compression_ratio: f64,
    statistics: CompressionStatistics,
}

struct CompressionStatistics {
    original_events: usize,
    unique_files: usize,
    unique_functions: usize,
    patterns_found: usize,
    max_pattern_length: usize,
    total_pattern_occurrences: usize,
    grammar_rules: usize,
}
```

**Stage 1: CLP Columnar Decomposition**

```rust
struct ColumnarTrace {
    line_sequence: Vec<u32>,    // Dense line numbers
    file_dict: Vec<String>,     // Unique files
    func_dict: Vec<String>,     // Unique functions
    file_indices: Vec<usize>,   // Event -> file dict index
    func_indices: Vec<usize>,   // Event -> func dict index
    thread_ids: Vec<u32>,
    timestamps: Vec<u64>,
}
// from_jsonl(content) -> parses JSONL trace events
```

**Stage 2: Matrix Profile Pattern Detection**

```rust
struct MatrixProfile {
    motifs: Vec<Motif>,
}

struct Motif {
    start_index: usize,
    length: usize,             // Window size
    occurrences: Vec<usize>,   // All start positions
    distance: f64,             // Avg distance between occurrences
}
// compute(sequence, min_window=2, max_window=50, min_occurrences=3)
// Sorted by compression benefit: occurrences * length
```

**Stage 3: Sequitur Grammar Induction**

```rust
struct Grammar {
    rules: Vec<GrammarRule>,
    start_rule: Vec<Symbol>,   // Compressed sequence
}

struct GrammarRule {
    id: usize,
    expansion: Vec<Symbol>,    // What this rule expands to
    usage_count: usize,        // How many times referenced
}

enum Symbol {
    Terminal(u32),              // Line number
    NonTerminal(usize),        // Rule reference
}
// from_sequence_and_motifs() -> greedy non-overlapping rule extraction
// to_compressed_string() -> "RULE_0 (used 15 times): L10 L11 L12\n@RULE_0 L50"
```

**Public Functions**

| Function | Description |
|----------|-------------|
| `compress_trace_log(content, min_iterations)` | Full 3-stage pipeline |
| `gzip_compress(data)` | flate2 gzip compression |

---

## Struct Relationships

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              OWNERSHIP GRAPH                                    │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  main.rs creates:                                                               │
│    └── WorkerAgentService ─────────────────────────────────────────────────────┐│
│         ├── config: WorkerConfig (owned)                                       ││
│         ├── system_info: Arc<Mutex<System>> ──── health_check, get_worker_info ││
│         ├── execution_lock: Arc<Mutex<ExecutionState>> ──── run_sample, stream ││
│         └── stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>            ││
│                                                    │                           ││
│                                                    │ set on EstablishStream    ││
│                                                    ▼                           ││
│  ┌───────────────────────────────────────────────────────────────────────────┐ ││
│  │                   StreamHandler (Arc-shared)                              │ ││
│  │  ├── worker_state: Arc<RwLock<WorkerState>> ──── mutable runtime state   │ ││
│  │  │   ├── capabilities, metadata, tools                                   │ ││
│  │  │   ├── health: HealthMetrics                                           │ ││
│  │  │   ├── current_job_id, current_run_id ──── telemetry correlation       │ ││
│  │  │   └── controller_disconnected, reconnect_allowed                      │ ││
│  │  ├── tx: mpsc::Sender ──── 100-capacity outbound channel ──→ Controller  │ ││
│  │  ├── worker_id: String (cloned from service)                             │ ││
│  │  ├── config: WorkerConfig (cloned from service)                          │ ││
│  │  └── execution_lock: Arc<Mutex<ExecutionState>> ──── shared with service │ ││
│  │       (NO Arc<WorkerAgentService> back-reference — breaks cycle)         │ ││
│  └───────────────────────────────────────────────────────────────────────────┘ ││
│                                                                                ││
│  Per-execution (created in engine::execute_run, scoped to function):           ││
│  ┌───────────────────────────────────────────────────────────────────────────┐ ││
│  │  ExecutionLockGuard ──── guards Arc<Mutex<ExecutionState>>               │ ││
│  │  RedEdrGuard ──── owns RedEdrCollector                                   │ ││
│  │  │                ├── config: RedEdrCollectorConfig                       │ ││
│  │  │                ├── client: reqwest::Client                             │ ││
│  │  │                └── seen_trace_ids: HashSet<u64>                        │ ││
│  │  ProcessGuard ──── owns tokio::process::Child                            │ ││
│  │  MonitorGuard ──── owns stop_tx, monitor handle, event consumer          │ ││
│  │                                                                           │ ││
│  │  Injected via trait:                                                       │ ││
│  │  sink: Arc<dyn ControlPlaneSink> ──── StreamSink(tx) or NullSink         │ ││
│  │    │                                                                       │ ││
│  │    └── StreamSink.tx: mpsc::Sender ──── cloned from StreamHandler.tx     │ ││
│  │        (NO Arc<StreamHandler> held — breaks second cycle)                 │ ││
│  │                                                                           │ ││
│  │  Spawned tasks:                                                           │ ││
│  │  ├── ExecutionMonitor.start() ──── polls PID + /api/stats                │ ││
│  │  │   ├── sink: Arc<dyn ControlPlaneSink> ◄───── (cloned from engine)     │ ││
│  │  │   └── sys: Arc<Mutex<System>> ──── per-PID metrics                    │ ││
│  │  ├── TraceCollector.start_server() ──── named pipe reader                │ ││
│  │  │   └── event_tx: mpsc::Sender<TraceEvent> ──→ streaming writer         │ ││
│  │  ├── streaming_handle ──── BufWriter 256KB -> trace_events.jsonl         │ ││
│  │  ├── stdout_handle, stderr_handle ──── infra::process::capture_stream    │ ││
│  │  └── event_consumer ──── log monitor events                              │ ││
│  └───────────────────────────────────────────────────────────────────────────┘ ││
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Data Flow Types

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           TYPE FLOW THROUGH SYSTEM                              │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  Controller sends ControllerMessage                                             │
│       │                                                                         │
│       ▼                                                                         │
│  StreamHandler.handle_stream() ──── dispatch by payload type                    │
│       │                                                                         │
│       ├── RunSampleCommand                                                      │
│       │       │                                                                 │
│       │       ▼                                                                 │
│       │   WorkerState.current_run_id = request_id  (correlation)                │
│       │   WorkerState.current_job_id = job_id                                   │
│       │       │                                                                 │
│       │       │ tokio::spawn                                                    │
│       │       ▼                                                                 │
│       │   ExecutionState.acquire(job_id, artifact, run_id)                      │
│       │       │                                                                 │
│       │       ├── RunRequest + RunContext built from config                      │
│       │       ├── sink = build_sink(Some(&tx))                                  │
│       │       │                                                                 │
│       │       ▼                                                                 │
│       │   engine::execute_run(request, context, sink)                           │
│       │       │                                                                 │
│       │       ├── RedEdrCollector.start_trace([artifact.exe])                   │
│       │       ├── infra::process::spawn_artifact(path, dir)                     │
│       │       ├── TraceCollector.start_server() ──→ TraceEvent                  │
│       │       │                                        │                        │
│       │       │                                        ▼ mpsc (100K)            │
│       │       │                                   trace_events.jsonl            │
│       │       │                                                                 │
│       │       ├── ExecutionMonitor.start(stop_rx, event_tx)                     │
│       │       │   sink.send_status() ──→ ExecutionStatusReport ──→ Controller   │
│       │       │                                                                 │
│       │       ├── (process completes or timeout)                                │
│       │       │   ├── infra::process::kill_process_tree() (on timeout)          │
│       │       │   └── infra::process::is_process_alive() (verification)         │
│       │       │                                                                 │
│       │       ├── RedEdrCollector.collect_all() ──→ Vec<TelemetryData>          │
│       │       ├── pipeline::package_trace_log() ──→ TelemetryData (trace_log)   │
│       │       ├── pipeline::collect_trace_log_binary() ──→ TelemetryData        │
│       │       ├── helpers::collect_bb_coverage() ──→ TelemetryData (coverage)   │
│       │       ├── helpers::collect_api_checkpoints() ──→ Vec<TelemetryData>     │
│       │       │                                                                 │
│       │       ▼                                                                 │
│       │   sink.send_telemetry(TelemetryBatch { events, is_final: true })        │
│       │       │                                                                 │
│       │       ▼                                                                 │
│       │   WorkerMessage::Telemetry ──────────────────────────→ Controller        │
│       │                                                                         │
│       │   RunOutcome → SampleResponse (via format_output)                       │
│       │       │                                                                 │
│       │       │ via tx channel                                                  │
│       │       ▼                                                                 │
│       │   WorkerMessage::SampleResponse ──────────────────────→ Controller       │
│       │                                                                         │
│       ├── HealthCheckRequest ──→ StatusReport ──→ WorkerMessage::Status          │
│       ├── Heartbeat ──→ update last_controller_heartbeat                        │
│       └── DisconnectNotice ──→ set controller_disconnected=true                 │
│                                                                                 │
│  Background:                                                                    │
│  heartbeat_loop(30s) ──→ StatusReport ──→ WorkerMessage::Status ──→ Controller  │
│                                                                                 │
│  Unary RPC path (api::run::run_sample):                                         │
│  SampleRequest ──→ engine::execute_run(sink from handler.sender())              │
│  ──→ SampleResponse (via tonic::Response)                                       │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Channel Type Summary

| Channel | Message Type | Capacity | Sender | Receiver |
|---------|-------------|----------|--------|----------|
| gRPC outbound `tx` | `Result<WorkerMessage, Status>` | 100 | StreamHandler methods / StreamSink | Controller (via ReceiverStream) |
| Trace events | `TraceEvent` | 100,000 | TraceCollector (pipe) | Streaming JSONL writer |
| Monitor events | `MonitorEvent` | 100 | ExecutionMonitor | Event consumer (logger) |
| Monitor stop | `bool` (watch) | 1 | engine (or MonitorGuard Drop) | ExecutionMonitor |

---

## Concurrency Primitives

| Primitive | Location | Purpose |
|-----------|----------|---------|
| `Arc<Mutex<ExecutionState>>` | WorkerAgentService, StreamHandler | One-at-a-time execution lock |
| `Arc<Mutex<System>>` | WorkerAgentService | Shared sysinfo for health checks |
| `Arc<RwLock<Option<Arc<StreamHandler>>>>` | WorkerAgentService | Lazy stream handler initialization |
| `Arc<RwLock<WorkerState>>` | StreamHandler | Mutable runtime state (capabilities, health, job tracking) |
| `Arc<Mutex<System>>` | ExecutionMonitor | Per-PID process metric refresh |
| `Arc<AtomicU32>` | TraceCollector | Lock-free sequence counter (text protocol) |
| `Arc<dyn ControlPlaneSink>` | engine, ExecutionMonitor | Transport-agnostic status/telemetry delivery |
| `mpsc::Sender` (tokio) | StreamHandler.tx | Outbound gRPC messages (bounded: 100) |
| `mpsc::Sender` (tokio) | TraceCollector.event_tx | Trace events (bounded: 100,000) |
| `mpsc::Sender` (tokio) | ExecutionMonitor event_tx | Monitor events (bounded: 100) |
| `watch::Sender<bool>` | MonitorGuard.stop_tx | Stop signal for ExecutionMonitor |
