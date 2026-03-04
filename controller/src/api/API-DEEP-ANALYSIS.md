# API Module Deep Analysis

## Overview

The `controller/src/api/` module is the **gRPC ingress layer** of the AutoMutate++ controller.
It implements the `Controller` service trait (generated from `controller.proto`) using **tonic** and serves as the single entry-point for all external interactions — the UI, CLI tooling, and worker agents communicate exclusively through these RPCs.

The module follows a strict **thin-handler** pattern: each RPC validates its request, delegates to either ElasticSearch storage or internal dispatch channels, maps results into proto responses, and returns. No business logic (round scheduling, mutation selection, build orchestration) lives here.

```
External clients (UI, CLI, Worker Agents)
            |
            v  (gRPC / tonic)
    +-------------------+
    | SchedulerService  |   <-- api/mod.rs
    +-------------------+
    | storage (EsStorage)
    | job_tx       --> Orchestrator (dispatch layer)
    | job_control_tx --> Orchestrator stop/control
    | targets      --> TargetManager (vm layer)
    | run_pool     --> RunPool (dispatch layer)
    +-------------------+
            |
            +----> api/job.rs       (job lifecycle)
            +----> api/artifact.rs  (build + deploy)
            +----> api/worker.rs    (worker mgmt + telemetry + monitoring + admin)
            +----> api/utility.rs   (ping, triage, query)
            +----> api/extract.rs   (JSON field helpers)
```

---

## File Inventory

| File | Lines | Role |
|------|------:|------|
| `mod.rs` | 242 | `SchedulerService` definition + `Controller` trait dispatch |
| `job.rs` | 953 | Job lifecycle: schedule, status, progress, stop, round detail, run comparison, token comparison, trace lines, status reports |
| `artifact.rs` | 287 | Build artifact (cross-compile), deploy artifact (stream to worker) |
| `worker.rs` | 492 | Worker listing, telemetry ingestion, metadata, pool metrics, orchestrator status, admin (ping/disconnect) |
| `utility.rs` | 121 | Ping, triage submission (legacy), query results |
| `extract.rs` | 51 | Safe JSON field extraction helpers for ES document mapping |

**Total: ~2,146 lines**

---

## 1. `mod.rs` — Service Entry Point (242 lines)

### SchedulerService

The central gRPC service struct. Implements `Clone` (all fields are `Arc` or channel senders):

```rust
pub struct SchedulerService {
    pub storage:        Arc<EsStorage>,            // ElasticSearch client
    pub job_tx:         mpsc::Sender<JobSession>,  // submit jobs to Orchestrator
    pub job_control_tx: mpsc::Sender<JobControlCommand>, // stop/control jobs
    pub targets:        Arc<TargetManager>,         // VM registry
    pub run_pool:       Arc<RunPool>,               // shared run queue
}
```

### Controller Trait Implementation

The `#[tonic::async_trait] impl Controller for SchedulerService` block maps **25 RPCs** to handler functions across the 4 handler modules. This is pure dispatch — every method is a one-liner delegation:

```rust
async fn schedule_job(&self, req) -> ... { job::schedule_job(self, req).await }
async fn build_artifact(&self, req) -> ... { artifact::build_artifact(self, req).await }
// etc.
```

### RPC Routing Table

| Category | RPC | Handler Module |
|----------|-----|---------------|
| **Utility** | `Ping` | `utility` |
| | `SubmitTriage` | `utility` |
| | `QueryResults` | `utility` |
| **Job Lifecycle** | `ScheduleJob` | `job` |
| | `GetJobStatus` | `job` |
| | `GetJobProgress` | `job` |
| | `StopJob` | `job` |
| | `ReportStatus` | `job` |
| **Round & Analysis** | `GetRound` | `job` |
| | `CompareRuns` | `job` |
| | `CompareTokens` | `job` |
| | `GetTraceLines` | `job` |
| **Artifact** | `BuildArtifact` | `artifact` |
| | `DeployArtifact` | `artifact` |
| **Worker / Telemetry** | `ListWorkers` | `worker` |
| | `StreamTelemetry` | `worker` |
| | `GetWorker` | `worker` |
| | `GetAvailableWorkers` | `worker` |
| | `GetWorkerMetadata` | `worker` |
| **Monitoring** | `GetPoolMetrics` | `worker` |
| | `GetOrchestratorStatus` | `worker` |
| **Admin** | `PingWorker` | `worker` |
| | `DisconnectWorker` | `worker` |
| | `DisconnectAllWorkers` | `worker` |

### Design Rationale

- **Single shared struct** avoids per-handler dependency injection; each handler receives `&SchedulerService`.
- **Channel-based decoupling** — `job_tx` and `job_control_tx` are fire-and-forget `mpsc::Sender`s. The API never blocks on Orchestrator completion.
- **Read-only access to dispatch internals** — `run_pool` and `targets` are shared via `Arc` for metrics/listing queries, but mutation scheduling flows only through channels.

---

## 2. `job.rs` — Job Lifecycle Handlers (953 lines)

The largest handler module. Contains 10 public RPC handlers and 5 private helper functions.

### 2.1 `schedule_job` (lines 29–161)

**Flow:**
1. **Validate** — `source` (payload .bin path) must be non-empty; `max_rounds` defaults to 10.
2. **Generate job ID** — `job-YYYYMMDD-HHMMSS-{uuid8}` (e.g. `job-20260304-143022-a1b2c3d4`).
3. **Build configuration** — Converts proto `ModuleSelection` to internal `ModuleSelectionSpec` (with defaults for empty fields). Constructs `ModularBuildSpec { modules, payload_path, encoding }`.
4. **Configure search space** — Maps request fields onto `JobSession.search_space`:
   - `selector_type` → `SelectorType::from_str_or_default()` (fuzzer/coverage/token/random)
   - `variation_strategy` → `VariationStrategy::from_str_or_default()`
   - `mutation_pool`, `mutation_targets`, `fixed_mutations`, `variable_categories`
5. **Set runtime options** — `stop_on_evasion`, `trace_mode`, `sc_checkpoint_count`, `cache_payload`, `msvc_compat`, `msvc_vcvarsall`.
6. **Index to ES** — `storage.index_job(&job)` (non-fatal on failure).
7. **Submit** — `job_tx.send(job)` sends the session to the Orchestrator. Returns `JobResponse { job_id, accepted, message }`.

**Key detail:** The API never waits for job execution. Submission is fire-and-forget via the MPSC channel.

### 2.2 `get_job_status` (lines 164–198)

Queries the `jobs-*` ES index by job_id. Returns `status`, `progress_percent` (computed as `current_round/max_rounds * 100`), and `current_phase` string. Returns `"not_found"` if the document doesn't exist.

### 2.3 `get_job_progress` (lines 201–239)

Like `get_job_status` but richer — uses `tokio::join!` to query job doc + round docs in parallel. Returns the full list of `RoundSummaryProto` entries alongside progress metadata.

### 2.4 `stop_job` (lines 242–277)

Sends `JobControlCommand::Stop { job_id }` through `job_control_tx`. Also updates the ES job doc's status to `"stopping"`. Does not guarantee immediate termination — the Orchestrator handles cancellation tokens.

### 2.5 `get_round` (lines 280–394)

The most complex single RPC handler. Reconstructs full round detail:

1. Queries the **round document** from ES.
2. Parses `mutation_recipe` (array of `{id, params}`) via `parse_mutation_recipe()`.
3. Fetches **both run documents** (`baseline_run_id`, `instrumented_run_id`) via `query_runs_by_ids`.
4. **Applies dryrun correction** via `apply_round_correction()` — overrides per-run `detected` flags using the round's stored `differential_category`.
5. Builds optional `BehaviorComparisonProto` if `behavior_match` exists.
6. Parses `function_coverage` array (per-function coverage stats).
7. Parses `modules` object (which module variants were used this round).
8. Assembles the full `RoundProto` response.

### 2.6 `compare_runs` (lines 397–498)

Compares baseline vs. instrumented run outcomes:

1. Fetches both run docs.
2. Looks up the **round document** (extra ES query) to get the authoritative `differential_category`.
3. Applies dryrun correction.
4. Computes differences list and confidence score.
5. Returns `BehaviorComparisonProto` with `differential_category` (one of: `real_detection`, `instrumentation_artifact`, `evasion`, `flaky`, `mutation_failed`, `payload_failed`, `static_detection`).

### 2.7 `compare_tokens` (lines 501–632)

Cross-round token-set comparison:

1. Resolves `run_id → (job_id, round_id)` for both runs.
2. Queries `token_set` documents for both rounds in parallel.
3. Extracts token arrays and computes `token_diff::compare_token_sets()` using the param-space `default_registry`.
4. Returns `TokenSetComparisonProto` with:
   - `only_in_a`, `only_in_b`, `common` token lists
   - Per-mutation `MutationComparisonProto` with param-level distances
   - `jaccard_distance` for overall set similarity

### 2.8 `report_status` (lines 635–676)

Worker-to-controller status reporting. Handles events like `success`, `error`, `timeout`, `stuck`, `crashed`:

- Logs a structured status line.
- Updates worker health via `targets.update_health()`.
- For terminal events (`success`/`error`/`timeout`), indexes to ES with a 10-second timeout guard.

### 2.9 `get_trace_lines` (lines 679–772)

Returns the instrumented execution trace (line-by-line path):

1. Queries trace events from ES.
2. **Resolves source code** by chaining: `run_id → run doc → (job_id, round_id) → round doc → assembled_source → SourceMap`.
3. For each trace event, resolves `payload_line` to actual source code text and function name via `SourceMap::resolve()`.

### 2.10 Private Helpers

| Function | Purpose |
|----------|---------|
| `round_doc_to_proto()` | Maps ES round `_source` → `RoundSummaryProto` |
| `run_doc_to_proto()` | Maps ES run `_source` → `RunResultProto` (with backward-compat for `detection_verdict` vs `detection_outcome`) |
| `apply_round_correction()` | Overrides per-run `detected` flags based on round-level `differential_category` when dryrun is present |
| `parse_mutation_recipe()` | Parses `mutation_recipe` (with params) or falls back to `mutations` (string array) |
| `build_behavior_comparison()` | Constructs `BehaviorComparisonProto` from round/run data |

### Dryrun Correction Logic

`apply_round_correction()` is critical for the two-run differential protocol's correctness:

```
has_dryrun=true + category=mutation_failed|payload_failed → both detected=false
has_dryrun=true + category=real_detection|flaky           → baseline detected=true
has_dryrun=true + category=evasion|instrumentation_artifact → baseline detected=false
```

`raw_detected` preserves the original per-run value for debugging. `detected` becomes the corrected value.

---

## 3. `artifact.rs` — Build & Deploy (287 lines)

### 3.1 `build_artifact` (lines 6–154)

On-demand artifact compilation:

1. **Validates** — `modular_build` field is required (legacy `SourceFile` path removed).
2. **Converts proto to internal types:**
   - `ModuleSelection` with defaults: carrier=`change_rw_rx`, decoder=`xor`, antiemulation=`none`, guardrail=`none`, virtualprotect=`standard`, decoy=`none`, deconditioner=`none`.
   - `EncodingType` from string (default `Xor`).
   - Proto `Mutation` messages → `build::mutator::MutationSpec { id, params }`.
3. **Invokes build crate** — `ArtifactBuilder::new(default_config).build(BuildInput::ModularTemplate { ... })`.
4. **Indexes metadata** — Stores `artifact_id`, `size_bytes`, `mutations_applied`, `trace_mode`, timestamps to ES (non-fatal on failure).
5. Returns `BuildResponse` with artifact_id (SHA256), size, storage path.

### 3.2 `deploy_artifact` (lines 156–287)

Streams a built artifact to a remote worker VM:

1. **Reads artifact** from disk at `{output_dir}/{artifact_id}.exe`.
2. **Verifies SHA256** — the artifact_id IS the SHA256 hash; mismatch is a hard error.
3. **Connects** to the worker's gRPC endpoint (`WorkerAgentClient::connect()`).
4. **Chunks** artifact into **4 MB** pieces via `chunk_artifact()`.
5. **Streams** chunks via the `send_artifact` client-streaming RPC.
6. Returns deployment confirmation with `chunks_sent` and `worker_storage_path`.

### Design Notes

- Build is **synchronous within the RPC** (the `ArtifactBuilder` is invoked inline, not queued). This is acceptable because builds take seconds, not minutes, and the caller expects to receive the `artifact_id` in the response.
- Deploy creates a **fresh gRPC connection** to the worker. In the automated dispatch path (`job_worker.rs`), artifacts are uploaded through the existing bidirectional stream instead.

---

## 4. `worker.rs` — Worker Management (492 lines)

### 4.1 Helper: `target_to_worker_info()` (lines 30–57)

Converts the internal `Target` struct to proto `WorkerInfo`:
- `last_ping_seconds_ago` computed from `last_seen.elapsed()`
- `tools` mapped via `tool_versions_from_map()` (rededr, defender, etw, llvm)
- `registration_type` → `"dynamic"` or `"static"`

### 4.2 `list_workers` (lines 60–70)

Lists all registered targets (any status). Returns `Vec<WorkerInfo>`.

### 4.3 `stream_telemetry` (lines 73–158)

**Client-streaming RPC** — receives a stream of `TelemetryData` from workers:

- **Timeout guard**: 30 seconds max collection window.
- **Batch limit**: 10,000 events max per stream.
- **ES indexing**: Bulk indexes the batch with a 10-second timeout.
- Tracks `events_count` for the acknowledgment.

### 4.4 `get_worker` (lines 161–178)

Lookup a single worker by ID from `TargetManager`. Returns `found: bool`.

### 4.5 `get_available_workers` (lines 181–221)

Filters workers by availability, OS, and capabilities:
- Delegates to `TargetManager::get_available_by_os_and_capabilities()`.
- If `target_os` is specified, filters to that OS; otherwise returns all.
- Returns `total_available` count.

### 4.6 `get_worker_metadata` (lines 224–288)

Enhanced worker information with health assessment:
- **Health threshold**: 120 seconds. A worker is `healthy` if `last_seen < 120s` AND `status != Offline`.
- Returns `connected_at` as unix timestamp.
- If `worker_id` is empty, returns metadata for all workers.

### 4.7 `get_pool_metrics` (lines 294–327)

Aggregates RunPool statistics:
- `total_runs_dispatched`, `total_runs_completed`, `total_rounds_completed`, `total_jobs_completed`
- `current_queue_size` from `run_pool.pool_size()`
- `worker_count` = count of Available + Busy VMs
- Single pool entry named `"shared-run-pool"`.

### 4.8 `get_orchestrator_status` (lines 335–380)

System-wide status snapshot:
- VM state counts: `total_workers`, `available_workers`, `busy_workers`
- Active jobs from `run_pool.list_jobs()` with per-job round progress
- Single pool ID: `"shared-run-pool"`
- `pending_jobs` = current run queue depth

### 4.9 Admin Commands

#### `ping_worker` (lines 388–425)

Sends a `HealthCheckRequest` through the bidirectional control stream:
- Constructs `ControllerMessage { payload: HealthCheck { request_id } }`.
- On success: updates health, reconciles Offline → Connected if stream is alive.
- Tests stream liveness without requiring a full run.

#### `disconnect_worker` (lines 428–457)

Disconnects a single worker via `TargetManager::disconnect_one()`. Accepts optional reason string (defaults to `"admin_disconnect"`).

#### `disconnect_all_workers` (lines 460–492)

Mass disconnect via `TargetManager::disconnect_all()`. Counts pre-disconnect active workers and reports `disconnected_count`. Supports `reconnect_allowed` flag.

---

## 5. `utility.rs` — General-Purpose Endpoints (121 lines)

### 5.1 `ping` (lines 9–23)

Simple health check. Returns `"pong: {message}"` with server timestamp and `"controller"` identifier.

### 5.2 `submit_triage` (lines 32–49)

**Legacy backwards-compatibility endpoint.** The internal triage pipeline (in `triage::extractor::extract_and_score()`) handles real token extraction and scoring during `JobWorker::finalize_round()`. This endpoint:
- Generates a `triage-{unix_secs}` ID.
- Returns success without performing any triage work.
- Kept so older clients don't break.

### 5.3 `query_results` (lines 55–120)

Searches the `runs-*` ES index:
- Filters by `job_ids` (array) and `date_from`/`date_to` range.
- Maps run documents to `AnalysisResult` proto with:
  - `job_id`, `artifact_hash`, `detected`, `detection_rate` (actually `detection_verdict`)
  - `telemetry_summary` HashMap with `run_id`, `round_id`, `elapsed_ms`, `vm_id`, `exit_code`, `run_type`, `timestamp`

---

## 6. `extract.rs` — JSON Field Helpers (51 lines)

Safe accessors for `serde_json::Value` documents returned by ElasticSearch. Every function returns a sensible default on missing/wrong-typed fields:

| Function | Return Type | Default |
|----------|------------|---------|
| `str_field(v, key)` | `String` | `""` |
| `u32_field(v, key)` | `u32` | `0` |
| `bool_field(v, key)` | `bool` | `false` |
| `f64_field(v, key)` | `f64` | `0.0` |
| `i32_field(v, key)` | `i32` | `0` |
| `u64_field(v, key)` | `u64` | `0` |
| `string_array_field(v, key)` | `Vec<String>` | `[]` |
| `parse_date_to_unix_secs(v)` | `i64` | `0` |

`parse_date_to_unix_secs` handles dual formats: RFC3339 strings (new) and raw unix seconds (old), providing backward compatibility across ES document versions.

---

## Cross-Cutting Patterns

### 1. Error Handling

All handlers return `Result<Response<T>, Status>` but **never return `Err`** — failures are encoded in the response proto's fields (`accepted: false`, `found: false`, `message: "..."`, `error: "..."`). `Status::internal()` / `Status::invalid_argument()` are only used for truly unrecoverable errors (e.g., build failure in `artifact.rs`, invalid endpoint in `deploy_artifact`).

### 2. ES Query + Proto Mapping

The canonical flow is:
```
ES _source (serde_json::Value)
    |  extract.rs helpers
    v
Proto response struct
```
All ES-to-proto conversion happens in the handler layer. The storage layer returns raw `Value` documents.

### 3. Concurrency

- `tokio::join!` for parallel ES queries (e.g., `get_job_progress` queries job + rounds simultaneously).
- `tokio::time::timeout` guards around ES operations (10s for writes, 30s for streaming reads).
- No mutexes in the handler layer — all shared state is behind `Arc<DashMap>` (in `TargetManager`, `RunPool`).

### 4. Backward Compatibility

Multiple compat shims exist:
- `detection_verdict` vs `detection_outcome` in `run_doc_to_proto()`
- `mutation_recipe` (rich) vs `mutations` (string array) in `parse_mutation_recipe()`
- RFC3339 vs unix seconds in `parse_date_to_unix_secs()`
- `submit_triage` endpoint (no-op, kept for old clients)

### 5. Channel Topology

```
schedule_job ---job_tx---> Orchestrator (receives JobSession)
stop_job ----job_control_tx---> Orchestrator (receives JobControlCommand::Stop)
```

The API layer is a **producer only**. It never reads from channels — result observation is always via ES queries.

---

## Relationship to the Global AutoMutate++ Project

The `api/` module is the **control plane boundary**:

| External Action | API RPC | Internal Effect |
|----------------|---------|----------------|
| Start a mutation campaign | `ScheduleJob` | Creates `JobSession` → Orchestrator → JobWorker loop |
| Monitor progress | `GetJobProgress`, `GetJobStatus` | Reads ES job/round docs |
| Inspect a specific round | `GetRound` | Fetches round + both runs + applies dryrun correction |
| Compare differential outcomes | `CompareRuns` | Fetches run pair + round doc, computes `DifferentialCategory` |
| Compare mutation token sets | `CompareTokens` | Fetches token docs, computes Jaccard + param distances |
| View execution trace | `GetTraceLines` | Fetches trace events, resolves source code via `SourceMap` |
| Build artifact on demand | `BuildArtifact` | Invokes `build` crate (cross-compile C → PE) |
| Deploy artifact to VM | `DeployArtifact` | Streams .exe to worker via chunked gRPC |
| Manage workers | `ListWorkers`, `PingWorker`, `DisconnectWorker` | Reads/writes `TargetManager` state |
| Receive telemetry | `StreamTelemetry` | Ingests ETW/event data → ES |
| Query historical results | `QueryResults` | Searches `runs-*` ES index |

The API module does **not** contain:
- Round scheduling logic (that's `dispatch/orchestrator.rs` + `dispatch/job_worker.rs`)
- Mutation selection (that's `triage/` selectors)
- Build compilation (that's the `build` crate)
- VM management (that's `vm/manager.rs`)
- Token extraction / scoring (that's `triage/extractor.rs`)

It is purely a **translation layer** between gRPC protocol buffers and the internal Rust domain types.
