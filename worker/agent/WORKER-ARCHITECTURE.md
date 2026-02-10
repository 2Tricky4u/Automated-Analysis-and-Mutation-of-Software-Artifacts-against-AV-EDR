# Worker Agent Architecture

Deep analysis of every `.rs` file in `worker/agent/src/`.

---

## Table of Contents

1. [Module Hierarchy](#1-module-hierarchy)
2. [Component Hierarchy & Ownership](#2-component-hierarchy--ownership)
3. [Channel Architecture](#3-channel-architecture)
4. [Execution Lifecycle](#4-execution-lifecycle)
5. [State Machines](#5-state-machines)
6. [Telemetry Collection](#6-telemetry-collection)
7. [RAII Guard System](#7-raii-guard-system)
8. [Struct Reference](#8-struct-reference)
9. [Concurrency Model](#9-concurrency-model)
10. [Capability Detection](#10-capability-detection)
11. [Key Design Decisions](#11-key-design-decisions)

---

## 1. Module Hierarchy

```
worker/agent/src/
├── main.rs                          # Entry point: config, logging, gRPC server
├── lib.rs                           # WorkerAgentService struct + proto re-exports
├── capabilities.rs                  # Auto-detection: RedEDR, Defender, MDE, Cortex XDR
├── stream_handler.rs                # Bidirectional gRPC stream (controller <-> worker)
│
├── service/                         # gRPC RPC handler implementations
│   ├── mod.rs                       # WorkerAgent trait impl (dispatches to handlers)
│   ├── sample_handlers.rs           # run_sample() - THE core execution function (~1370 lines)
│   ├── stream_handlers.rs           # establish_stream() - sets up bidirectional stream
│   ├── artifact_handlers.rs         # send_artifact() - chunked binary transfer + SHA256
│   ├── info_handlers.rs             # ping(), health_check(), get_worker_info(), get_telemetry()
│   └── helpers.rs                   # BB coverage + API checkpoint parsers
│
├── execution/                       # Execution management
│   ├── mod.rs                       # Module declarations
│   ├── guards.rs                    # RAII guards: RedEdr, Process, Monitor, ExecutionLock
│   └── monitor.rs                   # ExecutionMonitor: polls process + RedEDR every 3s
│
└── telemetry/                       # Telemetry collection
    ├── mod.rs                       # Module declarations
    ├── trace_compressor.rs          # CLP + MatrixProfile + Sequitur compression
    └── collectors/
        ├── mod.rs                   # Module declarations
        ├── rededr.rs                # RedEDR HTTP API collector (ETW/kernel events)
        └── trace.rs                 # Named pipe trace collector (line-level tracing)
```

### File Sizes (lines, approximate)

| File | Lines | Role |
|------|-------|------|
| `sample_handlers.rs` | 1378 | Core execution logic, telemetry collection, timeout handling |
| `trace.rs` | 600 | Named pipe binary/text protocol, auto-detection |
| `trace_compressor.rs` | 505 | 3-stage trace compression pipeline |
| `capabilities.rs` | 406 | System detection (EDR products, hardware metadata) |
| `stream_handler.rs` | 441 | Bidirectional stream message loop, heartbeat |
| `monitor.rs` | 402 | Process monitoring, stuck/timeout detection |
| `rededr.rs` | 411 | RedEDR HTTP polling, event transform |
| `info_handlers.rs` | 259 | Health, telemetry pull, worker info RPCs |
| `guards.rs` | 207 | RAII guards with Drop impls |
| `artifact_handlers.rs` | 87 | Chunked file transfer with integrity check |
| `stream_handlers.rs` | 79 | Stream establishment, registration |
| `lib.rs` | 78 | Central struct, proto includes |
| `main.rs` | 90 | Entry point |

---

## 2. Component Hierarchy & Ownership

```
main.rs
└── WorkerAgentService                    ← Central struct, passed to gRPC server
    ├── worker_id: String                 ← From config
    ├── config: WorkerConfig              ← TOML file (C:\AutoMutate\worker.toml)
    ├── system_info: Arc<Mutex<System>>   ← sysinfo for health metrics
    ├── execution_lock: Arc<Mutex<ExecutionState>>  ← ONE run at a time
    └── stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>
                                          ← Set on EstablishStream, used by run_sample
        StreamHandler
        ├── worker_state: Arc<RwLock<WorkerState>>
        │   ├── capabilities: Vec<String>
        │   ├── metadata: HashMap<String, String>
        │   ├── health: HealthMetrics
        │   ├── current_job_id: Option<String>
        │   ├── current_run_id: Option<String>  ← Correlates telemetry
        │   ├── controller_disconnected: bool
        │   └── reconnect_allowed: bool
        ├── tx: mpsc::Sender<WorkerMessage>     ← 100-capacity channel
        └── service: Arc<WorkerAgentService>    ← Back-reference
```

### Ownership Rules

1. `WorkerAgentService` is `Clone` (all fields are `Arc`-wrapped)
2. `StreamHandler` is created lazily on first `EstablishStream` RPC
3. `ExecutionState` mutex enforces one-at-a-time execution
4. `WorkerState` is the mutable runtime state (capabilities, health, job tracking)

---

## 3. Channel Architecture

```
                    Controller (gRPC)
                         │
           ┌─────────────┼─────────────┐
           │             │              │
           ▼             │              ▼
    ┌──────────┐         │       ┌──────────┐
    │Controller│         │       │  Worker   │
    │ Messages │         │       │ Messages  │
    │(inbound) │         │       │(outbound) │
    └────┬─────┘         │       └─────▲─────┘
         │               │             │
         ▼               │             │
    StreamHandler        │        tx (mpsc)
    .handle_stream()     │        capacity=100
         │               │             ▲
         ├── RunSample ──┼─────→ SampleResponse
         ├── HealthCheck ┼─────→ StatusReport
         ├── Heartbeat   ┼─────→ (no reply)
         ├── Disconnect  ┼─────→ (no reply)
         └── Ack         │             │
                         │             │
                         │      ┌──────┤
                         │      │      │
                    Telemetry  Exec   Registration
                    Batch      Status
```

### Channel Types

| Channel | Type | Capacity | Source -> Sink |
|---------|------|----------|---------------|
| **gRPC outbound** | `mpsc::Sender<Result<WorkerMessage, Status>>` | 100 | StreamHandler -> Controller |
| **Trace events** | `mpsc::Sender<TraceEvent>` | 100,000 | TraceCollector -> File writer |
| **Monitor events** | `mpsc::Sender<MonitorEvent>` | 100 | ExecutionMonitor -> Event logger |
| **Monitor stop** | `watch::Sender<bool>` | 1 | run_sample -> ExecutionMonitor |

### Message Types (Outbound to Controller)

```
WorkerMessage.payload = oneof {
    Registration    ← Sent once on stream establishment
    Status          ← Heartbeat (30s), health check response
    ExecutionStatus ← Monitor updates: started, heartbeat, stuck, timeout, terminated
    SampleResponse  ← Run result: exit_code, success, output, detected
    Telemetry       ← TelemetryBatch: events[], is_final=true
    Ack             ← Acknowledge RunSample command receipt
}
```

### Message Types (Inbound from Controller)

```
ControllerMessage.payload = oneof {
    RunSample       ← Execute artifact (job_id, artifact_id, timeout_seconds)
    HealthCheck     ← Request status report
    Heartbeat       ← Keep-alive with timestamp
    Disconnect      ← Graceful disconnect (reconnect_allowed?)
    Ack             ← Acknowledge worker message
    ArtifactChunks  ← TODO: stream-based artifact transfer
}
```

---

## 4. Execution Lifecycle

The core of the worker is `sample_handlers::run_sample()` (~1370 lines). This is the full lifecycle of executing one artifact.

### Phase 1: Lock Acquisition

```
    ┌─────────────────────────────────┐
    │         run_sample() called      │
    └─────────────┬───────────────────┘
                  │
                  ▼
    ┌─────────────────────────────────┐
    │  Acquire ExecutionLock mutex     │
    │  Check: state.busy?             │──── busy=true ──→ Return RESOURCE_EXHAUSTED
    │  Set: busy=true, job_id, artifact│
    │  Create: ExecutionLockGuard     │
    └─────────────┬───────────────────┘
                  │
                  ▼
            run_id resolution
            (from WorkerState.current_run_id
             set by stream_handler, or UUID)
```

### Phase 2: RedEDR Setup + Sanity Check

```
    ┌─────────────────────────────────┐
    │  Create RedEdrCollector          │
    │  Create RedEdrGuard (RAII)       │
    └─────────────┬───────────────────┘
                  │
                  ▼
    ┌─────────────────────────────────┐
    │  Sanity Check: collect_all()     │
    │  Check leftover events           │
    └─────────────┬───────────────────┘
                  │
         ┌────────┼────────┐
         │        │        │
    0 events  1 event   >1 events
    (clean)   (init     (contaminated)
         │     noise)        │
         │        │          ▼
         │        │    Force reset RedEDR
         │        │    Set new trace target
         │        │    Reset & re-start
         ▼        ▼
    ┌─────────────────────────────────┐
    │  start_trace([artifact.exe])     │
    │  RedEDR now watching artifact    │
    └─────────────────────────────────┘
```

### Phase 3: Process Spawn + Collectors

```
    ┌─────────────────────────────────┐
    │  Create telemetry directory      │
    │  C:\temp\artifacts\telemetry_*   │
    └─────────────┬───────────────────┘
                  │
    ┌─────────────┼─────────────────┐
    │             │                 │
    ▼             ▼                 ▼
  TraceCollector  Streaming       Process
  (named pipe)    Writer          Spawn
  \\.\pipe\       BufWriter       ├── stdout piped
  rededr_trace    256KB buffer    ├── stderr piped
    │             │               └── cwd = telemetry_dir
    │             │                    │
    ▼             ▼                    ▼
  trace_handle   streaming_handle   ProcessGuard
  (JoinHandle)   (JoinHandle)       (RAII kill)
                                     │
                                     ▼
                                  Get PID
```

### Phase 4: Monitoring + Wait

```
    ┌─────────────────────────────────┐
    │  Create ExecutionMonitor         │
    │  (run_id, job_id, pid, timeout)  │
    ├─────────────────────────────────┤
    │  Spawn: monitor.start()          │
    │  Spawn: event consumer           │
    │  Create: MonitorGuard (RAII)     │
    └─────────────┬───────────────────┘
                  │
                  ▼
    ┌─────────────────────────────────┐
    │  tokio::time::timeout(           │
    │    timeout_duration,             │
    │    process.wait()                │
    │  )                               │
    └─────────────┬───────────────────┘
                  │
         ┌────────┼────────┐
         │        │        │
    Process    Wait()    Timeout
    exited     failed    fired
    (code)     (-1)        │
         │        │        ▼
         │        │   Check try_wait()
         │        │   ├── Exited (race) → not timeout
         │        │   └── Still running → real timeout
         │        │       ├── taskkill /F /T /PID
         │        │       └── child.kill()
         ▼        ▼
    exit_code resolution:
     0    = success
    -1    = timeout or wait() failure
    -2    = externally terminated (no exit code, likely AV/EDR)
    other = NTSTATUS interpretation
```

### Phase 5: Telemetry Collection

```
    ┌─────────────────────────────────┐
    │  Stop monitor (MonitorGuard)     │
    │  Wait for trace pipe flush       │
    │  Abort trace_handle              │
    │  Drop trace_tx (close channel)   │
    │  Wait for streaming_handle       │
    └─────────────┬───────────────────┘
                  │
    ┌─────────────┼──────────────────────────────────┐
    │             │                │                  │
    ▼             ▼                ▼                  ▼
  RedEDR       Trace Log       BB Coverage      API Checkpoints
  collect_all  (trace_events   (coverage.bin    (checkpoints.log)
  (HTTP API)    .jsonl)         +coverage_bbs    JSON lines
    │             │              .txt)              │
    │             │                │                │
    │         ┌───┼───┐           │                │
    │     <=2MB   │  >2MB         │                │
    │     send    │  send         │                │
    │     whole   │  last 2MB     │                │
    │             │  + async      │                │
    │             │  compress     │                │
    ▼             ▼               ▼                ▼
    └─────────── telemetry_events[] ───────────────┘
                      │
                      ▼
               TelemetryBatch {
                 job_id, run_id,
                 events: [...],
                 is_final: true
               }
                      │
                      ▼
              StreamHandler.send_telemetry()
```

### Phase 6: Cleanup + Response

```
    ┌─────────────────────────────────┐
    │  rededr_guard.reset_now()        │
    │  (explicit reset for next run)   │
    │                                  │
    │  ExecutionLockGuard drops        │
    │  (state.busy = false)            │
    └─────────────┬───────────────────┘
                  │
                  ▼
    Return SampleResponse {
      job_id, success, exit_code,
      output, telemetry_ids: [run_id],
      detected: false  // TODO
    }
```

---

## 5. State Machines

### 5.1 Execution Lock

```
    IDLE ──────────────────→ BUSY ──────────────────→ IDLE
    (busy=false)  lock acquired  (busy=true,          lock released
                  set job_id      job_id=Some,         (Drop or explicit)
                  set artifact    artifact=Some)

    Invariant: At most ONE artifact executing at any time.
    Enforcement: Arc<Mutex<ExecutionState>>
    Cleanup: ExecutionLockGuard Drop impl
```

### 5.2 Stream Handler Lifecycle

```
    NULL ──→ ACTIVE ──→ DISCONNECTED ──→ RECONNECTED ──→ ACTIVE
     │         │              │                │
     │    EstablishStream  controller_        heartbeat
     │    RPC called       disconnected=true  succeeds
     │                                        → reset flag
     │
     stream_handler = Arc<RwLock<Option<Arc<StreamHandler>>>>
     None until first EstablishStream
```

### 5.3 Execution Monitor States

```
    STARTED ──→ HEARTBEAT ──→ HEARTBEAT ──→ ...
                    │              │
                    │              │
                    ▼              ▼
               (idle 3+)     (timeout-5s)
                STUCK      APPROACHING_TIMEOUT
                    │              │
                    └──────┬───────┘
                           ▼
                      TERMINATED
                     (process dead)

    Poll interval: 3 seconds
    Stuck threshold: 3 idle cycles (9+ seconds no new events)
    Timeout threshold: within 5 seconds of timeout_seconds
```

### 5.4 Monitor Event Types

| Event | Condition | Severity |
|-------|-----------|----------|
| `started` | Initial event on monitor start | Info |
| `heartbeat` | Process alive, events growing | Info |
| `stuck` | Process alive, no new events for 3+ cycles | Warn |
| `approaching_timeout` | Process alive, elapsed >= timeout - 5s | Warn |
| `terminated` | Process no longer alive (PID check fails) | Info |

---

## 6. Telemetry Collection

### 6.1 Five Telemetry Sources

```
    ┌──────────────────────────────────────────────────────┐
    │                  Artifact Execution                   │
    │                                                      │
    │  ┌─────────┐  ┌─────────┐  ┌────────┐  ┌──────────┐│
    │  │ stdout  │  │ stderr  │  │coverage│  │checkpoints││
    │  │ (piped) │  │ (piped) │  │.bin    │  │.log      ││
    │  └────┬────┘  └────┬────┘  │+bbs.txt│  │(JSON)    ││
    │       │            │       └───┬────┘  └────┬─────┘│
    └───────┼────────────┼───────────┼────────────┼──────┘
            │            │           │            │
            ▼            ▼           ▼            ▼
    ┌───────────┐  ┌──────────┐  ┌────────┐  ┌──────────┐
    │  stdout   │  │  stderr  │  │   BB   │  │   API    │
    │  capture  │  │  capture │  │coverage│  │checkpoint│
    │  (async)  │  │  (async) │  │parser  │  │ parser   │
    └───────────┘  └──────────┘  └────────┘  └──────────┘
            │            │           │            │
    ┌───────┼────────────┼───────────┼────────────┼──────┐
    │       ▼            ▼           ▼            ▼      │
    │         ┌─ telemetry_events: Vec<TelemetryData> ─┐ │
    │         │                                        │ │
    │         │  + RedEDR events (HTTP /api/logs/rededr)│ │
    │         │  + trace_log (named pipe events)       │ │
    │         │  + coverage (typed CoverageEvent)      │ │
    │         │  + checkpoints (typed CheckpointEvent) │ │
    │         └────────────────────────────────────────┘ │
    │                                                    │
    │  ┌──────────────────┐   ┌────────────────────┐    │
    │  │   Named Pipe     │   │   RedEDR HTTP API  │    │
    │  │\\.\pipe\rededr_  │   │ GET /api/logs/rededr│    │
    │  │    trace         │   │ GET /api/stats      │    │
    │  │  (binary/b64)    │   │ POST /api/trace/    │    │
    │  └──────────────────┘   │      start|reset    │    │
    │                         └────────────────────┘    │
    └────────────────────────────────────────────────────┘
```

### 6.2 RedEDR Collector (`rededr.rs`)

```
RedEdrCollector
├── config: RedEdrCollectorConfig
│   ├── base_url: "http://localhost:8081"
│   ├── flush_interval_ms: 1000
│   ├── job_id, run_id
├── client: reqwest::Client (5s timeout)
└── seen_trace_ids: HashSet<u64>  (dedup by trace_id)

API Endpoints Used:
├── GET  /api/logs/rededr    → Vec<RedEdrEvent> (JSON array)
├── GET  /api/stats          → {"events_count": N}
├── POST /api/trace/start    → {"trace": ["artifact.exe"]}
├── POST /api/trace/reset    → Clear all events
├── POST /api/lock/acquire   → (TODO: not yet used)
└── POST /api/lock/release   → (TODO: not yet used)

Event Transform:
  RedEdrEvent {date, type, trace_id, target, func, pid, tid, provider, ...}
      ↓
  TelemetryData {job_id, event_type, timestamp, payload: JSON bytes, metadata}
```

### 6.3 Trace Collector (`trace.rs`)

```
TraceCollector
├── pipe_name: "\\.\pipe\rededr_trace"
├── event_tx: mpsc::Sender<TraceEvent>  (capacity 100,000)
└── sequence_counter: AtomicU32

Protocol Auto-Detection (first 4 bytes):
├── 0x49535452 ('ISTR') → Binary protocol
│   InstRecordHeader (32 bytes, packed):
│   ├── magic: u32 (0x49535452)
│   ├── version: u16
│   ├── event_type: u16 (1=line, 2=checkpoint, 3=success, 4=failure)
│   ├── thread_id: u32
│   ├── seq_no: u64
│   ├── ts_us: u64
│   └── payload_len: u32
│   Payload: "file:line:func" (UTF-8)
│
└── Other → Base64 text protocol
    Formats:
    ├── "b64line:<base64>"  (old IR format)
    └── "YjY0<base64>"     (new AST format, YjY0 = Base64("b64"))
    Decoded: "line:file.c:42:main"
```

### 6.4 Trace Compression (`trace_compressor.rs`)

Three-stage pipeline for traces exceeding 2MB:

```
    Raw JSONL trace
         │
    Stage 1: CLP-inspired Columnar Decomposition
    ├── Extract line_sequence: Vec<u32>  (dense integer array)
    ├── file_dict: Vec<String>           (deduplicated)
    ├── func_dict: Vec<String>           (deduplicated)
    ├── file_indices, func_indices       (index arrays)
    └── thread_ids, timestamps
         │
    Stage 2: Matrix Profile Pattern Detection
    ├── Sliding window: min=2, max=50
    ├── Find patterns with >= N occurrences
    ├── Sort by compression benefit (occurrences * length)
    └── Output: Vec<Motif> {start, length, occurrences}
         │
    Stage 3: Sequitur-like Grammar Induction
    ├── Convert top motifs to grammar rules (non-overlapping greedy)
    ├── Build start_rule: mix of Terminal(line) and NonTerminal(rule_id)
    └── Output: "RULE_0 (used 15 times): L10 L11 L12"
                "@RULE_0 @RULE_0 L50 @RULE_0"
```

### 6.5 Trace Payload Sizing Strategy

```
    Trace size check
         │
    ┌────┼────────────┐
    │                  │
  <= 2MB             > 2MB
    │                  │
    ├── <= 4MB:       ├── Immediate: last 2MB (complete JSONL lines)
    │   send raw      │   → "trace_log" event in main batch
    │                  │
    └── > 4MB:        └── Async: full trace compression
        truncate          ├── Loop detection fits in 4MB → send
        tail              ├── + gzip fits → send as base64
                          └── Still too big → truncate first/last 100 lines
```

---

## 7. RAII Guard System

All guards use `Drop` implementations for cleanup on any exit path (success, error, panic).

### 7.1 Guard Hierarchy

```
    run_sample() scope
    ├── ExecutionLockGuard   ← Releases execution_lock on drop
    ├── RedEdrGuard          ← Resets RedEDR HTTP API on drop
    ├── ProcessGuard         ← Kills child process on drop
    └── MonitorGuard         ← Stops monitor + event consumer on drop
```

### 7.2 Guard Details

| Guard | Protects | Normal Exit | Drop Exit |
|-------|----------|-------------|-----------|
| `ExecutionLockGuard` | `Arc<Mutex<ExecutionState>>` | Lock released | `tokio::spawn` -> set busy=false |
| `RedEdrGuard` | RedEDR HTTP state | `reset_now()` (explicit) | `std::thread::spawn` -> POST /api/trace/reset |
| `ProcessGuard` | `tokio::process::Child` | `disarm()` (take child) | `std::thread::spawn` -> `child.kill()` |
| `MonitorGuard` | monitor task + event consumer | `stop()` (graceful) | Send stop signal + abort consumer |

### 7.3 Drop Cleanup Pattern

All async cleanup in Drop uses `std::thread::spawn` + new tokio runtime:

```rust
impl Drop for RedEdrGuard {
    fn drop(&mut self) {
        if self.reset_on_drop {
            let base_url = self.collector.config().base_url.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    // HTTP POST to reset RedEDR
                });
            });
        }
    }
}
```

This is necessary because `Drop` is synchronous but cleanup requires async I/O.

---

## 8. Struct Reference

### Core Structs

```rust
// lib.rs
pub struct WorkerAgentService {
    pub(crate) worker_id: String,
    pub(crate) config: WorkerConfig,
    pub(crate) system_info: Arc<Mutex<System>>,
    pub(crate) execution_lock: Arc<Mutex<ExecutionState>>,
    pub(crate) stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>,
}

// execution/guards.rs
pub struct ExecutionState {
    pub busy: bool,
    pub current_job_id: Option<String>,
    pub current_artifact: Option<String>,
}
```

### Stream & Communication

```rust
// stream_handler.rs
pub struct StreamHandler {
    pub worker_state: Arc<RwLock<WorkerState>>,
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,
    service: Arc<WorkerAgentService>,
}
```

### Capabilities & State

```rust
// capabilities.rs
pub struct WorkerCapabilities {
    pub capabilities: Vec<String>,      // ["rededr", "mde", "cortex"]
    pub tools: HashMap<String, String>, // {"rededr_version": "1.2.3"}
    pub metadata: HashMap<String, String>,
}

pub struct WorkerState {
    pub worker_id: String,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub tools: Option<ToolVersions>,  // Proto type
    pub health: HealthMetrics,
    pub current_job_id: Option<String>,
    pub current_run_id: Option<String>,
    pub last_controller_heartbeat: Option<i64>,
    pub controller_disconnected: bool,
    pub disconnect_reason: Option<String>,
    pub reconnect_allowed: bool,
}

pub struct HealthMetrics {
    pub cpu_percent: i32,
    pub memory_percent: i32,
    pub disk_percent: i32,
    pub active_jobs: i32,
    pub uptime_seconds: i64,
}

pub struct WindowsVersionInfo {
    pub product_name: Option<String>,
    pub edition_id: Option<String>,
    pub display_version: Option<String>,
    pub release_id: Option<String>,
    pub build: Option<u32>,
    pub ubr: Option<u32>,
    pub is_windows_11: Option<bool>,
}
```

### Telemetry Structs

```rust
// telemetry/collectors/rededr.rs
pub struct RedEdrCollectorConfig {
    pub base_url: String,
    pub flush_interval_ms: u64,
    pub job_id: String,
    pub run_id: String,
}

pub struct RedEdrCollector {
    config: RedEdrCollectorConfig,
    client: reqwest::Client,
    seen_trace_ids: HashSet<u64>,
}

pub struct RedEdrEvent {
    pub date: Option<String>,
    pub r#type: Option<String>,
    pub trace_id: Option<u64>,
    pub target: Option<String>,
    pub func: Option<String>,
    pub pid: Option<u32>,
    pub tid: Option<u32>,
    pub provider: Option<String>,
    pub event_id: Option<u32>,
    pub callstack: Option<serde_json::Value>,
    pub stack_trace: Option<Vec<StackTraceEntry>>,
    pub targets: Option<Vec<String>>,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// telemetry/collectors/trace.rs
pub struct TraceEvent {
    pub seq: u32,
    pub thread_id: u32,
    pub file: String,
    pub line: u32,
    pub func: String,
    pub ts_us: u64,
}

pub struct TraceCollector {
    pipe_name: String,
    event_tx: mpsc::Sender<TraceEvent>,
    sequence_counter: Arc<AtomicU32>,
}

// Binary protocol header (packed, 32 bytes)
#[repr(C, packed)]
struct InstRecordHeader {
    magic: u32,       // 0x49535452
    version: u16,
    event_type: u16,  // 1=line, 2=checkpoint, 3=success, 4=failure
    thread_id: u32,
    seq_no: u64,
    ts_us: u64,
    payload_len: u32,
}
```

### Execution Monitor

```rust
// execution/monitor.rs
pub struct ExecutionMonitor {
    pub run_id: String,
    pub job_id: String,
    pub worker_id: String,
    pub worker_ip: String,
    pub artifact_name: String,
    pub pid: u32,
    pub rededr_base_url: String,
    pub stream_handler: Option<Arc<StreamHandler>>,
    pub start_time: Instant,
    pub timeout_seconds: i32,
    client: reqwest::Client,         // 3s timeout for /api/stats
    sys: Arc<Mutex<sysinfo::System>>,// per-PID refresh
}
```

### RAII Guards

```rust
// execution/guards.rs
pub struct RedEdrGuard { collector, reset_on_drop: bool }
pub struct MonitorGuard { stop_tx, handle, event_consumer }
pub struct ProcessGuard { child: Option<Child>, should_kill: bool }
pub struct ExecutionLockGuard { lock: Arc<Mutex<ExecutionState>> }
```

### Compression

```rust
// telemetry/trace_compressor.rs
pub struct CompressedTrace {
    pub original_size: usize,
    pub compressed_size: usize,
    pub content: String,
    pub compression_ratio: f64,
    pub statistics: CompressionStatistics,
}

pub struct CompressionStatistics {
    pub original_events: usize,
    pub unique_files: usize,
    pub unique_functions: usize,
    pub patterns_found: usize,
    pub max_pattern_length: usize,
    pub total_pattern_occurrences: usize,
    pub grammar_rules: usize,
}
```

---

## 9. Concurrency Model

### Thread/Task Breakdown for One Execution

```
    Tokio Runtime (main)
    │
    ├── gRPC Server task (tonic)
    │   └── WorkerAgent trait handlers
    │
    ├── Stream handler task (spawned per EstablishStream)
    │   └── handle_stream() message loop
    │
    ├── Heartbeat task (spawned per EstablishStream)
    │   └── heartbeat_loop() every 30s
    │
    └── Per-execution tasks (spawned in run_sample):
        ├── trace_handle        ← TraceCollector.start_server() (named pipe)
        ├── streaming_handle    ← BufWriter to trace_events.jsonl
        ├── stdout_handle       ← capture stdout
        ├── stderr_handle       ← capture stderr
        ├── monitor_handle      ← ExecutionMonitor.start() (3s poll loop)
        └── event_consumer      ← Log monitor events
```

### Synchronization Primitives

| Primitive | Location | Protects |
|-----------|----------|----------|
| `Arc<Mutex<ExecutionState>>` | WorkerAgentService | One-at-a-time execution |
| `Arc<Mutex<System>>` | WorkerAgentService | sysinfo refresh (health check) |
| `Arc<RwLock<Option<Arc<StreamHandler>>>>` | WorkerAgentService | Lazy stream handler init |
| `Arc<RwLock<WorkerState>>` | StreamHandler | Mutable runtime state |
| `Arc<Mutex<System>>` | ExecutionMonitor | Per-PID process metrics |
| `AtomicU32` | TraceCollector | Sequence counter for text traces |
| `watch::Sender<bool>` | MonitorGuard | Stop signal for monitor |

### Critical Section: Execution Lock

```
    Request arrives → lock.lock().await
    ├── busy=true → reject (RESOURCE_EXHAUSTED)
    └── busy=false → acquire
        Set: busy=true, job_id, artifact
        Create: ExecutionLockGuard
        ... entire execution ...
        Drop: ExecutionLockGuard → busy=false
```

**Why single execution?** RedEDR captures kernel-level ETW events for a target process. Running multiple artifacts simultaneously would cause telemetry cross-contamination -- events from one artifact attributed to another.

---

## 10. Capability Detection

### Detection Methods

```
    detect_capabilities() ─────────────────────────────────────┐
    │                                                          │
    ├── RedEDR: HTTP GET localhost:8081/api/stats               │
    │   └── Version: GET /api/logs/agent, regex "RedEdr (\d+)" │
    │                                                          │
    ├── Defender: sc query WinDefend → "RUNNING"                │
    │   └── Version: PowerShell Get-MpComputerStatus            │
    │                                                          │
    ├── MDE: Registry HKLM\SOFTWARE\Microsoft\                  │
    │        Windows Advanced Threat Protection\OnboardedInfo    │
    │                                                          │
    ├── Cortex XDR: Registry CyveraService                      │
    │   └── OR filesystem: C:\ProgramData\Cyvera exists         │
    │   └── OR filesystem: C:\Program Files\Palo Alto\Traps     │
    │                                                          │
    └── System Metadata:                                        │
        ├── hostname (COMPUTERNAME env)                         │
        ├── cpu_cores (available_parallelism)                   │
        ├── ram_gb (sysinfo total_memory)                       │
        ├── os_key: "win11-build-22621" or "win10-build-19045"  │
        └── os_build: from registry CurrentBuildNumber          │
    ────────────────────────────────────────────────────────────┘
```

### Capability Strings

| Capability | Detection Method | Notes |
|------------|-----------------|-------|
| `rededr` | HTTP health check | Required for ETW telemetry |
| `mde` | Registry key present + non-empty OnboardedInfo | Microsoft Defender for Endpoint |
| `cortex` | Registry service OR filesystem | Palo Alto Cortex XDR |
| `defender` | Service running (commented out in code) | Windows Defender (not added to caps) |

### OS Key Format

```
os_key = "win{10|11}-build-{build_number}"
```

- Build >= 22000 = Windows 11
- Build < 22000 = Windows 10
- Used by controller for capability matching (RunEnvelope.required_os)

---

## 11. Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Single execution lock** | RedEDR ETW capture is per-machine, concurrent artifacts contaminate telemetry |
| **RAII guards for all resources** | Artifact execution can fail at any point; guards ensure cleanup |
| **`std::thread::spawn` in Drop** | Drop is sync, but cleanup requires async HTTP/process kill |
| **Named pipe auto-detection** | Support both old IR (Base64) and new AST (binary ISTR) trace formats |
| **100K trace channel** | Line-level tracing in tight loops generates very high event rates |
| **Streaming writer with 256KB buffer** | Reduce I/O syscalls during high-frequency tracing |
| **Thread-ID elision** | Only write thread_id to JSONL when it changes (compression optimization) |
| **Two-phase trace sending** | Immediate last-2MB for quick analysis + async compressed full trace |
| **Sanity check before each run** | Detect RedEDR contamination from previous run's incomplete cleanup |
| **30s heartbeat loop** | Keep stream alive, auto-detect controller reconnection |
| **`try_wait()` on timeout** | Handle race condition: process exits exactly as timeout fires |
| **NTSTATUS interpretation** | Windows exit codes are often NTSTATUS (0xC0000005 = ACCESS_VIOLATION) |
| **Contaminated event tagging** | If leftover events found, tag with `job_id=contaminated` metadata |
| **Stream handler lazy init** | Worker can run without controller stream (Phase 1 legacy mode) |
| **Artifact-specific telemetry dirs** | `telemetry_{artifact_id}/` prevents file collision between runs |

---

## Appendix: RPC Interface

The `WorkerAgent` gRPC service exposes:

| RPC | Handler | Direction | Description |
|-----|---------|-----------|-------------|
| `Ping` | `info_handlers::ping` | Unary | Liveness check |
| `RunSample` | `sample_handlers::run_sample` | Unary | Execute artifact (legacy, see stream) |
| `HealthCheck` | `info_handlers::health_check` | Unary | CPU/mem/busy status |
| `SendArtifact` | `artifact_handlers::send_artifact` | Client streaming | Chunked binary + SHA256 verify |
| `GetWorkerInfo` | `info_handlers::get_worker_info` | Unary | Capabilities, tools, metadata |
| `GetTelemetry` | `info_handlers::get_telemetry` | Server streaming | Pull telemetry on demand |
| `EstablishStream` | `stream_handlers::establish_stream` | Bidirectional | Real-time controller<->worker |

### Artifact Transfer Flow

```
    Controller                          Worker
    │                                    │
    │  SendArtifact(stream of chunks)    │
    │ ──────────────────────────────────→│
    │  chunk_0 {artifact_id, sha256,     │
    │           data, chunk_index=0}     │
    │ ──────────────────────────────────→│
    │  chunk_1 {data, chunk_index=1}     │
    │ ──────────────────────────────────→│
    │  ...                               │
    │  (stream closes)                   │
    │                                    │  Sort by chunk_index
    │                                    │  Reassemble bytes
    │                                    │  SHA256 verify
    │                                    │  Write to C:\temp\artifacts\{id}.exe
    │  TransferAck {received, path}      │
    │ ←──────────────────────────────────│
```
