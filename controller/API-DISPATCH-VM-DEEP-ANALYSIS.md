# Deep Analysis: api/ , dispatch/ , vm/

## 1) Role in the Global Project

The `controller` crate is the **central brain** of the AutoMutate++ framework. It is a long-running Rust server that:

1. Accepts job submissions via gRPC from external clients (CLI, UI).
2. Orchestrates the entire mutation-build-execute-triage feedback loop.
3. Manages a fleet of Windows VM workers (connect, reserve, dispatch, release).
4. Persists all results, telemetry, and token statistics to ElasticSearch.
5. Exposes monitoring, progress, and analysis APIs.

The three folders `api/`, `dispatch/`, and `vm/` together implement the **job scheduling pipeline** — from receiving a gRPC request, through building artifacts and dispatching them to VMs, to collecting results and feeding them back into the next mutation round.

```
 gRPC Client
     │
     ▼
 ┌────────┐   JobSession    ┌──────────────┐
 │  api/  │ ──────────────► │ Orchestrator │
 └────────┘   (channel)     │  (dispatch/) │
                            └──────┬───────┘
                                   │ spawns
                            ┌──────▼───────┐
                            │  JobWorker   │  builds artifacts,
                            │  (per job)   │  creates RunEnvelopes
                            └──────┬───────┘
                                   │ add_runs()
                            ┌──────▼───────┐
                            │   RunPool    │  OS-sharded queue
                            │  (shared)    │
                            └──────┬───────┘
                                   │ take_run()
                            ┌──────▼───────┐
                            │  VMExecutor  │  1 per connected VM
                            │  (dispatch/) │
                            └──────┬───────┘
                                   │ artifact upload + RunSampleCommand
                            ┌──────▼───────┐
                            │ TargetManager│  gRPC streams, state
                            │    (vm/)     │
                            └──────┬───────┘
                                   │
                            Windows VM (worker agent)
```

---

## 2) Startup Flow (`main.rs`)

```
1. Load ControllerConfig (TOML)
2. Init tracing (console + file)
3. Create Elasticsearch client → EsStorage
4. Ensure ES index templates
5. Create channels:
   - events_tx/rx  (TargetEvent: VM lifecycle + worker messages)
   - job_tx/rx     (JobSession: new job submissions)
   - job_control_tx/rx (JobControlCommand: stop signals)
6. Create shared RunPool
7. Create TargetManager (discovers VMs from automation/generated/*.toml)
8. Spawn Orchestrator (consumes all 3 rx channels)
9. Discover & register targets from TOML files
10. Query target metadata (OS, capabilities, tools)
11. Establish bidirectional gRPC streams → spawns VMExecutors
12. Spawn reconnect loop (periodic offline target recovery)
13. Create SchedulerService → start gRPC server
```

---

## 3) Module: `api/` — gRPC Service Layer

### 3.1 Purpose

Thin gRPC handlers implementing the `Controller` protobuf service. Each handler validates the request, delegates to storage or dispatch, and maps the result to a protobuf response. Handlers never contain business logic — they are pure protocol adapters.

### 3.2 Files

| File | Lines | Responsibility |
|------|-------|----------------|
| `mod.rs` | 241 | `SchedulerService` struct + `Controller` trait dispatch table |
| `job.rs` | 953 | Job lifecycle: schedule, status, progress, stop, round detail, run comparison, token comparison, trace lines, status reports |
| `artifact.rs` | 287 | Build artifact (modular template) + deploy artifact to worker VM |
| `worker.rs` | 492 | Worker listing, telemetry streaming, metadata, pool metrics, orchestrator status, ping/disconnect admin commands |
| `utility.rs` | 121 | Ping, triage submission (legacy), query results from ES |
| `extract.rs` | 51 | Safe JSON field extractors (`str_field`, `u32_field`, `bool_field`, etc.) |

### 3.3 `SchedulerService`

Central service struct holding all shared state:

```rust
pub struct SchedulerService {
    pub storage: Arc<EsStorage>,              // ES persistence
    pub job_tx: mpsc::Sender<JobSession>,     // submit jobs → Orchestrator
    pub job_control_tx: mpsc::Sender<JobControlCommand>, // stop jobs
    pub targets: Arc<TargetManager>,          // VM fleet
    pub run_pool: Arc<RunPool>,               // shared run queue
}
```

It implements the `Controller` gRPC trait by delegating every RPC to the appropriate handler module.

### 3.4 RPC Catalog

| Category | RPC | Handler | Description |
|----------|-----|---------|-------------|
| **Job** | `ScheduleJob` | `job::schedule_job` | Validate payload, create `JobSession`, index to ES, send to Orchestrator |
| **Job** | `GetJobStatus` | `job::get_job_status` | Query ES for job doc, return status + progress |
| **Job** | `GetJobProgress` | `job::get_job_progress` | Query ES for job + all round docs, return detailed round summaries |
| **Job** | `StopJob` | `job::stop_job` | Send `JobControlCommand::Stop` to Orchestrator via channel |
| **Job** | `GetRound` | `job::get_round` | Fetch round doc + both run docs, apply dryrun correction, parse mutations, modules, coverage |
| **Job** | `CompareRuns` | `job::compare_runs` | Fetch baseline + instrumented run docs, compute differential category |
| **Job** | `CompareTokens` | `job::compare_tokens` | Fetch token sets for two rounds, compute mutation-level Jaccard distance |
| **Job** | `GetTraceLines` | `job::get_trace_lines` | Fetch line trace events, resolve code via `SourceMap` |
| **Job** | `ReportStatus` | `job::report_status` | Worker status events (success/error/timeout), update health + index |
| **Artifact** | `BuildArtifact` | `artifact::build_artifact` | Direct artifact build via `ArtifactBuilder`, index metadata to ES |
| **Artifact** | `DeployArtifact` | `artifact::deploy_artifact` | Read artifact, verify SHA256, stream chunks to worker VM |
| **Worker** | `ListWorkers` | `worker::list_workers` | List all registered targets |
| **Worker** | `StreamTelemetry` | `worker::stream_telemetry` | Receive gRPC streaming telemetry, batch-index to ES |
| **Worker** | `GetWorker` | `worker::get_worker` | Fetch single worker by ID |
| **Worker** | `GetAvailableWorkers` | `worker::get_available_workers` | Filter by OS + capabilities |
| **Worker** | `GetWorkerMetadata` | `worker::get_worker_metadata` | Enhanced metadata with health, tools, connection time |
| **Worker** | `GetPoolMetrics` | `worker::get_pool_metrics` | RunPool metrics (dispatched, completed, queue size) |
| **Worker** | `GetOrchestratorStatus` | `worker::get_orchestrator_status` | Global system status (active jobs, VM states, pool) |
| **Admin** | `PingWorker` | `worker::ping_worker` | Send HealthCheck via bidi stream |
| **Admin** | `DisconnectWorker` | `worker::disconnect_worker` | Graceful disconnect of single worker |
| **Admin** | `DisconnectAllWorkers` | `worker::disconnect_all_workers` | Graceful shutdown of all workers |
| **Utility** | `Ping` | `utility::ping` | Health check ("pong") |
| **Utility** | `SubmitTriage` | `utility::submit_triage` | Legacy triage endpoint (internal pipeline handles this) |
| **Utility** | `QueryResults` | `utility::query_results` | Search `runs-*` ES index by job_ids + date range |

### 3.5 Key Design Patterns

- **Proto-to-domain mapping**: `extract.rs` provides safe accessors over `serde_json::Value` from ES `_source` documents. All field extraction goes through `str_field()`, `u32_field()`, `bool_field()`, etc., with sensible defaults for missing data.
- **Dryrun correction**: `apply_round_correction()` overrides per-run `detected` flags using the canonical `differential_category` stored in the round doc. This accounts for dryrun-based override logic (e.g., a "mutation_failed" category means neither run was truly detected — the artifact was broken).
- **Mutation recipe parsing**: `parse_mutation_recipe()` prefers the structured `mutation_recipe` array (with full params) over the legacy `mutations` string array.
- **Parallel ES queries**: `get_job_progress` uses `tokio::join!` to fetch job doc + round docs concurrently.

---

## 4) Module: `dispatch/` — JobWorker Orchestration

### 4.1 Purpose

The core execution engine. Implements a **producer-consumer architecture** where:
- **JobWorkers** (producers) build artifacts and push `RunEnvelope`s into a shared pool.
- **VMExecutors** (consumers) pull runs from the pool, dispatch them to VMs, and route results back.
- **Orchestrator** supervises everything: spawns workers, handles VM lifecycle, indexes results.

### 4.2 Files

| File | Lines | Responsibility |
|------|-------|----------------|
| `mod.rs` | 33 | Module declarations + public re-exports |
| `orchestrator.rs` | 738 | Central event loop: job submissions, VM events, round indexing, coverage computation |
| `job_worker.rs` | 1077 | Per-job lifecycle: round production, artifact building, result aggregation, triage |
| `run_pool.rs` | 587 | OS-sharded run queue with capability filtering and result routing |
| `vm_executor.rs` | 412 | Per-VM thin dispatcher: take runs, upload artifacts, dispatch commands, route results |
| `channels.rs` | 128 | Channel message types: `JobControlCommand`, `RemoteRunResult`, `JobWorkerEvent`, `CoverageCorrection`, `JobRunResult` |
| `types/mod.rs` | 23 | Re-exports for all type sub-modules |
| `types/ids.rs` | 72 | Newtype wrappers: `JobId`, `RoundId`, `RunId`, `WorkerId`, `TargetId` |
| `types/session.rs` | 199 | `JobSession` (ephemeral runtime state) + `JobStatus` + `JobOutcome` |
| `types/round.rs` | ~600 | `RoundSpec`, `RoundAgg`, `RoundSummary`, `DifferentialCategory`, `RunOutcome`, evasion scoring |
| `types/run.rs` | 143 | `RunEnvelope`, `RunType`, `ArtifactRef`, `VMInfo`, `WorkerInfo`, capability matching, artifact chunking |
| `types/config.rs` | 107 | `ModuleSelectionSpec`, `ModularBuildSpec` |

### 4.3 Architecture Diagram

```
                     ┌─────────────────────────────────┐
                     │         Orchestrator             │
                     │                                  │
  job_submit_rx ────►│  spawn_job_worker()              │
  job_control_rx ───►│  on_job_control()                │
  events_rx ────────►│  on_target_event()               │
  job_event_rx ─────►│  on_job_worker_event()           │
                     │    ├─ index_round_and_runs()     │
                     │    └─ compute_round_coverage()   │
                     └─────────────────────────────────┘
                                    │
                spawns (one per job submission)
                                    │
                     ┌──────────────▼──────────────────┐
                     │          JobWorker               │
                     │                                  │
                     │  produce_round():                │
                     │    1. selector.select()          │
                     │    2. build_artifact(baseline)   │
                     │    3. static_defender_scan()     │
                     │    4. build_artifact(instrumented)│
                     │    5. create_run_envelopes()     │
                     │    6. run_pool.add_runs()        │
                     │                                  │
                     │  on_result():                    │
                     │    1. match run to RoundAgg      │
                     │    2. if complete → finalize_round│
                     │                                  │
                     │  finalize_round():               │
                     │    1. RoundAgg::to_summary()     │
                     │    2. record_round_summary()     │
                     │    3. spawn triage extraction    │
                     │    4. emit RoundCompleted event  │
                     └──────────────┬──────────────────┘
                                    │ add_runs()
                     ┌──────────────▼──────────────────┐
                     │          RunPool                 │
                     │                                  │
                     │  Sharded by OS:                  │
                     │    "win10" → VecDeque<RunId>     │
                     │    "win11" → VecDeque<RunId>     │
                     │                                  │
                     │  DashMap<RunId, RunEnvelope>     │
                     │  (lock-free run storage)         │
                     │                                  │
                     │  Result routing:                 │
                     │    HashMap<JobId, Sender>        │
                     └──────────────┬──────────────────┘
                                    │ take_run(os, caps)
                     ┌──────────────▼──────────────────┐
                     │         VMExecutor               │
                     │         (per VM)                 │
                     │                                  │
                     │  dispatch():                     │
                     │    1. targets.reserve(vm_id)     │
                     │    2. send_artifact()            │
                     │    3. send RunSampleCommand      │
                     │                                  │
                     │  on_result_received():           │
                     │    1. targets.release(vm_id)     │
                     │    2. run_pool.route_result()    │
                     └─────────────────────────────────┘
```

### 4.4 Component Deep-Dive

#### 4.4.1 Orchestrator (`orchestrator.rs`)

The Orchestrator is a single-threaded `tokio::select!` event loop (biased priority):

1. **Job control** (highest): `JobControlCommand::Stop` → cancels the `CancellationToken` of the target `JobWorker`.
2. **Job submissions**: Receives `JobSession` → calls `spawn_job_worker()`.
3. **JobWorker events**: `RoundCompleted` (indexes round+runs to ES, computes coverage) or `JobCompleted` (updates job status in ES).
4. **Target events**: VM lifecycle (Connected/Disconnected) and worker messages (Registration, Status, Telemetry, SampleResponse).

**Constraint resolution**: When a job doesn't specify OS or capabilities, `resolve_job_constraints()` inspects available targets and auto-assigns the best match (preferring available > busy, alphabetical OS for determinism).

**Selector injection**: Based on `job.search_space.selector`, the Orchestrator creates the appropriate `Selector` implementation:
- `SelectorType::Fuzzer` → `FuzzerSelector`
- `SelectorType::Coverage` → `CoverageSelector`
- `SelectorType::Token` → `TokenSelector`
- `SelectorType::Random` → `RandomSelector`

**Coverage computation**: After a round completes, the Orchestrator spawns an async task that:
1. Fetches trace content from ES for the instrumented run.
2. Parses executed line numbers.
3. Computes coverage via `SourceMap::compute_coverage()`.
4. Updates the round doc in ES.
5. Computes a blended evasion score and sends a `CoverageCorrection` back to the JobWorker so subsequent selector calls see corrected scores.

#### 4.4.2 JobWorker (`job_worker.rs`)

Spawned per job. Owns the complete lifecycle of a single job, running as an independent tokio task.

**Main loop** (`run()`): A `tokio::select!` that:
- Listens for shutdown signals (job cancelled or pool shutdown).
- Receives `JobRunResult` from VMs (via RunPool routing).
- Receives `CoverageCorrection` from Orchestrator background tasks.
- Receives `TriageGuidance` from background triage extraction.
- Periodically checks if more rounds can be produced (100ms interval).

**Round production** (`produce_round()`):
1. Calls `selector.select()` with the job's search space, module defaults, round history, and triage guidance.
2. Builds a **baseline** artifact (`trace_mode=off`).
3. Runs a **static Defender scan** on the baseline — if detected, the round is short-circuited (no VM dispatch needed).
4. Builds an **instrumented** artifact (`trace_mode=lines`).
5. Creates 3 `RunEnvelope`s: baseline, instrumented, and dryrun.
6. Adds all 3 to the shared RunPool.

**Result aggregation** (`on_result()`):
- Uses `RoundAgg` as a join state: collects baseline, instrumented, and optional dryrun results.
- Once baseline + instrumented are done, starts a 5-second grace period for the dryrun.
- Finalizes the round when either all 3 results arrive or the grace period expires.

**Round finalization** (`finalize_round()`):
1. `RoundAgg::to_summary()` computes the `DifferentialCategory`, evasion score, and detection verdict.
2. Records the summary in the job session (selector reads history from here).
3. Spawns async triage extraction (`extract_and_score()`).
4. Emits `RoundCompleted` event for ES indexing.

**Backpressure controls**:
- `MAX_IN_FLIGHT_ROUNDS = 5`: no more than 5 rounds being aggregated simultaneously.
- `MAX_PENDING_RUNS = 9`: no more than 9 runs (3 rounds x 3 runs each) queued in the pool for this job.

**Payload caching**: Reads the raw payload file once, caches it across rounds. Optionally caches the precomputed encoded payload header (XOR'd/English-encoded) to skip re-encoding every round.

#### 4.4.3 RunPool (`run_pool.rs`)

The shared run queue connecting all JobWorkers to all VMExecutors.

**Sharding strategy**: Runs are partitioned by `required_os` (e.g., "win10", "win11"). Each OS has its own `Mutex<VecDeque<RunId>>`. VMExecutors only lock their own OS queue.

**Data structures**:
- `pending: DashMap<RunId, RunEnvelope>` — lock-free run storage.
- `by_os: DashMap<String, Mutex<VecDeque<RunId>>>` — per-OS ordered queues.
- `result_routers: RwLock<HashMap<JobId, Sender>>` — routes results back to the originating JobWorker.
- `job_registry: DashMap<JobId, JobInfo>` — lightweight job snapshots for API visibility.

**Capability filtering** (`take_run()`):
- VMExecutor calls `take_run(os, capabilities)`.
- The pool scans the OS queue (bounded by queue length to avoid infinite loops).
- For each candidate run, checks `capabilities_match()` (case-insensitive superset).
- **Dryrun isolation**: dryrun VMs only take dryrun runs, and non-dryrun VMs never take dryrun runs.

**Signal-driven**: Uses `tokio::sync::Notify` to wake VMExecutors when runs become available. No polling.

**Metrics tracking**: `RunPoolMetrics` tracks total runs added, taken, results routed, active jobs, and completed rounds/jobs.

#### 4.4.4 VMExecutor (`vm_executor.rs`)

A thin, stateless dispatcher bound to a single VM. One VMExecutor exists per connected VM.

**Main loop** (`run()`): Signal-driven `tokio::select!`:
1. **Shutdown** (priority): clean up in-flight run, route error back.
2. **VM result** (when in-flight): route result back via RunPool, immediately try to get more work.
3. **Pool signal** (when idle): wake up, try `take_run()`.

**Dispatch flow** (`dispatch()`):
1. Reserve VM via `targets.reserve()`.
2. Upload artifact to VM via `ArtifactSender` trait.
3. Build and send `RunSampleCommand` (gRPC `ControllerMessage`).
4. Track envelope in `in_flight: Option<InFlightRun>`.

**Result handling** (`on_result_received()`):
1. Match result `run_id` against in-flight envelope.
2. Release VM via `targets.release()`.
3. Route `JobRunResult` back to RunPool → JobWorker.

**Design principle**: VMExecutor doesn't know about jobs or rounds. It is "dumb" — it takes runs and returns results. All intelligence lives in the JobWorker and Orchestrator.

#### 4.4.5 Channel Types (`channels.rs`)

Defines the typed messages flowing between components:

| Type | Direction | Purpose |
|------|-----------|---------|
| `JobControlCommand` | Service → Orchestrator | Job stop signals |
| `RemoteRunResult` | VM stream → VMExecutor | Raw execution results from worker |
| `JobWorkerEvent` | JobWorker → Orchestrator | Round/job completion notifications |
| `RoundCompletedData` | (inside JobWorkerEvent) | All data for ES indexing |
| `CoverageCorrection` | Orchestrator → JobWorker | Async coverage + blended score update |
| `JobRunResult` | VMExecutor → RunPool → JobWorker | Routed run results |

#### 4.4.6 Types (`types/`)

**ID types** (`ids.rs`): Newtype wrappers (`JobId`, `RoundId`, `RunId`, `WorkerId`, `TargetId`) using a macro for Display, From, AsRef, Borrow.

**JobSession** (`session.rs`): Ephemeral runtime state for a running job. Tracks:
- Constraints (OS, capabilities)
- Build spec (modules, payload path, encoding)
- Search space (selector type, mutation pool, variation strategy)
- Progress (current/completed/max rounds)
- Round history (`BTreeMap<u32, RoundSummary>`)
- Flags: `stop_on_evasion`, `cache_payload`, `msvc_compat`

Key methods: `should_continue()` (checks max rounds + evasion stop), `start_round()` (increments counter, generates `RoundId`), `record_round_summary()`.

**RoundSpec / RoundAgg / RoundSummary** (`round.rs`):
- `RoundSpec`: immutable mutation recipe (what mutations to apply).
- `RoundAgg`: ephemeral join state that collects baseline + instrumented + optional dryrun results. Contains the `DifferentialCategory` logic.
- `RoundSummary`: finalized output with detection verdict, evasion score, coverage, and differential category.

**DifferentialCategory**: Encodes the two-run differential protocol:

| Baseline | Instrumented | Category |
|----------|-------------|----------|
| Detected | Detected | `RealDetection` |
| Not det. | Detected | `InstrumentationArtifact` |
| Detected | Not det. | `Flaky` |
| Not det. | Not det. | `Evasion` |

Plus: `MutationFailed` (dryrun crash, mutations present), `PayloadFailed` (dryrun crash, no mutations), `StaticDetection` (Defender file scan).

**RunEnvelope** (`run.rs`): The unit of work in the RunPool. Contains: run_id, job/round references, run type, artifact reference, mutations, timeout, OS + capability requirements.

**ModuleSelectionSpec** (`config.rs`): Which C template modules to plug in (carrier, decoder, antiemulation, deconditioner, guardrail, virtualprotect, decoy). Defaults to `alloc_rw_rx` carrier, `xor` decoder, everything else `none`.

---

## 5) Module: `vm/` — Target / VM Management

### 5.1 Purpose

Manages the fleet of Windows VM workers: discovery, registration, gRPC connections, bidirectional streams, state transitions, artifact deployment, and reconnection.

### 5.2 Files

| File | Lines | Responsibility |
|------|-------|----------------|
| `mod.rs` | 14 | Module declaration + re-exports |
| `manager.rs` | 1066 | `TargetManager`: registration, connection, streams, state, artifacts, discovery |

### 5.3 `TargetManager`

The `TargetManager` holds a `DashMap<TargetId, Target>` and provides all VM fleet operations.

**Target state machine**:
```
    ┌─────────┐
    │ Offline │ ◄──── initial / disconnected
    └────┬────┘
         │ mark_connected()
    ┌────▼─────┐
    │Available │ ◄──── release()
    └────┬─────┘
         │ reserve()
    ┌────▼─────┐
    │   Busy   │
    └──────────┘
```

**Target struct**:
```rust
pub struct Target {
    pub id: TargetId,
    pub address: String,          // "ip:port"
    pub os_version: String,       // "win10", "win11"
    pub capabilities: Vec<String>,// ["rededr", "mde", "dryrun"]
    pub metadata: HashMap<String, String>,
    pub tools: HashMap<String, String>, // {"rededr": "v1.2", "defender": "..."}
    pub status: TargetStatus,     // Available | Busy | Offline
    pub enabled: bool,
    pub registration_type: RegistrationType, // Static (TOML) | Dynamic (runtime)
    pub current_job: Option<JobId>,
    pub last_seen: SystemTime,
    pub connected_at: Option<SystemTime>,
    channel: Option<Channel>,     // gRPC channel (unary RPCs)
    stream_tx: Option<Sender>,    // bidi stream outgoing channel
}
```

### 5.4 Target Discovery

`discover_and_register_targets()`:
1. Reads `automation/generated/win*-worker-*.toml` files.
2. Parses `worker_id` + `ip_address:listen_port` from each TOML.
3. Detects duplicate IPs (warns and skips).
4. Registers each target as `TargetStatus::Offline` with `RegistrationType::Static`.

### 5.5 Connection & Stream Establishment

`establish_stream()`:
1. Creates a dedicated gRPC channel (no request timeout — the bidi stream is long-lived).
2. Opens a `WorkerAgentClient::establish_stream()` bidirectional stream.
3. Marks target as `Available`.
4. Spawns 3 concurrent tasks:
   - **Stream handler**: reads incoming `WorkerMessage`s, handles Registration (signals VMExecutor spawn), SampleResponse (forwards to VMExecutor), forwards all messages to Orchestrator.
   - **Deferred VMExecutor**: waits up to 15s for registration, then starts the VMExecutor with the live VM info (OS, capabilities).
   - **Heartbeat**: sends periodic `Heartbeat` messages every 30s.

**Reconnection**: `spawn_reconnect_loop()` periodically scans for offline+enabled targets and attempts `establish_stream()`.

### 5.6 Artifact Deployment

`send_artifact()`:
1. Reads artifact file from disk.
2. Gets or creates a gRPC channel to the target.
3. Splits data into 4MB chunks (`chunk_artifact()`).
4. Streams chunks via `WorkerAgentClient::send_artifact()`.

The `ArtifactSender` trait abstracts this for testability. `TargetArtifactSender` wraps `TargetManager::send_artifact()`.

### 5.7 Admin Operations

- `send_command()`: Send a `ControllerMessage` via the bidi stream. Auto-marks target offline if send fails.
- `broadcast()`: Send a message to all connected targets.
- `disconnect_one()`: Send `DisconnectNotice` + mark offline.
- `disconnect_all()`: Shutdown RunPool → wait → send disconnect notices → mark all offline.

---

## 6) Data Flow: Complete Round Lifecycle

```
1. Client calls ScheduleJob RPC
   └─► api/job.rs: validate, create JobSession, send to Orchestrator

2. Orchestrator receives JobSession
   └─► spawn_job_worker(): resolve constraints, create Selector, spawn JobWorker

3. JobWorker::produce_round()
   ├─► selector.select() → mutations + modules
   ├─► build_artifact(baseline, trace=off)
   ├─► static_defender_scan() → if detected, short-circuit
   ├─► build_artifact(instrumented, trace=lines)
   └─► run_pool.add_runs([baseline, instrumented, dryrun])

4. VMExecutor::dispatch() [per VM, concurrent]
   ├─► targets.reserve(vm)
   ├─► artifact_sender.send_artifact(vm, artifact)
   └─► send RunSampleCommand via bidi stream

5. Worker VM executes artifact, sends SampleResponse

6. stream_handler() → RemoteRunResult → VMExecutor

7. VMExecutor::on_result_received()
   ├─► targets.release(vm)
   └─► run_pool.route_result(JobRunResult)

8. JobWorker::on_result()
   ├─► match result to RoundAgg
   └─► if both runs done → finalize_round()

9. JobWorker::finalize_round()
   ├─► RoundAgg::to_summary() → DifferentialCategory + evasion score
   ├─► record_round_summary() → selector history update
   ├─► spawn triage extraction (async, non-blocking)
   └─► emit RoundCompleted → Orchestrator

10. Orchestrator::on_job_worker_event()
    ├─► index_round_and_runs() → ES
    └─► compute_round_coverage() → ES update + CoverageCorrection → JobWorker

11. Back to step 3 until max_rounds reached or evasion detected
```

---

## 7) Concurrency Model

| Component | Concurrency | Synchronization |
|-----------|-------------|-----------------|
| Orchestrator | Single task, `select!` loop | Channels (mpsc) |
| JobWorker | One task per job (parallel jobs) | Own `select!` loop |
| VMExecutor | One task per VM (parallel VMs) | Own `select!` loop |
| RunPool | Shared across all tasks | DashMap (lock-free), per-OS Mutex, Notify |
| TargetManager | Shared via `Arc` | DashMap (lock-free) |
| EsStorage | Shared via `Arc` | Stateless (ES HTTP) |

**Key concurrency properties**:
- Jobs run fully in parallel (no `active_job: Option` limit).
- VMs are "dumb executors" shared across all jobs.
- RunPool sharding by OS means win10 VMExecutors don't contend with win11 VMExecutors.
- Result routing is O(1) via `HashMap<JobId, Sender>`.
- All long operations (ES queries, artifact builds, VM dispatch) are async and non-blocking.

---

## 8) Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Producer-consumer via RunPool** | Decouples job logic from VM management. Jobs don't need to know which VM runs what. |
| **OS-sharded queues** | Reduces lock contention. VMs only compete within their own OS pool. |
| **3-run differential protocol** | Baseline + instrumented + dryrun per round enables ground-truth detection classification and artifact health checks. |
| **Static Defender scan before dispatch** | Avoids wasting VM time on artifacts that fail statically. Short-circuits to `StaticDetection`. |
| **Dryrun grace period (5s)** | Dryrun VMs may be scarce. Don't block round finalization waiting for optional data. |
| **Async triage extraction** | Token extraction + scoring runs in background; guidance flows back to JobWorker non-blocking. |
| **Async coverage correction** | Coverage computation (ES query + parse) runs after round indexing; blended score correction flows back to JobWorker for the selector. |
| **Payload caching** | Avoids re-reading and re-encoding the same payload every round. |
| **Content-addressed artifacts** | SHA256 naming enables deduplication; cleanup is deferred to job end to avoid breaking cross-round references. |
| **CancellationToken per job** | Clean shutdown: Orchestrator cancels a specific job without affecting others. |
| **DashMap everywhere** | Lock-free concurrent access for TargetManager and RunPool — scales with VM fleet size. |
