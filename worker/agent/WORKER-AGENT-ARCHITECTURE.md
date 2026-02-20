# Worker Agent Architecture

Deep analysis of every `.rs` file in `worker/agent/`, reflecting the current implementation.

---

## Table of Contents

1. [Module Hierarchy](#1-module-hierarchy)
2. [Component Hierarchy & Ownership](#2-component-hierarchy--ownership)
3. [Channel Architecture](#3-channel-architecture)
4. [Execution Lifecycle](#4-execution-lifecycle)
5. [State Machines](#5-state-machines)
6. [Telemetry Collection](#6-telemetry-collection)
7. [RAII Guard System](#7-raii-guard-system)
8. [Control Plane Sink](#8-control-plane-sink)
9. [Struct Reference](#9-struct-reference)
10. [Concurrency Model](#10-concurrency-model)
11. [Capability Detection](#11-capability-detection)
12. [Key Design Decisions](#12-key-design-decisions)

---

## 1. Module Hierarchy

```
worker/agent/
├── build.rs                              # Proto compilation (tonic-prost-build)
├── Cargo.toml                            # Dependencies, workspace member
├── src/
│   ├── main.rs                           # Entry point: config, logging, gRPC server
│   ├── lib.rs                            # WorkerAgentService struct + proto re-exports
│   ├── capabilities.rs                   # Auto-detection: RedEDR, Defender, MDE, Cortex XDR
│   │
│   ├── api/                              # gRPC RPC handler layer (thin adapters)
│   │   ├── mod.rs                        # WorkerAgent trait impl (dispatches to handlers)
│   │   ├── run.rs                        # run_sample() - unary RPC entry point
│   │   ├── artifacts.rs                  # send_artifact() - chunked binary + SHA256 verify
│   │   ├── info.rs                       # ping(), health_check(), get_worker_info(), get_telemetry()
│   │   └── stream.rs                     # establish_stream() - bidirectional stream setup
│   │
│   ├── dispatch/                         # Execution orchestration (core logic)
│   │   ├── mod.rs                        # Module declarations
│   │   ├── engine.rs                     # execute_run() - 9-phase execution pipeline
│   │   ├── guards.rs                     # RAII guards: RedEdr, Process, Monitor
│   │   ├── monitor.rs                    # ExecutionMonitor: polls process + RedEDR every 3s
│   │   ├── sink.rs                       # ControlPlaneSink trait (StreamSink / NullSink)
│   │   ├── state.rs                      # ExecutionState enum + ExecutionLockGuard
│   │   └── types.rs                      # RunRequest, RunContext, RunOutcome, RunPhaseTimings
│   │
│   ├── session/                          # Stream session and worker runtime state
│   │   ├── mod.rs                        # Module declarations
│   │   ├── stream_handler.rs             # Bidirectional gRPC stream message loop + heartbeat
│   │   └── worker_state.rs              # WorkerState, HealthMetrics (runtime state)
│   │
│   ├── infra/                            # OS + side effects (pluggable boundary)
│   │   ├── mod.rs                        # Module declarations
│   │   ├── helpers.rs                    # BB coverage + API checkpoint file parsers
│   │   ├── process.rs                    # spawn, kill, verify, capture (Windows-specific)
│   │   └── system.rs                     # Telemetry directory management
│   │
│   └── telemetry/                        # Telemetry collection and compression
│       ├── mod.rs                        # Module declarations
│       ├── pipeline.rs                   # Trace packaging: size → compress → batch
│       ├── trace_compressor.rs           # CLP + MatrixProfile + Sequitur compression
│       └── collectors/
│           ├── mod.rs                    # Module declarations
│           ├── rededr.rs                 # RedEDR HTTP API collector (ETW/kernel events)
│           └── trace.rs                  # Named pipe trace collector (line-level tracing)
│
└── tests/
    └── test_trace_pipe.rs                # Integration test: named pipe Base64 trace flow
```

### File Sizes

| File | Lines | Role |
|------|------:|------|
| `dispatch/engine.rs` | 698 | Core 9-phase execution pipeline |
| `telemetry/collectors/trace.rs` | 599 | Named pipe binary/text protocol, auto-detection |
| `session/stream_handler.rs` | 520 | Bidirectional stream message loop + heartbeat |
| `telemetry/trace_compressor.rs` | 504 | 3-stage trace compression pipeline (experimental) |
| `telemetry/pipeline.rs` | 444 | Trace packaging: size check → compress → batch |
| `dispatch/monitor.rs` | 414 | Process monitoring, stuck/timeout detection |
| `telemetry/collectors/rededr.rs` | 411 | RedEDR HTTP polling, event transform |
| `capabilities.rs` | 312 | System detection (EDR products, hardware metadata) |
| `api/info.rs` | 261 | Health, telemetry pull, worker info RPCs |
| `api/run.rs` | 235 | Unary RunSample RPC entry point |
| `tests/test_trace_pipe.rs` | 158 | Integration test for named pipe trace |
| `dispatch/guards.rs` | 156 | RAII guards with Drop impls |
| `infra/helpers.rs` | 124 | BB coverage + API checkpoint parsers |
| `dispatch/state.rs` | 122 | ExecutionState enum + lock guard |
| `dispatch/sink.rs` | 105 | ControlPlaneSink trait + StreamSink/NullSink |
| `session/worker_state.rs` | 97 | WorkerState struct, HealthMetrics |
| `main.rs` | 89 | Entry point |
| `api/artifacts.rs` | 84 | Chunked file transfer with integrity check |
| `infra/process.rs` | 81 | Process spawn, kill tree, verify alive, capture stream |
| `api/stream.rs` | 78 | Stream establishment, registration |
| `lib.rs` | 75 | Central struct, proto includes |
| `api/mod.rs` | 73 | WorkerAgent trait impl (dispatch table) |
| `dispatch/types.rs` | 52 | Typed domain types for execution |
| `build.rs` | 29 | Proto compilation |
| `infra/system.rs` | 15 | Telemetry directory management |
| **Total** | **~5,700** | |

---

## 2. Component Hierarchy & Ownership

```
main.rs
└── WorkerAgentService                    [Clone, all fields Arc-wrapped]
    ├── worker_id: String                 ← From config
    ├── config: WorkerConfig              ← TOML file (C:\AutoMutate\worker.toml)
    ├── system_info: Arc<Mutex<System>>   ← sysinfo for health metrics
    ├── execution_lock: Arc<Mutex<ExecutionState>>  ← ONE run at a time
    └── stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>
                                          ← Set on EstablishStream, used by run_sample
        StreamHandler [no Arc<WorkerAgentService> back-ref]
        ├── worker_state: Arc<RwLock<WorkerState>>
        │   ├── capabilities: Vec<String>
        │   ├── metadata: HashMap<String, String>
        │   ├── health: HealthMetrics
        │   ├── current_job_id: Option<String>
        │   ├── current_run_id: Option<String>  ← Correlates telemetry
        │   ├── controller_disconnected: bool
        │   └── reconnect_allowed: bool
        ├── tx: mpsc::Sender<WorkerMessage>     ← 100-capacity channel
        ├── worker_id: String                   ← Cloned from service
        ├── config: WorkerConfig                ← Cloned from service
        └── execution_lock: Arc<Mutex<ExecutionState>>  ← Shared with service
```

### Ownership Rules

1. `WorkerAgentService` is `Clone` (all fields are `Arc`-wrapped)
2. `StreamHandler` is created lazily on first `EstablishStream` RPC
3. **No Arc cycle**: `StreamHandler` does NOT hold `Arc<WorkerAgentService>`; it clones individual fields (`worker_id`, `config`, `execution_lock`) to break the reference cycle
4. `ExecutionState` mutex enforces one-at-a-time execution
5. `WorkerState` is the mutable runtime state (capabilities, health, job tracking)

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

| Channel | Type | Capacity | Source → Sink |
|---------|------|----------|---------------|
| **gRPC outbound** | `mpsc::Sender<Result<WorkerMessage, Status>>` | 100 | StreamHandler / Sink → Controller |
| **Trace events** | `mpsc::Sender<TraceEvent>` | 100,000 | TraceCollector → Streaming file writer |
| **Monitor events** | `mpsc::Sender<MonitorEvent>` | 100 | ExecutionMonitor → Event logger |
| **Monitor stop** | `watch::Sender<bool>` | 1 | engine → ExecutionMonitor |

### Message Types (Outbound to Controller)

```
WorkerMessage.payload = oneof {
    Registration       ← Sent once on stream establishment
    Status             ← Heartbeat (30s), health check response
    ExecutionStatus    ← Monitor updates: started, heartbeat, stuck, timeout, terminated
    SampleResponse     ← Run result: exit_code, success, output, detected
    Telemetry          ← TelemetryBatch: events[], is_final=true
    Ack                ← Acknowledge RunSample command receipt
}
```

### Message Types (Inbound from Controller)

```
ControllerMessage.payload = oneof {
    RunSample          ← Execute artifact (job_id, artifact_id, timeout_seconds)
    HealthCheck        ← Request status report
    Heartbeat          ← Keep-alive with timestamp
    Disconnect         ← Graceful disconnect (reconnect_allowed?)
    Ack                ← Acknowledge worker message
    ArtifactChunks     ← (not yet implemented)
}
```

---

## 4. Execution Lifecycle

The core pipeline is `dispatch::engine::execute_run()` (~698 lines). Two entry points invoke it:

1. **Unary RPC** (`api/run.rs`): `run_sample()` acquires the lock, builds `RunRequest`/`RunContext`, calls `execute_run()`
2. **Stream command** (`session/stream_handler.rs`): `handle_run_sample()` spawns a task that does the same

Both paths share the same engine. The engine is transport-agnostic via `ControlPlaneSink`.

### Phase 1: Validate Artifact

```
    ┌─────────────────────────────────┐
    │  Check artifact_path.exists()   │
    │  (from config.storage.          │
    │   artifacts_path / {id}.exe)    │
    └─────────────┬───────────────────┘
                  │
            Path missing? → RunError::ArtifactNotFound
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
         │        │          ├── strict_mode=true → FailedPrecondition error
         │        │          └── strict_mode=false:
         │        │              set trace target → force reset → continue
         ▼        ▼
    ┌─────────────────────────────────┐
    │  start_trace([artifact.exe])     │
    │  RedEDR now watching artifact    │
    └─────────────────────────────────┘
```

### Phase 3: Prepare Environment

```
    ┌─────────────────────────────────┐
    │  infra::system::prepare_         │
    │  telemetry_dir()                 │
    │  (remove stale + create fresh)   │
    └─────────────┬───────────────────┘
                  │
    ┌─────────────┼─────────────────┐
    │             │                 │
    ▼             ▼                 ▼
  TraceCollector  Streaming       trace_handle
  (named pipe)    Writer          (JoinHandle)
  \\.\pipe\       BufWriter
  rededr_trace    256KB buffer
    │             │
    ▼             ▼
  trace_tx ─────→ trace_rx ──→ trace_events.jsonl
  (100K cap)      streaming_handle (JoinHandle)
```

### Phase 4: Spawn Process

```
    ┌─────────────────────────────────┐
    │  infra::process::spawn_artifact │
    │  (artifact_path, telemetry_dir) │
    │  ├── stdin: null                │
    │  ├── stdout: piped              │
    │  └── stderr: piped              │
    └─────────────┬───────────────────┘
                  │
                  ▼
            ProcessGuard (RAII kill)
                  │
                  ├── Get PID
                  ├── capture_stream(stdout)
                  └── capture_stream(stderr)
```

### Phase 5: Start Monitoring

```
    ┌─────────────────────────────────┐
    │  Create ExecutionMonitor         │
    │  (run_id, job_id, pid, timeout,  │
    │   sink, rededr_base_url)         │
    ├─────────────────────────────────┤
    │  Spawn: monitor.start()          │
    │  Spawn: event consumer           │
    │  Create: MonitorGuard (RAII)     │
    └─────────────────────────────────┘
```

### Phase 6: Wait for Completion or Timeout

```
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
         │        │   try_wait() race check
         │        │   ├── Exited naturally → not timeout
         │        │   └── Still running:
         │        │       infra::process::kill_process_tree()
         │        │       infra::process::is_process_alive()
         ▼        ▼
    exit_code resolution:
     0    = success
    -1    = timeout or wait() failure
    -2    = externally terminated (no exit code, likely AV/EDR kill)
    other = NTSTATUS interpretation (Windows-specific)
```

### Phase 7: Collect Telemetry

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
  collect_all  package_         collect_bb_      collect_api_
  (HTTP API)   trace_log()      coverage()       checkpoints()
    │             │              (coverage.bin    (checkpoints.log)
    │             │               +bbs.txt)        JSON lines
    │         ┌───┼───┐           │                │
    │     <=2MB   │  >2MB         │                │
    │     single  │  last 2MB     │                │
    │     event   │  + async      │                │
    │             │  compress     │                │
    ▼             ▼               ▼                ▼
    └─────────── telemetry_events[] ───────────────┘
                      │
                      + phase_timings event
```

### Phase 8: Stream Telemetry to Controller

```
    ┌─────────────────────────────────┐
    │  TelemetryBatch {                │
    │    job_id, run_id,               │
    │    events: [...],                │
    │    is_final: true                │
    │  }                               │
    │  → sink.send_telemetry()         │
    └─────────────────────────────────┘
```

### Phase 9: Cleanup

```
    ┌─────────────────────────────────┐
    │  rededr_guard.reset_now()        │
    │  (explicit reset for next run)   │
    │  ProcessGuard.disarm()           │
    │  ExecutionLockGuard drops        │
    └─────────────┬───────────────────┘
                  │
                  ▼
    Return RunOutcome {
      exit_code, timed_out, stdout, stderr,
      telemetry_events, elapsed, phase_timings
    }
```

---

## 5. State Machines

### 5.1 Execution State (`dispatch/state.rs`)

```
    IDLE ──────────────────→ RUNNING ──────────────────→ IDLE
    (ExecutionState::Idle)   (ExecutionState::Running    (ExecutionLockGuard
     acquire() succeeds       { job_id, artifact,         Drop → tokio::spawn
                                run_id })                  → state.release())

    Invariant: At most ONE artifact executing at any time.
    Enforcement: Arc<Mutex<ExecutionState>>
    Cleanup: ExecutionLockGuard Drop impl (spawns async release)
```

`ExecutionState` is an enum (not separate bool + Option fields), ensuring state consistency:

```rust
pub enum ExecutionState {
    Idle,
    Running { job_id: String, artifact: String, run_id: String },
}
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

### 5.3 Execution Monitor States (`dispatch/monitor.rs`)

```
    STARTED ──→ HEARTBEAT ──→ HEARTBEAT ──→ ...
                    │              │
                    │              │
                    ▼              ▼
               (idle 3+       (timeout-5s)
                AND cpu<5%)   APPROACHING_TIMEOUT
                TELEMETRY_IDLE
                    │              │
                    └──────┬───────┘
                           ▼
                      TERMINATED
                     (process dead)

    Poll interval: 3 seconds
    Idle: 3+ cycles with no new events AND cpu_percent <= 5%
    Timeout warning: within 5 seconds of timeout_seconds
```

### 5.4 Monitor Event Types

| Event | Condition | Severity |
|-------|-----------|----------|
| `started` | Initial event on monitor start | Info |
| `heartbeat` | Process alive, events growing or CPU active | Info |
| `telemetry_idle` | Process alive, no new events for 3+ cycles AND low CPU | Warn |
| `approaching_timeout` | Process alive, elapsed >= timeout - 5s | Warn |
| `terminated` | Process no longer alive (PID check fails) | Info |

The monitor distinguishes true idle (no events + low CPU) from busy-but-silent (no events + high CPU). The latter resets the idle counter.

---

## 6. Telemetry Collection

### 6.1 Five Telemetry Sources

```
    ┌───────────────────────────────────────────────────────────────┐
    │                      Artifact Execution                       │
    │                                                               │
    │  ┌─────────┐  ┌─────────┐  ┌────────┐  ┌──────────────────┐ │
    │  │ stdout  │  │ stderr  │  │coverage│  │ checkpoints.log  │ │
    │  │ (piped) │  │ (piped) │  │.bin    │  │ (JSON lines)     │ │
    │  └────┬────┘  └────┬────┘  │+bbs.txt│  │ checkpoints +    │ │
    │       │            │       └───┬────┘  │ status events    │ │
    │       │            │           │       └────────┬─────────┘ │
    └───────┼────────────┼───────────┼────────────────┼───────────┘
            │            │           │                │
            ▼            ▼           ▼                ▼
    ┌───────────┐  ┌──────────┐  ┌────────┐  ┌──────────────────┐
    │  stdout   │  │  stderr  │  │   BB   │  │   checkpoint     │
    │  capture  │  │  capture │  │coverage│  │   parser         │
    │  (async)  │  │  (async) │  │parser  │  │ (reads file)     │
    └───────────┘  └──────────┘  └────────┘  └──────────────────┘
            │            │           │                │
    ┌───────┼────────────┼───────────┼────────────────┼──────┐
    │       ▼            ▼           ▼                ▼      │
    │         ┌─ telemetry_events: Vec<TelemetryData> ─────┐ │
    │         │                                            │ │
    │         │  + RedEDR events (HTTP /api/logs/rededr)   │ │
    │         │  + trace_log (named pipe → JSONL)          │ │
    │         │  + trace.log (binary protocol fallback)    │ │
    │         │  + coverage (typed CoverageEvent)          │ │
    │         │  + checkpoints (typed CheckpointEvent)     │ │
    │         │  + artifact status (success/failure)       │ │
    │         │  + phase_timings (observability event)     │ │
    │         └────────────────────────────────────────────┘ │
    │                                                        │
    │  Named Pipes (artifact → worker):                      │
    │  ┌──────────────────────────────────────────────────┐  │
    │  │ \\.\pipe\rededr_trace     (worker runs server)   │  │
    │  │   → line traces (binary ISTR / Base64)           │  │
    │  │   → streams to trace_events.jsonl via channel    │  │
    │  │                                                  │  │
    │  │ \\.\pipe\rededr_checkpoints (NO server)          │  │
    │  │   → artifact tries pipe, fails, falls back to    │  │
    │  │     checkpoints.log file in telemetry_dir (CWD)  │  │
    │  │   → worker reads file after execution completes  │  │
    │  └──────────────────────────────────────────────────┘  │
    │                                                        │
    │  ┌──────────────────────────────────────────────────┐  │
    │  │ RedEDR HTTP API                                  │  │
    │  │   GET  /api/logs/rededr  (ETW/kernel events)     │  │
    │  │   GET  /api/stats        (event count)           │  │
    │  │   POST /api/trace/start|reset                    │  │
    │  └──────────────────────────────────────────────────┘  │
    └────────────────────────────────────────────────────────┘
```

### 6.2 RedEDR Collector (`telemetry/collectors/rededr.rs`)

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
├── POST /api/lock/acquire   → (defined, not yet used)
└── POST /api/lock/release   → (defined, not yet used)

Event Transform:
  RedEdrEvent {date, type, trace_id, target, func, pid, tid, provider, ...}
      ↓
  TelemetryData {job_id, event_type, timestamp, payload: JSON bytes, metadata}
```

Two collection modes:
- **Polling** (`start()`): Continuous polling loop with dedup via `seen_trace_ids` — sends events to mpsc channel
- **Batch** (`collect_all()`): Single fetch of all events — used by the engine after execution completes

### 6.3 Trace Collector (`telemetry/collectors/trace.rs`)

```
TraceCollector
├── pipe_name: "\\.\pipe\rededr_trace"
├── event_tx: mpsc::Sender<TraceEvent>  (capacity 100,000)
└── sequence_counter: Arc<AtomicU32>

Protocol Auto-Detection (first 4 bytes):
├── 0x49535452 ('ISTR') → Binary protocol
│   InstRecordHeader (32 bytes, packed):
│   ├── magic: u32 (0x49535452)
│   ├── version: u16
│   ├── event_type: u16 (1=line_trace; 2-4 now use checkpoint pipe, warned if seen here)
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

**Streaming writer** receives events from the TraceCollector via the 100K-capacity mpsc channel, writes JSONL with 256KB buffered I/O, and elides `thread_id` when unchanged (compression optimization).

### 6.4 Telemetry Pipeline (`telemetry/pipeline.rs`)

Two-phase approach for trace logs:

```
    Trace file size check
         │
    ┌────┼────────────┐
    │                  │
  <= 2MB             > 2MB
    │                  │
    ├── <= 4MB:       ├── Immediate: last 2MB (complete JSONL lines)
    │   send whole    │   → "trace_log" event in main batch
    │                  │
    └── > 4MB:        └── Async: full trace compression (tokio::spawn)
        truncate          ├── compress_trace_log() fits in 4MB → send
        tail              ├── + gzip fits → send as base64
                          └── Still too big → first/last 100 lines
```

Binary protocol trace.log (`collect_trace_log_binary()`): Parses the ISTR binary format directly from file, extracting line traces only. Artifact status events (types 2-4) are now expected in `checkpoints.log` and are warned+ignored if found in trace.log.

### 6.5 Trace Compression (`telemetry/trace_compressor.rs`)

Three-stage pipeline (experimental, marked "NOT WORKING" in source):

```
    Raw JSONL trace
         │
    Stage 1: CLP-inspired Columnar Decomposition
    ├── Extract line_sequence: Vec<u32>  (dense integer array)
    ├── file_dict, func_dict             (deduplicated)
    └── file_indices, func_indices       (index arrays)
         │
    Stage 2: Matrix Profile Pattern Detection
    ├── Sliding window: min=2, max=50
    ├── Find patterns with >= N occurrences
    └── Sort by compression benefit (occurrences * length)
         │
    Stage 3: Sequitur-like Grammar Induction
    ├── Convert top motifs to grammar rules (non-overlapping greedy)
    └── Output: "RULE_0 (used 15 times): L10 L11 L12"
                "@RULE_0 @RULE_0 L50 @RULE_0"
```

Also provides `gzip_compress()` for when grammar compression is insufficient.

---

## 7. RAII Guard System

All guards use `Drop` implementations for cleanup on any exit path (success, error, panic).

### 7.1 Guard Hierarchy

```
    execute_run() scope
    ├── RedEdrGuard          ← Resets RedEDR HTTP API on drop
    ├── ProcessGuard         ← Kills child process on drop
    └── MonitorGuard         ← Stops monitor + event consumer on drop

    Caller scope (api/run.rs or stream_handler.rs)
    └── ExecutionLockGuard   ← Releases execution_lock on drop
```

### 7.2 Guard Details

| Guard | Protects | Normal Exit | Drop Exit (error/panic path) |
|-------|----------|-------------|------------------------------|
| `ExecutionLockGuard` | `Arc<Mutex<ExecutionState>>` | Implicit drop | `tokio::spawn` → `state.release()` |
| `RedEdrGuard` | RedEDR HTTP state | `reset_now()` (explicit, prevents double reset) | `Handle::try_current()` → spawn cleanup on existing runtime |
| `ProcessGuard` | `tokio::process::Child` | `disarm()` (takes child ownership) | `child.start_kill()` (synchronous kill signal) |
| `MonitorGuard` | monitor task + event consumer | `stop()` (graceful: signal + wait) | Send stop signal + abort consumer |

### 7.3 Drop Cleanup Patterns

**ProcessGuard** uses synchronous `start_kill()` which sends the kill signal without needing an async runtime:

```rust
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if self.should_kill {
            if let Some(ref mut child) = self.child {
                if let Err(e) = child.start_kill() { /* log */ }
            }
        }
    }
}
```

**RedEdrGuard** uses the existing tokio runtime via `Handle::try_current()` instead of creating a new one:

```rust
impl Drop for RedEdrGuard {
    fn drop(&mut self) {
        if self.reset_on_drop {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move { /* POST /api/trace/reset */ });
            } else {
                eprintln!("WARNING: RedEDR may be contaminated for the next run");
            }
        }
    }
}
```

**MonitorGuard** is already synchronous — sends stop signal and aborts the consumer task:

```rust
impl Drop for MonitorGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() { let _ = tx.send(true); }
        if let Some(consumer) = self.event_consumer.take() { consumer.abort(); }
    }
}
```

---

## 8. Control Plane Sink (`dispatch/sink.rs`)

The execution engine is transport-agnostic. It sends status updates and telemetry through the `ControlPlaneSink` trait rather than holding a reference to `StreamHandler`.

```rust
#[tonic::async_trait]
pub trait ControlPlaneSink: Send + Sync {
    async fn send_status(&self, status: ExecutionStatusReport) -> Result<()>;
    async fn send_telemetry(&self, batch: TelemetryBatch) -> Result<()>;
    async fn send_ack(&self, request_id: &str, success: bool, error: &str) -> Result<()>;
}
```

### Implementations

| Impl | When used | Behavior |
|------|-----------|----------|
| `StreamSink` | Stream is active (bidirectional mode) | Wraps `mpsc::Sender<WorkerMessage>` — sends to controller |
| `NullSink` | No stream available (standalone mode) | No-op — all sends succeed silently |

### Factory

```rust
pub fn build_sink(
    tx: Option<&mpsc::Sender<Result<WorkerMessage, Status>>>,
) -> Arc<dyn ControlPlaneSink>
```

Called by both `api/run.rs` (extracts tx from stream_handler) and `session/stream_handler.rs` (passes tx directly).

---

## 9. Struct Reference

### Core Structs

```rust
// lib.rs
#[derive(Clone)]
pub struct WorkerAgentService {
    pub(crate) worker_id: String,
    pub(crate) config: WorkerConfig,
    pub(crate) system_info: Arc<Mutex<System>>,
    pub(crate) execution_lock: Arc<Mutex<ExecutionState>>,
    pub(crate) stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>,
}
```

### Execution Domain Types (`dispatch/types.rs`)

```rust
pub struct RunRequest {
    pub job_id: String,
    pub artifact_id: String,
    pub timeout_seconds: u32,
    pub run_id: String,                   // From controller request_id or UUID
}

pub struct RunContext {
    pub worker_id: String,
    pub config: WorkerConfig,
    pub telemetry_dir: PathBuf,           // Derived from config.storage.artifacts_path
    pub artifact_path: PathBuf,           // Derived from config.storage.artifacts_path
    pub artifact_name: String,
}

pub struct RunOutcome {
    pub exit_code: i32,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub telemetry_events: Vec<TelemetryData>,
    pub elapsed: Duration,
    pub phase_timings: RunPhaseTimings,
}

#[derive(Debug, Default)]
pub struct RunPhaseTimings {
    pub rededr_setup_ms: u64,
    pub process_spawn_ms: u64,
    pub process_wait_ms: u64,
    pub telemetry_collect_ms: u64,
    pub rededr_reset_ms: u64,
}
```

### Execution State (`dispatch/state.rs`)

```rust
#[derive(Debug, Clone)]
pub enum ExecutionState {
    Idle,
    Running { job_id: String, artifact: String, run_id: String },
}

pub struct ExecutionBusyError {
    pub current_job_id: String,
    pub current_artifact: String,
}

pub struct ExecutionLockGuard {
    lock: Arc<Mutex<ExecutionState>>,
}
```

### Stream & Communication

```rust
// session/stream_handler.rs
pub struct StreamHandler {
    pub worker_state: Arc<RwLock<WorkerState>>,
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,
    worker_id: String,                    // Cloned, not Arc ref
    config: WorkerConfig,                 // Cloned, not Arc ref
    execution_lock: Arc<Mutex<ExecutionState>>,
}
```

### Capabilities & State

```rust
// capabilities.rs
pub struct WorkerCapabilities {
    pub capabilities: Vec<String>,         // ["rededr", "mde", "cortex"]
    pub tools: HashMap<String, String>,    // {"rededr_version": "1.2.3"}
    pub metadata: HashMap<String, String>, // {"hostname", "cpu_cores", "os_key"}
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

// session/worker_state.rs
pub struct WorkerState {
    pub worker_id: String,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub tools: Option<ToolVersions>,
    pub health: HealthMetrics,
    pub current_job_id: Option<String>,
    pub current_run_id: Option<String>,
    pub last_controller_heartbeat: Option<i64>,
    pub controller_disconnected: bool,
    pub disconnect_reason: Option<String>,
    pub reconnect_allowed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct HealthMetrics {
    pub cpu_percent: i32,
    pub memory_percent: i32,
    pub disk_percent: i32,
    pub active_jobs: i32,
    pub uptime_seconds: i64,
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
    client: reqwest::Client,              // 5s timeout
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
    pipe_name: String,                     // "\\.\pipe\rededr_trace"
    event_tx: mpsc::Sender<TraceEvent>,    // 100K capacity
    sequence_counter: Arc<AtomicU32>,
}

#[repr(C, packed)]
struct InstRecordHeader {                  // 32 bytes
    magic: u32,                            // 0x49535452
    version: u16,
    event_type: u16,                       // 1=line, 2=checkpoint, 3=success, 4=failure
    thread_id: u32,
    seq_no: u64,
    ts_us: u64,
    payload_len: u32,
}
```

### Execution Monitor

```rust
// dispatch/monitor.rs
pub struct ExecutionMonitor {
    pub run_id: String,
    pub job_id: String,
    pub worker_id: String,
    pub worker_ip: String,
    pub artifact_name: String,
    pub pid: u32,
    pub rededr_base_url: String,
    pub sink: Arc<dyn ControlPlaneSink>,   // Stream or Null
    pub start_time: Instant,
    pub timeout_seconds: i32,
    client: reqwest::Client,              // 3s timeout for /api/stats
    sys: Arc<Mutex<sysinfo::System>>,     // Per-PID process metrics
}
```

### RAII Guards

```rust
// dispatch/guards.rs
pub struct RedEdrGuard {
    collector: RedEdrCollector,
    reset_on_drop: bool,
}

pub struct MonitorGuard {
    stop_tx: Option<watch::Sender<bool>>,
    handle: Option<JoinHandle<()>>,
    event_consumer: Option<JoinHandle<()>>,
}

pub struct ProcessGuard {
    child: Option<tokio::process::Child>,
    should_kill: bool,
}
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

### Infrastructure (`infra/`)

```rust
// infra/process.rs
pub fn spawn_artifact(artifact_path: &Path, working_dir: &Path) -> io::Result<Child>;
pub async fn kill_process_tree(child: &mut Child, pid: Option<u32>);
pub fn is_process_alive(pid: u32) -> bool;           // Windows: OpenProcess, other: false
pub fn capture_stream<R: AsyncRead>(stream: Option<R>) -> JoinHandle<String>;

// infra/system.rs
pub fn prepare_telemetry_dir(dir: &Path) -> io::Result<()>;

// infra/helpers.rs
pub fn extract_filename(path: &str) -> String;
pub async fn collect_bb_coverage(...) -> Result<TelemetryData>;
pub async fn collect_api_checkpoints(...) -> Result<Vec<TelemetryData>>;
```

---

## 10. Concurrency Model

### Thread/Task Breakdown for One Execution

```
    Tokio Runtime (main)
    │
    ├── gRPC Server task (tonic)
    │   └── WorkerAgent trait handlers (api/mod.rs dispatch)
    │
    ├── Stream handler task (spawned per EstablishStream)
    │   └── handle_stream() message loop
    │
    ├── Heartbeat task (spawned per EstablishStream)
    │   └── heartbeat_loop() every 30s
    │
    └── Per-execution tasks (spawned in engine):
        ├── trace_handle        ← TraceCollector.start_server() (named pipe)
        ├── streaming_handle    ← BufWriter to trace_events.jsonl
        ├── stdout_handle       ← capture stdout → String
        ├── stderr_handle       ← capture stderr → String
        ├── monitor_handle      ← ExecutionMonitor.start() (3s poll loop)
        └── event_consumer      ← Log monitor events
```

### Synchronization Primitives

| Primitive | Location | Protects |
|-----------|----------|----------|
| `Arc<Mutex<ExecutionState>>` | WorkerAgentService + StreamHandler | One-at-a-time execution |
| `Arc<Mutex<System>>` | WorkerAgentService | sysinfo refresh (health check) |
| `Arc<RwLock<Option<Arc<StreamHandler>>>>` | WorkerAgentService | Lazy stream handler init |
| `Arc<RwLock<WorkerState>>` | StreamHandler | Mutable runtime state |
| `Arc<Mutex<sysinfo::System>>` | ExecutionMonitor | Per-PID process metrics |
| `AtomicU32` | TraceCollector | Sequence counter for text traces |
| `watch::Sender<bool>` | MonitorGuard | Stop signal for monitor |

### Critical Section: Execution Lock

```
    Request arrives (unary or stream)
    └── execution_lock.lock().await
        ├── Running{..} → reject (RESOURCE_EXHAUSTED)
        └── Idle → acquire
            Set: Running { job_id, artifact, run_id }
            Create: ExecutionLockGuard
            ... entire execution ...
            Drop: ExecutionLockGuard → tokio::spawn → state.release()
```

**Why single execution?** RedEDR captures kernel-level ETW events for a target process. Running multiple artifacts simultaneously would cause telemetry cross-contamination — events from one artifact attributed to another.

---

## 11. Capability Detection (`capabilities.rs`)

### Detection Methods

```
    detect_capabilities() ─────────────────────────────────────┐
    │                                                          │
    ├── RedEDR: HTTP GET localhost:8081/api/stats               │
    │   └── Version: GET /api/logs/agent, regex "RedEdr (\d+)" │
    │                                                          │
    ├── Defender: sc query WinDefend → "RUNNING"                │
    │   └── Version: PowerShell Get-MpComputerStatus            │
    │   └── (NOT added to capabilities list, only tools)       │
    │                                                          │
    ├── MDE: Registry HKLM\SOFTWARE\Microsoft\                  │
    │        Windows Advanced Threat Protection\OnboardedInfo    │
    │                                                          │
    ├── Cortex XDR:                                             │
    │   ├── Registry CyveraService                              │
    │   └── OR filesystem: C:\ProgramData\Cyvera               │
    │   └── OR filesystem: C:\Program Files\Palo Alto\Traps    │
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
| `rededr` | HTTP health check to localhost:8081 | Required for ETW telemetry |
| `mde` | Registry key present + non-empty OnboardedInfo | Microsoft Defender for Endpoint |
| `cortex` | Registry service OR filesystem footprint | Palo Alto Cortex XDR |
| (defender) | Service running | Detected but NOT added to capabilities list |

### OS Key Format

```
os_key = "win{10|11}-build-{build_number}"
```

- Build >= 22000 = Windows 11
- Build < 22000 = Windows 10
- Used by controller for capability matching

---

## 12. Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Single execution lock** | RedEDR ETW capture is per-machine; concurrent artifacts contaminate telemetry |
| **RAII guards for all resources** | Artifact execution can fail at any point; guards ensure cleanup |
| **No `Arc<WorkerAgentService>` in StreamHandler** | Breaks reference cycle; StreamHandler clones individual fields instead |
| **`ControlPlaneSink` trait** | Decouples engine from transport; same code works with stream or standalone |
| **`start_kill()` in ProcessGuard Drop** | Synchronous kill signal — no runtime needed in Drop path |
| **`Handle::try_current()` in RedEdrGuard Drop** | Reuses existing tokio runtime instead of creating a new one; fallback logs warning |
| **`ExecutionState` as enum** | Prevents desync between `busy` flag and job metadata; Idle vs Running{..} |
| **Named pipe auto-detection** | Support both old IR (Base64) and new AST (binary ISTR) trace formats |
| **100K trace channel** | Line-level tracing in tight loops generates very high event rates |
| **Streaming writer with 256KB buffer** | Reduce I/O syscalls during high-frequency tracing |
| **Thread-ID elision** | Only write thread_id to JSONL when it changes (compression optimization) |
| **Two-phase trace sending** | Immediate last-2MB for quick analysis + async compressed full trace |
| **Sanity check before each run** | Detect RedEDR contamination from previous run's incomplete cleanup |
| **30s heartbeat loop** | Keep stream alive, auto-detect controller reconnection |
| **`try_wait()` on timeout** | Handle race condition: process exits exactly as timeout fires |
| **NTSTATUS interpretation** | Windows exit codes are often NTSTATUS (0xC0000005 = ACCESS_VIOLATION) |
| **Artifacts path from config** | `config.storage.artifacts_path` (default `C:\AutoMutate\artifacts`) used everywhere |
| **Process ops in `infra/process.rs`** | Windows-specific `#[cfg]` code isolated from engine orchestration |
| **Stream handler lazy init** | Worker can run without controller stream (standalone mode) |
| **Artifact-specific telemetry dirs** | `telemetry_{artifact_id}/` prevents file collision between runs |
| **Monitor CPU-aware idle detection** | High CPU + no events = busy (not idle); prevents false stuck warnings |

---

## Appendix A: RPC Interface

The `WorkerAgent` gRPC service (defined in `api/mod.rs`):

| RPC | Handler | Direction | Description |
|-----|---------|-----------|-------------|
| `Ping` | `api::info::ping` | Unary | Liveness check |
| `RunSample` | `api::run::run_sample` | Unary | Execute artifact (legacy, see stream) |
| `HealthCheck` | `api::info::health_check` | Unary | CPU/mem/busy status |
| `SendArtifact` | `api::artifacts::send_artifact` | Client streaming | Chunked binary + SHA256 verify |
| `GetWorkerInfo` | `api::info::get_worker_info` | Unary | Capabilities, tools, metadata |
| `GetTelemetry` | `api::info::get_telemetry` | Server streaming | Pull telemetry on demand |
| `EstablishStream` | `api::stream::establish_stream` | Bidirectional | Real-time controller <-> worker |

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
    │                                    │  Write to {artifacts_path}\{id}.exe
    │  TransferAck {received, path}      │
    │ ←──────────────────────────────────│
```

---

## Appendix B: Configuration

The worker reads `WorkerConfig` from TOML (typically `C:\AutoMutate\worker.toml`).

Key fields used by the agent:

| Config Path | Used In | Purpose |
|------------|---------|---------|
| `worker.worker_id` | lib.rs, stream_handler | Worker identity |
| `worker.ip_address` | stream, monitor | Registration, status reports |
| `worker.listen_port` | main.rs | gRPC server bind address |
| `worker.os_version` | stream, info | Registration metadata |
| `storage.artifacts_path` | api/run, api/artifacts, stream_handler | Base path for artifact storage + telemetry dirs |
| `telemetry.rededr.base_url` | engine, monitor | RedEDR HTTP API endpoint |
| `telemetry.rededr.strict_contamination_check` | engine | Fail on leftover events vs. force-reset |
| `logging.level` | main.rs | Log verbosity (TRACE/DEBUG/INFO/WARN/ERROR) |

---

## Appendix C: Test Coverage

| Test File | Tests | What's tested |
|-----------|------:|---------------|
| `telemetry/collectors/trace.rs` | 3 | Base64 text protocol parsing (IR + AST formats) |
| `telemetry/collectors/rededr.rs` | 1 | Event transform (RedEdrEvent → TelemetryData) |
| `telemetry/trace_compressor.rs` | 3 | Columnar decomposition, pattern detection, full pipeline |
| `dispatch/monitor.rs` | 1 | Monitor construction with NullSink |
| `tests/test_trace_pipe.rs` | 1 | End-to-end named pipe: artifact writes Base64 → collector parses → channel receives |
| **Total** | **9** | |

All tests pass on Windows. The named pipe integration test uses Win32 `CreateFileA`/`WriteFile` to simulate an artifact connecting to the trace pipe.

---

## Appendix D: Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `automutate-config` | workspace | Shared TOML configuration |
| `tokio` | workspace | Async runtime |
| `tonic` / `prost` | workspace | gRPC framework + protobuf |
| `reqwest` | 0.13 | HTTP client for RedEDR API |
| `serde` / `serde_json` | workspace | JSON serialization |
| `chrono` | 0.4 | Timestamps |
| `sysinfo` | 0.37 | CPU/memory metrics, per-PID monitoring |
| `sha2` | 0.10 | Artifact integrity verification |
| `base64` | 0.22 | Trace protocol encoding |
| `flate2` | 1.1 | Gzip compression for large traces |
| `uuid` | 1.19 | Run ID generation |
| `regex` | 1.12 | RedEDR version parsing |
| `windows` | 0.62 | Win32 API (process, file, threading) |
| `winreg` | 0.55 | Registry access (MDE, Cortex, OS info) |