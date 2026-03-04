# Controller Module Reference

Complete struct-level reference for every public type, enum, and function in the controller crate.
Generated from `cargo modules structure` and `cargo doc --document-private-items` JSON output,
cross-referenced with CONTROLLER-ARCHITECTURE.md and CONTROLLER-ARCHITECTURE-2.md.

---

## 1. Module Tree

```
controller/
├── src/
│   ├── main.rs                          ← Entry point: config → ES → channels → Orchestrator → gRPC
│   ├── lib.rs                           ← Re-exports: api, dispatch, storage, triage, vm + protobuf
│   │
│   ├── api/                             ← gRPC ingress layer (25 RPCs, thin handlers)
│   │   ├── mod.rs                       ← SchedulerService struct
│   │   ├── artifact.rs                  ← BuildArtifact, DeployArtifact RPCs
│   │   ├── extract.rs                   ← JSON field extraction helpers
│   │   ├── job.rs                       ← Job lifecycle + round analysis RPCs
│   │   ├── utility.rs                   ← Ping, SubmitTriage, QueryResults RPCs
│   │   └── worker.rs                    ← Worker/monitoring/admin RPCs
│   │
│   ├── dispatch/                        ← Execution engine (producer-consumer pipeline)
│   │   ├── mod.rs                       ← Re-exports
│   │   ├── channels.rs                  ← Inter-component message types
│   │   ├── job_worker.rs                ← JobWorker: round production + aggregation
│   │   ├── orchestrator.rs              ← Orchestrator: central coordinator
│   │   ├── run_pool.rs                  ← RunPool: OS-sharded work queue
│   │   ├── vm_executor.rs               ← VMExecutor: per-VM dispatch loop
│   │   └── types/                       ← Domain types
│   │       ├── mod.rs                   ← Re-exports
│   │       ├── config.rs                ← ModularBuildSpec, ModuleSelectionSpec
│   │       ├── ids.rs                   ← Newtype IDs (JobId, RunId, RoundId, etc.)
│   │       ├── round.rs                 ← RoundAgg, RoundSummary, DifferentialCategory, evasion scoring
│   │       ├── run.rs                   ← RunEnvelope, RunType, ArtifactRef, VMInfo
│   │       └── session.rs               ← JobSession, JobInfo, JobStatus, JobOutcome
│   │
│   ├── storage/                         ← ElasticSearch data-access layer (6 index families)
│   │   ├── mod.rs                       ← EsStorage facade + TelemetryContext
│   │   ├── artifacts.rs                 ← Artifact indexing
│   │   ├── helpers.rs                   ← Index naming, timestamps, update-with-retry
│   │   ├── jobs.rs                      ← Job write operations
│   │   ├── queries.rs                   ← All read queries (returns raw JSON)
│   │   ├── rounds.rs                    ← Round indexing + coverage/score updates
│   │   ├── runs.rs                      ← Run result indexing
│   │   ├── telemetry.rs                 ← Bulk telemetry ingestion
│   │   └── templates.rs                 ← 5 ES index templates (jobs, rounds, runs, telemetry, tokens)
│   │
│   ├── triage/                          ← Intelligence layer (feedback loop)
│   │   ├── mod.rs                       ← Selector trait, SearchSpace, Selection, TriageGuidance
│   │   ├── coverage_selector.rs         ← CoverageSelector (epsilon-greedy, default)
│   │   ├── fuzzer_selector.rs           ← FuzzerSelector (genetic algorithm)
│   │   ├── random_selector.rs           ← RandomSelector (uniform baseline)
│   │   ├── token_selector.rs            ← TokenSelector (token-biased epsilon-greedy)
│   │   ├── extractor.rs                 ← Token extraction (9 categories) from telemetry
│   │   ├── scorer.rs                    ← Token scoring (lift × confidence → guidance)
│   │   ├── param_space.rs               ← Mutation parameter space (sample, perturb, distance)
│   │   ├── source_resolver.rs           ← SourceMap: line trace → coverage
│   │   └── token_diff.rs               ← Token set comparison (Jaccard distance)
│   │
│   └── vm/                              ← Physical VM connection layer
│       └── manager.rs                   ← TargetManager, Target, streams, heartbeat, reconnect
│
├── build.rs                             ← tonic/prost codegen (3 proto files → Rust)
└── automutate/                          ← Generated protobuf modules
    ├── common.rs                        ← ControllerMessage, WorkerMessage, etc.
    ├── controller.rs                    ← SchedulerService gRPC server/client stubs
    └── worker.rs                        ← Worker gRPC stubs
```

---

## 2. Module Overview

| Module | Lines | Files | Role | Key Struct |
|--------|------:|------:|------|-----------|
| `api/` | ~2,146 | 5 | gRPC boundary — translates proto ↔ domain types | `SchedulerService` |
| `dispatch/` | ~4,938 | 12 | Experiment loop — rounds, runs, aggregation | `Orchestrator`, `JobWorker`, `RunPool`, `VMExecutor` |
| `storage/` | ~1,788 | 9 | Persistence — all ES reads/writes | `EsStorage` |
| `triage/` | ~5,721 | 10 | Decision-making — token extraction, scoring, selection | `CoverageSelector`, `FuzzerSelector`, `TokenSelector` |
| `vm/` | ~1,081 | 2 | Transport — VM connections, bidi streams | `TargetManager` |
| `automutate/` | generated | 3 | Protobuf stubs (common, controller, worker) | — |
| **Total** | **~15,674** | **38** | | |

---

## 3. Module: `api/`

**Purpose:** Single entry-point for all external gRPC interactions. Pure translation layer — no business logic.
All handlers validate, delegate to storage/channels, map results to proto, and return.

**Key invariant:** The API is a **producer only** for channels (`job_tx`, `job_control_tx`).
All result observation is through ES queries.

### 3.1 `SchedulerService`

The gRPC server handler struct. Implements `Clone` (all fields are `Arc` or channel senders).

| Field | Type | Description |
|-------|------|-------------|
| `storage` | `Arc<EsStorage>` | ES read/write access for all query RPCs |
| `job_tx` | `mpsc::Sender<JobSession>` | Channel ① → Orchestrator (job submissions) |
| `job_control_tx` | `mpsc::Sender<JobControlCommand>` | Channel ② → Orchestrator (stop commands) |
| `targets` | `Arc<TargetManager>` | Read-only access for worker listing/metadata RPCs |
| `run_pool` | `Arc<RunPool>` | Read-only access for pool metrics and job queries |

**Methods:**
- `new(storage, job_tx, job_control_tx, targets, run_pool)` — Constructor

### 3.2 Functions by Submodule

#### `artifact.rs` — Artifact RPCs

| Function | Description |
|----------|-------------|
| `build_artifact(svc, req)` | Calls `ArtifactBuilder::build()` from the `build` crate. Returns build metadata. |
| `deploy_artifact(svc, req)` | Uploads built artifact to a target VM via `TargetManager::send_artifact()`. |

#### `job.rs` — Job Lifecycle + Analysis RPCs

| Function | Description |
|----------|-------------|
| `schedule_job(svc, req)` | Creates `JobSession` from proto, sends via `job_tx`. Non-blocking. |
| `stop_job(svc, req)` | Sends `JobControlCommand::Stop` via `job_control_tx`. |
| `get_job_status(svc, req)` | Queries ES for job document. |
| `get_job_progress(svc, req)` | Queries ES for job + rounds. Returns progress summary. |
| `report_status(svc, req)` | Legacy: indexes a run status report to ES. |
| `get_round(svc, req)` | Queries ES for round document by round_id. |
| `compare_runs(svc, req)` | Queries two runs from ES, builds differential comparison. |
| `compare_tokens(svc, req)` | Queries two token sets, computes Jaccard distance + mutation diffs. |
| `get_trace_lines(svc, req)` | Queries trace line events from ES telemetry. |

**Private helpers:**
- `round_doc_to_proto` / `run_doc_to_proto` — JSON → proto conversion
- `parse_mutation_recipe` — Extracts mutation YAML from round doc
- `apply_round_correction` — Applies blended evasion score to round proto
- `build_behavior_comparison` — Builds behavioral diff between two runs

#### `worker.rs` — Worker/Monitoring/Admin RPCs

| Function | Description |
|----------|-------------|
| `list_workers(svc, req)` | Lists all registered targets via `TargetManager::list_all()`. |
| `get_worker(svc, req)` | Gets single target info. |
| `get_available_workers(svc, req)` | Lists targets with `Available` status. |
| `get_worker_metadata(svc, req)` | Queries detailed metadata via `TargetManager::get_worker_info()` (unary gRPC). |
| `stream_telemetry(svc, req)` | Queries telemetry events from ES by job_id. |
| `get_pool_metrics(svc, req)` | Returns `RunPoolMetrics` from `RunPool::get_metrics()`. |
| `get_orchestrator_status(svc, req)` | Returns active job count, VM count, pool size. |
| `ping_worker(svc, req)` | Sends heartbeat to a specific VM via `TargetManager::send_command()`. |
| `disconnect_worker(svc, req)` | Disconnects a single VM. |
| `disconnect_all_workers(svc, req)` | Disconnects all VMs. |

#### `utility.rs` — Utility RPCs

| Function | Description |
|----------|-------------|
| `ping(svc, req)` | Health check — returns "pong". |
| `submit_triage(svc, req)` | Legacy: indexes a triage result to ES. |
| `query_results(svc, req)` | Generic ES query for analysis results. |

#### `extract.rs` — JSON Field Extraction Helpers

Pure utility functions for extracting typed values from `serde_json::Value`:

`str_field`, `string_array_field`, `u32_field`, `u64_field`, `i32_field`, `f64_field`, `bool_field`, `parse_date_to_unix_secs`

---

## 4. Module: `dispatch/`

**Purpose:** Implements the closed experiment loop: select mutations → build artifacts → dispatch to VMs → aggregate results → finalize differential category → trigger triage.

Four components form a producer-consumer pipeline:
`Orchestrator` → `JobWorker` → `RunPool` → `VMExecutor`

### 4.1 Core Components

#### `Orchestrator`

Single long-lived coordinator. Runs a biased `tokio::select!` loop receiving on 4 channels.

| Field | Type | Description |
|-------|------|-------------|
| `run_pool` | `Arc<RunPool>` | Shared work queue (reads/writes) |
| `targets` | `Arc<TargetManager>` | VM registry (reads for constraint resolution) |
| `storage` | `Arc<EsStorage>` | ES facade (indexes rounds, runs, coverage) |
| `job_workers` | `HashMap<JobId, JobHandle>` | Active job map (single-threaded access, no mutex) |
| `job_event_tx` | `mpsc::Sender<JobWorkerEvent>` | Channel ④ sender — cloned into each spawned JobWorker |
| `job_event_rx` | `mpsc::Receiver<JobWorkerEvent>` | Channel ④ receiver — JobWorker completions |
| `vms` | `HashMap<WorkerId, WorkerInfo>` | Connected VM info cache for constraint resolution |
| `events_rx` | `mpsc::Receiver<TargetEvent>` | Channel ③ receiver — VM lifecycle + telemetry events |
| `job_submit_rx` | `mpsc::Receiver<JobSession>` | Channel ① receiver — new job submissions |
| `job_control_rx` | `mpsc::Receiver<JobControlCommand>` | Channel ② receiver — stop commands |

**Methods:**

| Method | Description |
|--------|-------------|
| `new(...)` | Constructor with all channels and shared state |
| `run()` | Main `select!` loop: receives on 4 channels, dispatches to handlers |
| `spawn_job_worker(job)` | Creates `JobWorker`, spawns as tokio task, stores `JobHandle` |
| `on_target_event(event)` | Handles VM connect/disconnect, indexes telemetry (fire-and-forget) |
| `on_job_worker_event(event)` | Handles `RoundCompleted` (index + coverage) and `JobCompleted` (cleanup) |
| `on_job_control(cmd)` | Handles `Stop` → cancels job's `CancellationToken` |
| `resolve_job_constraints(job)` | Finds compatible VMs by OS + capabilities |
| `handle_worker_message(msg)` | Processes incoming worker messages |
| `shutdown_job(job_id)` | Cancels a single job |
| `shutdown_all_jobs()` | Cancels all active jobs |
| `active_job_count()` | Returns `job_workers.len()` |
| `vm_count()` | Returns `vms.len()` |

**Module-level functions:**
- `index_round_and_runs(es, data)` — Indexes round + run documents to ES
- `compute_round_coverage(es, data)` — Resolves traces → `SourceMap` → `CoverageResult` → updates ES → sends `CoverageCorrection`

#### `JobHandle`

Lightweight handle stored in `Orchestrator.job_workers` for each active job.

| Field | Type | Description |
|-------|------|-------------|
| `shutdown_token` | `CancellationToken` | Per-job cancellation (triggered by `StopJob` RPC) |
| `correction_tx` | `mpsc::Sender<CoverageCorrection>` | Channel ⑧ sender → JobWorker |

#### `JobWorker`

One per active job. Produces rounds, aggregates results, finalizes differential categories.

| Field | Type | Description |
|-------|------|-------------|
| `job` | `JobSession` | Job configuration and progress state |
| `run_pool` | `Arc<RunPool>` | Shared pool (registers job, adds runs) |
| `result_rx` | `mpsc::Receiver<JobRunResult>` | Channel ⑤ receiver — run results from RunPool routing |
| `result_tx` | `mpsc::Sender<JobRunResult>` | Channel ⑤ sender — registered with RunPool on job start |
| `round_aggs` | `HashMap<RoundId, RoundAgg>` | In-flight round join states (max `MAX_IN_FLIGHT_ROUNDS=5`) |
| `event_tx` | `mpsc::Sender<JobWorkerEvent>` | Channel ④ sender → Orchestrator |
| `selector` | `Arc<dyn Selector>` | Mutation selection strategy (chosen by `SearchSpace`) |
| `shutdown_token` | `CancellationToken` | Per-job cancellation |
| `artifact_cleanup` | `Vec<PathBuf>` | Artifact paths to delete on job completion |
| `baseline_payload` | `Option<PreparedPayload>` | Cached baseline payload (from `build` crate) |
| `instrumented_payload` | `Option<PreparedPayload>` | Cached instrumented payload |
| `cached_payload` | `Option<Vec<u8>>` | Raw payload bytes (if `cache_payload` enabled) |
| `correction_rx` | `mpsc::Receiver<CoverageCorrection>` | Channel ⑧ receiver ← Orchestrator |
| `storage` | `Option<Arc<EsStorage>>` | ES access for triage extraction spawns |
| `latest_guidance` | `Option<TriageGuidance>` | Most recent triage guidance (avoid/seek tokens) |
| `guidance_rx` | `mpsc::Receiver<TriageGuidance>` | Channel ⑨ receiver ← triage background task |
| `guidance_tx` | `mpsc::Sender<TriageGuidance>` | Channel ⑨ sender — passed to `extract_and_score()` spawns |

**Methods:**

| Method | Description |
|--------|-------------|
| `new(...)` | Constructor |
| `run()` | Main loop: every 100ms checks `can_produce_round()`, receives results, checks guidance/corrections |
| `can_produce_round()` | Backpressure: `current_round < max_rounds` AND `in_flight < 5` AND `pending_runs < 9` |
| `produce_round()` | Full round production: select → build × 2 → static scan → 3 RunEnvelopes → RunPool |
| `build_artifact(spec)` | Calls `ArtifactBuilder::build()` |
| `on_result(result)` | Matches result to RoundAgg, updates join state, checks completeness |
| `finalize_round(round_id)` | Computes `DifferentialCategory` + `evasion_score`, emits `RoundCompleted`, spawns triage |
| `apply_coverage_correction(corr)` | Updates round history with blended evasion score |
| `is_job_complete()` | `!should_continue() && round_aggs.is_empty()` |
| `cancellation_token()` | Returns shutdown token |
| `job_id()` | Returns job ID |

**Module-level functions:**
- `create_run_envelopes(...)` — Creates 3 `RunEnvelope`s (baseline + instrumented + dryrun)
- `static_defender_scan(path)` — Runs `MpCmdRun.exe -Scan -ScanType 3` from WSL; exit 2 = detected
- `build_round_completed_data(...)` — Packages `RoundCompletedData` for the Orchestrator

#### `RunPool`

Shared OS-sharded work queue connecting JobWorkers (producers) to VMExecutors (consumers).

| Field | Type | Description |
|-------|------|-------------|
| `pending` | `DashMap<RunId, RunEnvelope>` | Primary storage — lock-free concurrent access |
| `by_os` | `DashMap<String, Mutex<VecDeque<RunId>>>` | Per-OS queues — VMExecutors only lock their OS queue |
| `runs_available` | `Notify` | Broadcast wake — `notify_waiters()` on `add_runs()` |
| `result_routers` | `RwLock<HashMap<JobId, Sender<JobRunResult>>>` | Per-job result channels — routes results to correct JobWorker |
| `job_registry` | `DashMap<JobId, JobInfo>` | Persists after job completion (for API queries) |
| `shutdown_token` | `CancellationToken` | Global shutdown broadcast to all VMExecutors |
| `metrics` | `Mutex<RunPoolMetrics>` | Aggregate counters |

**Methods:**

| Method | Description |
|--------|-------------|
| `new()` | Creates empty pool |
| `register_job(job_id, info, result_tx)` | Registers job in `job_registry` + `result_routers` |
| `unregister_job(job_id)` | Removes from `result_routers`, removes pending runs |
| `add_runs(runs)` | Stores in `pending`, shards into `by_os` queues, calls `notify_waiters()` |
| `take_run(vm_os, vm_caps)` | Pops from OS queue, checks capabilities + dryrun guard. Returns `Option<RunEnvelope>` |
| `wait_for_runs()` | Awaits `runs_available.notified()` — VMExecutors sleep here |
| `route_result(result)` | Looks up `result_routers[job_id]`, sends `JobRunResult` to JobWorker |
| `remove_run(run_id)` | Removes from `pending` (stale queue entries skipped by `take_run`) |
| `remove_runs_for_job(job_id)` | Removes all pending runs for a job |
| `complete_job(job_id, outcome)` | Updates `job_registry` status |
| `record_round_completed(job_id)` | Increments round counter in `job_registry` |
| `update_job_progress(job_id, round, total)` | Updates progress in `job_registry` |
| `get_job_info(job_id)` | Returns `JobInfo` from registry |
| `list_jobs()` / `list_running_jobs()` | Lists all/running jobs from registry |
| `pending_runs_for_job(job_id)` | Counts pending runs for a job |
| `get_metrics()` | Returns `RunPoolMetrics` |
| `pool_size()` / `pool_size_by_os()` | Returns queue sizes |
| `job_count()` | Returns number of registered jobs |
| `shutdown()` | Cancels `shutdown_token` |
| `is_shutdown()` / `cancellation_token()` | Shutdown state queries |

#### `RunPoolMetrics`

| Field | Type | Description |
|-------|------|-------------|
| `total_runs_added` | `u64` | Cumulative runs added |
| `total_runs_taken` | `u64` | Cumulative runs dispatched |
| `total_results_routed` | `u64` | Cumulative results delivered |
| `active_jobs` | `usize` | Currently running jobs |
| `total_rounds_completed` | `u64` | Cumulative rounds finalized |
| `total_jobs_completed` | `u64` | Cumulative jobs completed |

#### `VMExecutor`

One per connected VM. Takes runs from pool, dispatches to VM, routes results back.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | VM identifier |
| `info` | `VMInfo` | VM OS, capabilities |
| `targets` | `Arc<TargetManager>` | For `reserve()`, `release()`, artifact upload |
| `run_pool` | `Arc<RunPool>` | For `take_run()`, `route_result()` |
| `remote_tx` | `mpsc::Sender<ControllerMessage>` | Channel ⑥ — commands to VM via StreamHandler |
| `remote_rx` | `mpsc::Receiver<RemoteRunResult>` | Channel ⑦ — results from StreamHandler |
| `artifact_sender` | `Arc<dyn ArtifactSender>` | Abstraction for artifact upload (testable) |
| `in_flight` | `Option<InFlightRun>` | Currently executing run (at most 1) |

**Methods:**

| Method | Description |
|--------|-------------|
| `new(...)` | Constructor |
| `run()` | Main `select!` loop: shutdown / result_rx / wait_for_runs |
| `dispatch(envelope)` | `reserve` → upload artifact → `RunSampleCommand` → `in_flight = Some(...)` |
| `on_result_received(result)` | Verify match → `release` → clear `in_flight` → `route_result()` |
| `route_error(error)` | Routes error result when dispatch fails |
| `id()` / `info()` / `is_idle()` | Accessors |

**Trait:**
- `ArtifactSender` — `async fn send_artifact(target_id, path, sha256) → Result<()>`. Implemented by `TargetArtifactSender`.

#### `InFlightRun`

| Field | Type | Description |
|-------|------|-------------|
| `envelope` | `RunEnvelope` | The currently-executing run envelope |

---

### 4.2 Channel Types (`channels.rs`)

#### `JobRunResult`

Routed from `RunPool` to `JobWorker` via per-job result channel (⑤).

| Field | Type | Description |
|-------|------|-------------|
| `run_id` | `RunId` | Which run completed |
| `job_id` | `JobId` | Which job owns this run (used for routing lookup) |
| `round_id` | `RoundId` | Which round this run belongs to |
| `outcome` | `RunOutcome` | Detection result, exit code, timing |
| `vm_id` | `String` | Which VM executed this run |

#### `RemoteRunResult`

From `StreamHandler` to `VMExecutor` via channel ⑦.

| Field | Type | Description |
|-------|------|-------------|
| `run_id` | `RunId` | Run identifier |
| `detected` | `bool` | Whether EDR detected the artifact |
| `exit_code` | `i32` | Process exit code |
| `success` | `bool` | Whether execution completed normally |
| `error` | `Option<String>` | Error message if execution failed |
| `elapsed_ms` | `f64` | Execution time in milliseconds |
| `detection_verdict` | `String` | Human-readable verdict string |
| `last_checkpoint` | `String` | Last reached execution checkpoint |

#### `CoverageCorrection`

From Orchestrator to JobWorker via channel ⑧. Sent after async coverage computation.

| Field | Type | Description |
|-------|------|-------------|
| `round_number` | `u32` | Which round to correct |
| `coverage_percent` | `f64` | Code coverage percentage (0–100) |
| `blended_evasion_score` | `f64` | Recomputed score: `0.7 × (coverage/100) + 0.3 × time_factor` |

#### `RoundCompletedData`

Emitted via `JobWorkerEvent::RoundCompleted`. Contains everything the Orchestrator needs for indexing.

| Field | Type | Description |
|-------|------|-------------|
| `job_id` | `JobId` | Job identifier |
| `round_id` | `RoundId` | Round identifier |
| `summary` | `RoundSummary` | Finalized round summary |
| `baseline_run_id` | `RunId` | Baseline run ID |
| `instrumented_run_id` | `RunId` | Instrumented run ID |
| `baseline_outcome` | `RunOutcome` | Baseline result |
| `instrumented_outcome` | `RunOutcome` | Instrumented result |
| `mutation_specs` | `Vec<MutationSpec>` | Applied mutations with params |
| `mutations` | `Vec<String>` | Mutation ID strings |
| `modules` | `ModuleSelectionSpec` | Selected module configuration |
| `baseline_vm_id` | `String` | VM that ran baseline |
| `instrumented_vm_id` | `String` | VM that ran instrumented |
| `round_started_at` | `SystemTime` | Round start timestamp |
| `assembled_source` | `Option<String>` | Pre-instrumentation C source (for line trace resolution) |
| `dryrun_run_id` | `Option<RunId>` | Dryrun run ID (None if dryrun not received) |
| `dryrun_outcome` | `Option<RunOutcome>` | Dryrun result |
| `dryrun_vm_id` | `String` | VM that ran dryrun |

#### `enum JobControlCommand`

| Variant | Description |
|---------|-------------|
| `Stop { job_id }` | Cancel a running job |

#### `enum JobWorkerEvent`

| Variant | Description |
|---------|-------------|
| `RoundCompleted(Box<RoundCompletedData>)` | A round has been finalized |
| `JobCompleted { job_id, outcome }` | A job has finished (completed/stopped/failed) |

---

### 4.3 Type System (`types/`)

#### `ids.rs` — Newtype Identifiers

All IDs are newtype wrappers around `String` with `new(impl Into<String>)` and `as_str()`.
All implement `Clone`, `Debug`, `Display`, `Hash`, `Eq`, `PartialEq`, `Serialize`, `Deserialize`.

| Type | Usage |
|------|-------|
| `JobId` | Identifies a mutation campaign (UUID) |
| `RoundId` | Identifies a round within a job (`{job_id}-round-{N}`) |
| `RunId` | Identifies a single execution (`{round_id}-{base\|inst\|dry}`) |
| `TargetId` | Identifies a VM target (from TOML config) |
| `WorkerId` | Identifies a worker agent (from VM registration) |

#### `config.rs` — Build Configuration

**`ModularBuildSpec`** — Per-job build configuration.

| Field | Type | Description |
|-------|------|-------------|
| `modules` | `ModuleSelectionSpec` | Module gene selections |
| `payload_path` | `PathBuf` | Path to shellcode payload |
| `encoding` | `String` | Payload encoding method (default: `"xor"`) |

**`ModuleSelectionSpec`** — Which module variant to use for each gene slot.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `carrier` | `String` | `"alloc_rw_rx"` | Memory allocation strategy |
| `decoder` | `String` | `"standard"` | Payload decoding method |
| `antiemulation` | `String` | `"none"` | Anti-emulation technique |
| `deconditioner` | `String` | `"none"` | Deconditioner module |
| `guardrail` | `String` | `"none"` | Execution guardrail |
| `virtualprotect` | `String` | `"none"` | Memory protection strategy |
| `decoy` | `String` | `"none"` | Decoy behavior |

- `from_proto_or_default(proto)` — Creates from protobuf with defaults for missing fields.

#### `round.rs` — Round Types

**`RoundSpec`** — Immutable round configuration (created at round start).

| Field | Type | Description |
|-------|------|-------------|
| `id` | `RoundId` | Round identifier |
| `job_id` | `JobId` | Parent job |
| `round_number` | `u32` | Sequential round number |
| `mutations` | `Vec<MutationSpec>` | Applied mutation specifications |
| `modules` | `ModuleSelectionSpec` | Module configuration |

**`MutationSpec`** — A single mutation with optional parameters.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Mutation identifier (e.g., `"ast.string_xor"`) |
| `params` | `Option<serde_json::Value>` | Mutation parameters (JSON) |

**`RoundAgg`** — Ephemeral join state for aggregating 2–3 run results.

| Field | Type | Description |
|-------|------|-------------|
| `spec` | `RoundSpec` | Round configuration |
| `baseline_run_id` | `RunId` | Expected baseline run ID |
| `instrumented_run_id` | `RunId` | Expected instrumented run ID |
| `dryrun_run_id` | `RunId` | Expected dryrun run ID |
| `baseline` | `Option<RunOutcome>` | Set when baseline completes |
| `instrumented` | `Option<RunOutcome>` | Set when instrumented completes |
| `dryrun` | `Option<RunOutcome>` | Set when dryrun completes |
| `baseline_vm_id` | `String` | VM that executed baseline |
| `instrumented_vm_id` | `String` | VM that executed instrumented |
| `dryrun_vm_id` | `String` | VM that executed dryrun |
| `started_at` | `SystemTime` | Round start time |
| `timeout_ms` | `u64` | Run timeout (for `survival_ratio` computation) |
| `assembled_source` | `Option<String>` | Pre-instrumentation C source |
| `baseline_artifact_path` | `PathBuf` | Baseline artifact on disk |
| `instrumented_artifact_path` | `PathBuf` | Instrumented artifact on disk |
| `dryrun_deadline` | `Option<Instant>` | Grace period deadline (5s after baseline+instrumented complete) |
| `static_scan_detected` | `bool` | Whether Defender static scan detected the artifact |

**Methods:**
- `new(spec, ...)` — Constructor
- `is_complete()` — `baseline.is_some() && instrumented.is_some()` (dryrun not required)
- `to_summary()` — Computes `DifferentialCategory` + `evasion_score` → `RoundSummary`
- `compute_evasion_score(baseline, instrumented, category)` — Per-category score with ranges

**`RunOutcome`** — Result of a single run.

| Field | Type | Description |
|-------|------|-------------|
| `detected` | `bool` | EDR detection flag |
| `exit_code` | `i32` | Process exit code |
| `error` | `Option<String>` | Error message |
| `success` | `bool` | Clean execution completion |
| `elapsed_ms` | `f64` | Execution duration |
| `detection_verdict` | `String` | Human-readable verdict |
| `last_checkpoint` | `String` | Last execution checkpoint reached |

**`RoundSummary`** — Finalized round result (stored in `JobSession.rounds`).

| Field | Type | Description |
|-------|------|-------------|
| `round_id` | `RoundId` | Round identifier |
| `round_number` | `u32` | Sequential number |
| `mutations` | `Vec<String>` | Mutation ID strings |
| `mutation_specs` | `Vec<MutationSpec>` | Full mutation specs with params |
| `modules` | `ModuleSelectionSpec` | Module configuration |
| `detected` | `bool` | Whether detection occurred |
| `behavior_match` | `bool` | Whether baseline and instrumented behaved identically |
| `evasion_score` | `f64` | Composite evasion score (per-category range) |
| `differential_category` | `DifferentialCategory` | Two-run differential result |
| `completed_at` | `SystemTime` | Completion timestamp |
| `dry_run_exit_code` | `Option<i32>` | Dryrun exit code (if available) |
| `has_dryrun` | `bool` | Whether dryrun result was received |
| `detection_verdict` | `String` | Human-readable verdict |
| `coverage_percent` | `Option<f64>` | Code coverage (set asynchronously via correction) |
| `time_factor` | `f64` | Normalized time component (stored for blended recomputation) |

**`enum DifferentialCategory`** — Two-run differential protocol outcome.

| Variant | Condition | `is_detected()` | `is_trustworthy()` |
|---------|-----------|:---:|:---:|
| `RealDetection` | Baseline detected, instrumented detected | yes | yes |
| `InstrumentationArtifact` | Baseline clean, instrumented detected | no | no |
| `Flaky` | Baseline detected, instrumented clean | no | no |
| `Evasion` | Both clean | no | yes |
| `MutationFailed` | Dryrun crash (artifact broken) | no | no |
| `PayloadFailed` | Dryrun crash (payload broken) | no | no |
| `StaticDetection` | Defender static file scan hit | yes | yes |

**Methods:** `as_str()`, `from_runs(baseline, instrumented)`, `is_detected()`, `is_trustworthy()`

**Evasion scoring (per-category ranges):**

| Category | Range | Formula |
|----------|-------|---------|
| `RealDetection` | 0.0–0.4 | `0.4 × survival_ratio` |
| `InstrumentationArtifact` | 0.5–0.7 | `0.5 + 0.2 × survival_ratio` |
| `Flaky` | 0.0–0.3 | `0.3 × survival_ratio` |
| `Evasion` | 0.6–1.0 | `0.6 + 0.2 × payload_reached + 0.2 × behavior_match` |
| `MutationFailed` / `PayloadFailed` / `StaticDetection` | 0.0 | Always 0 |

Where `survival_ratio = elapsed_ms / max(timeout_ms, 100s)`, `payload_reached = exit_code == 0 ? 1.0 : 0.0`.

**Free function:**
- `compute_blended_evasion_score(category, coverage_percent, time_factor)` — Async post-coverage recomputation: `blend = 0.7 × (coverage/100) + 0.3 × time_factor`, then per-category formula applied to `blend`.
- `override_with_dryrun(base, instr, dryrun)` — Dryrun crash overrides to `MutationFailed` or `PayloadFailed`.

#### `run.rs` — Run Types

**`RunEnvelope`** — A run waiting in the pool.

| Field | Type | Description |
|-------|------|-------------|
| `run_id` | `RunId` | Run identifier |
| `job_id` | `JobId` | Parent job |
| `round_id` | `RoundId` | Parent round |
| `round_number` | `u32` | Round sequence number |
| `run_type` | `RunType` | Baseline / Instrumented / DryRun |
| `artifact` | `ArtifactRef` | Path + SHA256 of the built PE |
| `mutations` | `Vec<String>` | Applied mutation IDs |
| `timeout_seconds` | `u32` | Execution timeout |
| `required_os` | `String` | Target OS (e.g., `"windows"`) |
| `required_capabilities` | `Vec<String>` | Required VM capabilities (e.g., `["defender"]`) |

**`enum RunType`**

| Variant | `trace_mode()` | `is_dryrun()` | Description |
|---------|:-:|:-:|-------------|
| `Baseline` | `"off"` | `false` | Ground-truth EDR behavior |
| `Instrumented` | `"lines"` | `false` | Execution path tracing |
| `DryRun` | `"off"` | `true` | Loader sanity check (no EDR) |

**`ArtifactRef`** — Reference to a built artifact.

| Field | Type | Description |
|-------|------|-------------|
| `path` | `PathBuf` | File path on disk |
| `sha256` | `Option<String>` | Content hash for deduplication |

**`VMInfo`** — VM identity for VMExecutor.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | VM identifier |
| `os` | `String` | OS version (e.g., `"windows"`) |
| `capabilities` | `Vec<String>` | VM capabilities |

**`WorkerInfo`** — Lightweight VM reference in Orchestrator.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `WorkerId` | Worker identifier |
| `os` | `String` | OS version |
| `capabilities` | `Vec<String>` | Capabilities |

**Free functions:**
- `capabilities_match(required, available)` — Checks if VM has all required capabilities
- `chunk_artifact(path)` — Reads artifact file into gRPC-sized chunks for upload

#### `session.rs` — Job Types

**`JobSession`** — Mutable per-job state, owned by `JobWorker`.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `JobId` | Job identifier (UUID) |
| `target_os` | `Option<String>` | Required OS constraint |
| `required_capabilities` | `Vec<String>` | Required VM capabilities |
| `build_spec` | `ModularBuildSpec` | Build configuration |
| `trace_mode` | `String` | Trace mode override |
| `search_space` | `SearchSpace` | Mutation search configuration |
| `current_round` | `u32` | Current round number |
| `completed_rounds` | `u32` | Rounds fully completed |
| `max_rounds` | `u32` | Maximum rounds to run |
| `stop_on_evasion` | `bool` | Stop early on `Evasion` category |
| `sc_checkpoint_count` | `Option<u32>` | Expected shellcode checkpoints |
| `cache_payload` | `bool` | Whether to cache encoded payload |
| `msvc_compat` | `bool` | MSVC compatibility mode |
| `msvc_vcvarsall` | `String` | MSVC vcvarsall.bat path |
| `rounds` | `BTreeMap<u32, RoundSummary>` | Completed round history (read by selector) |
| `last_round` | `Option<RoundSummary>` | Most recent round |
| `created_at` | `SystemTime` | Creation timestamp |
| `started_at` | `Option<SystemTime>` | First round start timestamp |

**Methods:**
- `new(id, build_spec, max_rounds)` — Constructor
- `with_constraints(os, caps)` — Sets OS/capability constraints
- `mark_started()` — Sets `started_at`
- `start_round()` — Increments `current_round`, returns `(round_number, round_id)`
- `record_round_summary(summary)` — Stores in `rounds` + `last_round`
- `should_continue()` — `false` when `current_round >= max_rounds` OR (`stop_on_evasion` AND last round was `Evasion`)
- `to_info()` — Converts to `JobInfo`

**`JobInfo`** — Lightweight job state for registry queries.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `JobId` | Job identifier |
| `status` | `JobStatus` | Current status |
| `current_round` | `u32` | Current round number |
| `completed_rounds` | `u32` | Completed rounds |
| `max_rounds` | `u32` | Maximum rounds |
| `target_os` | `Option<String>` | OS constraint |
| `started_at` | `Option<SystemTime>` | Start timestamp |

**`enum JobStatus`**: `Running`, `Completed`, `Stopped`, `Failed`

**`enum JobOutcome`**

| Variant | Description |
|---------|-------------|
| `Completed { rounds_completed }` | All rounds finished |
| `Stopped { reason }` | Manually stopped |
| `Failed { error }` | Error occurred |

- `to_status()` — Maps to `JobStatus`

---

## 5. Module: `storage/`

**Purpose:** Single persistence boundary between the controller and ElasticSearch.
Owns schema templates, all writes (typed Rust structs), and all reads (raw JSON).

### 5.1 Structs

#### `EsStorage`

Facade struct — all ES operations go through this.

| Field | Type | Description |
|-------|------|-------------|
| `client` | `elasticsearch::Elasticsearch` | ES HTTP client (internally `Arc`, connection-pooled) |

**Write methods** (accept typed params, return `Result<()>`, use `Refresh::WaitFor`):

| Method | Index | Description |
|--------|-------|-------------|
| `index_job(job)` | `jobs-YYYY.MM` | Creates job document |
| `index_round(params)` | `rounds-YYYY.MM` | Creates round document |
| `index_run_result(params)` | `runs-YYYY.MM` | Creates run result document |
| `index_run_status(report)` | `runs-YYYY.MM` | Legacy: indexes status report (auto-ID, no WaitFor) |
| `index_telemetry_batch(events, ctx)` | `telemetry-YYYY.MM.DD` | Bulk indexes telemetry events |
| `index_token_set(round_id, tokens)` | `tokens-YYYY.MM` | Indexes extracted token set |
| `index_artifact(metadata)` | `artifacts-YYYY.MM` | Indexes build metadata |

**Update methods** (find monthly index, update with 3-retry conflict handling):

| Method | Description |
|--------|-------------|
| `update_job_status(job_id, status)` | Updates job status field |
| `update_job_started(job_id, started_at)` | Sets started timestamp |
| `update_job_progress(job_id, round, total)` | Updates progress fields |
| `update_job_field(job_id, field, value)` | Generic field update |
| `update_round_coverage(round_id, coverage)` | Sets coverage percentage |
| `update_round_evasion_score(round_id, score)` | Sets blended evasion score |

**Read methods** (return raw `serde_json::Value`, proto mapping is API layer's job):

| Method | Returns | Description |
|--------|---------|-------------|
| `query_job(job_id)` | `Option<Value>` | Job document by ID |
| `query_round(round_id)` | `Option<Value>` | Round document by ID |
| `query_rounds(job_id)` | `Vec<Value>` | All rounds for a job |
| `query_runs_by_ids(run_ids)` | `Vec<Value>` | Multiple runs by IDs |
| `query_api_telemetry(job_id, types)` | `Vec<Value>` | Telemetry events by job + type |
| `query_checkpoint_events(job_id, round_id)` | `Vec<Value>` | Checkpoint events |
| `query_trace_lines(job_id, round_id)` | `Vec<Value>` | Trace line events |
| `query_trace_content(job_id, round_id)` | `Vec<Value>` | Full trace content |
| `query_token_sets(job_id)` | `Vec<Value>` | All token sets for a job |
| `query_token_set_by_round_id(round_id)` | `Option<Value>` | Token set for a specific round |
| `query_analysis_results(job_id)` | `Vec<Value>` | Analysis results |

**Templates:**
- `ensure_templates()` — Creates 5 index templates (jobs v3, rounds v6, runs v4, telemetry v3, tokens v1). Non-fatal on failure.

#### `TelemetryContext`

Attached to telemetry events for run/round/VM correlation.

| Field | Type | Description |
|-------|------|-------------|
| `run_id` | `Option<String>` | Current run (None if telemetry arrives before run assignment) |
| `round_id` | `Option<String>` | Current round (None if telemetry arrives before round) |
| `vm_id` | `String` | Always present — from StreamHandler context |

#### `RoundIndexParams`

Parameters for `index_round()`.

| Field | Type | Description |
|-------|------|-------------|
| `job_id` | `&str` | Parent job |
| `summary` | `&RoundSummary` | Round summary |
| `mutation_specs` | `&[MutationSpec]` | Mutation specs |
| `baseline_run_id` | `&str` | Baseline run ID |
| `instrumented_run_id` | `&str` | Instrumented run ID |
| `started_at` | `Option<&str>` | RFC3339 start timestamp |
| `modules` | `Option<&ModuleSelectionSpec>` | Module configuration |
| `assembled_source` | `Option<&str>` | Pre-instrumentation source |
| `dry_run_exit_code` | `Option<i32>` | Dryrun exit code |
| `has_dryrun` | `bool` | Whether dryrun was received |
| `dryrun_run_id` | `Option<&str>` | Dryrun run ID |

#### `RunIndexParams`

Parameters for `index_run_result()`.

| Field | Type | Description |
|-------|------|-------------|
| `job_id` | `&str` | Parent job |
| `round_id` | `&str` | Parent round |
| `run_id` | `&str` | Run identifier |
| `run_type` | `&str` | `"baseline"` / `"instrumented"` / `"dryrun"` |
| `outcome` | `&RunOutcome` | Run result |
| `mutations` | `&[String]` | Applied mutations |
| `vm_id` | `&str` | Executing VM |

### 5.2 Helper Functions (`helpers.rs`)

| Function | Description |
|----------|-------------|
| `es_index_name(prefix)` | Returns `"{prefix}-YYYY.MM"` |
| `es_index_name_daily(prefix)` | Returns `"{prefix}-YYYY.MM.DD"` |
| `now_rfc3339()` | Current time as RFC3339 string |
| `now_unix_secs()` | Current time as Unix seconds |
| `system_time_to_rfc3339(t)` | Convert `SystemTime` to RFC3339 |
| `check_index_response(resp)` | Validates ES index response |
| `update_doc_by_id(client, index, id, body)` | Update with `Refresh::WaitFor` + 3-retry conflict |
| `insert_optional_field(map, key, value)` | Inserts non-None values into JSON map |

### 5.3 Index Families

| Index Pattern | Rotation | Doc ID | Template Version |
|---------------|----------|--------|:---:|
| `jobs-YYYY.MM` | Monthly | `job_id` | v3 |
| `rounds-YYYY.MM` | Monthly | `job_id/round_id` | v6 |
| `runs-YYYY.MM` | Monthly | `run_id` | v4 |
| `telemetry-YYYY.MM.DD` | Daily | auto | v3 |
| `tokens-YYYY.MM` | Monthly | auto | v1 |
| `artifacts-YYYY.MM` | Monthly | auto | — |

---

## 6. Module: `triage/`

**Purpose:** Close the feedback loop. Transform execution results into actionable mutation guidance.

Pipeline: `Execution results` → `Token extraction` → `Token scoring` → `TriageGuidance` → `Selector` → `Next round`

### 6.1 Core Types (`mod.rs`)

#### `trait Selector`

```rust
async fn select(
    &self,
    history: &BTreeMap<u32, RoundSummary>,
    guidance: Option<&TriageGuidance>,
    search_space: &SearchSpace,
) -> Selection;
```

Implemented by: `CoverageSelector`, `FuzzerSelector`, `TokenSelector`, `RandomSelector`.
All return baseline defaults on round 1.

#### `Selection`

Returned by `Selector::select()`.

| Field | Type | Description |
|-------|------|-------------|
| `modules` | `ModuleSelectionSpec` | Chosen module configuration |
| `mutations` | `Vec<MutationSpec>` | Chosen mutations with parameters |
| `rationale` | `String` | Human-readable explanation of selection logic |

#### `SearchSpace`

Per-job search configuration from the `ScheduleJob` request.

| Field | Type | Description |
|-------|------|-------------|
| `selector` | `SelectorType` | Which selector to use |
| `strategy` | `VariationStrategy` | How to vary explored mutations |
| `variable_categories` | `Vec<String>` | Module categories to vary |
| `mutation_pool` | `Vec<String>` | Override: explored mutation IDs |
| `mutation_targets` | `Vec<String>` | Override: target mutation IDs |
| `fixed_mutations` | `Vec<String>` | Override: always-applied mutation IDs |
| `fuzzer_config` | `Option<FuzzerConfig>` | GA parameters (for `FuzzerSelector`) |

#### `TriageGuidance`

Feedback from token scoring → selector.

| Field | Type | Description |
|-------|------|-------------|
| `avoid_tokens` | `Vec<String>` | Tokens correlated with detection (lift > 1.5, confidence > 0.3) |
| `seek_tokens` | `Vec<String>` | Tokens correlated with evasion (lift < 0.667, confidence > 0.3) |

Both lists capped at 50 tokens.

#### `enum SelectorType`

`Coverage` (default), `Fuzzer`, `Token`, `Random`

- `as_str()`, `from_str_or_default(s)`

#### `enum VariationStrategy`

Determines how the explored mutation is selected.

- `as_str()`, `from_str_or_default(s)`

### 6.2 Selectors

#### `CoverageSelector` (default)

Epsilon-greedy (ε=0.3) over evasion scores from round history.

| Algorithm Step | Description |
|---------------|-------------|
| 1. Fixed mutations | Always: 1 LLVM IR (`opaque_predicate`) + 9 Binary (PE normalization) |
| 2. Module selection | Varies categories listed in `search_space.variable_categories` |
| 3. Explored mutation | ε=0.3: 30% random from untried, 70% best by mean `evasion_score` |

**Methods:** `new(search_space)`, `select(history, guidance, search_space)`, `select_mutations(history)`, `explore_one(history)`, `fixed_mutation_specs()`, `sample_mutation_params(id)`, `pseudo_random(n)`, `select_full(history, guidance)`

**`VariantStats`** (private): `{ count: u32, total_evasion_score: f64 }` — tracks per-mutation performance.

**Free function:** `select_modules(search_space)` — Varies module categories.

#### `FuzzerSelector`

Genetic algorithm: tournament selection, crossover, parameter perturbation, structural mutation.

**`FuzzerConfig`**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `population_size` | `usize` | 10 | Population size |
| `elitism` | `usize` | 2 | Top N carried forward unchanged |
| `param_mutation_rate` | `f64` | 0.3 | Probability of parameter perturbation |
| `structural_mutation_rate` | `f64` | 0.2 | Probability of adding/removing a mutation |
| `min_pool_mutations` | `usize` | 1 | Minimum explored mutations per recipe |
| `max_pool_mutations` | `usize` | 5 | Maximum explored mutations per recipe |
| `vary_fixed_params` | `bool` | true | Whether to perturb fixed mutation params |

**Methods:** `new(config, search_space)`, `select(history, guidance, search_space)`, `tournament_select(pool, rng)`, `crossover(a, b, rng)`, `evolve_recipe(parent, rng)`, `random_recipe(rng)`

**`Recipe`** (private): `{ mutations: Vec<MutationSpec>, fitness: Option<f64>, generation: u32, rationale: String }`

#### `TokenSelector`

Token-biased epsilon-greedy. Falls back to `CoverageSelector` when no guidance available.

Scoring: `base_score + token_bias(±0.5/+0.3) + novelty(+0.4)`, ε=0.3.

**Methods:** `new(search_space)`, `select(history, guidance, search_space)`, `token_guided_mutations(guidance, history)`, `token_guided_modules(guidance)`, `sample_mutation_params(id)`, `make_rng()`

#### `RandomSelector`

Uniform random. Ignores history and guidance. Used as evaluation baseline.

**Methods:** `new(search_space)`, `select(history, guidance, search_space)`, `random_pool_mutations(rng)`, `random_modules(rng)`, `fixed_mutation_specs()`, `sample_mutation_params(id)`

### 6.3 Token Extraction & Scoring

#### `extractor.rs`

| Function | Description |
|----------|-------------|
| `extract_and_score(es, summary, guidance_tx)` | Top-level: extract → index → score → build guidance → send via channel ⑨ |
| `extract_round_tokens(summary)` | In-memory extraction: `module:*` + `mutation:*` tokens from round modules/mutations |
| `extract_telemetry_tokens(es, job_id, round_id)` | ES query: `api:*`, `api_arg:*`, `seq2:*`, `image:*`, `etw:*`, `etw_event:*` tokens |
| `extract_checkpoint_tokens(es, job_id, round_id)` | ES query: `checkpoint:*` tokens |
| `extract_tokens_from_docs(docs)` | Parses telemetry documents into token strings |
| `format_mutation_token(spec)` | Formats `MutationSpec` into token string (e.g., `"mutation:ast.string_xor:key=42"`) |

**9 token categories:**

| Category | Source | Example |
|----------|--------|---------|
| `module` | In-memory (modules) | `module:carrier=alloc_rw_rx` |
| `mutation` | In-memory (mutations) | `mutation:ast.string_xor:key=42` |
| `api` | ES telemetry (RedEDR) | `api:NtAllocateVirtualMemory` |
| `api_arg` | ES telemetry (RedEDR) | `api_arg:NtProtectVirtualMemory:protect=R-X` |
| `seq2` | ES telemetry (RedEDR) | `seq2:NtAllocateVirtualMemory→NtProtectVirtualMemory` |
| `image` | ES telemetry | `image:ntdll.dll` |
| `etw` | ES telemetry | `etw:Microsoft-Windows-Kernel-Process/6` |
| `etw_event` | ES telemetry | `etw_event:ProcessStopStop` |
| `checkpoint` | ES checkpoints | `checkpoint:antiemulation_passed` |

#### `scorer.rs`

**`TokenScore`**

| Field | Type | Description |
|-------|------|-------------|
| `token` | `String` | Token string |
| `lift` | `f64` | `P(detected\|T) / P(detected)` |
| `confidence` | `f64` | `min(1.0, n_total / 5.0)` |
| `importance` | `f64` | `lift × confidence` |
| `n_detected` | `u32` | Rounds where token present AND detected |
| `n_total` | `u32` | Rounds where token present |

**Functions:**
- `compute_token_scores(token_sets)` — Computes lift/confidence/importance for all tokens
- `build_guidance(scores, lift_threshold=1.5, min_confidence=0.3)` — Splits into avoid (lift > 1.5) / seek (lift < 0.667) capped at 50 each

### 6.4 Mutation Parameter Space (`param_space.rs`)

#### `MutationParamSpace`

Defines the parameter space for a single mutation.

| Field | Type | Description |
|-------|------|-------------|
| `mutation_id` | `String` | Mutation identifier |
| `params` | `Vec<ParamDef>` | Parameter definitions |

**Methods:**
- `sample_params(rng)` — Random sample from parameter space
- `perturb_params(current, rng, rate)` — Perturbs each param with given probability
- `compare_params(a, b)` — Computes normalized distance between two param sets

#### `enum ParamDef`

| Variant | Description |
|---------|-------------|
| `Categorical { name, options, default }` | One of N string options |
| `IntRange { name, min, max, default }` | Integer in [min, max] |
| `FloatRange { name, min, max, default }` | Float in [min, max] |

**Methods:** `name()`, `default_value()`, `sample(rng)`, `perturb(current, rng)`, `distance(a, b)`

#### `SeededRng`

Deterministic PRNG (xorshift64) for reproducible selection.

| Field | Type | Description |
|-------|------|-------------|
| `state` | `u64` | RNG state |

**Methods:** `new(seed)`, `from_raw(state)`, `next_u64()`, `next_usize(n)`, `next_f64()`, `coin(p)`

**Free functions:**
- `default_registry()` — Returns all 22 `MutationParamSpace` definitions
- `find_param_space(id)` — Looks up param space by mutation ID

**Mutation catalog (22 mutations):**

| Layer | Count | Mutations |
|-------|------:|-----------|
| AST (explored) | 10 | `ast.decon_rounds`, `ast.fill_pattern`, `ast.exec_decoy`, `ast.timing_pattern`, `ast.protection_transition`, `ast.const_obfuscation`, `ast.string_xor`, `ast.benign_syscall_insert`, `ast.benign_preamble`, `ast.api_sequence_obfuscation` |
| LLVM IR (fixed) | 1+2 | `llvm.opaque_predicate` (fixed) + `llvm.instruction_substitution`, `llvm.bogus_control_flow` |
| Binary (fixed) | 9 | `binary.rich_header`, `binary.import_pad`, `binary.resource_inject`, `binary.section_rename`, `binary.entropy_normalize`, `binary.string_inject`, `binary.size_pad`, `binary.debug_dir`, `binary.timestamp` |

Default split: 10 fixed (1 LLVM + 9 Binary) always applied, 10 AST explored (1 per round, varied by selector).

### 6.5 Source Resolution & Coverage (`source_resolver.rs`)

#### `SourceMap`

Maps instrumented line numbers back to the pre-instrumentation assembled source.

| Field | Type | Description |
|-------|------|-------------|
| `lines` | `Vec<String>` | Source lines |
| `func_ranges` | `Vec<FuncRange>` | Detected function boundaries |

**Methods:**
- `new(source)` — Parses source, detects function boundaries
- `resolve(line_number)` → `ResolvedLine` — Maps line to code + function
- `resolve_many(line_numbers)` → `Vec<ResolvedLine>`
- `compute_coverage(executed_lines)` → `CoverageResult`
- `line_count()` — Total lines

#### `CoverageResult`

| Field | Type | Description |
|-------|------|-------------|
| `total_lines` | `usize` | Total source lines |
| `total_executable` | `usize` | Executable lines (non-blank, non-comment) |
| `executed_lines` | `usize` | Lines hit during execution |
| `coverage_percent` | `f64` | `executed / executable × 100` |
| `cutoff_line` | `Option<usize>` | First non-executed line (truncation point) |
| `cutoff_func` | `Option<String>` | Function containing cutoff |
| `functions` | `Vec<FunctionCoverage>` | Per-function coverage breakdown |

#### `FunctionCoverage`

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Function name |
| `start_line` / `end_line` | `usize` | Line range |
| `total_lines` / `executed_lines` | `usize` | Coverage counts |
| `percent` | `f64` | Coverage percentage |

#### `ResolvedLine`

| Field | Type | Description |
|-------|------|-------------|
| `line` | `usize` | Line number |
| `code` | `String` | Source code at that line |
| `func` | `Option<String>` | Enclosing function name |

### 6.6 Token Diff (`token_diff.rs`)

#### `TokenSetComparison`

Result of comparing two rounds' token sets.

| Field | Type | Description |
|-------|------|-------------|
| `only_in_a` / `only_in_b` | `Vec<String>` | Tokens unique to each set |
| `common` | `Vec<String>` | Shared tokens |
| `mutation_comparisons` | `Vec<MutationTokenComparison>` | Per-mutation diff |
| `jaccard_distance` | `f64` | 1 - |A∩B| / |A∪B| |
| `count_a` / `count_b` | `usize` | Set sizes |

#### `MutationTokenComparison`

| Field | Type | Description |
|-------|------|-------------|
| `mutation_id` | `String` | Mutation identifier |
| `presence` | `String` | `"both"`, `"only_a"`, `"only_b"` |
| `token_a` / `token_b` | `String` | Full token strings |
| `param_comparison` | `Option<MutationComparison>` | Parameter-level diff |
| `overall_distance` | `f64` | Normalized distance |

#### `MutationComparison`

| Field | Type | Description |
|-------|------|-------------|
| `mutation_id` | `String` | Mutation identifier |
| `param_distances` | `Vec<NamedParamDistance>` | Per-parameter distances |
| `overall_distance` | `f64` | Mean of param distances |

---

## 7. Module: `vm/`

**Purpose:** Bridge the logical dispatch system to real Windows sandboxes.
Manage connections, state, artifact transport, and auto-recovery.

### 7.1 Structs

#### `TargetManager`

Single struct backing all VM operations. Uses `DashMap` for lock-free concurrent access.

| Field | Type | Description |
|-------|------|-------------|
| `targets` | `DashMap<TargetId, Target>` | All registered VMs |
| `events_tx` | `mpsc::Sender<TargetEvent>` | Channel ③ → Orchestrator |
| `rpc_timeout` | `Duration` | Timeout for unary gRPC calls (10s) |
| `run_pool` | `Arc<RunPool>` | For spawning VMExecutors |

**Methods:**

| Method | Description |
|--------|-------------|
| `new(events_tx, run_pool)` | Constructor |
| `register(config)` | Registers a new target in `Offline` state |
| `register_with_metadata(config, metadata)` | Registers with additional metadata |
| `discover_and_register_targets(path)` | Loads `automation/generated/*.toml` files |
| `establish_stream(target_id)` | Opens bidi stream, spawns StreamHandler + Heartbeat + deferred VMExecutor |
| `establish_all_streams()` | Calls `establish_stream` for all enabled targets |
| `spawn_reconnect_loop(interval)` | Background task: reconnects `Offline + enabled` targets |
| `reserve(target_id)` | `Available → Busy`, sets `current_job` |
| `release(target_id)` | `Busy → Available`, clears `current_job` |
| `mark_connected(target_id)` | `Offline → Available` |
| `mark_offline(target_id)` | `Any → Offline` (warns if `Busy`) |
| `get(target_id)` | Returns `Target` clone |
| `get_available()` | Lists `Available` targets |
| `get_available_by_os_and_capabilities(os, caps)` | Filtered availability query |
| `list_all()` / `list_ids()` | Lists all targets |
| `send_command(target_id, msg)` | Sends `ControllerMessage` via `stream_tx` |
| `send_artifact(target_id, path, sha256)` | Uploads artifact via gRPC chunks |
| `broadcast(msg)` | Sends to all connected targets |
| `get_worker_info(target_id)` | Queries metadata via unary gRPC |
| `query_all_info()` | Queries all targets' metadata |
| `update_health(target_id, status)` | Updates `last_seen` |
| `count()` | Total registered targets |
| `disconnect_one(target_id)` / `disconnect_all()` | Disconnects targets |
| `run_pool()` | Returns `Arc<RunPool>` |

**Private methods:**
- `stream_handler(target_id, incoming, result_tx, events_tx)` — Reads `WorkerMessage`s, routes to appropriate channels
- `spawn_heartbeat(target_id, stream_tx)` — Sends heartbeat every 30s
- `get_channel(target_id)` — Gets or creates gRPC channel

#### `Target`

Represents a single VM target.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `TargetId` | Target identifier |
| `address` | `String` | IP:port (e.g., `"10.200.200.10:50052"`) |
| `os_version` | `String` | OS identifier (e.g., `"windows"`) |
| `capabilities` | `Vec<String>` | VM capabilities (e.g., `["defender", "rededr"]`) |
| `metadata` | `HashMap<String, String>` | Additional key-value metadata |
| `tools` | `HashMap<String, String>` | Installed tool versions |
| `status` | `TargetStatus` | `Available` / `Busy` / `Offline` |
| `enabled` | `bool` | Whether auto-reconnect should attempt this target |
| `registration_type` | `RegistrationType` | How this target was registered |
| `current_job` | `Option<JobId>` | Currently assigned job (when `Busy`) |
| `last_seen` | `SystemTime` | Last heartbeat/activity |
| `connected_at` | `Option<SystemTime>` | Stream establishment time |
| `channel` | `Option<Channel>` | Cached gRPC transport channel |
| `stream_tx` | `Option<Sender<ControllerMessage>>` | Channel ⑥ — commands to VM |

**State machine:**
```
Offline ──mark_connected()──► Available ──reserve()──► Busy ──release()──► Available
   ▲                              │                     │                     │
   └──────────────────────────────┴─────────────────────┴─────────────────────┘
                         (disconnect, error, stream closed)
```

#### `TargetConfig`

Registration configuration (from TOML discovery or API).

| Field | Type | Description |
|-------|------|-------------|
| `id` | `TargetId` | Target identifier |
| `address` | `String` | IP:port |
| `enabled` | `bool` | Auto-reconnect enabled |

#### `enum TargetStatus`

`Available`, `Busy`, `Offline`

#### `enum TargetEvent`

Sent via channel ③ to Orchestrator.

| Variant | Description |
|---------|-------------|
| `Message { target_id, message }` | Incoming `WorkerMessage` (telemetry, status, etc.) |
| `Connected { target_id }` | Stream established |
| `Disconnected { target_id, reason }` | Stream closed |

#### `enum RegistrationType`

`Manual`, `Auto`, `Discovered`

#### `TargetArtifactSender`

Implements `ArtifactSender` trait for VMExecutor.

| Field | Type | Description |
|-------|------|-------------|
| `manager` | `Arc<TargetManager>` | For `send_artifact()` delegation |

#### `TargetInfo` (private)

TOML deserialization helper for target discovery.

| Field | Type | Description |
|-------|------|-------------|
| `worker_id` | `String` | Worker identifier |
| `ip_address` | `String` | IP address |
| `listen_port` | `u16` | gRPC port |

---

## 8. Module: `automutate/` (Generated)

Protobuf-generated code from 3 proto files via `tonic-prost-build`:

| Proto | Package | Generated Module | Content |
|-------|---------|-----------------|---------|
| `common.proto` | `automutate.common` | `automutate::common` | `ControllerMessage`, `WorkerMessage`, `TelemetryData`, etc. |
| `controller.proto` | `automutate.controller` | `automutate::controller` | `SchedulerService` server/client stubs, request/response types |
| `worker.proto` | `automutate.worker` | `automutate::worker` | Worker service stubs |

Both server and client stubs generated. File descriptor set emitted for gRPC reflection.

---

## 9. Constants Reference

| Constant | Value | Location | Description |
|----------|-------|----------|-------------|
| `MAX_IN_FLIGHT_ROUNDS` | 5 | `dispatch/job_worker.rs` | Max concurrent rounds per job |
| `MAX_PENDING_RUNS` | 9 | `dispatch/job_worker.rs` | Max runs in pool per job (3 rounds × 3 runs) |
| `DRYRUN_GRACE_PERIOD_SECS` | 5 | `dispatch/job_worker.rs` | Seconds to wait for late dryrun result |
| Heartbeat interval | 30s | `vm/manager.rs` | Keepalive for bidi streams |
| Registration timeout | 15s | `vm/manager.rs` | Wait for VM Registration before spawning VMExecutor |
| RPC timeout | 10s | `vm/manager.rs` | Unary gRPC call timeout |
| `stream_tx` capacity | 128 | `vm/manager.rs` | Outgoing command channel buffer |
| `result_tx` capacity | 128 | `vm/manager.rs` | VM result channel buffer |
| `events_tx` capacity | 4096 | `main.rs` | VM events channel buffer |
| `job_tx` capacity | 128 | `main.rs` | Job submission channel buffer |
| `job_control_tx` capacity | 64 | `main.rs` | Job control channel buffer |
| `result_tx` (per-job) | 64 | `dispatch/job_worker.rs` | Per-job result channel buffer |
| Epsilon (ε) | 0.3 | `triage/coverage_selector.rs` | Exploration rate for epsilon-greedy |
| Lift threshold | 1.5 | `triage/scorer.rs` | Avoid token: lift > 1.5 |
| Min confidence | 0.3 | `triage/scorer.rs` | Minimum observations for token guidance |
| Max guidance tokens | 50 | `triage/scorer.rs` | Cap on avoid/seek token lists |
