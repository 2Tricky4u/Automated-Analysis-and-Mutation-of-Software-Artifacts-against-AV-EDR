# Dispatch Module Deep Analysis

## Overview

The `controller/src/dispatch/` module is the **execution engine** of the AutoMutate++ controller. It owns the entire lifecycle of mutation experiment jobs: from scheduling rounds of artifact builds, through dispatching runs to sandboxed Windows VMs, to aggregating differential results and computing evasion scores.

While the `api/` module translates gRPC into domain operations and the `vm/` module manages physical VM connections, the dispatch module is where **the actual experiment loop runs**.

```
    API (gRPC)                                   VM (connections)
       |                                              |
       | JobSession via mpsc channel                  | TargetEvent
       v                                              v
 +==============+    spawns    +============+    reserves/releases
 | Orchestrator | ----------> | JobWorker  | --+  via TargetManager
 +==============+             +============+   |
       |                           |           |
       | indexes round/run         | produces  |
       | to ES                     | RunEnvelopes
       |                           v           |
       |                    +==========+       |
       |                    | RunPool  | <-----+--- VMExecutor takes runs
       |                    +==========+       |
       |                         |             |
       |                         | routes results back
       |                         v             |
       |                    JobWorker.on_result |
       |                         |             |
       |                    finalize_round     |
       |                         |             |
       +--- async ES indexing <--+             |
       +--- async coverage computation         |
       +--- async triage extraction            |
```

---

## File Inventory

| File | Lines | Role |
|------|------:|------|
| `mod.rs` | 34 | Module declarations + re-exports |
| `channels.rs` | 128 | Channel message types between all components |
| `orchestrator.rs` | 738 | Central coordinator: spawns JobWorkers, handles VM events, indexes to ES |
| `job_worker.rs` | 1077 | Per-job lifecycle: round production, artifact builds, result aggregation, triage |
| `run_pool.rs` | 587 | OS-sharded run queue with capability filtering and result routing |
| `vm_executor.rs` | 412 | Thin per-VM dispatcher: reserve → upload → execute → release |
| `types/mod.rs` | 23 | Re-exports for all type submodules |
| `types/ids.rs` | 72 | Newtype ID wrappers (JobId, RoundId, RunId, WorkerId, TargetId) |
| `types/config.rs` | 107 | ModuleSelectionSpec (7 module slots) + ModularBuildSpec |
| `types/run.rs` | 143 | RunEnvelope, RunType, ArtifactRef, VMInfo, capability matching, artifact chunking |
| `types/round.rs` | ~1418 | DifferentialCategory, RoundSpec, RoundAgg, RoundSummary, evasion scoring, dryrun override logic |
| `types/session.rs` | 199 | JobSession (ephemeral state), JobStatus, JobOutcome, JobInfo |

**Total: ~4,938 lines** (including ~1,200 lines of tests)

---

## Architecture: The Four Components

The dispatch system is a **producer-consumer pipeline** with four distinct components:

```
                +--------------+
                | Orchestrator |   (1 instance, long-lived)
                +--------------+
                  |          ^
           spawns |          | JobWorkerEvent
                  v          |
            +------------+   |
            | JobWorker  |---+   (1 per active job, spawned as tokio task)
            +------------+
                  |
           adds runs
                  v
            +----------+
            | RunPool   |        (1 shared instance, OS-sharded queues)
            +----------+
                  ^
           takes runs
                  |
            +------------+
            | VMExecutor |       (1 per connected VM, spawned as tokio task)
            +------------+
```

### Data Flow Summary

1. **Orchestrator** receives `JobSession` via channel → spawns `JobWorker` task
2. **JobWorker** calls selector → builds artifacts → creates 3 `RunEnvelope`s → pushes to `RunPool`
3. **VMExecutor** wakes on `Notify` signal → takes a run from pool → reserves VM → uploads artifact → dispatches command → waits for result
4. **VMExecutor** receives result → releases VM → routes `JobRunResult` back through `RunPool`
5. **RunPool** routes result to originating `JobWorker` via per-job `mpsc::Sender`
6. **JobWorker** aggregates results in `RoundAgg` → when complete, finalizes round → emits `JobWorkerEvent::RoundCompleted`
7. **Orchestrator** indexes round + runs to ES, computes coverage asynchronously, sends `CoverageCorrection` back to JobWorker

---

## 1. `mod.rs` — Module Root (34 lines)

Declares submodules and re-exports the public API:

```rust
pub use channels::{JobControlCommand, RemoteRunResult};
pub use orchestrator::Orchestrator;
pub use run_pool::RunPool;
pub use vm_executor::{ArtifactSender, VMExecutor};
pub use types::{JobId, JobSession, ModularBuildSpec, ModuleSelectionSpec, ...};
```

---

## 2. `channels.rs` — Message Types (128 lines)

Defines all channel message types flowing between dispatch components. These form the inter-component protocol:

### Service → Orchestrator

```rust
enum JobControlCommand {
    Stop { job_id: JobId },
}
```

### VMExecutor → RunPool → JobWorker

```rust
struct RemoteRunResult {
    run_id, detected, exit_code, success, error,
    elapsed_ms, detection_verdict, last_checkpoint,
}

struct JobRunResult {
    run_id, job_id, round_id, outcome: RunOutcome, vm_id,
}
```

`RemoteRunResult` is what comes from the VM stream. It converts to `RunOutcome` via `From` impl, then gets wrapped in `JobRunResult` for pool routing.

### JobWorker → Orchestrator

```rust
enum JobWorkerEvent {
    RoundCompleted(Box<RoundCompletedData>),   // carries all data for ES indexing
    JobCompleted { job_id, outcome },
}
```

`RoundCompletedData` is the richest message — it carries both run outcomes, mutation specs, module selections, VM IDs, assembled source, and dryrun data. Boxed to keep the enum small (avoids stack bloat from the large variant).

### Orchestrator → JobWorker

```rust
struct CoverageCorrection {
    round_number, coverage_percent, blended_evasion_score,
}
```

Sent asynchronously after coverage computation completes. Patches the JobWorker's in-memory round history so the selector sees blended scores.

---

## 3. `orchestrator.rs` — Central Coordinator (738 lines)

### Struct

```rust
struct Orchestrator {
    run_pool:        Arc<RunPool>,
    targets:         Arc<TargetManager>,
    storage:         Arc<EsStorage>,
    job_workers:     HashMap<JobId, JobHandle>,  // shutdown_token + correction_tx
    job_event_tx/rx: mpsc channel,
    vms:             HashMap<WorkerId, WorkerInfo>,
    events_rx:       mpsc::Receiver<TargetEvent>,
    job_submit_rx:   mpsc::Receiver<JobSession>,
    job_control_rx:  mpsc::Receiver<JobControlCommand>,
}
```

### Main Loop

Uses `tokio::select! { biased; ... }` with four branches, in priority order:

1. **Job control** (`job_control_rx`) — Stop commands (highest priority, immediate cancellation)
2. **Job submissions** (`job_submit_rx`) — Spawn new JobWorkers
3. **JobWorker events** (`job_event_rx`) — Round/job completion → ES indexing
4. **Target events** (`events_rx`) — VM connect/disconnect/message (lowest priority)

The `biased` keyword ensures stop commands are always processed before new work.

### Job Spawning (`spawn_job_worker`)

1. **Constraint resolution** — If job has no `target_os` or `required_capabilities`, auto-resolves from available targets. Prefers available (idle) targets over busy ones.
2. **Selector injection** — Creates the appropriate `Selector` impl based on `job.search_space.selector`:
   - `SelectorType::Fuzzer` → `FuzzerSelector`
   - `SelectorType::Coverage` → `CoverageSelector`
   - `SelectorType::Token` → `TokenSelector`
   - `SelectorType::Random` → `RandomSelector`
3. **Spawns** `JobWorker::new(...).run()` as a `tokio::spawn` task.
4. **Stores** `JobHandle { shutdown_token, correction_tx }` for later stop/correction.
5. **Updates ES** — Sets job status to "running" (fire-and-forget spawn).

### Round Completion Handling

When `JobWorkerEvent::RoundCompleted` arrives:

1. **Spawns** `index_round_and_runs()` — indexes round doc + baseline run + instrumented run + optional dryrun run + updates job progress in ES.
2. **Spawns** `compute_round_coverage()` — queries trace data from ES, builds `SourceMap`, computes line coverage, updates ES round doc, computes blended evasion score, sends `CoverageCorrection` back to JobWorker.

Both are fire-and-forget spawns to avoid blocking the select loop.

### Target Event Handling

Handles VM lifecycle:
- `Connected` → marks target connected, tracks in local `vms` map
- `Disconnected` → marks target offline, removes from map
- `Message` → dispatches to `handle_worker_message()`:
  - `Registration` → registers metadata (OS, capabilities, tools) via TargetManager
  - `Status` → updates health timestamp
  - `Telemetry` → bulk indexes to ES (spawned async)
  - `SampleResponse` → logged (release handled by VMExecutor)
  - `ExecutionStatus`, `Ack` → debug logged

### Coverage Computation (`compute_round_coverage`)

Critical async flow:

1. Queries trace content from ES for the instrumented run.
2. Retries once after 1s if not found (edge-case ES refresh latency).
3. Parses `line` numbers from trace JSON lines.
4. Builds `SourceMap` from assembled source → computes `CoverageResult`.
5. Updates ES round doc with coverage data.
6. Computes `blended_evasion_score = 0.7 * coverage + 0.3 * time_factor` (per-category ranges).
7. Sends `CoverageCorrection` to JobWorker so selector sees updated scores.

---

## 4. `job_worker.rs` — Per-Job Lifecycle (1077 lines)

The most complex component. Each active job has one JobWorker running as an independent tokio task.

### Struct

```rust
struct JobWorker {
    job:                JobSession,
    run_pool:           Arc<RunPool>,
    result_rx:          mpsc::Receiver<JobRunResult>,
    result_tx:          mpsc::Sender<JobRunResult>,  // registered with pool
    round_aggs:         HashMap<RoundId, RoundAgg>,
    event_tx:           mpsc::Sender<JobWorkerEvent>,
    selector:           Arc<dyn Selector>,
    shutdown_token:     CancellationToken,
    artifact_cleanup:   Vec<PathBuf>,
    baseline_payload:   Option<PreparedPayload>,     // cached
    instrumented_payload: Option<PreparedPayload>,   // cached
    cached_payload:     Option<Vec<u8>>,              // raw .bin
    correction_rx:      mpsc::Receiver<CoverageCorrection>,
    storage:            Option<Arc<EsStorage>>,
    latest_guidance:    Option<TriageGuidance>,
    guidance_rx:        mpsc::Receiver<TriageGuidance>,
    guidance_tx:        mpsc::Sender<TriageGuidance>,
}
```

### Main Loop

`tokio::select! { biased; ... }` with six branches:

1. **Shutdown signal** (`shutdown_token.cancelled()`) — Job cancelled externally
2. **Pool shutdown** (`pool_shutdown.cancelled()`) — Global shutdown
3. **Run results** (`result_rx.recv()`) — From VMExecutor via RunPool
4. **Coverage corrections** (`correction_rx.recv()`) — Async blended scores from Orchestrator
5. **Triage guidance** (`guidance_rx.recv()`) — From async triage extraction
6. **Production check interval** (100ms tick) — Produce more rounds if possible, check dryrun grace expiry, check job completion

### Backpressure Constants

```rust
const MAX_IN_FLIGHT_ROUNDS: usize = 5;   // Max rounds being aggregated
const MAX_PENDING_RUNS: usize = 9;       // Max pending runs in pool (= 3 rounds × 3 runs)
const DRYRUN_GRACE_PERIOD_SECS: u64 = 5; // Wait for dryrun after core runs complete
```

### Round Production (`produce_round`)

The core experiment loop, called when `can_produce_round()` is true:

1. **Start round** — `job.start_round()` increments counter, generates `{job_id}-round-{N}` ID.
2. **Select mutations** — `selector.select(job_id, round_num, search_space, defaults, history, guidance)` returns `Selection { modules, mutations, rationale }`.
3. **Build baseline artifact** — `trace_mode = "off"`, no instrumentation.
4. **Static Defender scan** — Runs `MpCmdRun.exe -Scan -ScanType 3 -File <path>` from WSL. If exit code == 2 (detected), short-circuits with `StaticDetection` category — no VM dispatch needed.
5. **Build instrumented artifact** — `trace_mode = "lines"` (or job-configured mode).
6. **Create 3 RunEnvelopes** — baseline + instrumented + dryrun.
7. **Create RoundAgg** — ephemeral join state to track the 3 results.
8. **Add runs to pool** — `run_pool.add_runs(runs)` → wakes VMExecutors.

### Artifact Building (`build_artifact`)

- Creates `ArtifactBuilder` with optional MSVC-compat mode.
- **Payload caching** — Raw .bin bytes read once, stored in `cached_payload`. Precomputed payload headers (encoded + checkpoint stub) cached separately for baseline (`trace_mode=off`) and instrumented (`trace_mode=lines`). Cache disabled when `cache_payload=false`.
- Converts `MutationSpec` (with JSON params) to `build::mutator::MutationSpec` (with `HashMap<String,String>` params).
- Calls `ArtifactBuilder::build(BuildInput::ModularTemplate { ... })`.

### Result Handling (`on_result`)

1. **O(1) lookup** by `round_id` (not linear scan).
2. Matches `run_id` against `baseline_run_id`, `instrumented_run_id`, or `dryrun_run_id` in the `RoundAgg`.
3. **Completeness check** — Core runs (baseline + instrumented) must both be present.
4. **Dryrun grace period** — When core runs are done but dryrun is missing:
   - Sets `dryrun_deadline = Instant::now() + 5s`
   - If deadline expires (checked in production tick), removes unclaimed dryrun from pool, finalizes without it
   - If dryrun arrives before deadline, finalizes immediately with all 3

### Round Finalization (`finalize_round`)

1. Defers artifact path cleanup to job end (content-addressed SHA256 paths may collide across rounds).
2. Calls `agg.to_summary()` → computes `DifferentialCategory`, `evasion_score`, `behavior_match`, `detection_verdict`.
3. Records summary in `job.rounds` (selector reads history from here).
4. Updates job registry for API visibility.
5. **Spawns async triage extraction** — `crate::triage::extractor::extract_and_score()` runs in background. On completion, sends `TriageGuidance` (avoid/seek tokens) back to JobWorker via `guidance_tx`.
6. Emits `JobWorkerEvent::RoundCompleted(Box<RoundCompletedData>)` to Orchestrator for ES indexing.

### Static Defender Scan

Pre-VM-dispatch optimization:

```
WSL path → wslpath -w → Windows path
           → MpCmdRun.exe -Scan -ScanType 3 -File <path> -DisableRemediation
           → exit code 2 = statically detected
```

If detected, the round is immediately finalized with `DifferentialCategory::StaticDetection` and `evasion_score = 0.0`, saving two VM dispatches.

### Cleanup

On job exit (completion, cancellation, or pool shutdown):
1. Unregisters from RunPool.
2. Deduplicates and deletes all artifact files.
3. Emits `JobWorkerEvent::JobCompleted`.

---

## 5. `run_pool.rs` — OS-Sharded Run Queue (587 lines)

The shared queue connecting JobWorkers (producers) and VMExecutors (consumers).

### Struct

```rust
struct RunPool {
    pending:         DashMap<RunId, RunEnvelope>,           // lock-free storage
    by_os:           DashMap<String, Mutex<VecDeque<RunId>>>, // per-OS queues
    runs_available:  Notify,                                // signal VMExecutors
    result_routers:  RwLock<HashMap<JobId, Sender<JobRunResult>>>,
    job_registry:    DashMap<JobId, JobInfo>,                // API visibility
    shutdown_token:  CancellationToken,
    metrics:         Mutex<RunPoolMetrics>,
}
```

### OS Sharding

Runs are partitioned by `required_os`. Each OS has its own `Mutex<VecDeque<RunId>>`. VMExecutors only lock the queue for their OS, not the entire pool. This reduces contention when multiple OS types coexist.

### Run Lifecycle

1. **`add_runs(runs)`** — Stores in `pending` DashMap (lock-free), enqueues RunId into OS queue (short lock). Calls `runs_available.notify_waiters()` to wake ALL idle VMExecutors.
2. **`take_run(vm_os, vm_capabilities)`** — Pops from the OS queue, checks capabilities. **Bidirectional dryrun guard**: dryrun VMs only take dryrun runs, non-dryrun VMs never take dryrun runs. Non-matching runs are pushed back to queue tail. Bounded scan (at most `n` iterations) prevents infinite rotation.
3. **`route_result(result)`** — Looks up `result_routers` by `job_id`, sends `JobRunResult` through the per-job channel.
4. **`remove_run(run_id)`** — Removes from `pending` map. Stale entries in OS queues are skipped by `take_run()` (no need to also remove from queue).

### Job Registry

Maintained for API-layer queries (`list_jobs`, `list_running_jobs`, `get_job_info`):
- `register_job()` — on JobWorker start
- `update_job_progress()` — after each round
- `complete_job()` — on job completion
- Jobs are kept in registry even after completion (for historical visibility)

### Metrics

```rust
struct RunPoolMetrics {
    total_runs_added, total_runs_taken, total_results_routed,
    active_jobs, total_rounds_completed, total_jobs_completed,
}
```

### Backpressure

`MAX_QUEUE_SIZE = 50` per OS queue — warn but don't block. Actual backpressure is enforced by `JobWorker.can_produce_round()` checking `pending_runs_for_job()`.

---

## 6. `vm_executor.rs` — Thin VM Dispatcher (412 lines)

One VMExecutor per connected VM. It knows nothing about jobs or rounds — just takes runs and returns results.

### Struct

```rust
struct VMExecutor {
    id:              String,
    info:            VMInfo,           // os, capabilities
    targets:         Arc<TargetManager>,
    run_pool:        Arc<RunPool>,
    remote_tx:       mpsc::Sender<ControllerMessage>,  // to VM
    remote_rx:       mpsc::Receiver<RemoteRunResult>,   // from VM
    artifact_sender: Arc<dyn ArtifactSender>,
    in_flight:       Option<InFlightRun>,               // at most 1
}
```

### Main Loop

Signal-driven `tokio::select! { biased; ... }`:

1. **Shutdown** — Cleans up in-flight run (routes error result) or releases VM
2. **VM result** (only when in-flight) — Processes result, immediately tries to take another run
3. **Pool signal** (only when idle) — Wakes up, tries to take a run

### Dispatch Flow

```
take_run() → reserve VM → upload artifact → send RunSampleCommand → wait for result
                                                                        |
                                                                        v
           route_result() ← build JobRunResult ← receive RemoteRunResult ← release VM
```

1. **Reserve** — `targets.reserve(&self.id)` marks VM as Busy.
2. **Upload** — `artifact_sender.send_artifact(vm_id, artifact_id, path)` streams the .exe to the VM (chunked via the `ArtifactSender` trait).
3. **Command** — Sends `RunSampleCommand` with `trace_mode` from `RunType` (`off` for Baseline/DryRun, `lines` for Instrumented), timeout, `is_dryrun` flag.
4. **Result** — Verifies `run_id` matches, releases VM, routes `JobRunResult` via `run_pool.route_result()`.
5. **Error** — On any failure (missing SHA256, upload error, channel closed), releases VM and routes an error result with `detection_verdict = "infra_error"`.

### ArtifactSender Trait

```rust
trait ArtifactSender: Debug {
    fn send_artifact(&self, vm_id: &str, artifact_id: &str, path: &Path) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}
```

Implemented by `TargetManager` (real) and `MockArtifactSender` (tests).

---

## 7. `types/` — Data Model (6 files, ~1,962 lines)

### 7.1 `ids.rs` — Type-Safe IDs (72 lines)

Five newtype wrappers over `String`:

| Type | Meaning | Example |
|------|---------|---------|
| `JobId` | Job identity | `job-20260304-143022-a1b2c3d4` |
| `RoundId` | Round identity | `job-xxx-round-3` |
| `RunId` | Run identity | `job-xxx-round-3-baseline` |
| `WorkerId` | Ephemeral session identity | Changes each reconnect |
| `TargetId` | Stable machine identity | Persists across reconnects |

All use the `impl_id_type!` macro for boilerplate: `Display`, `AsRef<str>`, `Borrow<str>`, `From<String>`, `From<&str>`, plus `new()` and `as_str()`.

### 7.2 `config.rs` — Build Configuration (107 lines)

#### ModuleSelectionSpec

7 module slots corresponding to `// @MODULE:xxx` markers in the loader template:

| Slot | Default | Options |
|------|---------|---------|
| `carrier` | `alloc_rw_rx` | `alloc_rw_rx`, `change_rw_rx`, `peb_walk` |
| `decoder` | `xor` | `xor`, `english` |
| `antiemulation` | `none` | `none`, `sirallocalot`, `timeraw` |
| `deconditioner` | `none` | `none`, `alloc_loop` |
| `guardrail` | `none` | `none`, `env` |
| `virtualprotect` | `standard` | `standard`, `undersized` |
| `decoy` | `none` | `none`, `calc`, `winexec` |

`from_proto_or_default()` fills empty proto fields with defaults. Implements `Into<build::ModuleSelection>` for seamless conversion to the build crate.

#### ModularBuildSpec

```rust
struct ModularBuildSpec {
    modules:      ModuleSelectionSpec,
    payload_path: PathBuf,     // raw .bin payload
    encoding:     String,      // "xor" or "english"
}
```

### 7.3 `run.rs` — Run Types (143 lines)

#### RunType

```rust
enum RunType { Baseline, Instrumented, DryRun }
```

Each variant knows its `trace_mode()` (`off`/`lines`/`off`) and `is_dryrun()` flag.

#### RunEnvelope

The dispatch unit that sits in the RunPool:

```rust
struct RunEnvelope {
    run_id, job_id, round_id, round_number,
    run_type: RunType,
    artifact: ArtifactRef { path, sha256 },
    mutations: Vec<String>,
    timeout_seconds: u32,
    required_os: String,
    required_capabilities: Vec<String>,
}
```

#### Capability Matching

`capabilities_match(required, available)` — case-insensitive check that all required caps exist in available caps.

#### Artifact Chunking

`chunk_artifact(artifact_id, data)` — splits binary into 4 MB `ArtifactChunk` proto messages for gRPC streaming.

### 7.4 `round.rs` — Differential Protocol (1418 lines including tests)

The most domain-rich type file. Implements the two-run differential protocol from CLAUDE.md Section 5.

#### DifferentialCategory

```rust
enum DifferentialCategory {
    RealDetection,           // baseline detected, instrumented detected
    InstrumentationArtifact, // baseline clean, instrumented detected
    Flaky,                   // baseline detected, instrumented clean
    Evasion,                 // both clean
    MutationFailed,          // dryrun crash, artifact broken
    PayloadFailed,           // dryrun crash, .bin payload broken
    StaticDetection,         // Defender static file scan detected
}
```

Key methods:
- `from_runs(baseline_detected, instrumented_detected)` — classic 2x2 matrix
- `is_detected()` — true for `RealDetection` and `StaticDetection`
- `is_trustworthy()` — true for `RealDetection`, `Evasion`, `StaticDetection` (used by feedback loop)

#### RoundSpec

Immutable round recipe: `{id, job_id, round_number, mutations: Vec<MutationSpec>, modules}`.

#### RunOutcome

Minimal per-run result: `{detected, exit_code, error, success, elapsed_ms, detection_verdict, last_checkpoint}`.

#### RoundAgg

Ephemeral aggregator that joins 2-3 concurrent run results into a round summary:

```rust
struct RoundAgg {
    spec, baseline_run_id, instrumented_run_id, dryrun_run_id,
    baseline: Option<RunOutcome>,    // filled when baseline result arrives
    instrumented: Option<RunOutcome>, // filled when instrumented result arrives
    dryrun: Option<RunOutcome>,       // optional, filled when dryrun arrives
    baseline_vm_id, instrumented_vm_id, dryrun_vm_id,
    started_at, timeout_ms,
    assembled_source: Option<String>,
    baseline_artifact_path, instrumented_artifact_path,
    dryrun_deadline: Option<Instant>, // grace period for late dryrun
    static_scan_detected: bool,       // short-circuit flag
}
```

`is_complete()` — returns true when baseline AND instrumented are both `Some`.

#### `to_summary()` — The Core Computation

1. **Static scan short-circuit** — If `static_scan_detected`, returns `StaticDetection` immediately.
2. **Dryrun override** — If dryrun is available, calls `override_with_dryrun()` to produce authoritative `DetectionVerdict`:
   - Dryrun nonzero (not timeout) → `MutationFailed` or `PayloadFailed`
   - Dryrun timeout, didn't launch → `MutationFailed`
   - Both clean → `Evasion`
   - Dryrun clean, baseline nonzero → `Detected`
   - Both timeout + launched → `Evasion`
   - Both timeout + not launched → `MutationFailed`
   - Same nonzero exit code → `InfraError`
   - Different nonzero exit codes → `Detected`
3. **Broken artifact short-circuit** — If verdict is `MutationFailed` or `PayloadFailed`, returns `evasion_score=0.0` without differential computation.
4. **Differential category** — `DifferentialCategory::from_runs(effective_baseline_detected, instrumented_detected)`
5. **Behavior match** — `baseline.exit_code == instrumented.exit_code AND baseline.detected == instrumented.detected`
6. **Evasion score** — Per-category ranges:

| Category | Score Range | Formula |
|----------|-----------|---------|
| RealDetection | 0.0–0.4 | `0.4 × survival_ratio` |
| InstrumentationArtifact | 0.5–0.7 | `0.5 + 0.2 × survival_ratio` |
| Flaky | 0.0–0.3 | `0.3 × survival_ratio` |
| Evasion | 0.6–1.0 | `0.6 + 0.2 × payload_reached + 0.2 × behavior_match` |
| MutationFailed/PayloadFailed/Static | 0.0 | Always 0 |

Where `survival_ratio = elapsed_ms / max(timeout_ms, 100s)`.

#### Blended Evasion Score

After async coverage computation:

```
blended = 0.7 × (coverage/100) + 0.3 × time_factor
```

Applied to the same per-category ranges. Replaces the initial time-only score in the selector's history.

#### RoundSummary

Immutable snapshot stored in `JobSession.rounds` (in-memory) and indexed to ES:

```rust
struct RoundSummary {
    round_id, round_number, mutations, mutation_specs,
    modules, detected, behavior_match, evasion_score,
    differential_category, completed_at,
    dry_run_exit_code, has_dryrun, detection_verdict,
    coverage_percent, time_factor,
}
```

### 7.5 `session.rs` — Job Lifecycle State (199 lines)

#### JobSession

Ephemeral runtime state — not persisted directly (ES has its own job doc):

```rust
struct JobSession {
    id, target_os, required_capabilities,
    build_spec: ModularBuildSpec,
    trace_mode,
    search_space: SearchSpace,  // selector config
    current_round, completed_rounds, max_rounds,
    stop_on_evasion,
    sc_checkpoint_count, cache_payload,
    msvc_compat, msvc_vcvarsall,
    rounds: BTreeMap<u32, RoundSummary>,  // selector reads this
    last_round: Option<RoundSummary>,
    created_at, started_at,
}
```

Key methods:
- `should_continue()` — false when `current_round >= max_rounds` or (`stop_on_evasion` AND last round was `Evasion`). Does NOT stop on `InstrumentationArtifact`.
- `start_round()` → `(round_number, RoundId)`
- `record_round_summary()` — inserts into `rounds` BTreeMap + updates `last_round`
- `to_info(status)` → `JobInfo` lightweight snapshot for API

#### JobStatus / JobOutcome / JobInfo

```rust
enum JobStatus { Running, Completed, Stopped, Failed }
enum JobOutcome { Completed { rounds_completed }, Stopped { reason }, Failed { error } }
struct JobInfo { id, status, current_round, completed_rounds, max_rounds, target_os, started_at }
```

---

## Concurrency Model

### Shared State

| Resource | Ownership | Access Pattern |
|----------|-----------|----------------|
| `RunPool` | `Arc<RunPool>` shared by all | DashMap (lock-free reads), per-OS Mutex (short writes), Notify (signal) |
| `TargetManager` | `Arc<TargetManager>` shared by all | DashMap (lock-free reads) |
| `EsStorage` | `Arc<EsStorage>` shared by all | HTTP client, no internal locking needed |
| `JobSession` | Owned by its JobWorker | Single-writer, no sharing |
| `RoundAgg` | Owned by its JobWorker | Single-writer, no sharing |

### Channel Topology

```
API ──job_tx──> Orchestrator.job_submit_rx
API ──job_control_tx──> Orchestrator.job_control_rx
TargetManager ──events_tx──> Orchestrator.events_rx
JobWorker ──event_tx──> Orchestrator.job_event_rx
Orchestrator ──correction_tx──> JobWorker.correction_rx
RunPool ──result_routers[job_id]──> JobWorker.result_rx
JobWorker (triage spawn) ──guidance_tx──> JobWorker.guidance_rx
```

### Cancellation

- Per-job: `CancellationToken` in `JobHandle`, triggered by `StopJob` RPC
- Global: `CancellationToken` in RunPool, triggers all VMExecutors + JobWorkers

---

## Relationship to the Global AutoMutate++ Project

The dispatch module implements the **closed experimental loop** described in CLAUDE.md:

| CLAUDE.md Step | Dispatch Component |
|---------------|-------------------|
| Generate controlled mutations | `JobWorker.produce_round()` → Selector → build artifacts |
| Execute in sandboxed VMs | `RunPool` + `VMExecutor` → artifact upload + command dispatch |
| Two-run differential protocol | `RoundAgg.to_summary()` + `override_with_dryrun()` |
| Normalize into triage tokens | `finalize_round()` → async `extract_and_score()` spawn |
| Score tokens by correlation | Async triage → `TriageGuidance` → `latest_guidance` |
| Guide future mutations | `selector.select()` reads `job.rounds` history + `latest_guidance` |

The dispatch module does **not** contain:
- gRPC protocol handling (that's `api/`)
- VM connection management (that's `vm/manager.rs`)
- ElasticSearch schema/queries (that's `storage/`)
- Token extraction/scoring algorithms (that's `triage/`)
- C source compilation (that's the `build` crate)

It is the **orchestration layer** that wires all other components together into the automated mutation → execution → triage feedback loop.
