# Controller/Scheduler Module & Struct Reference

## Module Hierarchy

```
crate scheduler
│
├── main.rs                         Entry point, creates channels, spawns Orchestrator
│
├── mod api                         gRPC service handlers (pub(crate))
│   ├── mod.rs                      SchedulerService struct
│   ├── mod artifact                Build & deploy operations
│   ├── mod job                     Job lifecycle operations
│   ├── mod utility                 Ping, query, triage
│   └── mod worker                  Worker/orchestrator status
│
├── mod automutate                  Proto-generated types (pub)
│   ├── mod common                  TelemetryData, ControllerMessage, etc.
│   ├── mod controller              gRPC service definitions
│   └── mod worker                  Worker-side messages
│
├── mod dispatch                    Core dispatch system (pub(crate))
│   ├── mod channels                Channel message types
│   ├── mod job_worker              Per-job task
│   ├── mod orchestrator            Central coordinator
│   ├── mod run_pool                Shared run queue
│   ├── mod types                   Domain types (IDs, specs, envelopes)
│   └── mod vm_executor             Per-VM execution task
│
└── mod vm                          VM management (pub(crate))
    └── mod manager                 TargetManager, Target, registration
```

---

## Module Details

### `api` — gRPC Service Layer

```
api/
├── mod.rs              SchedulerService (main service struct)
├── artifact.rs         build_artifact, deploy_artifact
├── job.rs              schedule_job, get_job_status, stop_job, etc.
├── utility.rs          ping, query_results, submit_triage
└── worker.rs           list_workers, get_orchestrator_status
```

| Struct | Fields | Purpose |
|--------|--------|---------|
| `SchedulerService` | `es_client`, `job_tx`, `job_control_tx`, `targets`, `run_pool` | gRPC service handler, bridges API to dispatch |

**Key Methods:**
- `schedule_job()` → sends `JobSession` to `job_tx`
- `stop_job()` → sends `JobControlCommand::Stop` to `job_control_tx`
- `list_workers()` → queries `targets.list_all()`
- `get_orchestrator_status()` → queries `run_pool` metrics

---

### `dispatch` — Core Dispatch System

#### `dispatch::types` — Domain Types

**ID Types (Newtype Wrappers)**

| Type | Inner | Purpose |
|------|-------|---------|
| `JobId` | `String` | Unique job identifier |
| `RoundId` | `String` | Unique round identifier (format: `{job_id}-round-{n}`) |
| `RunId` | `String` | Unique run identifier |
| `WorkerId` | `String` | Ephemeral worker session ID |
| `TargetId` | `String` | Stable VM identity (persists across reconnects) |

**Job State**

```rust
struct JobSession {
    id: JobId,
    target_os: Option<String>,
    required_capabilities: Vec<String>,
    build_spec: ModularBuildSpec,

    // Progress
    current_round: u32,
    max_rounds: u32,
    stop_on_evasion: bool,

    // History
    rounds: BTreeMap<u32, RoundSummary>,
    last_round: Option<RoundSummary>,

    // Timestamps
    created_at: SystemTime,
    started_at: Option<SystemTime>,
}
```

| Method | Description |
|--------|-------------|
| `new(id, max_rounds, build_spec)` | Create new job session |
| `should_continue()` | Check if more rounds needed |
| `start_round()` | Increment round, return (number, RoundId) |
| `record_round_summary(summary)` | Store completed round |
| `to_info(status)` | Create lightweight `JobInfo` snapshot |

**Build Specification**

```rust
struct ModularBuildSpec {
    modules: ModuleSelectionSpec,
    payload_path: PathBuf,
    encoding: String,  // "xor" | "english"
}

struct ModuleSelectionSpec {
    carrier: String,        // alloc_rw_rx, change_rw_rx, peb_walk
    decoder: String,        // xor, english
    antiemulation: String,  // none, sirallocalot, timeraw
    guardrail: String,      // none, env
    virtualprotect: String, // standard, undersized
    decoy: String,          // none, calc, winexec
}
```

**Round State**

```rust
struct RoundSpec {
    id: RoundId,
    job_id: JobId,
    round_number: u32,
    mutations: Vec<MutationSpec>,
}

struct MutationSpec {
    id: String,
    params: Option<serde_json::Value>,
}

struct RoundAgg {
    spec: RoundSpec,
    baseline_run_id: RunId,
    instrumented_run_id: RunId,
    baseline: Option<RunOutcome>,      // Set when baseline completes
    instrumented: Option<RunOutcome>,  // Set when instrumented completes
}

struct RoundSummary {
    round_id: RoundId,
    round_number: u32,
    mutations: Vec<String>,
    detected: bool,
    behavior_match: bool,
    evasion_score: f64,
    completed_at: SystemTime,
}
```

| `RoundAgg` Method | Description |
|-------------------|-------------|
| `is_complete()` | Both baseline and instrumented results received |
| `to_summary()` | Compute `RoundSummary` from completed runs |

**Run State**

```rust
struct RunEnvelope {
    run_id: RunId,
    job_id: JobId,
    round_id: RoundId,
    round_number: u32,
    run_type: RunType,              // Baseline | Instrumented
    artifact: ArtifactRef,
    mutations: Vec<String>,
    timeout_seconds: u32,
    required_os: String,
    required_capabilities: Vec<String>,
}

struct ArtifactRef {
    path: PathBuf,
    sha256: Option<String>,
}

enum RunType {
    Baseline,      // trace_mode = "off"
    Instrumented,  // trace_mode = "lines"
}

struct RunOutcome {
    detected: bool,
    exit_code: i32,
    error: Option<String>,
}
```

**Status Types**

```rust
enum JobStatus {
    Running,
    Completed,
    Stopped,
    Failed,
}

enum JobOutcome {
    Completed { rounds_completed: u32 },
    Stopped { reason: String },
    Failed { error: String },
}

struct JobInfo {
    id: JobId,
    status: JobStatus,
    current_round: u32,
    max_rounds: u32,
    target_os: Option<String>,
    started_at: Option<SystemTime>,
}
```

**Worker Types**

```rust
struct WorkerInfo {
    id: WorkerId,
    os: String,
    capabilities: Vec<String>,
}

struct VMInfo {
    id: String,
    os: String,
    capabilities: Vec<String>,
}
```

---

#### `dispatch::channels` — Message Types

```rust
// Job submission control
enum JobControlCommand {
    Stop { job_id: JobId },
}

// JobWorker → Orchestrator events
enum JobWorkerEvent {
    RoundCompleted { job_id: JobId, round_id: RoundId, summary: RoundSummary },
    JobCompleted { job_id: JobId, outcome: JobOutcome },
}

// VMExecutor → JobWorker results
struct JobRunResult {
    run_id: RunId,
    job_id: JobId,
    round_id: RoundId,
    outcome: RunOutcome,
    vm_id: String,
}

// StreamHandler → VMExecutor results
struct RemoteRunResult {
    run_id: RunId,
    detected: bool,
    exit_code: i32,
    success: bool,
    error: Option<String>,
}
```

---

#### `dispatch::run_pool` — Shared Run Queue

```rust
struct RunPool {
    // Run storage (lock-free)
    pending: DashMap<RunId, RunEnvelope>,

    // Per-OS queues (sharded locking)
    by_os: DashMap<String, Mutex<VecDeque<RunId>>>,

    // Wake signal for VMExecutors
    runs_available: Notify,

    // Result routing: JobId → JobWorker's channel
    result_routers: RwLock<HashMap<JobId, Sender<JobRunResult>>>,

    // Job visibility for API
    job_registry: DashMap<JobId, JobInfo>,

    // Graceful shutdown
    shutdown_token: CancellationToken,

    // Metrics
    metrics: Mutex<RunPoolMetrics>,
}

struct RunPoolMetrics {
    total_runs_added: u64,
    total_runs_taken: u64,
    total_results_routed: u64,
    active_jobs: usize,
}
```

| Method | Called By | Description |
|--------|-----------|-------------|
| `register_job(job, result_tx)` | JobWorker | Register job's result channel |
| `unregister_job(job_id)` | JobWorker | Cleanup on job completion |
| `add_runs(runs)` | JobWorker | Add runs to pool, notify VMExecutors |
| `take_run(os, caps)` | VMExecutor | Take compatible run (locks only OS queue) |
| `route_result(result)` | VMExecutor | Route result to JobWorker via result_routers |
| `wait_for_runs()` | VMExecutor | Wait for runs_available notification |
| `update_job_progress(job)` | JobWorker | Update job_registry |
| `complete_job(job_id, outcome)` | JobWorker | Mark job completed in registry |
| `list_jobs()` | API | Get all jobs from registry |
| `list_running_jobs()` | API | Get running jobs only |
| `pool_size()` | API | Count pending runs |
| `get_metrics()` | API | Get RunPoolMetrics |

---

#### `dispatch::orchestrator` — Central Coordinator

```rust
struct Orchestrator {
    // Shared state
    run_pool: Arc<RunPool>,
    targets: Arc<TargetManager>,
    es_client: Elasticsearch,

    // Job tracking (local to Orchestrator task)
    job_workers: HashMap<JobId, CancellationToken>,
    vms: HashMap<WorkerId, WorkerInfo>,

    // Channels (owned receivers)
    job_event_tx: Sender<JobWorkerEvent>,
    job_event_rx: Receiver<JobWorkerEvent>,
    events_rx: Receiver<TargetEvent>,
    job_submit_rx: Receiver<JobSession>,
    job_control_rx: Receiver<JobControlCommand>,
}
```

| Method | Description |
|--------|-------------|
| `run()` | Main select! loop over 4 channels |
| `spawn_job_worker(job)` | Create JobWorker, spawn task, store token |
| `on_target_event(event)` | Handle VM connect/disconnect/telemetry |
| `on_job_worker_event(event)` | Handle round/job completion |
| `on_job_control(cmd)` | Handle stop commands |
| `handle_worker_message(msg)` | Route telemetry to ES |
| `shutdown_job(job_id)` | Cancel specific job |
| `shutdown_all_jobs()` | Cancel all jobs |

---

#### `dispatch::job_worker` — Per-Job Task

```rust
struct JobWorker {
    job: JobSession,
    run_pool: Arc<RunPool>,

    // Result channel (registered with RunPool)
    result_rx: Receiver<JobRunResult>,
    result_tx: Sender<JobRunResult>,

    // In-flight rounds waiting for results
    round_aggs: HashMap<RoundId, RoundAgg>,

    // Event output to Orchestrator
    event_tx: Sender<JobWorkerEvent>,

    // Shutdown signal
    shutdown_token: CancellationToken,
}
```

| Method | Description |
|--------|-------------|
| `run()` | Main loop: produce rounds, receive results |
| `can_produce_round()` | Check in-flight limits |
| `produce_round()` | Build artifacts, create runs, add to pool |
| `build_artifact(trace_mode)` | Invoke build system |
| `on_result(result)` | Update RoundAgg, check completion |
| `finalize_round(round_id)` | Compute summary, emit event |
| `is_job_complete()` | Check if job done |
| `shutdown()` | Cancel via token |

---

#### `dispatch::vm_executor` — Per-VM Task

```rust
struct VMExecutor {
    id: String,
    info: VMInfo,

    // Shared state
    targets: Arc<TargetManager>,
    run_pool: Arc<RunPool>,

    // Communication with remote VM
    remote_tx: Sender<ControllerMessage>,
    remote_rx: Receiver<RemoteRunResult>,
    artifact_sender: Arc<dyn ArtifactSender + Send + Sync>,

    // Current execution
    in_flight: Option<InFlightRun>,
}

struct InFlightRun {
    envelope: RunEnvelope,
}

trait ArtifactSender {
    async fn send_artifact(&self, target_id: &str, data: Vec<u8>, name: &str) -> Result<()>;
}
```

| Method | Description |
|--------|-------------|
| `run()` | Main select! loop: results, work, shutdown |
| `dispatch(envelope)` | Reserve VM, upload artifact, send command |
| `on_result_received(result)` | Release VM, route to RunPool |
| `route_error(run_id, error)` | Handle dispatch errors |
| `is_idle()` | Check if in_flight is None |

---

### `vm::manager` — VM Management

```rust
struct TargetManager {
    targets: DashMap<TargetId, Target>,
    events_tx: Sender<TargetEvent>,
    rpc_timeout: Duration,
    run_pool: Arc<RunPool>,
}

struct Target {
    id: TargetId,
    address: String,
    os_version: String,
    capabilities: Vec<String>,
    metadata: HashMap<String, String>,
    tools: HashMap<String, String>,

    status: TargetStatus,
    enabled: bool,
    registration_type: RegistrationType,

    current_job: Option<JobId>,
    last_seen: SystemTime,
    connected_at: Option<SystemTime>,

    // gRPC state
    channel: Option<Channel>,
    stream_tx: Option<Sender<ControllerMessage>>,
}

enum TargetStatus {
    Available,
    Busy,
    Offline,
}

enum RegistrationType {
    Static,   // From config file
    Dynamic,  // Self-registered
}

enum TargetEvent {
    Connected { id: TargetId, info: WorkerInfo },
    Disconnected { id: TargetId },
    Message { id: TargetId, message: WorkerMessage },
}

struct TargetConfig {
    id: TargetId,
    address: String,
    enabled: bool,
}
```

| Method | Description |
|--------|-------------|
| `register(config)` | Add target from config |
| `register_with_metadata(id, addr, os, caps, meta)` | Add with full info |
| `establish_stream(id)` | Create bidirectional gRPC, spawn VMExecutor |
| `establish_all_streams()` | Connect to all enabled targets |
| `reserve(id)` | Mark Busy, set current_job |
| `release(id)` | Mark Available, clear current_job |
| `get_available()` | List Available targets |
| `get_available_by_os_and_capabilities(os, caps)` | Filtered list |
| `send_command(id, cmd)` | Send via stream_tx |
| `send_artifact(id, data, name)` | Upload artifact chunks |
| `mark_connected(id)` | Update status on connect |
| `mark_offline(id)` | Update status on disconnect |

---

## Struct Relationships

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              OWNERSHIP GRAPH                                    │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  main.rs creates:                                                               │
│    ├── Arc<RunPool> ─────────────────────┬──────────────────────────────────┐   │
│    ├── Arc<TargetManager> ───────────────┼─────────────────────┐            │   │
│    ├── Orchestrator ─────────────────────┼─────────────────────┼────────────┤   │
│    └── SchedulerService ─────────────────┼─────────────────────┼────────────┤   │
│                                          │                     │            │   │
│                                          ▼                     ▼            ▼   │
│  ┌───────────────────────────────────────────────────────────────────────────┐ │
│  │                         SHARED STATE (Arc)                                │ │
│  │                                                                           │ │
│  │  RunPool                          TargetManager                           │ │
│  │  ├── pending: DashMap             ├── targets: DashMap                    │ │
│  │  ├── by_os: DashMap               ├── events_tx                           │ │
│  │  ├── result_routers: RwLock       └── run_pool: Arc<RunPool> ◄────────────┤ │
│  │  ├── job_registry: DashMap                                                │ │
│  │  └── shutdown_token                                                       │ │
│  └───────────────────────────────────────────────────────────────────────────┘ │
│                                                                                 │
│  Orchestrator spawns:                                                           │
│    └── JobWorker ────────────────────────────────────────────────────────────┐ │
│         ├── job: JobSession (owned)                                          │ │
│         ├── run_pool: Arc<RunPool> ◄─────────────────────────────────────────┤ │
│         ├── round_aggs: HashMap (owned)                                      │ │
│         ├── result_rx/tx (owned channel ends)                                │ │
│         └── event_tx (clone of Orchestrator's)                               │ │
│                                                                                 │
│  TargetManager spawns:                                                          │
│    └── VMExecutor ───────────────────────────────────────────────────────────┐ │
│         ├── info: VMInfo (owned)                                             │ │
│         ├── targets: Arc<TargetManager> ◄────────────────────────────────────┤ │
│         ├── run_pool: Arc<RunPool> ◄─────────────────────────────────────────┤ │
│         ├── in_flight: Option<InFlightRun> (owned)                           │ │
│         └── remote_tx/rx (owned channel ends)                                │ │
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
│  API Request                                                                    │
│       │                                                                         │
│       ▼                                                                         │
│  JobSession ──────────────► job_tx ──────────────► Orchestrator                 │
│       │                                                 │                       │
│       │                                                 │ spawn                 │
│       ▼                                                 ▼                       │
│  ModularBuildSpec ◄──────────────────────────── JobWorker.job                   │
│       │                                                 │                       │
│       │                                                 │ build                 │
│       ▼                                                 ▼                       │
│  ArtifactRef ────────────────────────────────► RunEnvelope                      │
│                                                        │                        │
│                                                        │ add_runs               │
│                                                        ▼                        │
│                                                   RunPool.pending               │
│                                                        │                        │
│                                                        │ take_run               │
│                                                        ▼                        │
│                                                   VMExecutor.dispatch           │
│                                                        │                        │
│                                                        │ gRPC                   │
│                                                        ▼                        │
│                                                   Remote VM Agent               │
│                                                        │                        │
│                                                        │ SampleResponse         │
│                                                        ▼                        │
│  RemoteRunResult ◄──────────────────────────── StreamHandler                    │
│       │                                                                         │
│       │ convert                                                                 │
│       ▼                                                                         │
│  JobRunResult ───────────► route_result ──────► JobWorker.result_rx             │
│       │                                                 │                       │
│       │                                                 │ aggregate             │
│       ▼                                                 ▼                       │
│  RunOutcome ─────────────────────────────────► RoundAgg.baseline/instrumented   │
│                                                        │                        │
│                                                        │ to_summary             │
│                                                        ▼                        │
│  RoundSummary ───────────► JobWorkerEvent ────► Orchestrator                    │
│       │                                                 │                       │
│       │                                                 │ index                 │
│       ▼                                                 ▼                       │
│  JobSession.rounds ◄─────────────────────────── Elasticsearch                   │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Channel Type Summary

| Channel | Message Type | Sender | Receiver |
|---------|-------------|--------|----------|
| `job_tx` | `JobSession` | SchedulerService | Orchestrator |
| `job_control_tx` | `JobControlCommand` | SchedulerService | Orchestrator |
| `events_tx` | `TargetEvent` | TargetManager, StreamHandler | Orchestrator |
| `job_event_tx` | `JobWorkerEvent` | JobWorker | Orchestrator |
| `result_tx` (per job) | `JobRunResult` | RunPool.route_result | JobWorker |
| `stream_tx` (per VM) | `ControllerMessage` | VMExecutor, Heartbeat | StreamHandler→VM |
| `remote_rx` (per VM) | `RemoteRunResult` | StreamHandler | VMExecutor |

---

## Concurrency Primitives

| Primitive | Location | Purpose |
|-----------|----------|---------|
| `DashMap` | RunPool.pending | Lock-free run storage |
| `DashMap` | RunPool.by_os | Lock-free OS queue map |
| `DashMap` | RunPool.job_registry | Lock-free job info |
| `DashMap` | TargetManager.targets | Lock-free VM registry |
| `Mutex` | RunPool.by_os[os].queue | Per-OS queue ordering |
| `Mutex` | RunPool.metrics | Metrics updates |
| `RwLock` | RunPool.result_routers | Job→channel mapping |
| `Notify` | RunPool.runs_available | Wake VMExecutors |
| `CancellationToken` | RunPool.shutdown_token | Global shutdown |
| `CancellationToken` | JobWorker (in Orchestrator.job_workers) | Per-job shutdown |
