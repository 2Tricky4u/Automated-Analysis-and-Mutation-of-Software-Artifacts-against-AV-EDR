# Elasticsearch Schema for AutoMutate++ Controller

## Index Overview

| Index Pattern | Purpose | Retention | Partitioning |
|---------------|---------|-----------|--------------|
| `jobs-YYYY.MM` | Job lifecycle & config | Permanent | Monthly |
| `rounds-YYYY.MM` | Round summaries | Permanent | Monthly |
| `runs-YYYY.MM` | Individual run outcomes | Permanent | Monthly |
| `telemetry-YYYY.MM.DD` | Raw ETW/trace events | Permanent | Daily |
| `tokens-YYYY.MM` | Per-run token sets for feedback loop scoring | Permanent | Monthly |
| `differential-YYYY.MM` | Baseline vs instrumented comparison | Permanent | Monthly |

> **Retention Policy:** All indices are retained until manual deletion. No ILM policies applied.

---

## Correlation Keys (Join-Safe)

All indices use consistent `keyword` fields for cross-index correlation:

| Field | Present In | Description |
|-------|------------|-------------|
| `job_id` | All indices | Top-level job identifier |
| `round_id` | rounds, runs, telemetry, tokens, differential | Round identifier |
| `run_id` | runs, telemetry, tokens | Individual run identifier |
| `vm_id` | runs, telemetry | Worker VM identifier |

> **Important:** These fields are top-level and canonical. Nested copies (e.g., `metadata.run_id`) may exist but top-level is authoritative.

---

## 1. `jobs-*` — Job Lifecycle

Tracks job submission, configuration, progress, and final outcome.

```json
{
  "index_patterns": ["jobs-*"],
  "template": {
    "settings": {
      "number_of_shards": 1,
      "number_of_replicas": 0,
      "refresh_interval": "5s"
    },
    "mappings": {
      "properties": {
        "job_id":              { "type": "keyword" },
        "status":              { "type": "keyword" },

        "template_name":       { "type": "keyword" },
        "source_file":         { "type": "keyword" },
        "trace_mode":          { "type": "keyword" },
        "encoding":            { "type": "keyword" },
        "priority":            { "type": "integer" },

        "current_round":       { "type": "integer" },
        "max_rounds":          { "type": "integer" },
        "stop_on_evasion":     { "type": "boolean" },
        "stop_on_detection":   { "type": "boolean" },

        "target_os":           { "type": "keyword" },
        "required_capabilities": { "type": "keyword" },

        "modules": {
          "type": "object",
          "properties": {
            "carrier":         { "type": "keyword" },
            "decoder":         { "type": "keyword" },
            "antiemulation":   { "type": "keyword" },
            "deconditioner":   { "type": "keyword" },
            "guardrail":       { "type": "keyword" },
            "virtualprotect":  { "type": "keyword" },
            "decoy":           { "type": "keyword" }
          }
        },

        "search_space": {
          "type": "object",
          "properties": {
            "variable_categories": { "type": "keyword" },
            "explore_string_xor":  { "type": "boolean" }
          }
        },

        "outcome": {
          "type": "object",
          "properties": {
            "detection_outcome": { "type": "keyword" },
            "total_rounds":      { "type": "integer" },
            "evasion_count":     { "type": "integer" },
            "detection_count":   { "type": "integer" },
            "best_evasion_score": { "type": "float" }
          }
        },

        "created_at":          { "type": "date" },
        "started_at":          { "type": "date" },
        "completed_at":        { "type": "date" },
        "updated_at":          { "type": "date" }
      }
    }
  }
}
```

### Status Values

| Status | Description |
|--------|-------------|
| `queued` | Job submitted, waiting for worker |
| `running` | Job actively executing rounds |
| `completed` | All rounds finished successfully |
| `stopped` | Manually stopped via API |
| `failed` | Job failed due to error |

### Data Source Analysis

| Field | Source | Controller Type | Notes |
|-------|--------|-----------------|-------|
| `job_id` | Controller | `JobSession.id` | Generated on job creation |
| `status` | Controller | `JobSession.to_info(status).status` | Tracked via state machine |
| `template_name` | Controller | `ModularBuildSpec` | From job request |
| `source_file` | Controller | `ModularBuildSpec.payload_path` | From job request |
| `trace_mode` | Controller | Constant per run type | "off" or "lines" |
| `encoding` | Controller | `ModularBuildSpec.encoding` | "xor" or "english" |
| `priority` | Controller | Job request param | Optional, default 0 |
| `current_round` | Controller | `JobSession.current_round` | Updated per round |
| `max_rounds` | Controller | `JobSession.max_rounds` | From job request |
| `stop_on_evasion` | Controller | `JobSession.stop_on_evasion` | From job request |
| `stop_on_detection` | Controller | Job request param | Optional |
| `target_os` | Controller | `JobSession.target_os` | From job request |
| `required_capabilities` | Controller | `JobSession.required_capabilities` | From job request |
| `modules.*` | Controller | `ModularBuildSpec.modules: ModuleSelectionSpec` | All 7 module selections |
| `search_space.*` | Controller | `JobSession.search_space: SearchSpace` | From JobRequest, controls selector |
| `outcome.*` | Controller | Computed from `JobSession.rounds` | Aggregated on completion |
| `created_at` | Controller | `JobSession.created_at` | Set on creation |
| `started_at` | Controller | `JobSession.started_at` | Set on first round start |
| `completed_at` | Controller | Compute on completion | `SystemTime::now()` |
| `updated_at` | Controller | Track state changes | `SystemTime::now()` |

> **Implementation:** All job fields are available from `JobSession` in `types.rs`. No proto needed. Fix `index_job()` to use `JobSession` instead of non-existent `crate::job::Job`.

---

## 2. `rounds-*` — Round Summaries

Each round produces two runs (baseline + instrumented) and aggregates their results.

```json
{
  "index_patterns": ["rounds-*"],
  "template": {
    "settings": {
      "number_of_shards": 1,
      "number_of_replicas": 0
    },
    "mappings": {
      "properties": {
        "round_id":            { "type": "keyword" },
        "job_id":              { "type": "keyword" },
        "round_number":        { "type": "integer" },

        "modules": {
          "type": "object",
          "properties": {
            "carrier":         { "type": "keyword" },
            "decoder":         { "type": "keyword" },
            "antiemulation":   { "type": "keyword" },
            "deconditioner":   { "type": "keyword" },
            "guardrail":       { "type": "keyword" },
            "virtualprotect":  { "type": "keyword" },
            "decoy":           { "type": "keyword" }
          }
        },

        "mutations":           { "type": "keyword" },
        "mutation_recipe": {
          "type": "nested",
          "properties": {
            "id":              { "type": "keyword" },
            "layer":           { "type": "keyword" },
            "params":          { "type": "flattened" }
          }
        },
        "seed":                { "type": "keyword" },

        "baseline_run_id":     { "type": "keyword" },
        "instrumented_run_id": { "type": "keyword" },

        "detected":            { "type": "boolean" },
        "behavior_match":      { "type": "boolean" },
        "evasion_score":       { "type": "float" },
        "status":              { "type": "keyword" },

        "selector_rationale":  { "type": "text" },

        "coverage": {
          "type": "object",
          "properties": {
            "lines_executed":    { "type": "integer" },
            "new_lines":         { "type": "integer" },
            "coverage_gain":     { "type": "float" },
            "jaccard_similarity": { "type": "float" }
          }
        },

        "truncation_line":     { "type": "integer" },
        "last_trace":          { "type": "keyword" },

        "started_at":          { "type": "date" },
        "completed_at":        { "type": "date" }
      }
    }
  }
}
```

### Mutation Layers

| Layer | Description |
|-------|-------------|
| `ast` | Source-level C mutations |
| `ir` | LLVM IR transforms |
| `binary` | PE manipulation |
| `behavioral` | Runtime behavior changes |

### Data Source Analysis

| Field | Source | Controller Type | Notes |
|-------|--------|-----------------|-------|
| `round_id` | Controller | `RoundSummary.round_id` | From `RoundAgg.spec.id` |
| `job_id` | Controller | `RoundSpec.job_id` | Passed to `index_round()` |
| `round_number` | Controller | `RoundSummary.round_number` | Sequential within job |
| `modules.*` | Controller | `RoundSpec.modules: ModuleSelectionSpec` | Per-round module selection from selector |
| `mutations` | Controller | `RoundSummary.mutations: Vec<String>` | Mutation IDs applied |
| `mutation_recipe` | Controller | `RoundSpec.mutations: Vec<MutationSpec>` | Full recipe with params |
| `seed` | Controller | Build seed | Track for reproducibility |
| `baseline_run_id` | Controller | `RoundAgg.baseline_run_id` | Set when creating RoundAgg |
| `instrumented_run_id` | Controller | `RoundAgg.instrumented_run_id` | Set when creating RoundAgg |
| `detected` | Controller | `RoundSummary.detected` | Aggregated from both runs |
| `behavior_match` | Controller | `RoundSummary.behavior_match` | exit_code comparison |
| `evasion_score` | Controller | `RoundSummary.evasion_score` | 1.0 if evaded, 0.0 if detected |
| `status` | Controller | Derived from run outcomes | "completed", "failed", etc. |
| `selector_rationale` | Controller | `Selection.rationale` | Human-readable selector decision |
| `coverage.*` | Controller | Computed post-run | Diff between runs |
| `truncation_line` | Controller | Parse from trace events | Last trace before termination |
| `last_trace` | Controller | Parse from trace events | File:line of last trace |
| `started_at` | Controller | Track on round start | `SystemTime::now()` |
| `completed_at` | Controller | `RoundSummary.completed_at` | Set in `RoundAgg.to_summary()` |

> **Implementation:** All round fields available from `RoundSummary` and `RoundAgg` in `types.rs`. Fix `index_round()` to use `types::RoundSummary` instead of non-existent `crate::round::RoundSummary`. Extend to include `RoundAgg` for run IDs. The `modules` field comes from `RoundSpec.modules` which the selector populates each round.

---

## 3. `runs-*` — Individual Run Outcomes

Each run is a single artifact execution on a worker VM.

```json
{
  "index_patterns": ["runs-*"],
  "template": {
    "settings": {
      "number_of_shards": 1,
      "number_of_replicas": 0
    },
    "mappings": {
      "properties": {
        "run_id":              { "type": "keyword" },
        "job_id":              { "type": "keyword" },
        "round_id":            { "type": "keyword" },
        "run_type":            { "type": "keyword" },

        "artifact": {
          "type": "object",
          "properties": {
            "id":              { "type": "keyword" },
            "name":            { "type": "keyword" },
            "sha256":          { "type": "keyword" },
            "parent_sha256":   { "type": "keyword" }
          }
        },
        "mutation_chain":      { "type": "keyword" },

        "vm_id":               { "type": "keyword" },
        "worker_id":           { "type": "keyword" },
        "worker_ip":           { "type": "ip" },
        "pid":                 { "type": "integer" },

        "status":              { "type": "keyword" },
        "detected":            { "type": "boolean" },
        "exit_code":           { "type": "integer" },
        "elapsed_seconds":     { "type": "integer" },
        "telemetry_events_count": { "type": "integer" },

        "detection_outcome":   { "type": "keyword" },

        "labels": {
          "type": "object",
          "properties": {
            "telemetry_seen":  { "type": "boolean" },
            "alert_level":     { "type": "keyword" },
            "blocked":         { "type": "boolean" },
            "detection_latency_ms": { "type": "integer" }
          }
        },

        "perf": {
          "type": "object",
          "properties": {
            "cpu_pct":         { "type": "float" },
            "mem_mb":          { "type": "integer" },
            "event_to_record_ms_p95": { "type": "integer" }
          }
        },

        "error": {
          "type": "object",
          "properties": {
            "class":           { "type": "keyword" },
            "message":         { "type": "text" },
            "code":            { "type": "keyword" },
            "retryable":       { "type": "boolean" }
          }
        },

        "mutations":           { "type": "keyword" },
        "trace_mode":          { "type": "keyword" },

        "details":             { "type": "text" },

        "enqueued_at":         { "type": "date" },
        "started_at":          { "type": "date" },
        "finished_at":         { "type": "date" },
        "timestamp":           { "type": "date" }
      }
    }
  }
}
```

### Run Status Values

| Status | Description |
|--------|-------------|
| `detected` | EDR detected and/or blocked the artifact |
| `not_detected` | Artifact executed without detection |
| `noisy` | Instrumentation caused detection (false positive) |
| `crash` | Process crashed during execution |
| `timeout` | Execution exceeded time limit |

### Run Type Values

| Type | Description |
|------|-------------|
| `baseline` | `--trace=off`, no instrumentation |
| `instrumented` | `--trace=lines` or `--trace=all` |

### Detection Outcome Values

| Outcome | Description |
|---------|-------------|
| `MUTATION_FAILED` | Process killed before payload runs (loader caught) |
| `MUTATION_SUCCESS` | Payload executes then killed (loader evaded, payload caught) |
| `FULL_EVASION` | Process completes successfully |

### Error Classes

| Class | Description |
|-------|-------------|
| `build_error` | Artifact compilation failed |
| `deploy_error` | Failed to transfer artifact to VM |
| `execution_error` | Runtime error during execution |
| `timeout_error` | Execution exceeded time limit |
| `vm_error` | VM connectivity or state issue |

### Scheduling Timestamps

| Field | Description |
|-------|-------------|
| `enqueued_at` | When run was added to RunPool queue |
| `started_at` | When VM began execution |
| `finished_at` | When execution completed |
| `timestamp` | Document indexing time (for ES sorting) |

### Data Source Analysis

| Field | Source | Controller Type / Proto | Notes |
|-------|--------|------------------------|-------|
| `run_id` | Proto | `StatusReport.run_id` | Worker reports this |
| `job_id` | Proto | `StatusReport.job_id` | Worker reports this |
| `round_id` | **Controller** | `RunEnvelope.round_id` | Lookup via `run_pool.pending.get(run_id)` |
| `run_type` | **Controller** | `RunEnvelope.run_type` | "baseline" or "instrumented" |
| `artifact.id` | **Controller** | Generated | UUID for artifact |
| `artifact.name` | Proto | `StatusReport.artifact_name` | Worker reports this |
| `artifact.sha256` | **Controller** | `RunEnvelope.artifact.sha256` | Computed on build |
| `artifact.parent_sha256` | **Controller** | Track in build | Previous generation |
| `mutation_chain` | **Controller** | `RunEnvelope.mutations` | Accumulated mutations |
| `vm_id` | **Controller** | Known at dispatch | `VMInfo.id` from executor |
| `worker_id` | Proto | `StatusReport.worker_id` | Worker reports this |
| `worker_ip` | Proto | `StatusReport.worker_ip` | Worker reports this |
| `pid` | Proto | `StatusReport.pid` | Worker reports this |
| `status` | Proto | `StatusReport.event_type` | Worker reports this |
| `detected` | **Controller** | Parse from `StatusReport.event_type` | "detected" -> true |
| `exit_code` | **Controller** | Parse from `StatusReport.details` | Extract from JSON |
| `elapsed_seconds` | Proto | `StatusReport.elapsed_seconds` | Worker reports this |
| `telemetry_events_count` | Proto | `StatusReport.telemetry_events_count` | Worker reports this |
| `detection_outcome` | **Controller** | Derive from exit_code + detected | MUTATION_FAILED/SUCCESS/FULL_EVASION |
| `labels.*` | **Controller** | Computed post-run | Analysis labels |
| `perf.*` | **Controller** | Computed post-run | Performance metrics |
| `error.*` | **Controller** | Parse from `StatusReport.details` | Structured error info |
| `mutations` | **Controller** | `RunEnvelope.mutations` | Applied mutations |
| `trace_mode` | **Controller** | `RunEnvelope.run_type.trace_mode()` | "off" or "lines" |
| `details` | Proto | `StatusReport.details` | Raw details string |
| `enqueued_at` | **Controller** | Track on `add_runs()` | Add timestamp to RunEnvelope |
| `started_at` | **Controller** | Track on `take_run()` | Record dispatch time |
| `finished_at` | **Controller** | Track on result arrival | `SystemTime::now()` |
| `timestamp` | **Controller** | `chrono::Utc::now()` | Index time |

> **Implementation:** Proto provides basic run info. Controller enriches with `RunEnvelope` lookup from `run_pool.pending`. Add timing fields to `RunEnvelope` or track separately. Key: before removing run from pending on completion, extract envelope data for indexing.

---

## 4. `telemetry-*` — Raw ETW/Trace Events

High-volume raw telemetry from worker VMs. Daily partitioning for query efficiency.

```json
{
  "index_patterns": ["telemetry-*"],
  "template": {
    "settings": {
      "number_of_shards": 2,
      "number_of_replicas": 0,
      "refresh_interval": "10s"
    },
    "mappings": {
      "dynamic": true,
      "dynamic_templates": [
        {
          "payload_numeric": {
            "path_match": "payload_*",
            "match_mapping_type": "long",
            "mapping": { "type": "long" }
          }
        },
        {
          "payload_keyword": {
            "path_match": "payload_*",
            "match_mapping_type": "string",
            "mapping": { "type": "keyword", "ignore_above": 256 }
          }
        }
      ],
      "properties": {
        "job_id":              { "type": "keyword" },
        "round_id":            { "type": "keyword" },
        "run_id":              { "type": "keyword" },
        "vm_id":               { "type": "keyword" },

        "event_type":          { "type": "keyword" },
        "source":              { "type": "keyword" },

        "timestamp":           { "type": "date" },
        "payload_ts_us":       { "type": "long" },
        "indexed_at":          { "type": "date" },

        "metadata": {
          "type": "object",
          "enabled": true
        },

        "payload_seq":         { "type": "integer" },
        "payload_file":        { "type": "keyword" },
        "payload_line":        { "type": "integer" },
        "payload_func":        { "type": "keyword" },
        "payload_thread_id":   { "type": "keyword" },

        "payload_total_bbs":   { "type": "integer" },
        "payload_bb_ids":      { "type": "integer" },
        "payload_hit_counts":  { "type": "integer" },
        "payload_bitmap_b64":  { "type": "keyword", "index": false },

        "payload_checkpoint_name": { "type": "keyword" },

        "payload_provider":    { "type": "keyword" },
        "payload_event_id":    { "type": "integer" },
        "payload_process_id":  { "type": "integer" },
        "payload_image_name":  { "type": "keyword" },

        "payload_api_name":    { "type": "keyword" },
        "payload_api_args":    { "type": "flattened" },
        "payload_return_value": { "type": "keyword" },

        "payload_protection":  { "type": "keyword" },
        "payload_address":     { "type": "keyword" },
        "payload_size":        { "type": "long" },

        "payload_dlls":        { "type": "object", "enabled": true },
        "payload_stack_trace": { "type": "object", "enabled": true },

        "error": {
          "type": "object",
          "properties": {
            "class":           { "type": "keyword" },
            "message":         { "type": "text" },
            "code":            { "type": "keyword" },
            "retryable":       { "type": "boolean" }
          }
        }
      }
    }
  }
}
```

### Event Type Values

| Type | Description |
|------|-------------|
| `etw` | Windows ETW provider events |
| `procmon` | Process monitor events |
| `network` | Network activity |
| `trace` | Line-level execution trace |
| `coverage` | Basic block coverage bitmap |
| `checkpoint` | Named execution checkpoints |
| `api` | API call interception |

### Source Values

| Source | Description |
|--------|-------------|
| `vm_etw` | ETW events from worker VM (RedEDR) |
| `vm_trace` | Line trace from instrumented artifact |
| `vm_coverage` | Coverage data from artifact |
| `edr_etw` | (Future) External EDR telemetry |

### Timestamp Contract

| Field | Role | Description |
|-------|------|-------------|
| `timestamp` | Canonical event time | When the event occurred (ISO8601) |
| `payload_ts_us` | Raw ETW time | Microseconds from ETW provider |
| `indexed_at` | Ingestion time | When indexed to Elasticsearch |

### Dynamic Mapping

- `dynamic: true` allows new `payload_*` fields automatically
- `dynamic_templates` ensure consistent types:
  - Numeric payloads -> `long` (aggregatable)
  - String payloads -> `keyword` with `ignore_above: 256`
- Complex nested payloads (`payload_dlls`, `payload_stack_trace`) preserved as objects

### Token Extraction Field Map

The TokenExtractor (feedback loop) derives triage tokens from these telemetry fields. Two distinct fields carry API-level information depending on event source:

| Token format | Source field | Event type | Description |
|---|---|---|---|
| `api:<func>` | `payload_api_name` | `api` | RedEDR API hooking interceptions (NtAllocateVirtualMemory, etc.) |
| `etw:<provider>/<event_id>` | `payload_provider` + `payload_event_id` | `etw` | ETW provider events |
| `seq2:<api1>-><api2>` | Consecutive `payload_api_name` by `timestamp` | `api` | Bigram API sequences |
| `trunc:<file>:<line>` | `payload_file` + `payload_line` | `trace` | Last trace before termination |

> **Important:** `payload_func` is the function name from **instrumented artifact line traces** (event_type=`trace`). `payload_api_name` is the intercepted API name from **RedEDR hooking** (event_type=`api`). The TokenExtractor queries `payload_api_name` for `api:` and `seq2:` tokens. Do not confuse these two fields.

### Data Source Analysis

| Field | Source | Controller Type / Proto | Notes |
|-------|--------|------------------------|-------|
| `job_id` | Proto | `TelemetryData.job_id` | Worker includes this |
| `round_id` | **Controller** | `RunEnvelope.round_id` | Lookup via run_id or worker mapping |
| `run_id` | **Controller** | Lookup via `active_runs[worker_id]` | Track which run is active per worker |
| `vm_id` | **Controller** | Known from gRPC context | `WorkerId` of sender |
| `event_type` | Proto | `TelemetryData.event_type` | Worker reports this |
| `source` | **Controller** | Constant "worker" | Literal for now |
| `timestamp` | Proto | `TelemetryData.timestamp` | Worker timestamp |
| `payload_ts_us` | Proto | `TelemetryData.typed_event.trace.ts_us` | Raw microseconds |
| `indexed_at` | **Controller** | `chrono::Utc::now()` | Index time |
| `metadata` | Proto | `TelemetryData.metadata` | Key-value pairs |
| `payload_seq` | Proto | `TelemetryData.typed_event.trace.seq` | Sequence number |
| `payload_file` | Proto | `TelemetryData.typed_event.trace.file` | Source file |
| `payload_line` | Proto | `TelemetryData.typed_event.trace.line` | Line number |
| `payload_func` | Proto | `TelemetryData.typed_event.trace.func` | Function name (line traces only) |
| `payload_api_name` | Proto | `TelemetryData.payload` (JSON) | API name (RedEDR hook events) |
| `payload_*` (dynamic) | Proto | `TelemetryData.payload` (JSON) | Parsed from payload bytes |
| `error.*` | **Controller** | Parse errors | Structured error info |

> **Implementation:** Proto provides event data. Controller must enrich with correlation keys:
> 1. Maintain `active_runs: DashMap<WorkerId, RunId>` - set on dispatch, cleared on result
> 2. On telemetry receive, lookup `run_id = active_runs[worker_id]`
> 3. Then lookup `envelope = run_pool.pending.get(run_id)` for `round_id`, `run_type`
> 4. Alternative: Add `run_id` to proto `TelemetryData` (requires worker change)

---

## 5. `tokens-*` — Per-Run Token Sets (Feedback Loop)

Stores normalized triage tokens extracted from telemetry after each round completes. This is the **computation index** for the feedback loop: the Scorer aggregates over `tokens-*` to compute lift scores, and the Selector uses those scores to pick modules for the next round.

One document per run (baseline and instrumented each get their own document).

```json
{
  "index_patterns": ["tokens-*"],
  "template": {
    "settings": {
      "number_of_shards": 1,
      "number_of_replicas": 0,
      "refresh_interval": "5s"
    },
    "mappings": {
      "properties": {
        "job_id":              { "type": "keyword" },
        "round_id":            { "type": "keyword" },
        "run_id":              { "type": "keyword" },

        "detected":            { "type": "boolean" },

        "modules": {
          "type": "object",
          "properties": {
            "carrier":         { "type": "keyword" },
            "decoder":         { "type": "keyword" },
            "antiemulation":   { "type": "keyword" },
            "deconditioner":   { "type": "keyword" },
            "guardrail":       { "type": "keyword" },
            "virtualprotect":  { "type": "keyword" },
            "decoy":           { "type": "keyword" }
          }
        },
        "mutations":           { "type": "keyword" },
        "tokens":              { "type": "keyword" },
        "token_count":         { "type": "integer" },
        "timestamp":           { "type": "date" }
      }
    }
  }
}
```

### Token Format Reference

| Token format | Example | Source |
|---|---|---|
| `api:<func>` | `api:NtAllocateVirtualMemory` | `telemetry-*.payload_api_name` |
| `etw:<provider>/<event_id>` | `etw:Microsoft-Windows-Kernel-Memory/42` | `telemetry-*.payload_provider` + `payload_event_id` |
| `seq2:<api1>-><api2>` | `seq2:NtAllocateVirtualMemory->NtProtectVirtualMemory` | Consecutive `payload_api_name` by timestamp |
| `module:<category>=<name>` | `module:deconditioner=alloc_loop` | From `RoundSpec.modules` |
| `exit:<code>` | `exit:-2` | From `RunOutcome.exit_code` |

### Scorer Aggregation Query

The Scorer computes lift per token using a single ES aggregation:

```json
GET tokens-*/_search
{
  "query": { "term": { "job_id": "<job_id>" } },
  "size": 0,
  "aggs": {
    "overall_detection_rate": { "avg": { "field": "detected" } },
    "by_token": {
      "terms": { "field": "tokens", "size": 500 },
      "aggs": {
        "detection_rate": { "avg": { "field": "detected" } },
        "doc_count": { "value_count": { "field": "detected" } }
      }
    }
  }
}
```

> **Note:** `tokens` is mapped as `keyword`, so `terms` aggregation works directly (no `.keyword` sub-field needed). ES computes `avg` on boolean fields by treating `true=1, false=0`.

### Lift Computation

```
For each token T:
  lift(T) = detection_rate_given_token(T) / overall_detection_rate
  confidence(T) = min(1.0, n_total(T) / 5)
  importance(T) = lift(T) * confidence(T)
```

### Example Document

```json
{
  "job_id": "job-001",
  "round_id": "job-001-round-3",
  "run_id": "job-001-round-3-baseline",
  "detected": true,
  "modules": {
    "carrier": "alloc_rw_rx",
    "decoder": "xor",
    "antiemulation": "cpuburn",
    "deconditioner": "alloc_loop",
    "guardrail": "none",
    "virtualprotect": "standard",
    "decoy": "none"
  },
  "mutations": ["ast.string_xor"],
  "tokens": [
    "api:NtAllocateVirtualMemory",
    "api:NtProtectVirtualMemory",
    "seq2:NtAllocateVirtualMemory->NtProtectVirtualMemory",
    "module:carrier=alloc_rw_rx",
    "module:deconditioner=alloc_loop",
    "exit:-2"
  ],
  "token_count": 6,
  "timestamp": "2026-02-15T14:30:00Z"
}
```

### Data Source Analysis

| Field | Source | Controller Type | Notes |
|-------|--------|-----------------|-------|
| `job_id` | Controller | `RoundSpec.job_id` | From round context |
| `round_id` | Controller | `RoundSpec.id` | From round context |
| `run_id` | Controller | `RoundAgg.baseline_run_id` or `instrumented_run_id` | Per-run document |
| `detected` | Controller | `RunOutcome.detected` | From RoundAgg |
| `modules.*` | Controller | `RoundSpec.modules` | Which modules this round used |
| `mutations` | Controller | `RoundSpec.mutations` | Applied mutation IDs |
| `tokens` | Controller | TokenExtractor output | Parsed from telemetry-* |
| `token_count` | Controller | `tokens.len()` | Convenience for queries |
| `timestamp` | Controller | `chrono::Utc::now()` | Extraction time |

> **Implementation:** The TokenExtractor is spawned async in `finalize_round()`. It queries `telemetry-*` for this round's run_ids, parses events into normalized tokens, and indexes to `tokens-*`. This is non-blocking: the next round can start before extraction completes. See FEEDBACK-LOOP-PLAN.md Step 1 for details.

---

## 6. `differential-*` — Baseline vs Instrumented Comparison

Pre-computed differential analysis between paired runs.

```json
{
  "index_patterns": ["differential-*"],
  "template": {
    "settings": {
      "number_of_shards": 1,
      "number_of_replicas": 0
    },
    "mappings": {
      "properties": {
        "diff_id":             { "type": "keyword" },
        "job_id":              { "type": "keyword" },
        "round_id":            { "type": "keyword" },
        "baseline_run_id":     { "type": "keyword" },
        "instrumented_run_id": { "type": "keyword" },

        "baseline_detected":   { "type": "boolean" },
        "instrumented_detected": { "type": "boolean" },
        "comparison_outcome":  { "type": "keyword" },

        "truncation_point": {
          "type": "object",
          "properties": {
            "file":            { "type": "keyword" },
            "line":            { "type": "integer" },
            "func":            { "type": "keyword" }
          }
        },
        "confirmed_trigger_line": { "type": "keyword" },

        "event_counts_delta": {
          "type": "object",
          "properties": {
            "etw":             { "type": "integer" },
            "procmon":         { "type": "integer" },
            "api":             { "type": "integer" },
            "trace":           { "type": "integer" }
          }
        },

        "differential_candidates": {
          "type": "nested",
          "properties": {
            "rank":            { "type": "integer" },
            "token":           { "type": "keyword" },
            "token_type":      { "type": "keyword" },
            "lift":            { "type": "float" },
            "confidence":      { "type": "float" },
            "occurrences_baseline": { "type": "integer" },
            "occurrences_instrumented": { "type": "integer" }
          }
        },

        "api_sequence_diff": {
          "type": "object",
          "properties": {
            "added":           { "type": "keyword" },
            "removed":         { "type": "keyword" },
            "ngram_jaccard":   { "type": "float" }
          }
        },

        "coverage_diff": {
          "type": "object",
          "properties": {
            "lines_only_baseline":     { "type": "integer" },
            "lines_only_instrumented": { "type": "integer" },
            "lines_common":            { "type": "integer" }
          }
        },

        "timestamp":           { "type": "date" }
      }
    }
  }
}
```

### Comparison Outcome Values

| Outcome | Run 1 (Baseline) | Run 2 (Instrumented) | Interpretation |
|---------|------------------|----------------------|----------------|
| `both_detected` | Detected | Detected | Real detection - mutate truncation line |
| `both_evaded` | Not detected | Not detected | Full evasion achieved |
| `instrumentation_caused_detection` | Not detected | Detected | Instrumentation noise |
| `baseline_only_detected` | Detected | Not detected | Unusual - investigate |

### Data Source Analysis

| Field | Source | Controller Type | Notes |
|-------|--------|-----------------|-------|
| `diff_id` | **Controller** | Generated UUID | Unique diff result ID |
| `job_id` | **Controller** | `RoundAgg.spec.job_id` | From round context |
| `round_id` | **Controller** | `RoundAgg.spec.id` | Round being compared |
| `baseline_run_id` | **Controller** | `RoundAgg.baseline_run_id` | From round aggregation |
| `instrumented_run_id` | **Controller** | `RoundAgg.instrumented_run_id` | From round aggregation |
| `baseline_detected` | **Controller** | `RoundAgg.baseline.detected` | From `RunOutcome` |
| `instrumented_detected` | **Controller** | `RoundAgg.instrumented.detected` | From `RunOutcome` |
| `comparison_outcome` | **Controller** | Derived from both | Logic in differential engine |
| `truncation_point.*` | **Controller** | Parse telemetry | Last trace event |
| `confirmed_trigger_line` | **Controller** | Analysis output | Identified trigger |
| `event_counts_delta.*` | **Controller** | Query telemetry-* | Count diffs by event_type |
| `differential_candidates` | **Controller** | Analysis output | Ranked tokens |
| `api_sequence_diff.*` | **Controller** | N-gram analysis | Sequence comparison |
| `coverage_diff.*` | **Controller** | Coverage analysis | Line diff |
| `timestamp` | **Controller** | `chrono::Utc::now()` | Analysis time |

> **Implementation:** Differential is entirely controller-computed from:
> 1. `RoundAgg` provides run IDs and outcomes
> 2. Telemetry queries provide event data for comparison
> 3. Analysis engine computes diffs, truncation, candidates
> No proto needed - this is post-hoc analysis.

---

## Index Relationships

```
┌──────────────────────────────────────────────────────────────────────────┐
│                              jobs-*                                       │
│  job_id ─────────────────────────────────────────────────────────────────┤
│  modules (default), search_space                                          │
└───────────────────────────────┬───────────────────────────────────────────┘
                                │ 1:N
                                ▼
┌──────────────────────────────────────────────────────────────────────────┐
│                             rounds-*                                      │
│  round_id, job_id                                                         │
│  modules (per-round, from selector)                                       │
│  baseline_run_id ──────┐                                                  │
│  instrumented_run_id ──┼──────────────────────────────────────────────────┤
└────────────────────────┼─────────────────────────────────────────────────┘
                         │ 1:2                        │ 1:1
                         ▼                            ▼
┌─────────────────────────────────────┐  ┌────────────────────────────────┐
│              runs-*                  │  │        differential-*          │
│  run_id, job_id, round_id, vm_id     │  │  baseline_run_id               │
│  run_type: baseline|instrumented     │  │  instrumented_run_id           │
└──────────┬──────────────┬───────────┘  └────────────────────────────────┘
           │ 1:N          │ 1:1
           ▼              ▼
┌────────────────────┐  ┌────────────────────────────────┐
│   telemetry-*      │  │          tokens-*               │
│  run_id, vm_id     │  │  run_id, detected, modules      │
│  payload_api_name  │  │  tokens[] (normalized)           │
│  payload_provider  │──│  (TokenExtractor reads           │
│  payload_event_id  │  │   telemetry, writes tokens)      │
└────────────────────┘  └──────────────┬──────────────────┘
                                       │ aggregated by
                                       ▼
                              Scorer (in-process)
                              ├── lift per token
                              ├── confidence
                              └──► Selector picks next round's modules
```

---

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Monthly partitioning for structured data** | Jobs/rounds/runs/tokens are small docs, monthly keeps shard count manageable |
| **Daily partitioning for telemetry** | High volume, enables efficient time-range queries |
| **Permanent retention** | Research data needs long-term storage for trend analysis |
| **`keyword` for IDs, enums, and tokens** | Exact match queries, `terms` aggregations, no analysis needed |
| **`nested` for mutation_recipe/candidates** | Preserves array element integrity for complex queries |
| **`flattened` for dynamic params** | Mutation params vary per mutation type |
| **Consistent correlation keys** | `job_id`, `round_id`, `run_id`, `vm_id` at top-level in all indices |
| **Separate `differential-*` index** | Pre-computed comparisons avoid expensive joins at query time |
| **Separate `tokens-*` index** | Decouples token extraction (async) from round lifecycle; scorer aggregates directly |
| **`tokens` as `keyword` (not `text`)** | Enables `terms` aggregation for lift scoring without `.keyword` sub-field |
| **`detected` as `boolean` in tokens** | ES `avg` on boolean (true=1, false=0) gives detection rate directly |
| **Per-round `modules` in rounds-\*** | Selector varies modules each round; must track what was actually used, not just job defaults |
| **`search_space` in jobs-\*** | Records which categories the selector was allowed to vary for this job |
| **`detection_outcome` enum** | Matches CLAUDE.md: MUTATION_FAILED/SUCCESS/FULL_EVASION |
| **`dynamic_templates` for payloads** | Stable types for new ETW fields without mapping conflicts |
| **`source` field in telemetry** | Future-proofs for multiple telemetry sources (vm_etw, edr_etw) |
| **Artifact lineage tracking** | `parent_sha256` enables mutation chain analysis |
| **Scheduling timestamps** | `enqueued_at`/`started_at`/`finished_at` for pipeline visibility |
| **Error taxonomy** | Structured error reporting for debugging and retry logic |
| **No replicas** | Lab environment, single node, saves storage |

---

## Implementation Status

| Index | Template | Indexing | Querying | Priority |
|-------|----------|----------|----------|----------|
| `jobs-*` | Partial | Partial (missing status updates) | Not implemented | HIGH |
| `rounds-*` | Exists | **NOT INDEXED** | Not implemented | HIGH |
| `runs-*` | Implicit | Partial (missing round_id, vm_id, artifact lineage, error, scheduling timestamps) | Not implemented | HIGH |
| `telemetry-*` | Dynamic | Partial (missing round_id, vm_id, source) | Not implemented | HIGH |
| `tokens-*` | **NOT EXISTS** | **NOT INDEXED** | Not implemented | HIGH (feedback loop prerequisite) |
| `differential-*` | **NOT EXISTS** | **NOT INDEXED** | Not implemented | MEDIUM |

---

## Data Source Summary

### Proto vs Controller Enrichment

| Category | Proto Provides | Controller Enriches |
|----------|---------------|---------------------|
| **Jobs** | -- | All fields from `JobSession` |
| **Rounds** | -- | All fields from `RoundSummary`, `RoundAgg`, `RoundSpec.modules` |
| **Runs** | `run_id`, `job_id`, `worker_id`, `worker_ip`, `artifact_name`, `pid`, `status`, `elapsed_seconds`, `telemetry_events_count`, `details` | `round_id`, `run_type`, `vm_id`, `artifact.sha256`, `mutations`, `detected`, `exit_code`, `detection_outcome`, `error.*`, `enqueued_at`, `started_at`, `finished_at` |
| **Telemetry** | `job_id`, `event_type`, `timestamp`, `metadata`, all `payload_*` fields | `round_id`, `run_id`, `vm_id`, `source`, `indexed_at` |
| **Tokens** | -- | All fields (async extraction from telemetry-*) |
| **Differential** | -- | All fields (post-hoc analysis) |

### Controller Enrichment Flow

```
Telemetry arrives with job_id
     │
     ▼
active_runs[worker_id] ──────► run_id
     │
     ▼
run_pool.pending[run_id] ────► RunEnvelope
     │                           ├── round_id
     │                           ├── round_number
     │                           ├── run_type (baseline/instrumented)
     │                           ├── artifact.sha256
     │                           └── mutations

Run result arrives with run_id, job_id
     │
     ▼
run_pool.pending.remove(run_id) ──► RunEnvelope (extract before remove)
     │
     ▼
Enrich StatusReport with RunEnvelope fields

Round completes (both runs finished)
     │
     ▼
TokenExtractor.extract_and_index() (async, non-blocking)
     ├── Query telemetry-* for this round's run_ids
     ├── Parse events into normalized tokens
     └── Index to tokens-*
```

### Required Controller State

| State | Type | Purpose |
|-------|------|---------|
| `run_pool.pending` | `DashMap<RunId, RunEnvelope>` | Already exists - lookup run context |
| `run_pool.job_registry` | `DashMap<JobId, JobInfo>` | Already exists - job info |
| `active_runs` | `DashMap<WorkerId, RunId>` | **NEW** - track active run per worker |
| `run_timings` | `DashMap<RunId, RunTimings>` | **NEW** - track enqueued_at, started_at |

### Proto Changes: None Required

The controller has all correlation context available at index time via `RunEnvelope` lookup.

---

### Critical TODOs

1. **Add correlation keys to telemetry** - `round_id`, `run_id`, `vm_id`, `source` fields (via controller lookup)
2. **Add correlation keys to runs** - `round_id`, `run_type`, `vm_id` fields (via `RunEnvelope` lookup)
3. **Add scheduling timestamps to runs** - `enqueued_at`, `started_at`, `finished_at` (track in controller)
4. **Add artifact lineage to runs** - `artifact.sha256`, `artifact.parent_sha256`, `mutation_chain` (via `RunEnvelope`)
5. **Add error taxonomy to runs** - `error.class`, `error.message`, `error.code`, `error.retryable` (parse from details)
6. **Add `active_runs: DashMap<WorkerId, RunId>`** - Track active run per worker for telemetry correlation
7. **Add `run_timings: DashMap<RunId, RunTimings>`** - Track scheduling timestamps
8. **Fix `index_job()`** - Use `JobSession` from `types.rs`, not non-existent `crate::job::Job`
9. **Fix `index_round()`** - Use `types::RoundSummary` and `RoundAgg`, not non-existent `crate::round::RoundSummary`
10. **Index rounds on RoundCompleted event** - currently not persisted
11. **Update job status to ES** - `queued` -> `running` -> `completed` transitions
12. **Add `detection_outcome` to runs** - MUTATION_FAILED/SUCCESS/FULL_EVASION label
13. **Create tokens-\* index template** - feedback loop computation index
14. **Create differential-\* template and indexing** - comparison storage
15. **Apply dynamic_templates to telemetry** - prevents mapping conflicts on new payload fields

---

## Example Queries

### Find all evasive runs for a job
```json
GET runs-*/_search
{
  "query": {
    "bool": {
      "must": [
        { "term": { "job_id": "job-20250208-abc123" } },
        { "term": { "detected": false } }
      ]
    }
  }
}
```

### Get telemetry for a specific run
```json
GET telemetry-*/_search
{
  "query": {
    "term": { "run_id": "run-xyz789" }
  },
  "sort": [{ "payload_seq": "asc" }]
}
```

### Join runs with telemetry by round
```json
GET runs-*,telemetry-*/_search
{
  "query": {
    "bool": {
      "must": [
        { "term": { "round_id": "round-abc123" } }
      ]
    }
  },
  "sort": [{ "timestamp": "asc" }]
}
```

### Aggregate detection rates by mutation
```json
GET rounds-*/_search
{
  "size": 0,
  "aggs": {
    "by_mutation": {
      "terms": { "field": "mutations" },
      "aggs": {
        "detection_rate": {
          "avg": {
            "script": "doc['detected'].value ? 1 : 0"
          }
        }
      }
    }
  }
}
```

### Find runs with errors
```json
GET runs-*/_search
{
  "query": {
    "exists": { "field": "error.class" }
  }
}
```

### Token lift scores for a job (Scorer query)
```json
GET tokens-*/_search
{
  "query": { "term": { "job_id": "job-001" } },
  "size": 0,
  "aggs": {
    "overall_detection_rate": { "avg": { "field": "detected" } },
    "by_token": {
      "terms": { "field": "tokens", "size": 500 },
      "aggs": {
        "detection_rate": { "avg": { "field": "detected" } },
        "doc_count": { "value_count": { "field": "detected" } }
      }
    }
  }
}
```

### Detection rate by module combination
```json
GET tokens-*/_search
{
  "query": { "term": { "job_id": "job-001" } },
  "size": 0,
  "aggs": {
    "by_carrier": {
      "terms": { "field": "modules.carrier" },
      "aggs": {
        "by_deconditioner": {
          "terms": { "field": "modules.deconditioner" },
          "aggs": {
            "detection_rate": { "avg": { "field": "detected" } },
            "round_count": { "value_count": { "field": "round_id" } }
          }
        }
      }
    }
  }
}
```

### Find differential candidates with high lift
```json
GET differential-*/_search
{
  "query": {
    "nested": {
      "path": "differential_candidates",
      "query": {
        "range": {
          "differential_candidates.lift": { "gte": 5.0 }
        }
      }
    }
  }
}
```

### Calculate scheduling latency
```json
GET runs-*/_search
{
  "size": 0,
  "aggs": {
    "avg_queue_time": {
      "avg": {
        "script": "doc['started_at'].value.toInstant().toEpochMilli() - doc['enqueued_at'].value.toInstant().toEpochMilli()"
      }
    },
    "avg_execution_time": {
      "avg": {
        "script": "doc['finished_at'].value.toInstant().toEpochMilli() - doc['started_at'].value.toInstant().toEpochMilli()"
      }
    }
  }
}
```

### Trace artifact lineage
```json
GET runs-*/_search
{
  "query": {
    "term": { "artifact.parent_sha256": "abc123..." }
  },
  "sort": [{ "enqueued_at": "asc" }]
}
```

### Which rounds used a specific module?
```json
GET rounds-*/_search
{
  "query": {
    "bool": {
      "must": [
        { "term": { "job_id": "job-001" } },
        { "term": { "modules.deconditioner": "thread_alloc" } }
      ]
    }
  },
  "sort": [{ "round_number": "asc" }]
}
```