# Worker Agent — Corrected Diagrams

All diagrams from the original WORKER-AGENT-ARCHITECTURE.md, corrected to match the actual implementation.

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
│   ├── constants.rs                      # Tuning parameters (6 constants)
│   │
│   ├── api/                              # gRPC RPC handler layer (thin adapters)
│   │   ├── mod.rs                        # WorkerAgent trait impl (dispatches to handlers)
│   │   ├── run.rs                        # run_sample() - unary RPC entry point
│   │   ├── artifacts.rs                  # send_artifact() - chunked binary + SHA256 verify
│   │   ├── info.rs                       # ping(), health_check(), get_worker_info(), get_telemetry()
│   │   └── stream.rs                     # establish_stream() - bidirectional stream setup
│   │
│   ├── execution/                        # Execution orchestration (core logic)
│   │   ├── mod.rs                        # Module declarations
│   │   ├── engine.rs                     # execute_run() - 10-phase execution pipeline
│   │   ├── classifier.rs                 # classify_run() - 7-verdict detection classifier
│   │   ├── guards.rs                     # RAII guards: RedEdr, Process, Monitor
│   │   ├── monitor.rs                    # ExecutionMonitor: polls process + RedEDR every 3s
│   │   ├── sink.rs                       # ControlPlaneSink trait (StreamSink / NullSink)
│   │   ├── state.rs                      # ExecutionState enum + ExecutionLockGuard
│   │   └── types.rs                      # RunRequest, RunContext, RunOutcome, RunPhaseTimings,
│   │                                     #   exit codes, SampleResponse builders
│   │
│   ├── session/                          # Stream session and worker runtime state
│   │   ├── mod.rs                        # Module declarations
│   │   ├── stream_handler.rs             # Bidirectional gRPC stream message loop + heartbeat
│   │   └── worker_state.rs              # WorkerState, HealthMetrics (runtime state)
│   │
│   ├── infra/                            # OS + side effects (pluggable boundary)
│   │   ├── mod.rs                        # Module declarations
│   │   ├── process.rs                    # spawn, kill, verify, capture (Windows-specific)
│   │   ├── system.rs                     # System metrics, telemetry dir, artifact cleanup
│   │   └── time.rs                       # Unix timestamp wrapper (chrono)
│   │
│   └── telemetry/                        # Telemetry collection and compression
│       ├── mod.rs                        # Module declarations
│       ├── pipeline.rs                   # Trace dedup, packaging, BB coverage, checkpoints
│       ├── trace_compressor.rs           # CLP + MatrixProfile + Sequitur compression (experimental)
│       └── collectors/
│           ├── mod.rs                    # Module declarations
│           ├── rededr.rs                 # RedEDR HTTP API collector (ETW/kernel events)
│           └── trace.rs                  # Named pipe trace collector (line-level tracing)
│
└── tests/
    └── test_trace_pipe.rs                # Integration test: named pipe Base64 trace flow
```

---

## 2. Component Hierarchy & Ownership

```
main.rs
└── WorkerAgentService                    [Clone, all fields Arc-wrapped]
    ├── worker_id: String                 ← From config
    ├── config: WorkerConfig              ← TOML file (C:\AutoMutate\worker.toml)
    ├── system_info: Arc<Mutex<System>>   ← sysinfo for health metrics
    ├── execution_lock: Arc<Mutex<ExecutionState>>  ← ONE run at a time
    ├── stream_handler: Arc<RwLock<Option<Arc<StreamHandler>>>>
    │                                      ← Set on EstablishStream, used by run_sample
    ├── heartbeat_handle: Arc<RwLock<Option<JoinHandle<()>>>>
    │                                      ← Background heartbeat task (aborted on reconnect)
    └── capabilities: Arc<WorkerCapabilities>
                                           ← Detected tools + metadata (cached at startup)

        StreamHandler [no Arc<WorkerAgentService> back-ref]
        ├── worker_state: Arc<RwLock<WorkerState>>
        │   ├── worker_id: String
        │   ├── capabilities: Vec<String>
        │   ├── metadata: HashMap<String, String>
        │   ├── tools: Option<ToolVersions>
        │   ├── health: HealthMetrics
        │   ├── current_job_id: Option<String>
        │   ├── current_run_id: Option<String>  ← Correlates telemetry
        │   ├── last_controller_heartbeat: Option<i64>
        │   ├── controller_disconnected: bool
        │   ├── disconnect_reason: Option<String>
        │   └── reconnect_allowed: bool
        ├── tx: mpsc::Sender<Result<WorkerMessage, Status>>  ← 100-capacity channel
        ├── worker_id: String                   ← Cloned from service
        ├── config: WorkerConfig                ← Cloned from service
        └── execution_lock: Arc<Mutex<ExecutionState>>  ← Shared with service
```

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
         ├── Heartbeat   ┼─────→ (no reply; updates worker_state.last_controller_heartbeat)
         ├── Disconnect  ┼─────→ (no reply; sets controller_disconnected=true)
         └── Ack         │             │
                         │             │
                         │      ┌──────┤
                         │      │      │
                    Telemetry  Exec   Registration
                    Batch      Status
```

### Outbound Message Types (Worker → Controller)

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

### Inbound Message Types (Controller → Worker)

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

## 4. Execution Pipeline Overview (10 Phases)

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
   │ Extract PID           │ 3s poll: alive? CPU?      │ ├─ Timeout → kill tree (-3)
   │                       │ RedEDR event count        │ └─ Wait error → -1
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
   │ sink.send_telemetry()        │ (disarms Drop guard)      │ Delete telemetry dir
   │ ├─ StreamSink → stream       │                            │
   │ └─ NullSink → /dev/null      │                            │
   ▼                              ▼                            ▼
                              Return RunOutcome
```

---

## 5. Per-Phase Diagrams

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
                  └── Get PID
```

### Phase 5: Start Monitoring

```
    ┌─────────────────────────────────┐
    │  capture_stream(stdout)          │
    │  capture_stream(stderr)          │
    │  (spawned async tasks)           │
    └─────────────┬───────────────────┘
                  │
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
    (code or   (-1)        │
     -2 if
     no code)
         │        │        ▼
         │        │   try_wait() race check
         │        │   ├── Exited naturally → use real exit code
         │        │   └── Still running:
         │        │       infra::process::kill_process_tree()
         │        │       infra::process::is_process_alive()
         │        │       exit_code = EXIT_TIMEOUT (-3)
         ▼        ▼
    ProcessGuard.disarm() (process handled, prevent kill-on-drop)
    exit_code resolution:
     0    = success (clean exit)
    -1    = EXIT_WAIT_FAILED (wait() system call failed)
    -2    = EXIT_NO_CODE (externally terminated, no exit code — likely AV/EDR kill)
    -3    = EXIT_TIMEOUT (timeout expired, process killed)
    -4    = EXIT_INFRA (never reached execution — spawn/setup failure)
    other = NTSTATUS interpretation (Windows-specific, e.g. 0xC0000005)
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
    │             │              (coverage_       (checkpoints.log)
    │             │               bbs.txt)        JSON lines
    │         ┌───┼───┐           │                │
    │     <=2MB   │  >2MB         │                │
    │     single  │  last 2MB     │                │
    │     event   │  + async      │                │
    │             │  compress     │                │
    ▼             ▼               ▼                ▼
    └─────────── telemetry_events[] ───────────────┘
                      │
                      + trace.log binary event
                      + phase_timings event
```

### Phase 7b: Classify Detection Outcome

```
    ┌─────────────────────────────────┐
    │  classifier::classify_run()      │
    │                                  │
    │  Inputs:                         │
    │  ├── exit_code                   │
    │  ├── timed_out                   │
    │  └── telemetry_events[]          │
    │      (scans for checkpoints:     │
    │       has_launched, last_chkpt)  │
    └─────────────┬───────────────────┘
                  │
                  ▼
    ┌─────────────────────────────────────────────────┐
    │            Decision Tree (11 steps)               │
    │                                                   │
    │  1. exit_code == -4 (EXIT_INFRA)?                 │
    │     └── yes → InfraError                          │
    │                                                   │
    │  2. exit_code == -1 (EXIT_WAIT_FAILED)?           │
    │     └── yes → InfraError                          │
    │                                                   │
    │  3. exit_code in [10,20) (guardrail codes)?       │
    │     └── yes → InfraError                          │
    │                                                   │
    │  4. exit_code == 0?                               │
    │     └── yes → Evasion                             │
    │                                                   │
    │  5. timed_out + has_launched?                      │
    │     └── yes → Evasion                             │
    │  6. timed_out + !has_launched?                     │
    │     └── yes → Stalled                             │
    │                                                   │
    │  5b. exit_code == -3 (EXIT_TIMEOUT, defensive)?   │
    │      ├── has_launched → Evasion                    │
    │      └── !has_launched → Stalled                   │
    │                                                   │
    │  7. exit_code == -2 (EXIT_NO_CODE)?               │
    │     └── yes → Detected                            │
    │                                                   │
    │  8. AV NTSTATUS (0xC0000906, 0xC0000907)?         │
    │     └── yes → Detected                            │
    │                                                   │
    │  9. Crash NTSTATUS (0xC0000005, 0xC0000409,       │
    │     0xC00000FD, 0xC0000374, 0xC0000094)?          │
    │     └── yes → Ambiguous                           │
    │                                                   │
    │  10. exit_code in [30,40) (carrier codes)?        │
    │      └── yes → Ambiguous                          │
    │                                                   │
    │  11. Other nonzero                                │
    │      └── Ambiguous                                │
    └─────────────────────────────────────────────────┘

    7 Verdicts:
    ├── Evasion        (is_detected = false)
    ├── Detected       (is_detected = true)
    ├── Ambiguous      (is_detected = true, conservative)
    ├── Stalled        (is_detected = false)
    ├── InfraError     (is_detected = false)
    ├── MutationFailed (is_detected = false, controller-side only)
    └── Anomaly        (is_detected = false, controller-side only)
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

### Phase 9: Reset RedEDR

```
    ┌─────────────────────────────────┐
    │  rededr_guard.reset_now()        │
    │  (explicit reset, disarms Drop)  │
    └─────────────────────────────────┘
    Note: ProcessGuard was already disarmed in Phase 6.
    ExecutionLockGuard is managed by the caller (api/run.rs
    or stream_handler.rs), not by the engine.
```

### Phase 10: Cleanup

```
    ┌─────────────────────────────────┐
    │  infra::system::cleanup_run_     │
    │  artifacts()                     │
    │  ├── remove_file(artifact.exe)   │
    │  └── remove_dir_all(telemetry_   │
    │      dir)                        │
    │  (non-fatal: warns on failure)   │
    └─────────────┬───────────────────┘
                  │
                  ▼
    Return RunOutcome {
      exit_code, timed_out, stdout, stderr,
      telemetry_events, elapsed, phase_timings,
      detection_verdict, last_checkpoint
    }
```

### Dryrun Path (Lightweight, 6 Phases)

```
Phase 1 ─ Validate ──► Phase 2 ─ Spawn ──► Phase 3 ─ Wait ──► Phase 4 ─ Classify
   │                     │                    │                    │
   │ artifact exists?    │ spawn_artifact()   │ timeout + wait()   │ classify_run()
   │                     │ (no RedEDR)        │ (no monitor)       │ (empty telemetry)
   ▼                     │ (no trace pipe)    │ (no capture)       ▼
                         ▼                    ▼              Phase 5 ─ Cleanup
                                                               │
                                                               │ remove artifact file
                                                               ▼
                                                          Phase 6 ─ Return RunOutcome
```

---

## 6. State Machines

### 6.1 Execution State (`execution/state.rs`)

```
    IDLE ──────────────────→ RUNNING ──────────────────→ IDLE
    (ExecutionState::Idle)   (ExecutionState::Running    (ExecutionLockGuard
     acquire() succeeds       { job_id, artifact,         Drop → tokio::spawn
                                run_id })                  → state.release())

    Invariant: At most ONE artifact executing at any time.
    Enforcement: Arc<Mutex<ExecutionState>>
    Cleanup: ExecutionLockGuard Drop impl (spawns async release)
```

### 6.2 Stream Handler Lifecycle

```
    Per-connection lifecycle (each EstablishStream creates a NEW StreamHandler):

    NULL ──→ ACTIVE ──→ stream drops ──→ NULL
     │         │              │
     │    EstablishStream     │  StreamHandler is dropped
     │    RPC called          │  (previous heartbeat_handle aborted,
     │                        │   worker_state remains in Arc)
     │                        │
     │                        └──→ New EstablishStream → NEW StreamHandler → ACTIVE
     │                             (not a state transition — a fresh instance)
     │
     stream_handler = Arc<RwLock<Option<Arc<StreamHandler>>>>
     None until first EstablishStream
     On reconnect: old StreamHandler replaced, new one created with fresh tx channel
```

### 6.3 Execution Monitor States (`execution/monitor.rs`)

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

    Poll interval: 3 seconds (MONITOR_POLL_INTERVAL_SECS)
    Idle: 3+ cycles (IDLE_COUNT_THRESHOLD) with no new events AND cpu <= 5% (CPU_IDLE_THRESHOLD)
    Timeout warning: within 5 seconds (TIMEOUT_APPROACH_SECS) of timeout_seconds
```

---

## 7. Telemetry Architecture

### 7.1 Six Telemetry Sources (2 Real-Time + 5 Batch in Phase 7, with overlap)

```
    ┌───────────────────────────────────────────────────────────────┐
    │                      Artifact Execution                       │
    │                                                               │
    │  ┌─────────┐  ┌─────────┐  ┌────────┐  ┌──────────────────┐ │
    │  │ stdout  │  │ stderr  │  │coverage│  │ checkpoints.log  │ │
    │  │ (piped) │  │ (piped) │  │_bbs.txt│  │ (JSON lines)     │ │
    │  └────┬────┘  └────┬────┘  └───┬────┘  │ checkpoints +    │ │
    │       │            │           │       │ status events    │ │
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
    │         │  + trace.log (binary protocol)             │ │
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

### 7.2 Real-Time vs Batch Collection

```
During execution (real-time):
    RedEDR start() poll ──► mpsc channel ──► gRPC stream ──► controller
      (ExecutionMonitor calls rededr.start() which polls
       GET /api/logs/rededr every flush_interval_ms=1000,
       deduplicates via seen_trace_ids HashSet,
       sends new events via monitor's event_tx channel)
    Named pipe trace ──► mpsc channel ──► disk file (trace_events.jsonl)

After execution (batch, Phase 7):
    trace_events.jsonl ──► deduplicate ──► package_trace_log() ──┐
    trace.log (binary) ──► parse ──► collect_trace_log_binary() ─┤
    coverage_bbs.txt ──► parse ──► collect_bb_coverage() ────────┤
    checkpoints.log ──► parse ──► collect_api_checkpoints() ─────┤
    RedEDR events ──► collect_all() (second pass, full batch) ───┤
                                                                  ▼
                                                        Vec<TelemetryData>
                                                                  │
                                                        sink.send_telemetry()
                                                                  │
                                                                  ▼
                                                            controller

Note: RedEDR events are collected TWICE — once real-time via start()
during execution (streamed to controller), and once via collect_all()
in Phase 7 (included in the final telemetry batch).
```

### 7.3 RedEDR Collector (`telemetry/collectors/rededr.rs`)

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

### 7.4 Trace Collector (`telemetry/collectors/trace.rs`)

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

### 7.5 Telemetry Pipeline (`telemetry/pipeline.rs`)

```
    package_trace_log() flow:
    ┌──────────────────────────────────────────────────┐
    │  1. Read trace_events.jsonl                       │
    │  2. deduplicate_trace_jsonl()                     │
    │     Key: (file, line, func)                       │
    │     Keep: highest seq per key, add count: N        │
    │     Output: ~200 unique lines (95%+ reduction)     │
    │  3. Serialize to JSON                              │
    │  4. Size check: ≤ MAX_SERIALIZED_PAYLOAD (3.5MB)? │
    │     ├── YES → ship full content as trace_log event │
    │     └── NO  → progressive tail truncation:         │
    │               ├── Cut slice in half                │
    │               ├── Advance to next \n boundary      │
    │               ├── Re-serialize, re-check size      │
    │               ├── Repeat until fits                │
    │               └── Ship TAIL (most recent lines —   │
    │                   where detection happens)         │
    └──────────────────────────────────────────────────┘

    collect_trace_log_binary() — separate path:
    Parses trace.log (ISTR binary format) directly,
    extracts line traces only. Checkpoint events (types 2-4)
    are warned+ignored if found in trace.log.
```

### 7.6 Trace Compression (`telemetry/trace_compressor.rs` — experimental, NOT integrated)

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

---

## 8. RAII Guard System

### Guard Hierarchy

```
    execute_run() scope
    ├── RedEdrGuard          ← Resets RedEDR HTTP API on drop
    ├── ProcessGuard         ← Kills child process on drop
    └── MonitorGuard         ← Stops monitor + event consumer on drop

    Caller scope (api/run.rs or stream_handler.rs)
    └── ExecutionLockGuard   ← Releases execution_lock on drop
```

### Drop Behavior

```
    RedEdrGuard Drop:
    ├── reset_on_drop == true?
    │   ├── Handle::try_current() → spawn async POST /api/trace/reset
    │   └── No runtime? → eprintln warning (RedEDR may be contaminated)
    └── reset_on_drop == false → no-op (already reset via reset_now())

    ProcessGuard Drop:
    ├── should_kill == true?
    │   └── child.start_kill() (synchronous signal, no runtime needed)
    └── should_kill == false → no-op (already disarmed)

    MonitorGuard Drop:
    ├── stop_tx.take() → send(true) (stop signal)
    └── event_consumer.take() → abort() (kill consumer task)

    ExecutionLockGuard Drop:
    └── tokio::spawn → state.release() (Idle)
```

---

## 9. Shared State Model

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
│                                                                   │
│  ┌────────────────────────────┐                                  │
│  │ heartbeat_handle           │                                  │
│  │ Arc<RwLock<Option<         │                                  │
│  │   JoinHandle<()>>>>        │                                  │
│  │                            │                                  │
│  │ Aborted on stream reconnect│                                  │
│  └────────────────────────────┘                                  │
└─────────────────────────────────────────────────────────────────┘

Arc-cycle prevention:
    WorkerAgentService → stream_handler → WorkerAgentService  (CYCLE — prevented)
    StreamHandler clones worker_id, config; shares execution_lock by Arc
```

---

## 10. Control Plane Sink (`execution/sink.rs`)

```
    ┌─────────────────────────────────────────────┐
    │  trait ControlPlaneSink: Send + Sync          │
    │  ├── send_status(ExecutionStatusReport)       │
    │  ├── send_telemetry(TelemetryBatch)           │
    │  └── send_ack(request_id, success, error)     │
    └─────────────────┬───────────────────────────┘
                      │
            ┌─────────┼─────────┐
            │                   │
            ▼                   ▼
    ┌──────────────┐    ┌──────────────┐
    │  StreamSink  │    │  NullSink    │
    │              │    │              │
    │  Wraps mpsc  │    │  All sends   │
    │  Sender →    │    │  succeed     │
    │  controller  │    │  silently    │
    │              │    │  (no stream) │
    └──────────────┘    └──────────────┘

    Factory: build_sink(Option<&Sender>) → Arc<dyn ControlPlaneSink>
    Called by: api/run.rs, session/stream_handler.rs
```

---

## 11. Concurrency Model

### Task Tree for One Execution

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

---

## 12. Capability Detection (`capabilities.rs`)

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

---

## 13. Artifact Transfer Flow

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

## 14. Inter-Module Dependency Graph

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

## 15. Layered Architecture

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

---

## 16. Communication Architecture

### Phase 1 — Unary RPCs (Original)

```
Controller ──RunSample──►  Worker ──SampleResponse──► Controller
Controller ──SendArtifact──► Worker ──TransferAck──► Controller
Controller ──HealthCheck──► Worker ──HealthResponse──► Controller
Controller ──GetTelemetry──► Worker ══TelemetryData══► Controller (stream)
```

### Phase 2 — Bidirectional Stream (Current)

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
