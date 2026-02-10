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
├── mod capabilities                System detection & runtime state (pub)
│   ├── struct WorkerCapabilities   Detection results snapshot
│   ├── struct WorkerState          Mutable runtime state (shared via RwLock)
│   ├── struct HealthMetrics        CPU/memory/disk metrics
│   └── struct WindowsVersionInfo   OS build info from registry
│
├── mod stream_handler              Bidirectional gRPC stream (pub)
│   ├── struct StreamHandler        Message router + outbound channel
│   └── fn heartbeat_loop           30s periodic status sender
│
├── mod service                     gRPC RPC handler implementations (pub)
│   ├── mod sample_handlers         run_sample() - core execution (~1370 lines)
│   ├── mod stream_handlers         establish_stream() - bidirectional setup
│   ├── mod artifact_handlers       send_artifact() - chunked binary transfer
│   ├── mod info_handlers           ping, health_check, get_worker_info, get_telemetry
│   └── mod helpers                 BB coverage + API checkpoint parsers
│
├── mod execution                   Execution management (pub)
│   ├── mod guards                  RAII guards with Drop cleanup
│   │   ├── struct RedEdrGuard      Resets RedEDR HTTP API on drop
│   │   ├── struct ProcessGuard     Kills child process on drop
│   │   ├── struct MonitorGuard     Stops monitor tasks on drop
│   │   ├── struct ExecutionLockGuard  Releases execution lock on drop
│   │   └── struct ExecutionState   Busy flag + current job/artifact
│   └── mod monitor                 Process monitoring during execution
│       └── struct ExecutionMonitor Polls PID + RedEDR every 3s
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
    └── mod trace_compressor        3-stage compression pipeline
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
struct WorkerAgentService {
    worker_id: String,
    config: WorkerConfig,                                    // TOML config
    system_info: Arc<Mutex<System>>,                         // sysinfo for health
    execution_lock: Arc<Mutex<ExecutionState>>,               // ONE run at a time
    stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>,  // Lazy init on stream
}
```

| Method | Description |
|--------|-------------|
| `new(worker_id, config)` | Create service with idle execution state |
| `get_execution_state()` | Clone current ExecutionState (for health check) |
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

**Runtime State (mutable, shared via RwLock)**

```rust
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
struct HealthMetrics {
    cpu_percent: i32,
    memory_percent: i32,
    disk_percent: i32,       // TODO: always 0
    active_jobs: i32,        // 0 or 1
    uptime_seconds: i64,     // TODO: always 0
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

### `stream_handler` -- Bidirectional gRPC Stream

```rust
struct StreamHandler {
    worker_state: Arc<RwLock<WorkerState>>,
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,  // capacity: 100
    service: Arc<WorkerAgentService>,                  // back-reference
}
```

| Method | Direction | Description |
|--------|-----------|-------------|
| `new(worker_state, service)` | -- | Returns `(Self, mpsc::Receiver)` |
| `handle_stream(stream)` | Inbound | Message loop dispatching controller messages |
| `handle_run_sample(cmd)` | Inbound | ACK + spawn async execution |
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

### `service` -- gRPC RPC Handlers

#### `service::mod.rs` -- WorkerAgent Trait Implementation

Implements `WorkerAgent` gRPC trait, delegates to handler modules:

| RPC | Handler | Direction | Description |
|-----|---------|-----------|-------------|
| `Ping` | `info_handlers::ping` | Unary | Liveness check |
| `RunSample` | `sample_handlers::run_sample` | Unary | Execute artifact |
| `HealthCheck` | `info_handlers::health_check` | Unary | CPU/mem/busy status |
| `SendArtifact` | `artifact_handlers::send_artifact` | Client streaming | Chunked transfer |
| `GetWorkerInfo` | `info_handlers::get_worker_info` | Unary | Capabilities + tools |
| `GetTelemetry` | `info_handlers::get_telemetry` | Server streaming | Pull telemetry |
| `EstablishStream` | `stream_handlers::establish_stream` | Bidirectional | Real-time stream |

---

#### `service::sample_handlers` -- Core Execution

Single function `run_sample()` (~1370 lines) orchestrating all execution phases.

**Key Helper Functions**

| Function | Visibility | Description |
|----------|-----------|-------------|
| `run_sample(service, request)` | pub | Full execution lifecycle |
| `describe_exit(exit_code)` | private | Human-readable exit code description |
| `ntstatus_to_message(status)` | private | Windows NTSTATUS -> message string |
| `looks_like_ntstatus(code)` | private | Check if code has 0x8000_0000 bit set |
| `get_handle(stream)` | private | Spawn async stdout/stderr capture task |

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

#### `service::artifact_handlers` -- Binary Transfer

```
send_artifact(service, request: Streaming<ArtifactChunk>) -> TransferAck

Flow:
1. Receive all chunks from stream
2. Sort by chunk_index
3. Reassemble flat byte array
4. SHA256 verify against expected hash
5. Write to C:\temp\artifacts\{artifact_id}.exe
6. Return TransferAck { received, chunks_received, storage_path }
```

---

#### `service::info_handlers` -- Information RPCs

| Function | Returns | Description |
|----------|---------|-------------|
| `ping(service, req)` | `PingResponse` | Echo with timestamp |
| `health_check(service, req)` | `HealthResponse` | CPU%, mem%, active_jobs, healthy flag |
| `get_worker_info(service, req)` | `WorkerInfoResponse` | Capabilities, tools, metadata, health |
| `get_telemetry(service, req)` | `Stream<TelemetryData>` | Pull RedEDR events on demand |

---

#### `service::helpers` -- Telemetry Parsers

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
```

---

#### `service::stream_handlers` -- Stream Establishment

```
establish_stream(service, request: Streaming<ControllerMessage>)
    -> ReceiverStream<WorkerMessage>

Flow:
1. detect_capabilities()          -> WorkerCapabilities
2. WorkerState::new()             -> Arc<RwLock<WorkerState>>
3. StreamHandler::new()           -> (handler, rx)
4. Store handler in service.stream_handler
5. send_registration()            -> Controller
6. Spawn: handle_stream(stream)   -> message loop
7. Spawn: heartbeat_loop(30s)     -> periodic status
8. Return: ReceiverStream(rx)     -> outbound messages
```

---

### `execution::guards` -- RAII Guards

**ExecutionState**

```rust
struct ExecutionState {
    busy: bool,
    current_job_id: Option<String>,
    current_artifact: Option<String>,
}
```

**Guards**

```rust
struct ExecutionLockGuard {
    lock: Arc<Mutex<ExecutionState>>,
}
// Drop: tokio::spawn -> set busy=false, clear job_id/artifact

struct RedEdrGuard {
    collector: RedEdrCollector,
    reset_on_drop: bool,
}
// Drop: std::thread::spawn -> POST /api/trace/reset
// Normal: reset_now() -> explicit reset, prevent double-reset

struct ProcessGuard {
    child: Option<tokio::process::Child>,
    should_kill: bool,
}
// Drop: std::thread::spawn -> child.kill()
// Normal: disarm() -> take child, prevent kill

struct MonitorGuard {
    stop_tx: Option<watch::Sender<bool>>,
    handle: Option<JoinHandle<()>>,
    event_consumer: Option<JoinHandle<()>>,
}
// Drop: send stop signal + abort consumer
// Normal: stop() -> graceful shutdown with 10s timeout
```

| Guard | Normal Exit | Error/Panic Exit |
|-------|-------------|------------------|
| `ExecutionLockGuard` | (dropped) -> release | (dropped) -> tokio::spawn release |
| `RedEdrGuard` | `reset_now()` -> POST reset | (dropped) -> std::thread POST reset |
| `ProcessGuard` | `disarm()` -> take child | (dropped) -> std::thread kill |
| `MonitorGuard` | `stop()` -> signal + wait | (dropped) -> signal + abort |

---

### `execution::monitor` -- Process Monitoring

```rust
struct ExecutionMonitor {
    run_id: String,
    job_id: String,
    worker_id: String,
    worker_ip: String,
    artifact_name: String,
    pid: u32,
    rededr_base_url: String,
    stream_handler: Option<Arc<StreamHandler>>,
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
| `send_status_to_controller(...)` | Via StreamHandler with 1s timeout |
| `is_process_alive(pid)` | Windows `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` |
| `get_process_metrics(pid)` | sysinfo per-PID refresh -> (cpu%, mem_mb) |

**Event Types Emitted**

| Event | Trigger | Threshold |
|-------|---------|-----------|
| `started` | Initial | Once on start |
| `heartbeat` | Process alive, events growing | Every 3s |
| `stuck` | Process alive, no new events | 3+ idle cycles (9s) |
| `approaching_timeout` | Process alive, near timeout | elapsed >= timeout - 5s |
| `terminated` | PID no longer exists | Process check fails |

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
| `parse_on_event_type(hdr, type, payload)` | Dispatch binary event by type |
| `handle_binary_line_trace(hdr, payload)` | Type 1: parse "file:line:func" |
| `handle_artifact_status(hdr, payload, type)` | Types 2-4: checkpoint/success/failure |
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
    event_type: u16,   // 1=line, 2=checkpoint, 3=success, 4=failure
    thread_id: u32,
    seq_no: u64,
    ts_us: u64,
    payload_len: u32,
}
```

| event_type | Name | Payload Format |
|------------|------|---------------|
| 1 | LINE_TRACE | `"file:line:func"` (UTF-8) |
| 2 | CHECKPOINT | `"checkpoint_name"` (UTF-8) |
| 3 | SUCCESS | `"success_message"` (UTF-8) |
| 4 | FAILURE | `"message\|error_code"` (UTF-8) |

**Protocol Auto-Detection**

```
First 4 bytes == 0x49535452 ('ISTR') → Binary protocol
Otherwise → Base64 text protocol
  ├── "b64line:<base64>"  → Old IR format
  └── "YjY0<base64>"     → New AST format (YjY0 = Base64("b64"))
Decoded payload: "line:file.c:42:main"
```

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
│    └── WorkerAgentService ────────────────────────────────────────────────────┐│
│         ├── config: WorkerConfig (owned)                                      ││
│         ├── system_info: Arc<Mutex<System>> ──── health_check, get_worker_info││
│         ├── execution_lock: Arc<Mutex<ExecutionState>> ──── run_sample        ││
│         └── stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>           ││
│                                                    │                          ││
│                                                    │ set on EstablishStream   ││
│                                                    ▼                          ││
│  ┌───────────────────────────────────────────────────────────────────────────┐││
│  │                      StreamHandler (Arc-shared)                           │││
│  │  ├── worker_state: Arc<RwLock<WorkerState>> ──── mutable runtime state   │││
│  │  │   ├── capabilities, metadata, tools                                   │││
│  │  │   ├── health: HealthMetrics                                           │││
│  │  │   ├── current_job_id, current_run_id ──── telemetry correlation       │││
│  │  │   └── controller_disconnected, reconnect_allowed                      │││
│  │  ├── tx: mpsc::Sender ──── 100-capacity outbound channel ──→ Controller  │││
│  │  └── service: Arc<WorkerAgentService> ──── back-reference                │││
│  └───────────────────────────────────────────────────────────────────────────┘││
│                                                                               ││
│  Per-execution (created in run_sample, scoped to function):                   ││
│  ┌───────────────────────────────────────────────────────────────────────────┐││
│  │  ExecutionLockGuard ──── guards Arc<Mutex<ExecutionState>>               │││
│  │  RedEdrGuard ──── owns RedEdrCollector                                   │││
│  │  │                ├── config: RedEdrCollectorConfig                       │││
│  │  │                ├── client: reqwest::Client                             │││
│  │  │                └── seen_trace_ids: HashSet<u64>                        │││
│  │  ProcessGuard ──── owns tokio::process::Child                            │││
│  │  MonitorGuard ──── owns stop_tx, monitor handle, event consumer          │││
│  │                                                                           │││
│  │  Spawned tasks:                                                           │││
│  │  ├── ExecutionMonitor.start() ──── polls PID + /api/stats                │││
│  │  │   ├── stream_handler: Option<Arc<StreamHandler>> ◄───── (cloned)      │││
│  │  │   └── sys: Arc<Mutex<System>> ──── per-PID metrics                    │││
│  │  ├── TraceCollector.start_server() ──── named pipe reader                │││
│  │  │   └── event_tx: mpsc::Sender<TraceEvent> ──→ streaming writer         │││
│  │  ├── streaming_handle ──── BufWriter 256KB -> trace_events.jsonl         │││
│  │  ├── stdout_handle, stderr_handle ──── async stream capture              │││
│  │  └── event_consumer ──── log monitor events                              │││
│  └───────────────────────────────────────────────────────────────────────────┘││
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
│       │   sample_handlers::run_sample(service, SampleRequest)                   │
│       │       │                                                                 │
│       │       ├── ExecutionState.busy = true                                    │
│       │       ├── RedEdrCollector.start_trace([artifact.exe])                   │
│       │       ├── tokio::process::Command::new(artifact.exe)                    │
│       │       ├── TraceCollector.start_server() ──→ TraceEvent                  │
│       │       │                                        │                        │
│       │       │                                        ▼ mpsc (100K)            │
│       │       │                                   trace_events.jsonl            │
│       │       │                                                                 │
│       │       ├── ExecutionMonitor.start() ──→ MonitorEvent                     │
│       │       │                                    │                            │
│       │       │                                    ▼                            │
│       │       │                               send_execution_status()           │
│       │       │                                    │                            │
│       │       │                                    ▼                            │
│       │       │                            ExecutionStatusReport ──→ Controller  │
│       │       │                                                                 │
│       │       ├── (process completes or timeout)                                │
│       │       │                                                                 │
│       │       ├── RedEdrCollector.collect_all() ──→ Vec<TelemetryData>          │
│       │       ├── trace_events.jsonl ──→ TelemetryData (trace_log)              │
│       │       ├── coverage_bbs.txt ──→ TelemetryData (CoverageEvent)            │
│       │       ├── checkpoints.log ──→ Vec<TelemetryData> (CheckpointEvent)      │
│       │       │                                                                 │
│       │       ▼                                                                 │
│       │   TelemetryBatch { job_id, run_id, events[], is_final: true }           │
│       │       │                                                                 │
│       │       │ send_telemetry()                                                │
│       │       ▼                                                                 │
│       │   WorkerMessage::Telemetry ──────────────────────────→ Controller        │
│       │                                                                         │
│       │   SampleResponse { job_id, exit_code, success, output }                 │
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
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Channel Type Summary

| Channel | Message Type | Capacity | Sender | Receiver |
|---------|-------------|----------|--------|----------|
| gRPC outbound `tx` | `Result<WorkerMessage, Status>` | 100 | StreamHandler methods | Controller (via ReceiverStream) |
| Trace events | `TraceEvent` | 100,000 | TraceCollector (pipe) | Streaming JSONL writer |
| Monitor events | `MonitorEvent` | 100 | ExecutionMonitor | Event consumer (logger) |
| Monitor stop | `bool` (watch) | 1 | run_sample | ExecutionMonitor |

---

## Concurrency Primitives

| Primitive | Location | Purpose |
|-----------|----------|---------|
| `Arc<Mutex<ExecutionState>>` | WorkerAgentService | One-at-a-time execution lock |
| `Arc<Mutex<System>>` | WorkerAgentService | Shared sysinfo for health checks |
| `Arc<RwLock<Option<Arc<StreamHandler>>>>` | WorkerAgentService | Lazy stream handler initialization |
| `Arc<RwLock<WorkerState>>` | StreamHandler | Mutable runtime state (capabilities, health, job tracking) |
| `Arc<Mutex<System>>` | ExecutionMonitor | Per-PID process metric refresh |
| `Arc<AtomicU32>` | TraceCollector | Lock-free sequence counter (text protocol) |
| `mpsc::Sender` (tokio) | StreamHandler.tx | Outbound gRPC messages (bounded: 100) |
| `mpsc::Sender` (tokio) | TraceCollector.event_tx | Trace events (bounded: 100,000) |
| `mpsc::Sender` (tokio) | ExecutionMonitor event_tx | Monitor events (bounded: 100) |
| `watch::Sender<bool>` | MonitorGuard.stop_tx | Stop signal for ExecutionMonitor |