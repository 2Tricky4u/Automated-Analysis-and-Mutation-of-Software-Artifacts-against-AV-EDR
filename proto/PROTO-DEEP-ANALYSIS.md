# Proto Deep Analysis

Comprehensive analysis of the `proto/` folder — the Protobuf/gRPC contract definitions for the AutoMutate++ system.

---

## 1. Overview

The `proto/` folder contains three `.proto` files that define the **entire communication contract** between all AutoMutate++ components. Every gRPC call, every streaming message, every data structure exchanged between the controller, workers, UI, and internal services is defined here.

### Three-File Architecture

| File | Package | Role |
|------|---------|------|
| `common.proto` | `automutate.common` | Shared domain types — identity, telemetry, execution, streaming envelopes |
| `controller.proto` | `automutate.controller` | Controller API surface — job orchestration, triage, selection, monitoring |
| `worker.proto` | `automutate.worker` | Worker agent interface — execution, artifact transfer, health |

**Why three files:**
- `common.proto` contains types referenced by both sides of every RPC boundary. Putting them in a shared file avoids circular imports and ensures canonical type identity.
- `controller.proto` defines the controller's **inbound** API (what clients and workers call on the controller).
- `worker.proto` defines the worker's **inbound** API (what the controller calls on worker agents).

### Import Graph

```
controller.proto ──imports──► common.proto ◄──imports── worker.proto
```

No transitive imports. Both service files import only `common.proto`.

---

## 2. File Inventory

| File | Lines | Package | Services | RPCs | Messages | Enums |
|------|-------|---------|----------|------|----------|-------|
| `common.proto` | 269 | `automutate.common` | 0 | 0 | 30 | 1 |
| `controller.proto` | 732 | `automutate.controller` | 3 | 28 | 77 | 0 |
| `worker.proto` | 159 | `automutate.worker` | 2 | 9 | 14 | 0 |
| **Total** | **1160** | — | **5** | **37** | **121** | **1** |

---

## 3. Per-File Deep Analysis

### 3.1 `common.proto` — Shared Domain Types

**Role:** Canonical data types shared by all crates. Every identity, telemetry event, execution message, and streaming envelope lives here to ensure a single source of truth.

#### 3.1.1 Identity Types

| Message | Key Field | Format | Purpose |
|---------|-----------|--------|---------|
| `JobId` | `value: string` | `job-NNNNNN` | Unique job identifier |
| `ArtifactId` | `sha256: string` | SHA-256 hex | Content-addressable artifact identity |
| `WorkerId` | `value: string` | `worker-NN` | Worker instance identifier |
| `RunId` | `uuid: string` | UUID v4 | Unique execution run identifier |
| `Timestamp` | `unix_ms: int64` | Unix epoch ms | Canonical timestamp type |

**Design note:** Identity types are wrapped messages (not bare strings) to provide type safety at the proto level — a `JobId` cannot accidentally be passed where a `RunId` is expected.

#### 3.1.2 Mutation Specification

```protobuf
message Mutation {
  string id = 1;                   // e.g., "ast.import_reshape"
  map<string, string> params = 2;  // key-value parameters
}
```

The core mutation unit. Referenced by `RunResult.mutations`, `RoundProto.mutations`, `SelectionResponse.mutations`, `BuildRequest.mutations`, and the YAML recipe format in CLAUDE.md §3.

#### 3.1.3 Run Model

**`RunLabels`** — Detection outcome labels (CLAUDE.md §6):
| Field | Type | Values |
|-------|------|--------|
| `telemetry_seen` | bool | Whether any telemetry was generated |
| `alert_level` | string | `none`, `low`, `med`, `high` |
| `blocked` | bool | Whether EDR blocked execution |
| `detection_latency_ms` | int32 | Time from execution to detection |

**`PerfMetrics`** — Resource consumption during a run:
| Field | Type | Description |
|-------|------|-------------|
| `cpu_pct` | float | CPU utilization percentage |
| `mem_mb` | int32 | Memory usage in MB |
| `event_to_record_ms_p95` | int32 | P95 telemetry recording latency |

**`RunResult`** — The canonical run outcome (CLAUDE.md §6):
| Field | Type | Description |
|-------|------|-------------|
| `run_id` | RunId | Unique run identifier |
| `artifact_id` | ArtifactId | Which artifact was executed |
| `worker_id` | WorkerId | Which worker executed it |
| `mutations` | repeated Mutation | Applied mutations |
| `start_ts` / `end_ts` | Timestamp | Execution window |
| `status` | string | `detected` \| `not_detected` \| `noisy` \| `crash` |
| `labels` | RunLabels | Structured detection outcome |
| `perf` | PerfMetrics | Resource metrics |
| `notes` | string | Free-text notes |
| `telemetry_events` | repeated TelemetryData | Batch telemetry (non-streamed) |

#### 3.1.4 Telemetry Subsystem

Three typed event messages feed into the polymorphic `TelemetryData` envelope:

**`TraceEvent`** — Line-level execution trace (Lepori 2023 binary protocol):
| Field | Type | Description |
|-------|------|-------------|
| `seq` | uint32 | Execution sequence number |
| `file` | string | Source file path |
| `line` | uint32 | Line number |
| `func` | string | Function name |
| `ts_us` | uint64 | Timestamp in microseconds |
| `thread_id` | uint32 | Thread ID (0 for Base64, real TID for binary) |

**`CoverageEvent`** — AFL-style basic-block coverage:
| Field | Type | Description |
|-------|------|-------------|
| `bitmap` | bytes | 64KB AFL-style coverage bitmap |
| `bb_ids` | repeated uint32 | Executed basic block IDs |
| `hit_counts` | repeated uint32 | Hit count per BB |
| `total_bbs` | uint32 | Total BBs hit |

**`CheckpointEvent`** — WINNIE-style behavioral markers:
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Checkpoint name (e.g., `"api:VirtualAlloc"`) |
| `ts_us` | uint64 | Timestamp since process start |

**`TelemetryData`** — Polymorphic telemetry envelope:
| Field | Type | Description |
|-------|------|-------------|
| `job_id` | string | Owning job |
| `event_type` | string | `etw`, `procmon`, `network`, `trace`, `coverage`, `checkpoint` |
| `timestamp` | int64 | Event timestamp |
| `payload` | bytes | Raw binary payload |
| `metadata` | map | Key-value metadata |
| `typed_event` | oneof | `trace: TraceEvent` \| `coverage: CoverageEvent` \| `checkpoint: CheckpointEvent` |

**`TelemetryAck`** — Acknowledgement for streamed telemetry:
| Field | Type | Description |
|-------|------|-------------|
| `received` | bool | Success flag |
| `events_count` | int32 | Number of events acknowledged |

**`TelemetryBatch`** — Batched telemetry for stream transport:
| Field | Type | Description |
|-------|------|-------------|
| `job_id` | string | Owning job |
| `run_id` | string | Owning run |
| `events` | repeated TelemetryData | Batch of events |
| `is_final` | bool | True if last batch for this job |

#### 3.1.5 Run Type Enum

```protobuf
enum RunType {
  RUN_TYPE_UNSPECIFIED = 0;
  RUN_TYPE_BASELINE = 1;       // No instrumentation
  RUN_TYPE_INSTRUMENTED = 2;   // Full tracing
  RUN_TYPE_DRYRUN = 3;         // Clean-VM exit code only
}
```

Maps to the **Two-Run Differential Protocol** (CLAUDE.md §5) plus the dryrun extension. `BASELINE` = Run B (ground-truth), `INSTRUMENTED` = Run A (trace mode), `DRYRUN` = clean-VM control.

#### 3.1.6 Execution Protocol

**`SampleRequest`** — Execution command sent to workers:
| Field | Type | Description |
|-------|------|-------------|
| `job_id` | string | Parent job |
| `artifact_id` | string | Reference to deployed artifact |
| `timeout_seconds` | int32 | Max execution time |
| `enable_etw` | bool | Enable ETW collection |
| `trace_mode` | string | `off`, `api`, `lines`, `all` |
| `is_dryrun` | bool | Skip RedEDR/telemetry (exit code only) |

**`SampleResponse`** — Execution result from worker:
| Field | Type | Description |
|-------|------|-------------|
| `job_id` | string | Parent job |
| `success` | bool | Execution completed without error |
| `exit_code` | int32 | Process exit code |
| `output` | string | Captured stdout/stderr |
| `telemetry_ids` | repeated string | References to stored telemetry |
| `run_id` | string | Matches `RunSampleCommand.request_id` |
| `detected` | bool | EDR/AV detection flag |
| `error` | string | Error message if failed |
| `elapsed_ms` | double | Wall-clock execution time |
| `detection_verdict` | string | Fine-grained verdict (e.g., `"killed_pre_payload"`) |
| `last_checkpoint` | string | Last checkpoint before exit (e.g., `"Launching"`) |

#### 3.1.7 Bidirectional Stream Envelope Messages (Phase 2)

The real-time communication channel between controller and worker uses two envelope messages with `oneof` payloads:

**`ControllerMessage`** — Controller → Worker (6 variants):
| Variant | Type | Purpose |
|---------|------|---------|
| `run_sample` | RunSampleCommand | Execute artifact command |
| `artifact_chunks` | ArtifactChunkBatch | Stream artifact binary |
| `health_check` | HealthCheckRequest | Request health status |
| `heartbeat` | Heartbeat | Keep-alive ping |
| `ack` | Ack | Acknowledge worker message |
| `disconnect` | DisconnectNotice | Graceful disconnect |

**`WorkerMessage`** — Worker → Controller (6 variants):
| Variant | Type | Purpose |
|---------|------|---------|
| `registration` | WorkerRegistration | Sent once on connect |
| `status` | StatusReport | Periodic heartbeats |
| `telemetry` | TelemetryBatch | Stream telemetry events |
| `sample_response` | SampleResponse | Execution results |
| `ack` | Ack | Acknowledge controller message |
| `execution_status` | ExecutionStatusReport | Detailed execution monitoring |

**Supporting messages:**

| Message | Fields | Purpose |
|---------|--------|---------|
| `RunSampleCommand` | `request_id`, `request: SampleRequest` | Wraps SampleRequest with correlation ID |
| `ArtifactChunkBatch` | `artifact_id`, `chunks: repeated ArtifactChunk` | Batched artifact transfer |
| `HealthCheckRequest` | `request_id` | Health check with correlation ID |
| `Heartbeat` | `timestamp: int64` | Keep-alive |
| `Ack` | `request_id`, `success`, `error` | Generic acknowledgement |
| `DisconnectNotice` | `reason`, `reconnect_allowed` | Graceful teardown signal |

#### 3.1.8 Artifact Transfer

**`ArtifactChunk`** — Binary transfer unit:
| Field | Type | Description |
|-------|------|-------------|
| `artifact_id` | string | SHA256 hash |
| `data` | bytes | Binary chunk (4MB max) |
| `chunk_index` | uint32 | 0-based sequence |
| `total_chunks` | uint32 | Total chunk count |
| `sha256` | string | Expected hash for verification |

#### 3.1.9 Worker Metadata

**`ToolVersions`** — Installed tool versions:
| Field | Type | Example |
|-------|------|---------|
| `rededr_version` | string | `"1.2.3"` |
| `defender_version` | string | `"4.18.2106.1"` |
| `etw_version` | string | `"native"` |
| `llvm_version` | string | `"17.0.6"` |

**`WorkerRegistration`** — Sent by worker on startup:
| Field | Type | Description |
|-------|------|-------------|
| `worker_id` | string | e.g., `"win10-worker-01"` |
| `ip_address` | string | e.g., `"10.200.200.100"` |
| `os_version` | string | `"windows10"`, `"windows11"` |
| `capabilities` | repeated string | `["rededr", "defender", "etw", "gpu"]` |
| `metadata` | map | `{"cpu_cores": "8", "ram_gb": "32"}` |
| `tools` | ToolVersions | Installed tool versions |

#### 3.1.10 Status Reporting

**`StatusReport`** — Periodic heartbeat from worker:
| Field | Type | Description |
|-------|------|-------------|
| `worker_id` / `worker_ip` | string | Worker identity |
| `cpu_percent` / `memory_mb` | int32 | Resource usage |
| `active_jobs` | int32 | Current job count |
| `event_type` | string | `"heartbeat"`, `"job_start"`, `"job_complete"` |
| `current_job_id` | string | Active job ID |

**`ExecutionStatusReport`** — Detailed per-run monitoring:
| Field | Type | Description |
|-------|------|-------------|
| `worker_id` / `worker_ip` | string | Worker identity |
| `job_id` / `run_id` | string | Execution identity |
| `artifact_name` | string | Artifact being executed |
| `pid` | int32 | Process ID |
| `elapsed_seconds` | int32 | Time since start |
| `process_alive` | bool | Whether process is still running |
| `telemetry_events_count` | int32 | Events collected so far |
| `event_type` | string | `"started"`, `"heartbeat"`, `"stuck"`, `"approaching_timeout"`, `"terminated"` |
| `cpu_percent` / `memory_mb` | int32 | Resource metrics |
| `details` | string | Human-readable details |

---

### 3.2 `controller.proto` — Controller API Surface

**Role:** Defines the controller's gRPC API — the central orchestration point that clients (UI, CLI) and internal services call. Contains 3 services with 28 RPCs and 77 messages.

#### 3.2.1 Service: `Controller` (24 RPCs)

The main orchestration service, grouped by domain:

**Job Lifecycle (4 RPCs)**

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `ScheduleJob` | Unary | `JobRequest` → `JobResponse` | Submit new analysis job |
| `GetJobStatus` | Unary | `JobStatusRequest` → `JobStatusResponse` | Query job status |
| `GetJobProgress` | Unary | `JobProgressRequest` → `JobProgressResponse` | Get all rounds for a job |
| `StopJob` | Unary | `StopJobRequest` → `StopJobResponse` | Manually stop a running job |

**Round & Differential Analysis (4 RPCs)**

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `GetRound` | Unary | `GetRoundRequest` → `GetRoundResponse` | Get round detail (CLAUDE.md §5) |
| `CompareRuns` | Unary | `CompareRunsRequest` → `CompareRunsResponse` | Two-run differential |
| `CompareTokens` | Unary | `CompareTokensRequest` → `CompareTokensResponse` | Token set diff + Jaccard |
| `GetTraceLines` | Unary | `GetTraceLinesRequest` → `GetTraceLinesResponse` | Retrieve execution trace |

**Build & Deploy (2 RPCs)**

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `BuildArtifact` | Unary | `BuildRequest` → `BuildResponse` | Cross-compile artifact (WSL) |
| `DeployArtifact` | Unary | `DeployRequest` → `DeployResponse` | Push artifact to worker |

**Worker Management (4 RPCs)**

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `ListWorkers` | Unary | `ListWorkersRequest` → `ListWorkersResponse` | List all workers |
| `GetWorker` | Unary | `GetWorkerRequest` → `GetWorkerResponse` | Get single worker |
| `GetAvailableWorkers` | Unary | `GetAvailableWorkersRequest` → `GetAvailableWorkersResponse` | Filter by OS/capabilities |
| `GetWorkerMetadata` | Unary | `GetWorkerMetadataRequest` → `GetWorkerMetadataResponse` | Enhanced metadata |

**Monitoring (2 RPCs)**

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `GetPoolMetrics` | Unary | `GetPoolMetricsRequest` → `GetPoolMetricsResponse` | Pool-level run stats |
| `GetOrchestratorStatus` | Unary | `GetOrchestratorStatusRequest` → `GetOrchestratorStatusResponse` | System-wide status |

**Admin Commands (3 RPCs)**

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `PingWorker` | Unary | `PingWorkerRequest` → `PingWorkerResponse` | Ping specific worker via stream |
| `DisconnectWorker` | Unary | `DisconnectWorkerRequest` → `DisconnectWorkerResponse` | Disconnect single worker |
| `DisconnectAllWorkers` | Unary | `DisconnectAllWorkersRequest` → `DisconnectAllWorkersResponse` | Disconnect all workers |

**Telemetry & Status (2 RPCs)**

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `StreamTelemetry` | Client-streaming | `stream TelemetryData` → `TelemetryAck` | Workers push telemetry |
| `ReportStatus` | Unary | `StatusReport` → `StatusAck` | Workers push status |

**Utility (3 RPCs)**

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `Ping` | Unary | `PingRequest` → `PingResponse` | Connectivity test |
| `SubmitTriage` | Unary | `TriageRequest` → `TriageResponse` | Submit triage data |
| `QueryResults` | Unary | `QueryRequest` → `QueryResponse` | Query stored results |

#### 3.2.2 Service: `Selector` (2 RPCs)

Mutation selection with feedback loop (CLAUDE.md §8):

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `SelectMutation` | Unary | `SelectionRequest` → `SelectionResponse` | Get next mutation set |
| `ReportOutcome` | Unary | `OutcomeReport` → `OutcomeAck` | Feed back detection result |

#### 3.2.3 Service: `Triage` (2 RPCs)

Hypothesis generation (CLAUDE.md §7, §10):

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `AnalyzeRun` | Unary | `AnalysisRequest` → `AnalysisResponse` | Generate hypotheses for a run |
| `GetAvoidList` | Unary | `AvoidListRequest` → `AvoidListResponse` | Get features to avoid |

#### 3.2.4 Key Message Groups

**`JobRequest`** — Full job configuration (22 fields):

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Job name |
| `artifact_type` | string | `exe`, `dll`, `shellcode` |
| `source` | string | Path to payload `.bin` file |
| `max_rounds` | uint32 | Maximum mutation rounds (default: 10) |
| `stop_on_evasion` | bool | Stop when `not_detected` |
| `target_os` | string | `"win10"`, `"win11"` (empty = any) |
| `required_capabilities` | repeated string | Worker capability filter |
| `modules` | ModuleSelection | Template module selection |
| `encoding` | string | `"xor"`, `"english"`, `"subbyte"`, `"none"` |
| `trace_mode` | string | `"off"`, `"api"`, `"bb"`, `"api+bb"`, `"lines"`, `"all"` |
| `variable_categories` | repeated string | Module categories the selector may vary |
| `variation_strategy` | string | `"mutation"` (default) or `"full"` |
| `mutation_pool` | repeated string | Mutation IDs to explore (empty = full catalog) |
| `mutation_targets` | repeated string | Module names to apply mutations to |
| `fixed_mutations` | repeated string | Always applied after round 1 |
| `sc_checkpoint_count` | uint32 | INT3 shellcode checkpoints (0 = disabled) |
| `selector_type` | string | `"coverage"` (default) or `"fuzzer"` |
| `cache_payload` | bool | Cache encoded payload across rounds |
| `msvc_compat` | bool | Use clang-cl + link.exe for MSVC-native PE |
| `msvc_vcvarsall` | string | WSL path to vcvarsall.bat |

**`ModuleSelection`** — 7-slot module specification:

| Slot | Values | Purpose |
|------|--------|---------|
| `carrier` | `alloc_rw_rx`, `change_rw_rx`, `peb_walk` | Memory allocation strategy |
| `decoder` | `xor`, `english`, `subbyte` | Payload decoding method |
| `antiemulation` | `none`, `sirallocalot`, `timeraw` | Anti-emulation technique |
| `guardrail` | `none`, `env` | Execution guardrails |
| `virtualprotect` | `standard`, `undersized` | Memory protection variant |
| `decoy` | `none`, `winexec` | Decoy API calls |
| `deconditioner` | `none`, `alloc_loop` | EDR deconditioning |

**`RoundProto`** — Full round detail (CLAUDE.md §5):

| Field | Type | Description |
|-------|------|-------------|
| `round_id` / `job_id` | string | Identity |
| `round_number` | uint32 | Sequence in job |
| `mutations` | repeated Mutation | Applied mutations |
| `baseline_run` | RunResultProto | Run B (no instrumentation) |
| `instrumented_run` | RunResultProto | Run A (with tracing) |
| `dryrun_run` | RunResultProto | Clean-VM control run |
| `status` | string | Round status |
| `behavior_match` | BehaviorComparisonProto | Differential analysis result |
| `assembled_source` | string | Generated C source |
| `coverage_percent` | double | Line coverage |
| `cutoff_line` / `cutoff_func` | uint32/string | Where execution stopped |
| `function_coverage` | repeated FunctionCoverageProto | Per-function coverage |
| `modules` | ModuleSelection | Module configuration used |
| `coverage_total_lines` / `executable_lines` / `executed_lines` | uint32 | Coverage counters |
| `dry_run_exit_code` | int32 | Dryrun exit code |
| `detection_verdict` | string | Final verdict |

**`RoundSummaryProto`** — Compact round summary for job progress:

| Field | Type | Description |
|-------|------|-------------|
| `round_id` | string | Round identity |
| `round_number` | uint32 | Sequence number |
| `mutations` | repeated string | Applied mutation IDs |
| `detected` | bool | Detection outcome |
| `behavior_match` | bool | Differential match |
| `evasion_score` | double | Evasion metric |
| `differential_category` | string | `real_detection`, `instrumentation_artifact`, `flaky`, `evasion` |
| `coverage_percent` | double | Line coverage |
| `dry_run_exit_code` | int32 | Dryrun exit code |
| `detection_verdict` | string | Final verdict |

**`BehaviorComparisonProto`** — Two-run differential (CLAUDE.md §5):

| Field | Type | Description |
|-------|------|-------------|
| `outcome_match` | bool | Both runs agree |
| `baseline_detected` / `instrumented_detected` | bool | Per-run detection |
| `baseline_exit_code` / `instrumented_exit_code` | int32 | Per-run exit codes |
| `differences` | repeated string | Human-readable diffs |
| `confidence` | double | Comparison confidence |
| `differential_category` | string | Classification result |

**`TokenSetComparisonProto`** — Token diff with Jaccard distance:

| Field | Type | Description |
|-------|------|-------------|
| `only_in_a` / `only_in_b` | repeated string | Tokens unique to each run |
| `common` | repeated string | Shared tokens |
| `mutation_comparisons` | repeated MutationComparisonProto | Per-mutation param distances |
| `jaccard_distance` | double | Set distance metric |
| `count_a` / `count_b` | uint32 | Token counts |

**`MutationComparisonProto`** — Per-mutation diff:

| Field | Type | Description |
|-------|------|-------------|
| `mutation_id` | string | Mutation identifier |
| `presence` | string | `"both"`, `"only_a"`, `"only_b"` |
| `token_a` / `token_b` | string | Token representation per run |
| `params` | repeated ParamComparisonProto | Per-param distances |
| `overall_distance` | double | Aggregate distance |

**`FeedbackProto`** — Selector feedback (CLAUDE.md §8):

| Field | Type | Description |
|-------|------|-------------|
| `detected` | bool | Detection outcome |
| `avoid_features` | repeated string | High-lift tokens to avoid |
| `seek_features` | repeated string | Coverage-gain tokens to seek |
| `evasion_score` | double | Evasion metric |

**`BuildRequest`** — Artifact build specification:

| Field | Type | Description |
|-------|------|-------------|
| `template_name` | string | Legacy mode template |
| `source_file` | string | Legacy mode source |
| `compiler_flags` | repeated string | Custom flags |
| `mutations` | repeated Mutation | Mutations to apply |
| `trace_mode` | string | Instrumentation level |
| `modular_build` | ModularBuildSpec | Preferred modular build mode |

**`ModularBuildSpec`** — Modular template build:

| Field | Type | Description |
|-------|------|-------------|
| `modules` | ModuleSelection | Module configuration |
| `payload` | bytes | Raw payload bytes |
| `encoding` | string | `"xor"`, `"english"`, `"subbyte"`, `"none"` |

**Worker Info Messages:**

`WorkerInfo` — Worker status (used in ListWorkers, GetWorker, GetAvailableWorkers):

| Field | Type | Description |
|-------|------|-------------|
| `worker_id` | string | Identity |
| `address` | string | gRPC endpoint |
| `status` | string | `"available"`, `"busy"`, `"offline"` |
| `current_job_id` | string | Active job |
| `last_ping_seconds_ago` | int64 | Health staleness |
| `enabled` | bool | Enabled in config |
| `os_version` | string | OS version |
| `capabilities` | repeated string | Worker capabilities |
| `metadata` | map | Key-value metadata |
| `tools` | ToolVersions | Tool versions |
| `registration_type` | string | `"static"` (TOML) or `"dynamic"` (RPC) |

`WorkerMetadataEntry` — Enhanced metadata (used in GetWorkerMetadata):

| Field | Type | Description |
|-------|------|-------------|
| `worker_id` / `address` / `status` | string | Identity |
| `os_version` | string | OS |
| `capabilities` | repeated string | Capabilities |
| `metadata` | map | Key-value |
| `tools` | ToolVersions | Tools |
| `last_seen_seconds_ago` | int64 | Staleness |
| `healthy` | bool | Based on threshold |
| `current_job_id` | string | Active job |
| `connected_at` | int64 | Connection timestamp |

**Monitoring Messages:**

`PoolMetricsEntry`:
| Field | Type | Description |
|-------|------|-------------|
| `pool_id` | string | Pool identity |
| `total_runs_dispatched` / `completed` | uint64 | Run counts |
| `total_rounds_completed` | uint64 | Round count |
| `total_jobs_completed` | uint64 | Job count |
| `current_queue_size` | uint32 | Pending work |
| `worker_count` | uint32 | Workers in pool |
| `current_job_id` | string | Active job |

`GetOrchestratorStatusResponse`:
| Field | Type | Description |
|-------|------|-------------|
| `pending_jobs` | uint32 | Queued jobs |
| `active_pools` / `total_workers` | uint32 | Pool/worker counts |
| `available_workers` / `busy_workers` | uint32 | Worker availability |
| `pool_ids` | repeated string | Pool identifiers |
| `active_jobs` | repeated ActiveJobEntry | Currently running jobs |

**Dynamic Registration Messages** (unused in Phase 1, designed for dynamic worker management):

| Message | Purpose |
|---------|---------|
| `RegistrationAck` | Controller acknowledges worker registration |
| `WorkerDeregistration` | Worker announces shutdown |
| `DeregistrationAck` | Controller acknowledges deregistration |
| `WorkerMetadataUpdate` | Worker reports capability changes |
| `MetadataAck` | Controller acknowledges metadata update |

---

### 3.3 `worker.proto` — Worker Agent Interface

**Role:** Defines the worker-side gRPC API — what the controller calls on each Windows VM agent. Contains 2 services with 9 RPCs and 14 messages.

#### 3.3.1 Service: `WorkerAgent` (7 RPCs)

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `Ping` | Unary | `PingRequest` → `PingResponse` | Connectivity test |
| `RunSample` | Unary | `SampleRequest` → `SampleResponse` | Execute artifact (Phase 1) |
| `HealthCheck` | Unary | `HealthRequest` → `HealthResponse` | Get worker health |
| `SendArtifact` | Client-streaming | `stream ArtifactChunk` → `TransferAck` | Upload artifact binary |
| `GetWorkerInfo` | Unary | `WorkerInfoRequest` → `WorkerInfoResponse` | Query worker capabilities |
| `GetTelemetry` | Server-streaming | `TelemetryRequest` → `stream TelemetryData` | Pull telemetry from worker |
| `EstablishStream` | Bidirectional | `stream ControllerMessage` ↔ `stream WorkerMessage` | Real-time channel (Phase 2) |

**Phase progression:**
- Phase 1 RPCs (`RunSample`, `SendArtifact`, `GetWorkerInfo`, `GetTelemetry`) use simple unary/streaming patterns where the controller actively polls/pushes.
- Phase 2 RPC (`EstablishStream`) consolidates all communication into a single bidirectional stream using `ControllerMessage`/`WorkerMessage` envelopes.

#### 3.3.2 Service: `Harness` (2 RPCs)

| RPC | Type | Request → Response | Purpose |
|-----|------|--------------------|---------|
| `Execute` | Unary | `ExecuteRequest` → `ExecuteResponse` | Run artifact with full telemetry |
| `Monitor` | Server-streaming | `MonitorRequest` → `stream MonitorEvent` | Real-time execution monitoring |

#### 3.3.3 Key Messages

**`WorkerInfoResponse`** — Worker capabilities (Phase 1 pull model):
| Field | Type | Description |
|-------|------|-------------|
| `worker_id` / `ip_address` | string | Identity |
| `os_version` | string | OS version |
| `capabilities` | repeated string | Available features |
| `metadata` | map | Key-value metadata |
| `tools` | ToolVersions | Tool versions |
| `health` | HealthMetrics | Current health |
| `current_job_id` | string | Active job |

**`HealthMetrics`** — Detailed health status:
| Field | Type | Description |
|-------|------|-------------|
| `cpu_percent` | int32 | CPU usage |
| `memory_percent` | int32 | Memory usage |
| `disk_percent` | int32 | Disk usage |
| `active_jobs` | int32 | Running job count |
| `uptime_seconds` | int64 | Worker uptime |

**`TransferAck`** — Artifact upload acknowledgement:
| Field | Type | Description |
|-------|------|-------------|
| `received` | bool | Success flag |
| `chunks_received` | uint32 | Number of chunks received |
| `error` | string | Error message |
| `storage_path` | string | Where artifact was written on worker |

**`TelemetryRequest`** — Telemetry pull parameters:
| Field | Type | Description |
|-------|------|-------------|
| `job_id` | string | Filter by job |
| `since_timestamp` | int64 | Time cursor (0 = all) |
| `max_events` | int32 | Limit (0 = all) |

**`ExecuteRequest`** — Harness execution command:
| Field | Type | Description |
|-------|------|-------------|
| `job_id` | JobId | Parent job |
| `artifact_id` | string | Artifact to execute |
| `timeout_seconds` | int32 | Execution timeout |
| `telemetry_providers` | repeated string | Which providers to enable |

**`MonitorEvent`** — Harness monitoring stream:
| Field | Type | Description |
|-------|------|-------------|
| `event_type` | string | `started`, `running`, `heartbeat`, `stuck`, `crashed`, `completed` |
| `timestamp` | int64 | Event timestamp |
| `details` | string | Human-readable details |
| `status` | ExecutionStatus | Process-level metrics |

**`ExecutionStatus`** — Process health snapshot:
| Field | Type | Description |
|-------|------|-------------|
| `pid` | int32 | Process ID |
| `elapsed_seconds` | int32 | Time since start |
| `process_alive` | bool | Still running |
| `cpu_percent` / `memory_mb` | int32 | Resource usage |
| `telemetry_events_count` | int32 | Events collected |
| `last_activity` | string | Last API call or `"idle"` |

---

## 4. Architecture

### 4.1 Proto Compilation Pipeline

Three `build.rs` scripts compile the protos into Rust code via `tonic-prost-build`:

```
proto/
├── common.proto
├── controller.proto
└── worker.proto
        │
        ▼
┌─────────────────────────────────────────────────────────────────┐
│                    build.rs Compilation                          │
├────────────────────┬────────────────────┬───────────────────────┤
│ controller/build.rs│ worker/agent/      │ ui/backend/build.rs   │
│                    │ build.rs           │                       │
│ Protos: all 3      │ Protos: all 3      │ Protos: common +      │
│ Server: YES        │ Server: YES        │         controller    │
│ Client: YES        │ Client: YES        │ Server: NO            │
│ Descriptor: YES    │ Descriptor: YES    │ Client: YES           │
│                    │                    │ Descriptor: NO        │
└────────────────────┴────────────────────┴───────────────────────┘
```

| build.rs | Protos Compiled | Server | Client | Descriptor | Purpose |
|----------|----------------|--------|--------|------------|---------|
| `controller/build.rs` | common + controller + worker | Yes | Yes | Yes | Hosts Controller/Selector/Triage servers; calls WorkerAgent/Harness as client |
| `worker/agent/build.rs` | common + controller + worker | Yes | Yes | Yes | Hosts WorkerAgent/Harness servers; calls Controller as client |
| `ui/backend/build.rs` | common + controller | No | Yes | No | UI backend calls Controller/Selector/Triage as client only |

### 4.2 Communication Topology

```
                    ┌──────────────────┐
                    │     UI/CLI       │
                    │  (client only)   │
                    └────────┬─────────┘
                             │ gRPC (controller.proto)
                             ▼
                    ┌──────────────────┐
                    │   Controller     │
                    │ (server+client)  │
                    │                  │
                    │ Services:        │
                    │  • Controller    │
                    │  • Selector      │
                    │  • Triage        │
                    └────────┬─────────┘
                             │ gRPC (worker.proto)
                ┌────────────┼────────────┐
                ▼            ▼            ▼
         ┌───────────┐┌───────────┐┌───────────┐
         │ Worker 1  ││ Worker 2  ││ Worker N  │
         │(server+   ││(server+   ││(server+   │
         │ client)   ││ client)   ││ client)   │
         │           ││           ││           │
         │ Services: ││ Services: ││ Services: │
         │• Worker-  ││• Worker-  ││• Worker-  │
         │  Agent    ││  Agent    ││  Agent    │
         │• Harness  ││• Harness  ││• Harness  │
         └───────────┘└───────────┘└───────────┘
```

### 4.3 Streaming Architecture

Five RPCs use streaming patterns:

| RPC | Pattern | Direction | Purpose |
|-----|---------|-----------|---------|
| `Controller.StreamTelemetry` | Client-streaming | Worker → Controller | Push telemetry events |
| `WorkerAgent.SendArtifact` | Client-streaming | Controller → Worker | Upload artifact binary |
| `WorkerAgent.GetTelemetry` | Server-streaming | Worker → Controller | Pull telemetry events |
| `Harness.Monitor` | Server-streaming | Worker → Controller | Real-time execution monitoring |
| `WorkerAgent.EstablishStream` | Bidirectional | Controller ↔ Worker | Full-duplex real-time channel |

**Phase 1 vs Phase 2:**
- Phase 1: Individual unary/streaming RPCs (RunSample, SendArtifact, GetTelemetry, GetWorkerInfo)
- Phase 2: Single bidirectional stream (`EstablishStream`) multiplexes all communication through `ControllerMessage`/`WorkerMessage` envelopes

---

## 5. Cross-Proto Relationships

### 5.1 Shared Types Used Across Boundaries

Types defined in `common.proto` and referenced by both `controller.proto` and `worker.proto`:

| Common Type | Used in controller.proto | Used in worker.proto |
|-------------|------------------------|---------------------|
| `JobId` | SelectionRequest, AvoidListRequest, ExecuteRequest | ExecuteRequest |
| `ArtifactId` | — | — |
| `RunId` | OutcomeReport, AnalysisRequest | MonitorRequest |
| `Mutation` | SelectionResponse, BuildRequest, RoundProto | — |
| `RunResult` | OutcomeReport | ExecuteResponse |
| `RunType` | RunResultProto | — |
| `TelemetryData` | StreamTelemetry (stream) | GetTelemetry (stream), ExecuteResponse |
| `TelemetryAck` | StreamTelemetry (return) | — |
| `SampleRequest` | — | RunSample (input) |
| `SampleResponse` | — | RunSample (output) |
| `ArtifactChunk` | — | SendArtifact (stream) |
| `ToolVersions` | WorkerInfo, WorkerMetadataEntry | WorkerInfoResponse |
| `WorkerRegistration` | — | (via WorkerMessage in stream) |
| `ControllerMessage` | — | EstablishStream (input stream) |
| `WorkerMessage` | — | EstablishStream (output stream) |

### 5.2 Envelope Encapsulation

The bidirectional stream (`EstablishStream`) uses `ControllerMessage` and `WorkerMessage` as envelopes that **wrap** other common types:

```
ControllerMessage
├── RunSampleCommand ──wraps──► SampleRequest
├── ArtifactChunkBatch ──contains──► ArtifactChunk[]
├── HealthCheckRequest
├── Heartbeat
├── Ack
└── DisconnectNotice

WorkerMessage
├── WorkerRegistration
├── StatusReport
├── TelemetryBatch ──contains──► TelemetryData[]
├── SampleResponse
├── Ack
└── ExecutionStatusReport
```

This design allows a single TCP connection to carry all message types, with the `oneof` discriminator enabling type-safe multiplexing.

---

## 6. Role in the Global Project

### 6.1 Pipeline Stage Mapping

| Pipeline Stage (CLAUDE.md §1) | Proto Types Involved |
|-------------------------------|---------------------|
| 1. Schedule job | `JobRequest` → `JobResponse` |
| 2. Select mutations | `SelectionRequest` → `SelectionResponse`, `FeedbackProto` |
| 3. Build artifact | `BuildRequest` (ModularBuildSpec, ModuleSelection, Mutation) → `BuildResponse` |
| 4. Deploy to worker | `DeployRequest` / `ArtifactChunk` stream → `DeployResponse` / `TransferAck` |
| 5. Execute (baseline) | `SampleRequest` (RunType=BASELINE) → `SampleResponse` |
| 6. Execute (instrumented) | `SampleRequest` (RunType=INSTRUMENTED) → `SampleResponse` |
| 7. Execute (dryrun) | `SampleRequest` (is_dryrun=true) → `SampleResponse` |
| 8. Collect telemetry | `TelemetryData`, `TelemetryBatch`, `TraceEvent`, `CoverageEvent`, `CheckpointEvent` |
| 9. Differential analysis | `CompareRunsRequest` → `BehaviorComparisonProto` |
| 10. Token comparison | `CompareTokensRequest` → `TokenSetComparisonProto` |
| 11. Triage/hypotheses | `AnalysisRequest` → `AnalysisResponse` (Hypothesis[]) |
| 12. Feedback loop | `FeedbackProto` (avoid/seek), `OutcomeReport`, `AvoidListResponse` |
| 13. Monitor progress | `JobProgressResponse` (RoundSummaryProto[]), `StatusReport`, `ExecutionStatusReport` |

### 6.2 Service-to-Module Mapping

| Service | Crate Module | Role |
|---------|-------------|------|
| `Controller` | `controller/src/api/` | External API for UI/CLI + worker communication |
| `Selector` | `controller/src/dispatch/` | Mutation selection with token-driven feedback |
| `Triage` | `controller/src/triage/` (planned) | Hypothesis generation from telemetry |
| `WorkerAgent` | `worker/agent/src/` | Artifact execution on Windows VMs |
| `Harness` | `worker/agent/src/harness/` | Low-level process execution + monitoring |

### 6.3 Streaming vs Unary Patterns

| Pattern | When Used | Why |
|---------|-----------|-----|
| **Unary** (32 RPCs) | Job scheduling, status queries, build, deploy orchestration, admin commands | Request-response semantics, simple, reliable |
| **Client-streaming** (2 RPCs) | Telemetry push, artifact upload | Large/unbounded data flows toward receiver |
| **Server-streaming** (2 RPCs) | Telemetry pull, execution monitoring | Continuous data from source to consumer |
| **Bidirectional** (1 RPC) | Full-duplex real-time channel | Multiplexed commands + responses + telemetry on single connection |

---

## 7. Summary Statistics

| Metric | common.proto | controller.proto | worker.proto | **Total** |
|--------|-------------|-----------------|-------------|-----------|
| Messages | 30 | 77 | 14 | **121** |
| Enums | 1 | 0 | 0 | **1** |
| Services | 0 | 3 | 2 | **5** |
| RPCs | 0 | 28 | 9 | **37** |
| — Unary | 0 | 27 | 4 | **31** |
| — Client-streaming | 0 | 1 | 1 | **2** |
| — Server-streaming | 0 | 0 | 2 | **2** |
| — Bidirectional | 0 | 0 | 1 | **1** |
| `oneof` fields | 2 | 0 | 0 | **2** |
| `map` fields | 2 | 4 | 2 | **8** |
| Lines | 269 | 732 | 159 | **1160** |

### Message Count by Domain

| Domain | Messages | Key Types |
|--------|----------|-----------|
| Identity | 5 | JobId, ArtifactId, WorkerId, RunId, Timestamp |
| Mutation | 1 | Mutation |
| Run model | 3 | RunResult, RunLabels, PerfMetrics |
| Telemetry | 6 | TelemetryData, TraceEvent, CoverageEvent, CheckpointEvent, TelemetryAck, TelemetryBatch |
| Execution | 4 | SampleRequest, SampleResponse, RunSampleCommand, RunType |
| Stream envelopes | 8 | ControllerMessage, WorkerMessage, ArtifactChunkBatch, HealthCheckRequest, Heartbeat, Ack, DisconnectNotice |
| Status reporting | 2 | StatusReport, ExecutionStatusReport |
| Worker metadata | 2 | WorkerRegistration, ToolVersions |
| Artifact transfer | 1 | ArtifactChunk |
| Job lifecycle | 8 | JobRequest, JobResponse, JobStatusRequest/Response, JobProgressRequest/Response, StopJobRequest/Response |
| Build & deploy | 7 | BuildRequest, BuildResponse, ModularBuildSpec, ModuleSelection, DeployRequest, DeployResponse |
| Round & differential | 8 | RoundProto, RoundSummaryProto, RunResultProto, BehaviorComparisonProto, FunctionCoverageProto, CompareRunsRequest/Response |
| Token comparison | 5 | CompareTokensRequest/Response, TokenSetComparisonProto, MutationComparisonProto, ParamComparisonProto |
| Triage & selection | 10 | SelectionRequest/Response, OutcomeReport/Ack, AnalysisRequest/Response, Hypothesis, AvoidListRequest/Response, FeedbackProto |
| Worker management | 16 | ListWorkersRequest/Response, WorkerInfo, GetWorker*, GetAvailableWorkers*, GetWorkerMetadata*, WorkerMetadataEntry, Registration*, Deregistration*, MetadataUpdate/Ack |
| Monitoring | 6 | GetPoolMetrics*, PoolMetricsEntry, GetOrchestratorStatus*, ActiveJobEntry |
| Admin | 6 | PingWorker*, DisconnectWorker*, DisconnectAllWorkers* |
| Harness | 5 | ExecuteRequest/Response, MonitorRequest, MonitorEvent, ExecutionStatus |
| Worker agent | 5 | HealthRequest/Response, TransferAck, WorkerInfoRequest/Response, HealthMetrics, TelemetryRequest |
| Utility | 8 | PingRequest/Response (×2 files), TriageRequest/Response, QueryRequest/Response, AnalysisResult, StatusAck |
| Trace | 3 | GetTraceLinesRequest/Response, TraceLine |
