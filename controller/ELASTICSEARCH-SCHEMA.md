# Elasticsearch Schema for AutoMutate++ Controller

## Index Overview

| Index Pattern | Purpose | Retention | Partitioning |
|---------------|---------|-----------|--------------|
| `jobs-YYYY.MM` | Job lifecycle & config | Permanent | Monthly |
| `rounds-YYYY.MM` | Round summaries | Permanent | Monthly |
| `runs-YYYY.MM` | Individual run outcomes | Permanent | Monthly |
| `telemetry-YYYY.MM.DD` | Raw ETW/trace events | Permanent | Daily |
| `triage-YYYY.MM` | Hypothesis & feature rankings | Permanent | Monthly |
| `differential-YYYY.MM` | Baseline vs instrumented comparison | Permanent | Monthly |

> **Retention Policy:** All indices are retained until manual deletion. No ILM policies applied.

---

## Correlation Keys (Join-Safe)

All indices use consistent `keyword` fields for cross-index correlation:

| Field | Present In | Description |
|-------|------------|-------------|
| `job_id` | All indices | Top-level job identifier |
| `round_id` | rounds, runs, telemetry, triage, differential | Round identifier |
| `run_id` | runs, telemetry, triage | Individual run identifier |
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
            "guardrail":       { "type": "keyword" },
            "virtualprotect":  { "type": "keyword" },
            "decoy":           { "type": "keyword" }
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
| `modules.*` | Controller | `ModularBuildSpec.modules: ModuleSelectionSpec` | All 6 module selections |
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
| `mutations` | Controller | `RoundSummary.mutations: Vec<String>` | Mutation IDs applied |
| `mutation_recipe` | Controller | `RoundSpec.mutations: Vec<MutationSpec>` | Full recipe with params |
| `seed` | Controller | Build seed | Track for reproducibility |
| `baseline_run_id` | Controller | `RoundAgg.baseline_run_id` | Set when creating RoundAgg |
| `instrumented_run_id` | Controller | `RoundAgg.instrumented_run_id` | Set when creating RoundAgg |
| `detected` | Controller | `RoundSummary.detected` | Aggregated from both runs |
| `behavior_match` | Controller | `RoundSummary.behavior_match` | exit_code comparison |
| `evasion_score` | Controller | `RoundSummary.evasion_score` | 1.0 if evaded, 0.0 if detected |
| `status` | Controller | Derived from run outcomes | "completed", "failed", etc. |
| `coverage.*` | Controller | Computed post-run | Diff between runs |
| `truncation_line` | Controller | Parse from trace events | Last trace before termination |
| `last_trace` | Controller | Parse from trace events | File:line of last trace |
| `started_at` | Controller | Track on round start | `SystemTime::now()` |
| `completed_at` | Controller | `RoundSummary.completed_at` | Set in `RoundAgg.to_summary()` |

> **Implementation:** All round fields available from `RoundSummary` and `RoundAgg` in `types.rs`. Fix `index_round()` to use `types::RoundSummary` instead of non-existent `crate::round::RoundSummary`. Extend to include `RoundAgg` for run IDs.

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
| `detected` | **Controller** | Parse from `StatusReport.event_type` | "detected" → true |
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
  - Numeric payloads → `long` (aggregatable)
  - String payloads → `keyword` with `ignore_above: 256`
- Complex nested payloads (`payload_dlls`, `payload_stack_trace`) preserved as objects

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
| `payload_func` | Proto | `TelemetryData.typed_event.trace.func` | Function name |
| `payload_*` (dynamic) | Proto | `TelemetryData.payload` (JSON) | Parsed from payload bytes |
| `error.*` | **Controller** | Parse errors | Structured error info |

> **Implementation:** Proto provides event data. Controller must enrich with correlation keys:
> 1. Maintain `active_runs: DashMap<WorkerId, RunId>` - set on dispatch, cleared on result
> 2. On telemetry receive, lookup `run_id = active_runs[worker_id]`
> 3. Then lookup `envelope = run_pool.pending.get(run_id)` for `round_id`, `run_type`
> 4. Alternative: Add `run_id` to proto `TelemetryData` (requires worker change)

---

## 5. `triage-*` — Hypothesis & Feature Rankings

Post-analysis results from the triage engine with ranked hypotheses.

```json
{
  "index_patterns": ["triage-*"],
  "template": {
    "settings": {
      "number_of_shards": 1,
      "number_of_replicas": 0
    },
    "mappings": {
      "properties": {
        "triage_id":           { "type": "keyword" },
        "job_id":              { "type": "keyword" },
        "round_id":            { "type": "keyword" },
        "run_id":              { "type": "keyword" },

        "detected":            { "type": "boolean" },
        "av_product":          { "type": "keyword" },
        "detection_type":      { "type": "keyword" },

        "hypotheses": {
          "type": "nested",
          "properties": {
            "rank":            { "type": "integer" },
            "description":     { "type": "text" },
            "evidence_fields": { "type": "keyword" },
            "confidence":      { "type": "float" },
            "recommendation":  { "type": "keyword" }
          }
        },

        "feature_attributions": {
          "type": "nested",
          "properties": {
            "feature":         { "type": "keyword" },
            "importance":      { "type": "float" },
            "direction":       { "type": "keyword" }
          }
        },

        "avoid_features":      { "type": "keyword" },
        "seek_features":       { "type": "keyword" },

        "iocs": {
          "type": "object",
          "properties": {
            "api_sequences":   { "type": "keyword" },
            "memory_patterns": { "type": "keyword" },
            "file_artifacts":  { "type": "keyword" }
          }
        },

        "timestamp":           { "type": "date" }
      }
    }
  }
}
```

### Hypothesis Report Format

| Rank | Hypothesis | Evidence | Confidence | Recommendation |
|------|------------|----------|------------|----------------|
| 1 | Write→Protect sequence triggers detection | api_sequence, lift=8.5 | 0.95 | avoid |
| 2 | Short RWX window + anon thread start | mem.write_to_execute_ms<15 | 0.82 | avoid |
| 3 | RWX protection flag | flProtect=0x40 | 0.78 | avoid |

### Data Source Analysis

| Field | Source | Controller Type / Proto | Notes |
|-------|--------|------------------------|-------|
| `triage_id` | **Controller** | Generated UUID | Unique triage result ID |
| `job_id` | Proto / Controller | `TriageRequest.job_id` | From triage API request |
| `round_id` | **Controller** | Lookup from job context | Which round being triaged |
| `run_id` | **Controller** | Lookup from job context | Specific run if applicable |
| `detected` | Proto | `TriageRequest.detected` | User-reported detection |
| `av_product` | Proto | `TriageRequest.av_product` | Which AV product |
| `detection_type` | Proto | Extend `TriageRequest` | Scan-time vs run-time |
| `hypotheses` | **Controller** | Triage engine output | Ranked hypothesis list |
| `feature_attributions` | **Controller** | ML surrogate model | Feature importance |
| `avoid_features` | **Controller** | Derived from hypotheses | Tokens to avoid |
| `seek_features` | **Controller** | Derived from hypotheses | Tokens that evade |
| `iocs.*` | **Controller** | Analysis output | Indicators of compromise |
| `timestamp` | **Controller** | `chrono::Utc::now()` | Triage time |

> **Implementation:** Triage is post-analysis. Proto provides user input (`TriageRequest`). All analysis output (hypotheses, features, recommendations) generated by controller's triage engine.

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
┌─────────────────────────────────────────────────────────────────────────┐
│                              jobs-*                                      │
│  job_id ────────────────────────────────────────────────────────────────┤
└────────────────────────────────┬────────────────────────────────────────┘
                                 │ 1:N
                                 ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                             rounds-*                                     │
│  round_id, job_id                                                        │
│  baseline_run_id ──────┐                                                 │
│  instrumented_run_id ──┼─────────────────────────────────────────────────┤
└────────────────────────┼────────────────────────────────────────────────┘
                         │ 1:2                        │ 1:1
                         ▼                            ▼
┌─────────────────────────────────────┐  ┌────────────────────────────────┐
│              runs-*                  │  │        differential-*          │
│  run_id, job_id, round_id, vm_id     │  │  baseline_run_id               │
│  run_type: baseline|instrumented     │  │  instrumented_run_id           │
└──────────────────┬──────────────────┘  └────────────────────────────────┘
                   │ 1:N
                   ▼
┌─────────────────────────────────────┐
│           telemetry-*               │
│  job_id, round_id, run_id, vm_id    │
│  source: vm_etw | vm_trace | ...    │
└─────────────────────────────────────┘

┌─────────────────────────────────────┐
│            triage-*                 │
│  job_id, round_id, run_id           │
│  (post-analysis results)            │
└─────────────────────────────────────┘
```

---

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Monthly partitioning for structured data** | Jobs/rounds/runs are small docs, monthly keeps shard count manageable |
| **Daily partitioning for telemetry** | High volume, enables efficient time-range queries |
| **Permanent retention** | Research data needs long-term storage for trend analysis |
| **`keyword` for IDs and enums** | Exact match queries, aggregations, no analysis needed |
| **`nested` for hypotheses/candidates** | Preserves array element integrity for complex queries |
| **`flattened` for dynamic params** | Mutation params vary per mutation type |
| **Consistent correlation keys** | `job_id`, `round_id`, `run_id`, `vm_id` at top-level in all indices |
| **Separate `differential-*` index** | Pre-computed comparisons avoid expensive joins at query time |
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
| `triage-*` | **NOT EXISTS** | **NOT INDEXED** | Not implemented | MEDIUM |
| `differential-*` | **NOT EXISTS** | **NOT INDEXED** | Not implemented | MEDIUM |

---

## Data Source Summary

### Proto vs Controller Enrichment

| Category | Proto Provides | Controller Enriches |
|----------|---------------|---------------------|
| **Jobs** | — | All fields from `JobSession` |
| **Rounds** | — | All fields from `RoundSummary`, `RoundAgg` |
| **Runs** | `run_id`, `job_id`, `worker_id`, `worker_ip`, `artifact_name`, `pid`, `status`, `elapsed_seconds`, `telemetry_events_count`, `details` | `round_id`, `run_type`, `vm_id`, `artifact.sha256`, `mutations`, `detected`, `exit_code`, `detection_outcome`, `error.*`, `enqueued_at`, `started_at`, `finished_at` |
| **Telemetry** | `job_id`, `event_type`, `timestamp`, `metadata`, all `payload_*` fields | `round_id`, `run_id`, `vm_id`, `source`, `indexed_at` |
| **Triage** | `job_id`, `detected`, `av_product` | All analysis output (hypotheses, features, recommendations) |
| **Differential** | — | All fields (post-hoc analysis) |

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
11. **Update job status to ES** - `queued` → `running` → `completed` transitions
12. **Add `detection_outcome` to runs** - MUTATION_FAILED/SUCCESS/FULL_EVASION label
13. **Create triage-* template and indexing** - hypothesis storage
14. **Create differential-* template and indexing** - comparison storage
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
