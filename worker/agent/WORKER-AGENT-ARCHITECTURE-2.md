# Worker Agent — Architecture & Goal

Comprehensive architecture document for `worker/agent/` — the Windows VM execution daemon of the AutoMutate++ system. Synthesized from the 5 submodule deep analyses (`api/`, `execution/`, `infra/`, `session/`, `telemetry/`) and the 4 root source files (`lib.rs`, `main.rs`, `capabilities.rs`, `constants.rs`).

---

## 1. Goal

The worker agent is the **remote execution and observation daemon** that runs on each Windows VM in the AutoMutate++ pool. Its single purpose is:

> **Receive a compiled PE artifact, execute it under full behavioral monitoring, classify whether it was detected by AV/EDR, collect all telemetry (ETW events, line traces, coverage, checkpoints), and return the complete result to the controller.**

This makes it the system's **observation boundary** — the point where mutations meet reality. Everything upstream (mutation selection, artifact building, job scheduling) produces artifacts; everything downstream (triage tokens, differential analysis, feedback loop) consumes the telemetry that this agent collects.

### What It Does

1. **Self-describes** — detects installed security tools (RedEDR, Defender, MDE, Cortex XDR), OS version, and hardware specs at startup; reports capabilities to the controller on connection
2. **Receives artifacts** — accepts cross-compiled PE binaries via chunked gRPC transfer with SHA-256 integrity verification
3. **Executes under monitoring** — spawns the artifact as a child process with RedEDR ETW tracing and line-level named pipe instrumentation running concurrently
4. **Classifies outcomes** — determines whether the artifact was detected, evaded, crashed, stalled, or hit an infrastructure error using a 7-verdict decision tree
5. **Collects telemetry** — gathers 6 telemetry sources (RedEDR events, trace JSONL, binary trace log, BB coverage, API checkpoints, execution metrics) and packages them as protobuf messages
6. **Reports results** — streams telemetry and status updates back to the controller via gRPC (unary RPCs or bidirectional stream)

### What It Does NOT Do

- No mutation logic — receives pre-built artifacts
- No triage or token scoring — sends raw telemetry to the controller
- No job scheduling — executes one artifact at a time when told to
- No persistence — stateless between runs (all state is ephemeral)

---

## 2. Position in the Global Pipeline

```
┌────────────────────────────────────────────────────────────────────────┐
│                         AutoMutate++ Pipeline                          │
│                                                                        │
│  ┌──────────┐    ┌──────────┐    ┌──────────────────────────────────┐ │
│  │ Selector  │    │  Build   │    │       Worker Agent (this crate)  │ │
│  │ picks     │───►│ compiles │───►│                                  │ │
│  │ mutations │    │ artifact │    │  Execute → Monitor → Classify    │ │
│  └──────────┘    └──────────┘    │  → Collect telemetry → Return    │ │
│       ▲                          └──────────────┬───────────────────┘ │
│       │                                         │                      │
│       │  feedback                               │  RunOutcome +        │
│       │  (avoid/seek tokens)                    │  TelemetryData[]     │
│       │                                         ▼                      │
│  ┌────┴─────┐                          ┌──────────────┐               │
│  │  Triage  │◄─────────────────────────│  Controller  │               │
│  │  Engine  │   tokens, differentials  │  (storage +  │               │
│  │          │                          │   analysis)  │               │
│  └──────────┘                          └──────────────┘               │
└────────────────────────────────────────────────────────────────────────┘
```

The agent sits between **build** (produces the PE) and **triage** (consumes telemetry). Its output — `RunOutcome` containing exit code, detection verdict, and `Vec<TelemetryData>` — is the raw material for:

- **Differential analysis:** comparing Run A (instrumented) vs Run B (uninstrumented) to distinguish real detections from instrumentation artifacts
- **Token extraction:** converting ETW events and traces into normalized triage tokens (`api:VirtualProtect`, `seq3:alloc→write→thread`, `trunc:loader.c:143`)
- **Mutation feedback:** identifying which behaviors (tokens) correlate with detection, guiding future mutation selection

---

## 3. Root Files — Crate Skeleton

### 3.1 `lib.rs` — Crate Definition (85 lines)

The crate's public interface and central struct definition.

**Proto code generation:**

```rust
pub mod automutate {
    pub mod common    { tonic::include_proto!("automutate.common"); }
    pub mod controller { tonic::include_proto!("automutate.controller"); }
    pub mod worker     { tonic::include_proto!("automutate.worker"); }
}
```

Includes all 3 proto packages — `common` for shared types, `controller` for controller-side RPCs (used as client), `worker` for the agent's own service definition.

**Module declarations:**

```
pub mod api;           // gRPC handlers (thin adapters)
pub mod capabilities;  // Startup self-detection
pub mod constants;     // Tuning parameters
pub mod execution;     // Core execution engine
pub mod infra;         // OS-level utilities
pub mod session;       // Bidirectional stream
pub mod telemetry;     // Data collection
```

**`WorkerAgentService` — The Central Struct:**

```rust
pub struct WorkerAgentService {
    worker_id: String,
    config: WorkerConfig,
    system_info: Arc<Mutex<System>>,
    execution_lock: Arc<Mutex<ExecutionState>>,
    stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>,
    heartbeat_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    capabilities: Arc<WorkerCapabilities>,
}
```

| Field | Type | Purpose | Mutability |
|-------|------|---------|------------|
| `worker_id` | `String` | Unique identity (e.g., `"win10-worker-01"`) | Immutable |
| `config` | `WorkerConfig` | TOML configuration (paths, ports, logging) | Immutable (Clone) |
| `system_info` | `Arc<Mutex<System>>` | Shared sysinfo object for CPU/memory metrics | Interior mutable |
| `execution_lock` | `Arc<Mutex<ExecutionState>>` | Single-execution guarantee — `Idle` or `Running{job,artifact,run}` | Interior mutable |
| `stream_handler` | `Arc<RwLock<Option<Arc<StreamHandler>>>>` | Active bidirectional stream session (or `None`) | Interior mutable |
| `heartbeat_handle` | `Arc<RwLock<Option<JoinHandle<()>>>>` | Background heartbeat task handle (aborted on reconnect) | Interior mutable |
| `capabilities` | `Arc<WorkerCapabilities>` | Detected tools + metadata (cached at startup) | Immutable |

**Design choice — `#[derive(Clone)]`:** The struct is `Clone` because tonic requires the service type to be cloneable (it clones the service for each incoming connection). All mutable state is behind `Arc<Mutex/RwLock>`, so clones share the same underlying state.

**Utility method — `truncate_middle_output()`:** Truncates long stdout/stderr to first 400 + last 400 chars. Prevents gRPC message bloat from verbose artifact output.

### 3.2 `main.rs` — Startup Sequence (97 lines)

The binary entry point. Performs a linear startup sequence:

```
1. Load WorkerConfig from TOML
   └── C:\AutoMutate\worker.toml (or hostname-specific path)
   └── On failure: print solutions and exit(1)

2. Initialize tracing/logging
   └── Per-crate log level suppression from config

3. Extract worker identity
   └── worker_id, listen port (env WORKER_PORT overrides config)

4. Detect capabilities (async)
   └── capabilities::detect_capabilities()
   └── Merge extra_capabilities from config (e.g., "dryrun" for clean VMs)

5. Register Ctrl+C handler
   └── tokio::spawn → process::exit(0)

6. Create WorkerAgentService
   └── new(worker_id, config, capabilities)

7. Start gRPC server
   └── tonic::Server::builder()
       .add_service(WorkerAgentServer::new(worker_service))
       .serve("0.0.0.0:{port}")
```

**No controller connection on startup:** The agent is a gRPC server, not a client. It waits passively for the controller to connect. The controller discovers workers through configuration (not service discovery).

**Config-driven capabilities:** The `extra_capabilities` list in the config file allows operators to tag workers with capabilities that can't be auto-detected (e.g., `"dryrun"` for clean VMs without AV).

### 3.3 `capabilities.rs` — Startup Self-Detection (330 lines)

Probes the Windows environment to discover installed security tools, OS version, and hardware.

**`WorkerCapabilities` struct:**

| Field | Type | Content |
|-------|------|---------|
| `capabilities` | `Vec<String>` | Feature tags: `"rededr"`, `"mde"`, `"cortex"` |
| `tools` | `HashMap<String, String>` | Tool versions: `rededr_version`, `defender_version` |
| `metadata` | `HashMap<String, String>` | System info: `hostname`, `cpu_cores`, `ram_gb`, `os_key`, `os_build` |

**Detection methods:**

| Target | Method | Signal |
|--------|--------|--------|
| RedEDR | HTTP GET `localhost:8081/api/stats` | 200 OK → present |
| RedEDR version | HTTP GET `localhost:8081/api/logs/agent` | Regex `RedEdr\s+(\d+\.\d+)` |
| Windows Defender | `sc query WinDefend` | Output contains `RUNNING` |
| Defender version | PowerShell `(Get-MpComputerStatus).AMProductVersion` | Version string |
| MDE (Microsoft Defender for Endpoint) | Registry `HKLM\SOFTWARE\Microsoft\Windows Advanced Threat Protection\OnboardedInfo` | Non-empty binary value |
| Cortex XDR | Registry `HKLM\SYSTEM\CurrentControlSet\Services\CyveraService` OR `C:\ProgramData\Cyvera` exists | Service registered or data directory present |
| Windows version | Registry `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` | Build ≥ 22000 → Win11 |
| Hardware | `sysinfo` crate + `available_parallelism()` | CPU cores, RAM GB |

**Platform portability:** All detection functions have `#[cfg(not(windows))]` stubs that return `false` / `None`, allowing compilation on Linux/macOS for development.

**`to_tool_versions()`:** Converts the `tools` HashMap into the protobuf `ToolVersions` message for gRPC registration.

### 3.4 `constants.rs` — Tuning Parameters (17 lines)

Six constants that control execution behavior:

| Constant | Value | Used By | Purpose |
|----------|-------|---------|---------|
| `CLEANUP_TIMEOUT_SECS` | 10 | `guards.rs` | Max wait for monitor shutdown in Drop |
| `MONITOR_POLL_INTERVAL_SECS` | 3 | `monitor.rs` | How often to check process alive + query RedEDR stats |
| `CPU_IDLE_THRESHOLD` | 5% | `monitor.rs` | Below this = process idle (no work being done) |
| `IDLE_COUNT_THRESHOLD` | 3 | `monitor.rs` | Consecutive idle polls before `telemetry_idle` flag |
| `TIMEOUT_APPROACH_SECS` | 5 | `monitor.rs` | Seconds before timeout to start warning |
| `MAX_SERIALIZED_PAYLOAD` | 3.5MB | `pipeline.rs` | Max telemetry payload size (gRPC default limit is 4MB) |

---

## 4. Module Architecture

### 4.1 Layered Design

The crate follows a strict layered architecture where each layer only depends on layers below it:

```
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 0: main.rs                                                    │
│  Startup, config loading, capability detection, gRPC server launch   │
└──────────────────────────────┬──────────────────────────────────────┘
                               │ creates
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 1: lib.rs — WorkerAgentService                                │
│  Central struct, shared state (execution_lock, stream_handler, etc.) │
└──────────────────────────────┬──────────────────────────────────────┘
                               │ dispatches to
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 2: api/                                                       │
│  gRPC thin adapters — 7 RPCs of WorkerAgent service                  │
│  mod.rs → run.rs, artifacts.rs, info.rs, stream.rs                   │
│                                                                      │
│  558 lines │ 7 functions │ Zero business logic                       │
└──────────────────────────────┬──────────────────────────────────────┘
                               │ delegates to
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 3: Domain Logic                                               │
│                                                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐  │
│  │  execution/   │  │  session/    │  │  telemetry/              │  │
│  │               │  │              │  │                          │  │
│  │  engine.rs    │  │  stream_     │  │  collectors/rededr.rs    │  │
│  │  classifier   │  │  handler.rs  │  │  collectors/trace.rs     │  │
│  │  guards.rs    │  │  worker_     │  │  pipeline.rs             │  │
│  │  monitor.rs   │  │  state.rs    │  │  trace_compressor.rs     │  │
│  │  sink.rs      │  │              │  │  (not integrated)        │  │
│  │  state.rs     │  │              │  │                          │  │
│  │  types.rs     │  │              │  │                          │  │
│  │               │  │              │  │                          │  │
│  │  2272 lines   │  │  495 lines   │  │  1935 lines              │  │
│  └──────┬───────┘  └──────┬───────┘  └──────────┬───────────────┘  │
│         │                 │                      │                   │
└─────────┼─────────────────┼──────────────────────┼───────────────────┘
          │                 │                      │
          ▼                 ▼                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 4: infra/                                                     │
│  OS boundary — process management, filesystem, metrics, time         │
│  process.rs, system.rs, time.rs                                      │
│                                                                      │
│  137 lines │ 8 functions │ Zero business logic                       │
└─────────────────────────────────────────────────────────────────────┘
          │
          ▼
    Windows API, filesystem, sysinfo, chrono
```

### 4.2 Module Responsibilities

| Module | Lines | Role | Analogy |
|--------|-------|------|---------|
| `api/` | 558 | gRPC request routing — thin adapters | HTTP controller in web frameworks |
| `execution/` | 2272 | Core execution pipeline — the "brain" | Service layer / business logic |
| `session/` | 495 | Bidirectional stream lifecycle — persistent channel | WebSocket session manager |
| `telemetry/` | 1935 | Data collection from 6 sources — the "senses" | Data ingestion pipeline |
| `infra/` | 137 | OS abstraction — process, filesystem, metrics | Repository / infrastructure layer |
| `capabilities.rs` | 330 | Startup environment detection | Health check / feature flags |
| `constants.rs` | 17 | Tuning parameters | Configuration constants |
| `lib.rs` | 85 | Crate definition + central struct | Application context |
| `main.rs` | 97 | Binary entry point | Main function |

**Total crate size: ~5,926 lines of Rust** (excluding tests).

### 4.3 Inter-Module Dependency Graph

```
main.rs
  │
  ├──► capabilities.rs ──► reqwest, winreg, sysinfo
  │
  └──► lib.rs (WorkerAgentService)
        │
        ├──► api/mod.rs ──► api/run.rs ────────────────┐
        │                   api/artifacts.rs             │
        │                   api/info.rs ──► infra/      │
        │                   api/stream.rs ──► session/  │
        │                                               │
        ├──► execution/engine.rs ◄──────────────────────┘
        │    │                              (both api/run.rs and
        │    │                               session/stream_handler.rs
        │    │                               call execute_run/dryrun)
        │    │
        │    ├──► execution/classifier.rs
        │    ├──► execution/guards.rs ──► telemetry/collectors/rededr.rs
        │    ├──► execution/monitor.rs ──► infra/process.rs
        │    ├──► execution/sink.rs
        │    ├──► execution/state.rs
        │    ├──► execution/types.rs
        │    │
        │    ├──► telemetry/pipeline.rs
        │    ├──► telemetry/collectors/rededr.rs
        │    └──► telemetry/collectors/trace.rs
        │
        ├──► session/stream_handler.rs ──► execution/engine.rs
        │    session/worker_state.rs                 execution/state.rs
        │                                            execution/sink.rs
        │                                            execution/types.rs
        │
        └──► infra/process.rs
             infra/system.rs
             infra/time.rs
```

---

## 5. Communication Architecture

### 5.1 Two Coexisting Communication Models

The agent supports two gRPC communication models that coexist and share the same execution engine:

**Phase 1 — Unary RPCs (Original):**

```
Controller ──RunSample──►  Worker ──SampleResponse──► Controller
Controller ──SendArtifact──► Worker ──TransferAck──► Controller
Controller ──HealthCheck──► Worker ──HealthResponse──► Controller
Controller ──GetTelemetry──► Worker ══TelemetryData══► Controller (stream)
```

Each operation is a separate gRPC call. Simple but requires multiple round trips and has no persistent connection.

**Phase 2 — Bidirectional Stream (Current):**

```
Controller ══════ EstablishStream (bidi gRPC) ══════ Worker
                         │
           ┌─────────────┼──────────────┐
           ▼             ▼              ▼
    ControllerMessage          WorkerMessage
    ├── RunSample              ├── Registration
    ├── HealthCheck            ├── Status
    ├── Heartbeat              ├── Ack
    ├── Disconnect             ├── SampleResponse
    ├── Ack                    ├── Telemetry
    └── ArtifactChunks         └── ExecutionStatus
```

All communication multiplexed over a single TCP connection. Enables real-time status updates during execution and controller-initiated heartbeats.

**Convergence:** Both paths share:

| Shared Component | Purpose |
|-----------------|---------|
| `execution_lock` (`Arc<Mutex<ExecutionState>>`) | Prevents concurrent runs regardless of entry path |
| `execution::engine::execute_run()` | Same 10-phase pipeline |
| `execution::engine::execute_dryrun()` | Same lightweight path |
| `execution::classifier::classify_run()` | Same 7-verdict decision tree |
| `ControlPlaneSink` trait | Status/telemetry delivery abstraction |

### 5.2 gRPC Service: WorkerAgent (7 RPCs)

| RPC | Type | Handler | Purpose |
|-----|------|---------|---------|
| `Ping` | Unary | `api/info.rs` | Connectivity test |
| `RunSample` | Unary | `api/run.rs` | Execute artifact and return result |
| `HealthCheck` | Unary | `api/info.rs` | CPU/memory/job status |
| `SendArtifact` | Client-streaming | `api/artifacts.rs` | Transfer PE binary in 4MB chunks |
| `GetWorkerInfo` | Unary | `api/info.rs` | Full capability + health report |
| `GetTelemetry` | Server-streaming | `api/info.rs` | Pull RedEDR events for a job |
| `EstablishStream` | Bidirectional | `api/stream.rs` | Persistent command/status channel |

---

## 6. Execution Pipeline

The execution engine implements a 10-phase pipeline for full monitored execution:

```
Phase 1 ─ Validate ──► Phase 2 ─ RedEDR Setup ──► Phase 3 ─ Environment
   │                       │                           │
   │ Check artifact        │ Create collector           │ Create telemetry dir
   │ exists on disk        │ Sanity check (0 events?)  │ Start trace collector
   │                       │ Start tracing target       │ (named pipe server)
   ▼                       ▼                           ▼
Phase 4 ─ Spawn ──────► Phase 5 ─ Monitor ──────► Phase 6 ─ Wait
   │                       │                           │
   │ spawn_artifact()      │ Capture stdout/stderr     │ timeout(duration, wait)
   │ ProcessGuard (RAII)   │ ExecutionMonitor start    │ ├─ Normal exit → code
   │ Extract PID           │ 3s poll: alive? CPU?      │ ├─ Timeout → kill tree
   │                       │ RedEDR event count        │ └─ Error → EXIT_WAIT
   ▼                       ▼                           ▼
Phase 7 ─ Collect ─────────────────────────────► Phase 7b ─ Classify
   │                                                │
   │ Stop monitor                                   │ classify_run()
   │ Drain trace pipe (500ms)                       │ exit_code × timed_out
   │ Collect from 5 sources:                        │ × checkpoints
   │ ├─ RedEDR events (HTTP collect_all)            │ → DetectionVerdict
   │ ├─ Trace JSONL (dedup + package)               │   (7 verdicts)
   │ ├─ Trace binary (parse trace.log)              │
   │ ├─ BB coverage (coverage_bbs.txt)              │
   │ └─ Checkpoints (checkpoints.log)               │
   ▼                                                ▼
Phase 8 ─ Stream telemetry ──► Phase 9 ─ Reset RedEDR ──► Phase 10 ─ Cleanup
   │                              │                            │
   │ TelemetryBatch (is_final)    │ rededr_guard.reset_now()  │ Delete artifact.exe
   │ sink.send_telemetry()        │ Disarm guard              │ Delete telemetry dir
   │ ├─ StreamSink → stream       │                            │
   │ └─ NullSink → /dev/null      │                            │
   ▼                              ▼                            ▼
                              Return RunOutcome
```

### 6.1 Detection Classifier (v3)

The classifier produces one of 7 verdicts from local signals only:

| Verdict | `is_detected()` | Meaning |
|---------|-----------------|---------|
| `Evasion` | false | Clean exit (code 0) or timeout while active |
| `Detected` | true | Externally killed by AV/EDR (no exit code, or NTSTATUS 0xC0000906/07) |
| `Ambiguous` | true (conservative) | Crash or carrier error — could be AV or bug |
| `Stalled` | false | Timeout without reaching payload (anti-emulation stuck) |
| `InfraError` | false | Never executed (setup failure, guardrail, wait error) |
| `MutationFailed` | false | Invalid artifact (controller-side only) |
| `Anomaly` | false | Unexpected behavior (controller-side only) |

**Evidence extraction:** Scans checkpoint telemetry events for `has_launched` (payload execution started) and `last_checkpoint` (diagnostic context).

### 6.2 RAII Resource Safety

Three guard types guarantee cleanup on all exit paths:

| Guard | Wraps | Normal Path | Drop Path |
|-------|-------|-------------|-----------|
| `RedEdrGuard` | `RedEdrCollector` | `reset_now()` (Phase 9) | Fire-and-forget HTTP POST to `/api/trace/reset` |
| `ProcessGuard` | `tokio::process::Child` | `disarm()` after normal exit | `start_kill()` (synchronous signal send) |
| `MonitorGuard` | Stop channel + task handles | `stop()` with graceful await | Send stop signal + abort (no await in sync Drop) |

### 6.3 Dryrun Path

Lightweight execution for the third leg of the differential protocol:

```
Phase 1 ─ Validate ──► Phase 2 ─ Spawn ──► Phase 3 ─ Wait ──► Phase 4 ─ Classify
                         (no RedEDR)          (no monitor)        (empty telemetry)
                         (no trace pipe)      (no capture)        ──► Cleanup ──► Return
```

Skips all telemetry infrastructure. Used to establish ground-truth behavior on clean VMs.

---

## 7. Telemetry Architecture

### 7.1 Six Telemetry Sources

| Source | Collector | Wire Protocol | Transport | Output |
|--------|-----------|--------------|-----------|--------|
| RedEDR events | `collectors/rededr.rs` | HTTP JSON (poll `localhost:8081`) | Real-time stream | `TelemetryData` (generic JSON payload) |
| Named pipe trace | `collectors/trace.rs` | Binary or Base64 named pipe | Real-time stream | `TraceEvent` → JSONL file |
| Trace JSONL | `pipeline.rs` | Disk file (from pipe collector) | Batch | `TelemetryData` (event_type `trace_log`) |
| Trace binary | `pipeline.rs` | Disk file (`trace.log`) | Batch | `TelemetryData` (event_type `trace_line`) |
| BB coverage | `pipeline.rs` | Disk file (`coverage_bbs.txt`) | Batch | `TelemetryData` (typed `CoverageEvent`) |
| API checkpoints | `pipeline.rs` | Disk file (`checkpoints.log`) | Batch | `TelemetryData` (typed `CheckpointEvent`) |

### 7.2 Real-Time vs Batch Collection

```
During execution (real-time):
    RedEDR HTTP poll ──► mpsc channel ──► gRPC stream ──► controller
    Named pipe trace ──► mpsc channel ──► disk file (trace_events.jsonl)

After execution (batch):
    trace_events.jsonl ──► deduplicate ──► package_trace_log() ──┐
    trace.log (binary) ──► parse ──► collect_trace_log_binary() ─┤
    coverage_bbs.txt ──► parse ──► collect_bb_coverage() ────────┤
    checkpoints.log ──► parse ──► collect_api_checkpoints() ─────┤
    RedEDR events ──► collect_all() ─────────────────────────────┤
                                                                  ▼
                                                        Vec<TelemetryData>
                                                                  │
                                                        sink.send_telemetry()
                                                                  │
                                                                  ▼
                                                            controller
```

### 7.3 Deduplication Strategy

**Trace dedup (`pipeline.rs`):** Collapses repeated `(file, line, func)` tuples from loops, keeps highest `seq` per key, adds `count: N` for multi-hit lines. Reduces 10,000-line traces to ~200 unique lines (typical 95%+ reduction).

**RedEDR dedup (`rededr.rs`):** Tracks `trace_id` in `HashSet<u64>`, filters already-seen events during real-time polling.

**Payload truncation (`pipeline.rs`):** If deduplicated trace exceeds `MAX_SERIALIZED_PAYLOAD` (3.5MB), progressively halves from the front, keeping the **tail** (most recent execution lines — where detection happens).

---

## 8. Shared State Model

All mutable state is accessed through interior mutability (`Arc<Mutex/RwLock>`):

```
┌─────────────────────────────────────────────────────────────────┐
│                    WorkerAgentService                             │
│                                                                   │
│  ┌────────────────────────────┐   ┌────────────────────────┐    │
│  │ execution_lock             │   │ stream_handler         │    │
│  │ Arc<Mutex<ExecutionState>> │   │ Arc<RwLock<Option<     │    │
│  │                            │   │   Arc<StreamHandler>>>> │    │
│  │ States:                    │   │                        │    │
│  │ ├── Idle                   │   │ Contains:              │    │
│  │ └── Running {              │   │ ├── worker_state       │    │
│  │       job_id,              │   │ │   (Arc<RwLock>)      │    │
│  │       artifact,            │   │ ├── tx channel         │    │
│  │       run_id               │   │ └── execution_lock     │    │
│  │     }                      │   │     (same Arc ↑)       │    │
│  └─────────┬──────────────────┘   └─────────┬──────────────┘    │
│            │                                │                    │
│            │  shared by:                    │  shared by:        │
│            │  ├── api/run.rs               │  ├── api/run.rs    │
│            │  ├── api/stream.rs            │  ├── api/stream.rs │
│            │  └── stream_handler.rs        │  └── heartbeat_loop│
│            │                                │                    │
│  ┌────────────────────────────┐   ┌────────────────────────┐    │
│  │ system_info                │   │ capabilities           │    │
│  │ Arc<Mutex<System>>         │   │ Arc<WorkerCapabilities>│    │
│  │                            │   │                        │    │
│  │ Used by: api/info.rs       │   │ Immutable after startup│    │
│  └────────────────────────────┘   └────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

**Arc-cycle prevention:** The `StreamHandler` does NOT hold `Arc<WorkerAgentService>`. Instead, individual fields (`worker_id`, `config`) are cloned, and `execution_lock` is shared by reference. This breaks:
```
WorkerAgentService → stream_handler → WorkerAgentService  (CYCLE — prevented)
```

**Single-execution lock invariant:** The `ExecutionState` enum guarantees:
1. Only one artifact runs at a time (second `RunSample` → `resource_exhausted`)
2. State and metadata are always consistent (enum, not separate bool + fields)
3. Lock is always released (RAII `ExecutionLockGuard` in Drop)
4. Both execution paths (unary RPC and stream command) share the same lock

---

## 9. Platform Portability

The agent is designed for **production on Windows VMs** but **compiles on Linux/macOS** for development:

| Component | Windows | Non-Windows |
|-----------|---------|-------------|
| gRPC server (`main.rs`) | Full | Full |
| Capability detection | Full (registry, PowerShell) | Stubs return false/None |
| Process spawn/kill | `taskkill /F /T` + `child.kill()` | `child.kill()` only |
| Process alive check | `OpenProcess` API | Always returns false |
| Named pipe trace | `tokio::net::windows::named_pipe` | `bail!("not supported")` |
| RedEDR HTTP collector | Full | Full (HTTP is cross-platform) |
| Telemetry pipeline | Full | Full (pure Rust I/O) |
| Trace compressor | Full | Full (pure algorithms) |
| System metrics | Full | Full (sysinfo is cross-platform) |
| NTSTATUS description | `RtlNtStatusToDosError` + `FormatMessageW` | Returns None |

Platform branching uses `#[cfg(target_os = "windows")]` / `#[cfg(not(windows))]` at the function level, concentrated in `infra/`, `capabilities.rs`, and `collectors/trace.rs`.

---

## 10. Design Patterns

| Pattern | Where | Why |
|---------|-------|-----|
| **Thin adapter** | `api/` (7 handlers) | Separates gRPC mechanics from business logic; handlers are free functions, not methods |
| **RAII guards** | `execution/guards.rs` | Guarantees resource cleanup (RedEDR, process, monitor) on all exit paths including panics |
| **Strategy trait** | `execution/sink.rs` (`ControlPlaneSink`) | Decouples execution engine from transport; `StreamSink` vs `NullSink` |
| **State machine** | `execution/state.rs` (`ExecutionState` enum) | Prevents invalid states; `Idle` ↔ `Running{metadata}` with RAII guard |
| **Interior mutability** | `lib.rs` (all `Arc<Mutex/RwLock>` fields) | Enables tonic's `Clone` requirement while sharing mutable state |
| **Arc-cycle prevention** | `session/stream_handler.rs` | Clone individual fields instead of `Arc<Self>` to prevent reference cycles |
| **Channel-based streaming** | `session/`, `telemetry/` | mpsc channels decouple producers (collectors) from consumers (gRPC stream) |
| **Deduplication** | `pipeline.rs`, `rededr.rs` | Reduce telemetry volume: trace dedup by `(file,line,func)`, event dedup by `trace_id` |
| **Progressive truncation** | `pipeline.rs` | Binary-search halving keeps tail (most relevant lines) within gRPC payload limits |
| **Auto-detection** | `collectors/trace.rs` | Peek first 4 bytes to distinguish binary vs Base64 protocol; supports both instrumentation backends |
| **Startup caching** | `capabilities.rs` | Expensive I/O (registry reads, HTTP probes, PowerShell) done once at startup, cached for lifetime |

---

## 11. External Dependencies

| Crate | Usage |
|-------|-------|
| `tonic` | gRPC server + generated protobuf types |
| `tokio` | Async runtime, process management, named pipes, channels, timers |
| `sysinfo` | CPU/memory metrics (system-wide and per-PID) |
| `reqwest` | HTTP client for RedEDR API |
| `serde` / `serde_json` | JSON serialization for telemetry events |
| `sha2` | SHA-256 integrity verification for artifact transfer |
| `chrono` | UTC timestamps |
| `base64` | Base64 decoding for text-format trace protocol |
| `flate2` | Gzip compression (trace compressor, not yet integrated) |
| `anyhow` | Error handling in trace collector |
| `uuid` | UUID v4 generation for fallback run IDs |
| `regex` | Version string extraction from RedEDR logs |
| `winreg` | Windows registry access (MDE, Cortex, OS version detection) |
| `windows` | Win32 API (`OpenProcess`, `CloseHandle`, `RtlNtStatusToDosError`) |
| `tracing` / `tracing-subscriber` | Structured logging |
| `automutate_config` | Shared `WorkerConfig` type from TOML |

---

## 12. Summary Statistics

| Metric | Value |
|--------|-------|
| **Total crate size** | **~5,926 lines** (excluding tests) |
| Source modules | 7 (`api`, `execution`, `session`, `telemetry`, `infra`, `capabilities`, `constants`) |
| Source files | 24 |
| gRPC RPCs implemented | 7 (WorkerAgent service) |
| Stream message types handled | 6 incoming + 5 outgoing |
| Execution phases | 10 (full run) / 4 (dryrun) |
| Detection verdicts | 7 |
| Telemetry sources | 6 |
| Wire protocols | 5 (gRPC, HTTP JSON, binary pipe, Base64 pipe, disk JSONL) |
| RAII guards | 3 |
| Trait definitions | 1 (`ControlPlaneSink`) |
| Tuning constants | 6 |
| Unit tests | 29 (classifier: 18, telemetry: 7, monitor: 1, compressor: 3) |
| Platform-conditional code | `infra/process.rs`, `capabilities.rs`, `collectors/trace.rs`, `execution/types.rs` |
| Not yet integrated | `trace_compressor.rs` (3 blockers), stream-based artifact transfer (`ArtifactChunks`) |
