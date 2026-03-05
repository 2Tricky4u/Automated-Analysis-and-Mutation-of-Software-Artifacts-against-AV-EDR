# Execution Module — Deep Analysis

Deep analysis of `worker/agent/src/execution/` — the core artifact execution engine, detection classifier, monitoring system, and resource management layer.

---

## 1. Overview

### Purpose

The `execution/` folder is the **heart of the worker agent**. It owns the entire lifecycle of running a PE artifact on a Windows VM: validating the artifact, configuring RedEDR telemetry, spawning the process, monitoring it in real-time, collecting all telemetry (ETW, traces, coverage, checkpoints), classifying the detection outcome, and cleaning up everything afterwards.

### Role in the Global Project

In the AutoMutate++ pipeline, each mutation round requires executing an artifact and observing what happens. The execution module is where **observation meets reality** — it is the component that transforms a built artifact into a `RunOutcome` containing the exit code, detection verdict, telemetry events, and timing data that the controller uses for differential analysis and triage.

```
Controller dispatches run
    │
    ▼
┌─────────────────────────────────────────────────────┐
│              execution/ (this folder)                 │
│                                                       │
│  ┌─────────┐  ┌───────────┐  ┌──────────────────┐   │
│  │ engine   │  │ classifier│  │ monitor          │   │
│  │          │  │           │  │                  │   │
│  │ execute_ │──│ classify_ │  │ poll RedEDR      │   │
│  │ run()    │  │ run()     │  │ track process    │   │
│  │ execute_ │  │           │  │ detect idle/stuck│   │
│  │ dryrun() │  └───────────┘  └──────────────────┘   │
│  └─────┬────┘                                         │
│        │ uses                                         │
│  ┌─────┴────┐  ┌───────────┐  ┌──────────────────┐   │
│  │ guards   │  │ sink      │  │ state            │   │
│  │          │  │           │  │                  │   │
│  │ RedEdr   │  │ StreamSink│  │ ExecutionState   │   │
│  │ Process  │  │ NullSink  │  │ Idle ↔ Running   │   │
│  │ Monitor  │  │           │  │ ExecutionLock    │   │
│  └──────────┘  └───────────┘  └──────────────────┘   │
│                                                       │
│  ┌──────────────────────────────────────────────┐    │
│  │ types                                         │    │
│  │ RunRequest, RunContext, RunOutcome             │    │
│  │ RunPhaseTimings, exit codes, SampleResponse   │    │
│  │ builders, exit code interpretation            │    │
│  └──────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
    │
    ▼
RunOutcome → SampleResponse → Controller
```

### Design Principles

1. **Transport-agnostic:** The engine communicates outward only through the `ControlPlaneSink` trait. It has no knowledge of gRPC, streams, or the api layer.
2. **RAII resource safety:** Every resource (RedEDR, child process, monitor) is wrapped in a guard that guarantees cleanup on all exit paths — including panics.
3. **Single execution:** One artifact at a time per worker, enforced by `ExecutionState`. This prevents RedEDR telemetry cross-contamination.
4. **Phase-timed observability:** Every execution phase is independently timed, and timings are emitted as a telemetry event for performance analysis.

---

## 2. File Inventory

| File | Lines | Purpose | Key Exports |
|------|-------|---------|-------------|
| `mod.rs` | 11 | Module declarations | — |
| `engine.rs` | 810 | Core execution pipeline | `execute_run()`, `execute_dryrun()`, `RunError` |
| `classifier.rs` | 384 | Detection outcome classification | `classify_run()`, `DetectionVerdict` (re-export) |
| `guards.rs` | 163 | RAII resource cleanup guards | `RedEdrGuard`, `ProcessGuard`, `MonitorGuard` |
| `monitor.rs` | 380 | Real-time execution monitoring | `ExecutionMonitor`, `MonitorConfig` |
| `sink.rs` | 107 | Transport abstraction trait | `ControlPlaneSink`, `StreamSink`, `NullSink`, `build_sink()` |
| `state.rs` | 122 | Execution lock state machine | `ExecutionState`, `ExecutionLockGuard`, `ExecutionBusyError` |
| `types.rs` | 295 | Domain types + helpers | `RunRequest`, `RunContext`, `RunOutcome`, `RunPhaseTimings`, exit codes, `sample_response_ok/error()`, `format_output()` |
| **Total** | **2272** | — | — |

---

## 3. Per-Module Deep Analysis

### 3.1 `engine.rs` — Core Execution Pipeline (810 lines)

The central orchestrator. Contains two public entry points: `execute_run()` for full monitored execution and `execute_dryrun()` for clean-VM exit-code-only runs.

#### 3.1.1 `execute_dryrun()`

Lightweight execution path used for the dryrun leg of the three-run differential protocol. Skips all telemetry infrastructure.

**Pipeline:**

```
1. Validate artifact exists on disk
2. Spawn process (infra::process::spawn_artifact)
3. Wait with timeout (tokio::time::timeout)
4. Classify outcome (classifier::classify_run with empty telemetry)
5. Cleanup artifact file
6. Return RunOutcome
```

**What it skips vs full execution:**
- No RedEDR setup/reset
- No telemetry directory creation
- No trace collector (named pipe)
- No execution monitor
- No telemetry collection or streaming
- No output capture (stdout/stderr)

**Why it exists:** The dryrun runs on a clean VM without AV/EDR to establish a ground-truth exit code. If the artifact crashes on a clean VM with the same exit code as on the AV VM, the failure is a carrier bug, not a detection. This is the third leg added to the two-run differential protocol.

#### 3.1.2 `execute_run()` — Full Execution Pipeline

The main execution function. 10 phases, each with explicit timing:

```
Phase 1: Validate artifact
    │  Check artifact_path exists on disk
    ▼
Phase 2: Setup RedEDR
    │  Create RedEdrCollector → RedEdrGuard (RAII)
    │  Sanity check: collect_all("sanity-check")
    │  ├── 0 events → clean, proceed
    │  ├── 1 event → initialization noise, discard silently
    │  └── >1 events → contamination
    │      ├── strict_mode → FailedPrecondition error
    │      └── lenient → force-reset, set new trace target
    │  Start tracing: collector.start_trace([artifact_name])
    ▼
Phase 3: Prepare environment
    │  Create/clean telemetry directory
    │  Start trace collector (named pipe → channel → JSONL file)
    │  ├── TraceCollector: reads from \\.\pipe\rededr_trace
    │  ├── Streaming writer: channel → BufWriter → trace_events.jsonl
    │  └── Optimized: thread_id omitted when unchanged
    ▼
Phase 4: Spawn process
    │  infra::process::spawn_artifact() → tokio::process::Child
    │  Wrap in ProcessGuard (RAII — kill on drop)
    │  Extract PID for monitoring
    ▼
Phase 5: Start monitoring
    │  Capture stdout/stderr streams
    │  Create ExecutionMonitor → MonitorGuard (RAII — stop on drop)
    │  Monitor polls RedEDR /api/stats every 3 seconds
    │  Sends ExecutionStatusReport via ControlPlaneSink
    ▼
Phase 6: Wait for process completion or timeout
    │  tokio::time::timeout(timeout_duration, child.wait())
    │  ├── Ok(Ok(status)) → normal exit, extract code
    │  │   └── status.code() == None → EXIT_NO_CODE (externally killed)
    │  ├── Ok(Err(e)) → wait() failed → EXIT_WAIT_FAILED
    │  └── Err(_) → timeout
    │      ├── try_wait() succeeds → race condition, process already exited
    │      └── try_wait() fails → kill process tree, EXIT_TIMEOUT
    │  Disarm ProcessGuard (process handled)
    ▼
Phase 7: Collect telemetry
    │  Stop monitor (before telemetry window)
    │  Wait 500ms for trace collector to drain pipe
    │  Abort trace collector, close streaming writer
    │  Collect from multiple sources:
    │  ├── RedEDR events (collector.collect_all)
    │  ├── Trace events JSONL (telemetry::pipeline::package_trace_log)
    │  ├── Trace binary log (telemetry::pipeline::collect_trace_log_binary)
    │  ├── BB coverage (coverage.bin + coverage_bbs.txt)
    │  └── API checkpoints (checkpoints.log)
    ▼
Phase 7b: Classify detection outcome
    │  classifier::classify_run(exit_code, timed_out, telemetry_events)
    │  → (DetectionVerdict, Option<last_checkpoint>)
    │  Append phase_timings as telemetry event
    ▼
Phase 8: Stream telemetry to controller
    │  Build TelemetryBatch (is_final=true)
    │  sink.send_telemetry(batch)
    │  ├── StreamSink → send via bidirectional stream
    │  └── NullSink → silently drop
    ▼
Phase 9: Reset RedEDR
    │  rededr_guard.reset_now() (explicit reset, disarms Drop)
    │  On failure: next run will detect contamination in Phase 2
    ▼
Phase 10: Cleanup artifacts
    │  infra::system::cleanup_run_artifacts()
    │  Delete artifact file + telemetry directory
    ▼
Return RunOutcome
```

#### 3.1.3 `RunError` — Setup Error Types

| Variant | gRPC Status | Cause |
|---------|-------------|-------|
| `ArtifactNotFound` | `not_found` | Artifact not on disk (forgot SendArtifact?) |
| `RedEdrSetupFailed` | `internal` | RedEDR API call failed |
| `EnvironmentSetupFailed` | `internal` | Telemetry directory creation failed |
| `ProcessSpawnFailed` | `internal` | Child process spawn failed |
| `FailedPrecondition` | `failed_precondition` | RedEDR contaminated (strict mode) |

#### 3.1.4 Telemetry Collection Sources

The engine collects telemetry from 5 independent sources in Phase 7:

| Source | File/API | Telemetry Type | Proto Event |
|--------|----------|---------------|-------------|
| RedEDR events | HTTP `/api/events` | ETW-based behavioral events | `TelemetryData` (generic) |
| Trace events | `trace_events.jsonl` (named pipe → file) | Line-level execution trace | `TraceEvent` |
| Trace binary | `trace.log` (binary protocol fallback) | Line-level execution trace | `TraceEvent` |
| BB coverage | `coverage.bin` + `coverage_bbs.txt` | AFL-style basic block coverage | `CoverageEvent` |
| API checkpoints | `checkpoints.log` | WINNIE-style behavioral markers | `CheckpointEvent` |

---

### 3.2 `classifier.rs` — Detection Outcome Classifier (384 lines)

Determines whether an artifact execution was detected, evaded, or produced an ambiguous result. This is the **v3 classifier** with explicit ambiguity handling.

#### 3.2.1 Design Philosophy

The classifier produces **provisional** verdicts from **local signals only** (exit code, timeout, checkpoints). It does NOT have access to dryrun results — the controller-side `override_with_dryrun()` performs that correction. This separation keeps the worker stateless and the classification logic composable.

#### 3.2.2 Detection Verdict Enum

| Verdict | `is_detected()` | Meaning |
|---------|-----------------|---------|
| `Evasion` | false | Process completed cleanly or timed out while active |
| `Detected` | true | Process was externally killed by AV/EDR |
| `Ambiguous` | true (conservative) | Crash or carrier error — could be AV or bug |
| `Stalled` | false | Timeout without reaching payload (anti-emulation stuck) |
| `InfraError` | false | Infrastructure failure (never executed, guardrail, wait failed) |
| `MutationFailed` | false | Mutation produced invalid artifact (used controller-side) |
| `Anomaly` | false | Unexpected behavior (used controller-side) |

#### 3.2.3 Decision Tree

```
                    exit_code
                       │
          ┌────────────┼────────────────────────┐
          ▼            ▼                        ▼
    EXIT_INFRA(-4)  EXIT_WAIT(-1)          Guardrail(10-19)
    → InfraError    → InfraError           → InfraError
                                                │
                                                ▼
                                           exit_code == 0
                                           → Evasion
                                                │
                                                ▼
                                           timed_out?
                                        ┌──yes──┼──no──┐
                                        ▼              ▼
                                   has_launched?   EXIT_NO_CODE(-2)
                                   ┌──yes──┐       → Detected
                                   ▼       ▼            │
                               Evasion  Stalled         ▼
                                                   AV NTSTATUS?
                                                   (0xC0000906/07)
                                                   → Detected
                                                        │
                                                        ▼
                                                   Crash NTSTATUS?
                                                   (0xC0000005, etc.)
                                                   → Ambiguous
                                                        │
                                                        ▼
                                                   Carrier codes?
                                                   (30-39)
                                                   → Ambiguous
                                                        │
                                                        ▼
                                                   Other nonzero
                                                   → Ambiguous
```

#### 3.2.4 Evidence Extraction

The classifier scans `TelemetryData` events for checkpoint events to determine:
- **`has_launched`:** Whether the artifact reached the `Launching` checkpoint (payload execution started). Uses the `automutate_common::has_launched()` helper which checks checkpoint name patterns.
- **`last_checkpoint`:** The name of the last checkpoint event, used for diagnostic reporting (e.g., `"Decoding"`, `"Launching"`, `"GuardrailPassed"`).

#### 3.2.5 Known Exit Code Tables

**AV/EDR NTSTATUS codes** (always `Detected`):
| Code | Name |
|------|------|
| `0xC0000906` | STATUS_VIRUS_INFECTED |
| `0xC0000907` | STATUS_VIRUS_DELETED |

**Crash NTSTATUS codes** (always `Ambiguous`):
| Code | Name |
|------|------|
| `0xC0000005` | STATUS_ACCESS_VIOLATION |
| `0xC0000409` | STATUS_STACK_BUFFER_OVERRUN |
| `0xC00000FD` | STATUS_STACK_OVERFLOW |
| `0xC0000374` | STATUS_HEAP_CORRUPTION |
| `0xC0000094` | STATUS_INTEGER_DIVIDE_BY_ZERO |

#### 3.2.6 Test Coverage

The module contains 18 unit tests covering every decision tree branch, verdict roundtrip serialization, backward compatibility with v2 verdict strings (`killed_pre_payload` → `Detected`, `crashed` → `Ambiguous`, etc.), and `is_detected()` semantics for all 7 verdict variants.

---

### 3.3 `guards.rs` — RAII Resource Cleanup Guards (163 lines)

Three guard types ensure resources are cleaned up on every exit path — normal return, early error return, and panic.

#### 3.3.1 `RedEdrGuard`

**Wraps:** `RedEdrCollector`
**Invariant:** RedEDR is reset before the next run, even if the current run panics.

| Method | Behavior |
|--------|----------|
| `new(collector)` | Creates guard, `reset_on_drop = true` |
| `collector()` | Returns `&RedEdrCollector` for operations |
| `reset_now()` | Explicit reset, sets `reset_on_drop = false` (disarms) |
| `Drop` | If `reset_on_drop`: spawns fire-and-forget HTTP POST to `/api/trace/reset` |

**Drop behavior:** Since `Drop` cannot be async, the guard uses `tokio::runtime::Handle::try_current()` to get the current runtime and spawns a cleanup task. This is a best-effort safety net — the normal path is to call `reset_now()` explicitly in Phase 9.

#### 3.3.2 `ProcessGuard`

**Wraps:** `tokio::process::Child`
**Invariant:** Child process is killed if execution is interrupted before normal completion.

| Method | Behavior |
|--------|----------|
| `new(child)` | Creates guard, `should_kill = true` |
| `child_mut()` | Returns `&mut Child` for waiting/output capture |
| `disarm()` | Takes ownership of child, sets `should_kill = false` |
| `Drop` | If `should_kill`: calls `child.start_kill()` (synchronous signal send) |

**Key detail:** `start_kill()` is used instead of `kill()` because `Drop` is synchronous. It sends the kill signal but doesn't await termination.

#### 3.3.3 `MonitorGuard`

**Wraps:** Stop signal channel + monitor task handle + event consumer handle
**Invariant:** Monitor background task is stopped and event consumer is aborted on all exit paths.

| Method | Behavior |
|--------|----------|
| `new(stop_tx, handle, event_consumer)` | Creates guard |
| `stop()` | Graceful shutdown: send stop signal → abort consumer → await monitor with timeout |
| `Drop` | Send stop signal + abort consumer (no awaiting — Drop is sync) |

**Shutdown order matters:** The event consumer is aborted BEFORE awaiting the monitor. If the monitor tries to send on a full channel while the consumer is blocked, the system would deadlock.

---

### 3.4 `monitor.rs` — Real-Time Execution Monitoring (380 lines)

Lightweight polling-based monitor that runs alongside artifact execution, providing real-time observability to the controller.

#### 3.4.1 Architecture

```
┌────────────────────────────────────────────────┐
│              ExecutionMonitor                    │
│                                                  │
│  Every 3 seconds:                                │
│  ┌──────────────────────────────────────────┐   │
│  │ 1. Check process alive (is_process_alive)│   │
│  │ 2. Get CPU/memory (sysinfo per-PID)     │   │
│  │ 3. Query RedEDR /api/stats (event count)│   │
│  │ 4. Classify event type:                  │   │
│  │    ├── "started" (initial)               │   │
│  │    ├── "heartbeat" (normal)              │   │
│  │    ├── "telemetry_idle" (stuck)          │   │
│  │    ├── "approaching_timeout" (<5s left)  │   │
│  │    └── "terminated" (process dead)       │   │
│  │ 5. Send to controller (ControlPlaneSink) │   │
│  │ 6. Send to local channel (logging)       │   │
│  └──────────────────────────────────────────┘   │
│                                                  │
│  Stops on: stop_rx signal OR process terminated  │
└────────────────────────────────────────────────┘
```

#### 3.4.2 `MonitorConfig`

| Field | Type | Purpose |
|-------|------|---------|
| `run_id` | String | Run identity for status reports |
| `job_id` | String | Job identity |
| `worker_id` / `worker_ip` | String | Worker identity |
| `artifact_name` | String | Artifact being executed |
| `pid` | u32 | Process ID to monitor |
| `rededr_base_url` | String | RedEDR API endpoint |
| `timeout_seconds` | i32 | Execution timeout |

#### 3.4.3 Idle Detection Algorithm

The monitor uses a two-signal idle detection to avoid false positives:

```
events_stale = (event_count == last_event_count) AND process_alive
cpu_idle = cpu_percent <= CPU_IDLE_THRESHOLD (5%)

if events_stale AND cpu_idle:
    idle_count += 1    ← truly idle
elif events_stale AND NOT cpu_idle:
    idle_count = 0     ← busy but no new events (not truly idle)
else:
    idle_count = 0     ← new events, not idle

if idle_count >= IDLE_COUNT_THRESHOLD (3):
    event_type = "telemetry_idle"
```

**Why two signals:** A process can be CPU-busy without generating telemetry events (e.g., during anti-emulation loops). Incrementing `idle_count` only when BOTH signals indicate idle prevents false "stuck" reports.

#### 3.4.4 Timeout Approach Detection

When `elapsed_seconds >= (timeout_seconds - 5)`, the monitor emits `"approaching_timeout"` events to alert the controller that the process is about to be killed.

#### 3.4.5 Status Delivery

Status updates are sent through two channels:
1. **ControlPlaneSink** → controller (via bidirectional stream or NullSink)
2. **mpsc channel** → local event consumer (for logging)

The sink send has a 1-second timeout to prevent the monitor from blocking if the stream is congested.

#### 3.4.6 Tuning Constants

| Constant | Value | Location | Purpose |
|----------|-------|----------|---------|
| `MONITOR_POLL_INTERVAL_SECS` | 3 | constants.rs | Polling frequency |
| `CPU_IDLE_THRESHOLD` | 5% | constants.rs | Below this = idle |
| `IDLE_COUNT_THRESHOLD` | 3 | constants.rs | Consecutive idle polls before flagging |
| `TIMEOUT_APPROACH_SECS` | 5 | constants.rs | Seconds before timeout to warn |
| `CLEANUP_TIMEOUT_SECS` | 10 | constants.rs | Max wait for monitor shutdown |

---

### 3.5 `sink.rs` — Transport Abstraction (107 lines)

Decouples the execution engine from the gRPC transport layer using a trait-based strategy pattern.

#### 3.5.1 `ControlPlaneSink` Trait

```rust
#[tonic::async_trait]
pub trait ControlPlaneSink: Send + Sync {
    async fn send_status(&self, status: ExecutionStatusReport) -> Result<()>;
    async fn send_telemetry(&self, batch: TelemetryBatch) -> Result<()>;
    async fn send_ack(&self, request_id: &str, success: bool, error: &str) -> Result<()>;
}
```

Three operations cover all outbound communication during execution:
- **Status:** `ExecutionStatusReport` — periodic monitoring updates
- **Telemetry:** `TelemetryBatch` — collected events after execution
- **Ack:** Acknowledgements for controller commands

#### 3.5.2 Implementations

| Implementation | When Used | Behavior |
|---------------|-----------|----------|
| `StreamSink` | Bidirectional stream is active | Wraps `mpsc::Sender<Result<WorkerMessage, Status>>`, sends messages as `WorkerMessage` envelope variants |
| `NullSink` | No stream (standalone worker mode) | All operations are no-ops with debug logging |

#### 3.5.3 `build_sink()` Factory

```rust
pub fn build_sink(
    tx: Option<&mpsc::Sender<Result<WorkerMessage, Status>>>,
) -> Arc<dyn ControlPlaneSink>
```

Constructs the appropriate sink based on whether a stream tx channel is available. Called by `api/run.rs` before invoking the engine.

#### 3.5.4 Arc-Cycle Prevention

The `StreamSink` holds only the `mpsc::Sender` channel, NOT an `Arc<StreamHandler>`. This is critical — if it held `Arc<StreamHandler>` and the `StreamHandler` held `Arc<WorkerAgentService>` which held `Arc<StreamHandler>`, the reference count would never reach zero.

---

### 3.6 `state.rs` — Execution Lock State Machine (122 lines)

Ensures only one artifact executes at a time per worker. This is critical for clean RedEDR telemetry — concurrent executions would produce cross-contaminated event streams.

#### 3.6.1 `ExecutionState` Enum

```rust
pub enum ExecutionState {
    Idle,
    Running {
        job_id: String,
        artifact: String,
        run_id: String,
    },
}
```

**Why an enum, not `busy: bool`:** The enum makes it impossible to have `busy=true` with `current_job_id=None` or vice versa. State and metadata are always consistent.

#### 3.6.2 State Transitions

```
         acquire(job, artifact, run_id)
Idle ─────────────────────────────────► Running { job, artifact, run_id }
  ▲                                         │
  │              release()                  │
  └─────────────────────────────────────────┘

  acquire() while Running → ExecutionBusyError
```

| Method | From | To | Returns |
|--------|------|----|---------|
| `acquire()` | Idle | Running | `Ok(())` |
| `acquire()` | Running | Running (unchanged) | `Err(ExecutionBusyError)` |
| `release()` | Running | Idle | `(job_id, artifact)` |
| `release()` | Idle | Idle | `("unknown", "unknown")` |

#### 3.6.3 `ExecutionLockGuard` — RAII Automatic Release

```rust
pub struct ExecutionLockGuard {
    lock: Arc<Mutex<ExecutionState>>,
}
```

On `Drop`, spawns a tokio task to acquire the mutex and call `release()`. This guarantees the lock is freed even if the execution function panics or returns early.

**Why spawn instead of blocking:** `Drop` is synchronous, but `Mutex::lock()` is async. The guard spawns a task on the current tokio runtime to perform the async release.

#### 3.6.4 `ExecutionBusyError`

Returned when `acquire()` is called while the worker is already executing. Contains the current job's identity for diagnostic reporting. Mapped to `Status::resource_exhausted` at the API layer.

---

### 3.7 `types.rs` — Domain Types & Helpers (295 lines)

Shared types and utility functions used by the engine, api layer, and session stream handler.

#### 3.7.1 Synthetic Exit Codes

| Constant | Value | Meaning | Classifier Verdict |
|----------|-------|---------|-------------------|
| `EXIT_WAIT_FAILED` | -1 | OS error on `child.wait()` | InfraError |
| `EXIT_NO_CODE` | -2 | Process externally terminated (no exit code) | Detected |
| `EXIT_TIMEOUT` | -3 | Timeout expired, process killed | Evasion/Stalled |
| `EXIT_INFRA` | -4 | Setup failure, never executed | InfraError |

These are always negative, distinguishing them from real Windows exit codes (non-negative).

#### 3.7.2 Core Types

**`RunRequest`** — Per-execution parameters:
| Field | Type | Source |
|-------|------|--------|
| `job_id` | String | From SampleRequest |
| `artifact_id` | String | From SampleRequest |
| `timeout_seconds` | u32 | From SampleRequest |
| `run_id` | String | Controller-assigned or UUID |

**`RunContext`** — Worker-level context (derived from config):
| Field | Type | Derivation |
|-------|------|------------|
| `worker_id` | String | From service |
| `config` | WorkerConfig | From service |
| `artifact_path` | PathBuf | `{artifacts_path}/{artifact_id}.exe` |
| `telemetry_dir` | PathBuf | `{artifacts_path}/telemetry_{artifact_id}` |
| `artifact_name` | String | `{artifact_id}.exe` |

**`RunOutcome`** — Complete execution result:
| Field | Type | Description |
|-------|------|-------------|
| `exit_code` | i32 | Process exit code or synthetic code |
| `timed_out` | bool | Whether timeout was hit |
| `stdout` / `stderr` | String | Captured output |
| `telemetry_events` | Vec\<TelemetryData\> | All collected events |
| `elapsed` | Duration | Wall-clock execution time |
| `phase_timings` | RunPhaseTimings | Per-phase timing breakdown |
| `detection_verdict` | String | Classifier verdict string |
| `last_checkpoint` | String | Last checkpoint before exit |

**`RunPhaseTimings`** — Execution phase breakdown:
| Field | Type | Phase |
|-------|------|-------|
| `rededr_setup_ms` | u64 | Phase 2: RedEDR configuration |
| `process_spawn_ms` | u64 | Phase 4: Child process spawn |
| `process_wait_ms` | u64 | Phase 6: Wait for completion |
| `telemetry_collect_ms` | u64 | Phase 7: Telemetry collection |
| `rededr_reset_ms` | u64 | Phase 9: RedEDR reset |

#### 3.7.3 Response Builders

**`sample_response_ok()`** — Maps `RunOutcome` to `SampleResponse`:
- Uses classifier verdict for `detected` flag when available
- Falls back to legacy exit code logic: `EXIT_NO_CODE` → detected, `0` → not detected, positive nonzero → detected
- Computes `elapsed_ms` from `Duration`

**`sample_response_error()`** — Builds error `SampleResponse`:
- Sets `exit_code = EXIT_INFRA`, `success = false`, `detection_verdict = "infra_error"`

#### 3.7.4 Exit Code Interpretation

`describe_exit()` provides human-readable descriptions for exit codes:

| Code Range | Description |
|-----------|-------------|
| -4 to -1 | Synthetic engine codes |
| 0 | Success |
| 10-19 | Guardrail failed |
| 30 | Carrier: VirtualAlloc failed |
| 31 | Carrier: VirtualProtect failed |
| 32-33 | Carrier: PEB resolution failed |
| 34-39 | Carrier: unknown error |
| 0x80000000+ | NTSTATUS (Windows-only: uses `RtlNtStatusToDosError` + `FormatMessageW` for human-readable messages) |

The NTSTATUS translation is `#[cfg(target_os = "windows")]` conditional — on non-Windows (cross-compilation), it returns `None`.

#### 3.7.5 `resolve_run_id()`

Prefers a controller-provided run ID over a locally generated UUID:
```rust
requested.filter(|s| !s.is_empty())
    .map(String::from)
    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
```

---

## 4. Cross-Module Data Flow

### 4.1 Full Execution Sequence

```
api/run.rs OR session/stream_handler.rs
    │
    │  Calls with RunRequest + RunContext + ControlPlaneSink
    ▼
engine::execute_run()
    │
    ├──► guards::RedEdrGuard::new(collector)
    │        └── RAII: resets RedEDR on drop
    │
    ├──► guards::ProcessGuard::new(child)
    │        └── RAII: kills process on drop
    │
    ├──► monitor::ExecutionMonitor::new(config, sink)
    │    └──► guards::MonitorGuard::new(stop_tx, handle, consumer)
    │             └── RAII: stops monitor on drop
    │
    ├──► [wait for process or timeout]
    │
    ├──► [collect telemetry from 5 sources]
    │
    ├──► classifier::classify_run(exit_code, timed_out, events)
    │        └── Returns (DetectionVerdict, Option<last_checkpoint>)
    │
    ├──► sink.send_telemetry(batch)
    │        ├── StreamSink → bidirectional stream → controller
    │        └── NullSink → /dev/null
    │
    ├──► rededr_guard.reset_now()
    │
    └──► Return RunOutcome
              │
              ▼
         types::sample_response_ok()
              │
              ▼
         SampleResponse → gRPC → Controller
```

### 4.2 Shared State Dependencies

| Module | Uses `ExecutionState` | Uses `ControlPlaneSink` | Uses Guards |
|--------|----------------------|------------------------|-------------|
| engine.rs | No (caller manages) | Yes (for telemetry + status) | Yes (all 3) |
| monitor.rs | No | Yes (for status reports) | No |
| classifier.rs | No | No | No |
| sink.rs | No | Defines trait | No |
| state.rs | Defines type | No | No |
| guards.rs | No | No | Defines types |
| types.rs | No | No | No |

### 4.3 External Dependencies

| Dependency | Used By | Purpose |
|-----------|---------|---------|
| `telemetry::collectors::rededr` | engine.rs, guards.rs | RedEDR HTTP API client |
| `telemetry::collectors::trace` | engine.rs | Named pipe trace reader |
| `telemetry::pipeline` | engine.rs | Telemetry packaging (trace, coverage, checkpoints) |
| `infra::process` | engine.rs | Process spawn, kill, alive check |
| `infra::system` | engine.rs | Telemetry dir management, artifact cleanup |
| `automutate_common` | classifier.rs, types.rs | `DetectionVerdict` enum, `has_launched()` |
| `sysinfo` | monitor.rs | Per-PID CPU/memory metrics |
| `reqwest` | monitor.rs, guards.rs | RedEDR HTTP API calls |

---

## 5. Summary Statistics

| Metric | Value |
|--------|-------|
| Files | 8 |
| Total lines | 2272 |
| Public functions | 12 |
| RAII guards | 3 (RedEdrGuard, ProcessGuard, MonitorGuard) |
| Trait definitions | 1 (ControlPlaneSink) |
| Trait implementations | 2 (StreamSink, NullSink) |
| Error types | 2 (RunError, ExecutionBusyError) |
| Domain types | 5 (RunRequest, RunContext, RunOutcome, RunPhaseTimings, ExecutionState) |
| Detection verdicts | 7 (Evasion, Detected, Ambiguous, Stalled, InfraError, MutationFailed, Anomaly) |
| Synthetic exit codes | 4 (-1 through -4) |
| Execution phases | 10 (in execute_run) |
| Telemetry sources | 5 (RedEDR, trace JSONL, trace binary, BB coverage, checkpoints) |
| Unit tests | 19 (18 in classifier, 1 in monitor) |
| Tuning constants | 6 (in constants.rs) |
