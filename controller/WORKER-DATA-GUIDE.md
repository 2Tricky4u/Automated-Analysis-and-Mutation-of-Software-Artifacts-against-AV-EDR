# Worker → Controller Data Guide

## What the Worker Sends (6 message types over bidirectional gRPC stream)

```
WorkerMessage.payload (oneof)
  ├── 1. WorkerRegistration     (once, on connect)
  ├── 2. StatusReport           (every 30s heartbeat)
  ├── 3. ExecutionStatusReport  (every ~3s during execution)
  ├── 4. Ack                    (immediately on receiving RunSampleCommand)
  ├── 5. TelemetryBatch         (once per run, after execution completes)
  └── 6. SampleResponse         (once per run, final result)
```

---

## Message-by-Message Breakdown

### 1. TelemetryBatch (the richest payload — all telemetry lives here)

Sent once after execution. Contains `events: Vec<TelemetryData>` with **6 sub-event types**:

| Event type | Source | Key fields in `payload` (JSON bytes) | `typed_event` | Key `metadata` keys |
|---|---|---|---|---|
| **RedEDR events** (`"etw"`, `"dll_loaded"`, `"api_call"`, etc.) | `GET /api/logs/rededr` | `func`, `pid`, `tid`, `provider`, `event_id`, `callstack`, `stack_trace`, `targets` | None | `source=rededr`, `event_type`, `pid`, `tid`, `provider`, `trace_id` |
| **`"trace_log"`** | `trace_events.jsonl` (named pipe) | JSONL content: `{"seq":1, "thread_id":100, "file":"loader.c", "line":42, "func":"main", "ts_us":...}` | None | `trace_file`, `event_count`, `original_size_bytes`, `compression`, `final_size_bytes` |
| **`"trace_line"`** | `trace.log` (binary, `0x49535452` magic) | UTF-8 string: `"loader.c:42:main"` | None | (empty) |
| **`"coverage"`** | `coverage.bin` + `coverage_bbs.txt` | (empty) | `CoverageEvent { bb_ids, hit_counts, total_bbs }` | (empty) |
| **`"checkpoint"` / `"artifact_success"` / `"artifact_failure"`** | `checkpoints.log` (JSON-lines) | (empty) | `CheckpointEvent { name, ts_us }` | `status_type`, `error_code` (for failures) |
| **`"phase_timings"`** | Engine timing | (empty) | None | `rededr_setup_ms`, `process_spawn_ms`, `process_wait_ms`, `telemetry_collect_ms` |

#### RedEDR Event JSON payload structure

Parsed from RedEDR HTTP API (`GET /api/logs/rededr`). Each event is a `RedEdrEvent` struct serialized to JSON:

```json
{
  "date": "2025-11-02-15-30-00",
  "type": "etw",
  "trace_id": 42,
  "target": "artifact.exe",
  "func": "NtAllocateVirtualMemory",
  "pid": 1234,
  "tid": 5678,
  "provider": "Microsoft-Windows-Kernel-Process",
  "event_id": 10,
  "callstack": ["..."],
  "stack_trace": [{"addr": 140000000, "addr_info": "ntdll.dll+0x1234", "idx": 0}],
  "targets": ["artifact.exe"]
}
```

The `type` field becomes the `event_type` on the `TelemetryData` wrapper. Known values: `"etw"`, `"dll_loaded"`, `"api_call"`, `"kernel"`, `"unknown"`.

#### Trace log content format

Individual lines inside `trace_events.jsonl`:

```json
{"seq":1, "thread_id":100, "file":"loader.c", "line":42, "func":"main", "ts_us":1699999000000}
```

Compression logic for large traces:
- Trace <= 2MB and <= 4MB: sent as-is, `compression=none`
- Trace <= 2MB but > 4MB: truncated tail, `compression=truncated_tail`
- Trace > 2MB: last 2MB sent immediately, async compression spawned (`loop_detection`, `gzip`, or `truncated` fallback)

#### Checkpoint log format

Individual lines in `checkpoints.log`:

```json
{"ts_us":500000,"checkpoint":"api:VirtualAlloc"}
{"ts_us":1200000,"checkpoint":"Payload executed successfully","type":"success"}
{"ts_us":800000,"checkpoint":"VirtualProtect failed","type":"failure","error_code":5}
```

#### Coverage data format

`coverage_bbs.txt` lines: `BB_ID HIT_COUNT` (e.g., `12 100`), comments start with `#`.

---

### 2. SampleResponse (final execution result)

| Field | Type | How populated | Notes |
|---|---|---|---|
| `job_id` | string | From RunRequest | |
| `run_id` | string | From RunSampleCommand.request_id | |
| `success` | bool | `!timed_out && exit_code == 0` | |
| `exit_code` | i32 | Process exit code | **-2** = killed by AV/EDR, **-1** = timeout, **0** = clean exit |
| `detected` | bool | **Always `false`** | BUG: worker never sets this to true |
| `output` | string | Human-readable summary | e.g. `"Execution completed in 12.34s"` |
| `error` | string | Error msg or empty | |
| `telemetry_ids` | repeated string | `[run_id]` | |

#### Exit code interpretation

| Exit code | Meaning |
|---|---|
| `-2` | Externally terminated (heuristic) — process killed by AV/EDR (no exit code available) |
| `-1` | `wait()` failed or timeout |
| `0` | Success / clean exit |
| `0x8XXXXXXX` | NTSTATUS code — looked up via `RtlNtStatusToDosError` + `FormatMessageW` |
| Other | Raw exit code reported with hex |

**Controller routing:** SampleResponse is processed twice:
1. `vm/manager.rs` converts it to `RemoteRunResult { run_id, detected, exit_code, success, error }` → VMExecutor → RunPool → JobWorker
2. `orchestrator.rs` receives it via event channel → debug log only (no ES indexing)

---

### 3. WorkerRegistration (once on connect)

| Field | Example |
|---|---|
| `worker_id` | `"win10-worker-01"` |
| `ip_address` | `"10.200.200.100:50052"` |
| `os_version` | `"windows10"` |
| `capabilities` | `["rededr", "mde", "cortex"]` |
| `metadata` | `{"hostname":"WIN10-EDR-LAB", "cpu_cores":"8", "ram_gb":"32", "os_build":"19045"}` |
| `tools.rededr_version` | `"1.2.3"` (parsed from RedEDR API) |
| `tools.defender_version` | `"4.18.2106.1"` (from PowerShell) |

Capability detection:
- `"rededr"`: `GET http://localhost:8081/api/stats` returns 200
- `"mde"`: Windows registry key `HKLM\SOFTWARE\Microsoft\Windows Advanced Threat Protection\OnboardedInfo` exists
- `"cortex"`: `HKLM\SYSTEM\CurrentControlSet\Services\CyveraService` exists OR Palo Alto directories exist

---

### 4. ExecutionStatusReport (every ~3s during execution)

| Field | Example |
|---|---|
| `worker_id`, `worker_ip`, `job_id`, `run_id`, `artifact_name` | Identifiers |
| `pid` | `4892` |
| `elapsed_seconds` | `12` |
| `process_alive` | `true` |
| `telemetry_events_count` | `47` (from RedEDR `/api/stats`) |
| `event_type` | `"started"`, `"heartbeat"`, `"telemetry_idle"`, `"approaching_timeout"`, `"terminated"` |
| `cpu_percent`, `memory_mb` | Per-process metrics |
| `details` | `"pid=4892, events=47, cpu=25%, mem=18MB, elapsed=12s"` |

---

### 5. StatusReport (heartbeat)

| Field | Example |
|---|---|
| `worker_id` | `"win10-worker-01"` |
| `worker_ip` | `"10.200.200.100"` |
| `cpu_percent` | `15` |
| `memory_mb` | `42` (**BUG**: actually contains `memory_percent`, not MB) |
| `active_jobs` | `0` or `1` |
| `event_type` | `"heartbeat"` or `"health_check"` |
| `current_job_id` | `"job-000001"` or `""` |

---

### 6. Ack (command receipt)

| Field | Example |
|---|---|
| `request_id` | `"job-000001-round-1-baseline"` |
| `success` | `true` |
| `error` | `""` |

---

## What the Controller Actually Does With Each (Current State)

### ES Indexing Status

| Message | Indexed to ES? | What's stored | What's lost |
|---|---|---|---|
| **TelemetryBatch** | Partial: `telemetry-YYYY.MM` | `{event_type, timestamp, job_id, metadata}` | **ALL payload bytes** (RedEDR events, trace content), **ALL typed_events** (coverage, checkpoints), `run_id`, `round_id`, `vm_id`, `source` |
| **SampleResponse** | **No** (debug log only) | Nothing in ES | exit_code, detected, output, success — reach JobWorker via VMExecutor→RunPool but never ES |
| **WorkerRegistration** | **No** | In-memory TargetManager only | Everything lost on restart |
| **ExecutionStatusReport** | **No** | Debug log only | Real-time pid, elapsed, events_count, cpu, mem — all lost |
| **StatusReport** | **No** | In-memory heartbeat touch | Health metrics lost |
| **Ack** | **No** | Debug log only | — |

### Derived Data Indexing Status

| Derived data | Indexed? | Notes |
|---|---|---|
| `rounds-*` | **NO** — `orchestrator.rs:263` says `// TODO: Index round to ES` | `RoundSummary` computed but never persisted |
| `jobs-*` status updates | **NO** — `orchestrator.rs:281` says `// TODO: Update job status in ES` | Always shows `"queued"` |
| `runs-*` full | **Only partial** via `report_status` legacy RPC path | Missing: `round_id`, `run_type`, `vm_id`, `exit_code`, `detected`, `elapsed_seconds`, `detection_outcome` |
| `tokens-*` | **NO** — feedback loop not implemented | TokenExtractor does not exist yet |

---

## Controller Internal Types (what arrives after proto conversion)

### RemoteRunResult (from SampleResponse via vm/manager.rs)

```rust
pub struct RemoteRunResult {
    pub run_id: RunId,
    pub detected: bool,    // Always false from worker — needs controller fix
    pub exit_code: i32,
    pub success: bool,
    pub error: Option<String>,
}
```

**Proto fields IGNORED in conversion:** `job_id`, `output`, `telemetry_ids`

### RunOutcome (stored in RoundAgg)

```rust
pub struct RunOutcome {
    pub detected: bool,
    pub exit_code: i32,
    pub error: Option<String>,
}
```

### RoundAgg (ephemeral join state)

```rust
pub struct RoundAgg {
    pub spec: RoundSpec,
    pub baseline_run_id: RunId,
    pub instrumented_run_id: RunId,
    pub baseline: Option<RunOutcome>,      // filled when baseline completes
    pub instrumented: Option<RunOutcome>,   // filled when instrumented completes
}
```

`to_summary()` logic:
- `detected = baseline.detected || instrumented.detected` (naive OR — does NOT implement differential protocol correctly)
- `behavior_match = baseline.exit_code == instrumented.exit_code`
- `evasion_score = if detected { 0.0 } else { 1.0 }` (binary)

### RoundSummary (final round result)

```rust
pub struct RoundSummary {
    pub round_id: RoundId,
    pub round_number: u32,
    pub mutations: Vec<String>,
    pub detected: bool,
    pub behavior_match: bool,
    pub evasion_score: f64,
    pub completed_at: SystemTime,
}
```

### RunEnvelope (dispatch queue item — source of correlation keys)

```rust
pub struct RunEnvelope {
    pub run_id: RunId,
    pub job_id: JobId,
    pub round_id: RoundId,
    pub round_number: u32,
    pub run_type: RunType,          // Baseline | Instrumented
    pub artifact: ArtifactRef,      // path + sha256
    pub mutations: Vec<String>,
    pub timeout_seconds: u32,
    pub required_os: String,
    pub required_capabilities: Vec<String>,
}
```

### RoundSpec (immutable round recipe)

```rust
pub struct RoundSpec {
    pub id: RoundId,
    pub job_id: JobId,
    pub round_number: u32,
    pub mutations: Vec<MutationSpec>,
    // NOTE: missing `modules: ModuleSelectionSpec` — needs to be added per FEEDBACK-LOOP-PLAN
}
```

### JobSession (runtime state)

```rust
pub struct JobSession {
    pub id: JobId,
    pub target_os: Option<String>,
    pub required_capabilities: Vec<String>,
    pub build_spec: ModularBuildSpec,
    pub current_round: u32,
    pub max_rounds: u32,
    pub stop_on_evasion: bool,
    pub rounds: BTreeMap<u32, RoundSummary>,
    pub last_round: Option<RoundSummary>,
    pub created_at: SystemTime,
    pub started_at: Option<SystemTime>,
    // NOTE: missing `search_space: SearchSpace` — needs to be added per FEEDBACK-LOOP-PLAN
}
```

---

## Legacy Rich Indexer (Dead Code — Reference Implementation)

`controller/scheduler/src/storage/elasticsearch.rs` contains a far richer telemetry indexer that is **not compiled** (module not declared in `main.rs`, references non-existent types `crate::job::Job`, `crate::round::RoundSummary`). It is the **intended design** and should be revived.

### Payload flattening logic (`index_telemetry_batch()`)

1. Parse `event.payload` (bytes) as JSON
2. Handle `typed_event` variants:
   - `Trace`: extracts `seq`, `file`, `line`, `func`, `ts_us`
   - `Coverage`: extracts `total_bbs`, `bb_ids`, `hit_counts`, `bitmap_b64` (Base64)
   - `Checkpoint`: extracts `checkpoint_name`, `ts_us`
3. Build base document: `{job_id, event_type, timestamp, metadata, indexed_at}`
4. Merge payload fields with `payload_` prefix to avoid collisions
5. Smart number conversion:
   - Pointer/address fields → hex string `"0x1234"`
   - Numbers > i64::MAX → hex string
   - Fields like `"addr"`, `"pid"`, `"tid"` with value `"unsupported"` → null

### ES document shape (what it would produce)

```json
{
  "job_id": "job-001",
  "event_type": "etw",
  "timestamp": 1707400000000,
  "metadata": { "source": "rededr", "pid": "1234", "provider": "..." },
  "indexed_at": "2026-02-15T14:30:00Z",

  "payload_func": "NtAllocateVirtualMemory",
  "payload_pid": 1234,
  "payload_tid": 5678,
  "payload_provider": "Microsoft-Windows-Kernel-Process",
  "payload_event_id": 10,
  "payload_target": "artifact.exe",
  "payload_trace_id": 42,
  "payload_callstack": ["0x7FF..."],
  "payload_stack_trace": [{"addr": "0x...", "addr_info": "ntdll.dll+0x1234"}]
}
```

---

## How to Verify What Data You Actually Receive

### Approach A: Log-level capture (no code changes)

```bash
RUST_LOG=debug cargo run -p scheduler 2>&1 | grep -E "Registration|TelemetryBatch|SampleResponse|ExecutionStatus|heartbeat"
```

Shows field values but not telemetry payloads.

### Approach B: Dump raw proto messages to files (recommended)

Add diagnostic in `orchestrator.rs::handle_worker_message()`:

```rust
// At top of handle_worker_message():
use crate::automutate::common::worker_message::Payload;

let variant = match &msg.payload {
    Some(Payload::Registration(_)) => "registration",
    Some(Payload::Status(_)) => "status",
    Some(Payload::Telemetry(batch)) => {
        // Dump each telemetry event's metadata + event_type
        for (i, ev) in batch.events.iter().enumerate() {
            let dump = serde_json::json!({
                "event_type": ev.event_type,
                "timestamp": ev.timestamp,
                "job_id": ev.job_id,
                "metadata": ev.metadata,
                "payload_len": ev.payload.len(),
                "payload_utf8_preview": String::from_utf8_lossy(&ev.payload[..ev.payload.len().min(2000)]),
                "has_typed_event": ev.typed_event.is_some(),
            });
            let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
            let _ = std::fs::create_dir_all("debug_messages");
            let _ = std::fs::write(
                format!("debug_messages/{}_telemetry_event_{}.json", ts, i),
                serde_json::to_string_pretty(&dump).unwrap_or_default(),
            );
        }
        "telemetry_batch"
    }
    Some(Payload::SampleResponse(resp)) => {
        let dump = serde_json::json!({
            "run_id": resp.run_id,
            "job_id": resp.job_id,
            "exit_code": resp.exit_code,
            "success": resp.success,
            "detected": resp.detected,
            "output": resp.output,
            "error": resp.error,
            "telemetry_ids": resp.telemetry_ids,
        });
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let _ = std::fs::create_dir_all("debug_messages");
        let _ = std::fs::write(
            format!("debug_messages/{}_sample_response.json", ts),
            serde_json::to_string_pretty(&dump).unwrap_or_default(),
        );
        "sample_response"
    }
    Some(Payload::ExecutionStatus(es)) => {
        let dump = serde_json::json!({
            "worker_id": es.worker_id,
            "job_id": es.job_id,
            "run_id": es.run_id,
            "pid": es.pid,
            "elapsed_seconds": es.elapsed_seconds,
            "process_alive": es.process_alive,
            "telemetry_events_count": es.telemetry_events_count,
            "event_type": es.event_type,
            "cpu_percent": es.cpu_percent,
            "memory_mb": es.memory_mb,
            "details": es.details,
        });
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let _ = std::fs::create_dir_all("debug_messages");
        let _ = std::fs::write(
            format!("debug_messages/{}_exec_status.json", ts),
            serde_json::to_string_pretty(&dump).unwrap_or_default(),
        );
        "exec_status"
    }
    Some(Payload::Ack(_)) => "ack",
    None => "empty",
};
info!("[Orchestrator] Received {} from {}", variant, target_id);
```

Run one execution cycle, then inspect `debug_messages/` for exact payloads.

### Approach C: Intercept at ES bulk indexing point

Replace ES `client.index(...)` calls with file writes to see the exact document shape:

```rust
// In orchestrator.rs inline index_telemetry(), before the ES call:
let _ = std::fs::create_dir_all("debug_es_docs");
let _ = std::fs::write(
    format!("debug_es_docs/telemetry_{}.json", chrono::Utc::now().timestamp_millis()),
    serde_json::to_string_pretty(&doc).unwrap_or_default(),
);
```

---

## What Needs Fixing Before ES Schema Is Implementable

| Priority | Fix | Where | What it enables |
|---|---|---|---|
| **1** | **Revive payload flattening** in telemetry indexer | Orchestrator telemetry handler (port logic from `storage/elasticsearch.rs`) | All `payload_*` fields in ES — `payload_api_name`, `payload_func`, `payload_provider`, `payload_event_id` — the entire foundation for token extraction |
| **2** | **Add correlation keys** to telemetry docs | Orchestrator telemetry handler (use `RunEnvelope` lookup via `active_runs` map) | `run_id`, `round_id`, `vm_id` on every telemetry doc — required for token extraction scoped to a run |
| **3** | **Derive `detected` from exit_code** | Controller-side: `exit_code == -2` → `detected = true` | Correct `detected` field (worker always sends `false`) |
| **4** | **Index round summaries** | Orchestrator `RoundCompleted` handler (replace TODO at line 263) | `rounds-*` index — per-round modules, mutations, detected, evasion_score |
| **5** | **Index run results from SampleResponse** | Orchestrator SampleResponse handler or VMExecutor result path | `runs-*` with exit_code, detected, elapsed, run_type, round_id |
| **6** | **Update job status** in ES on completion | Orchestrator `JobCompleted` handler (replace TODO at line 281) | `jobs-*` transitions: `"queued"` → `"running"` → `"completed"` |
| **7** | **Create `tokens-*` index template + TokenExtractor** | New `triage` module per FEEDBACK-LOOP-PLAN | Feedback loop computation index |

Fixes 1–3 are **hard prerequisites** for the feedback loop. Without payload flattening, there's nothing to extract tokens from. Without correlation keys, you can't scope telemetry to a specific run. Without correct `detected`, the scorer's lift computation is meaningless.

---

## Dead / Unused Code Audit (Controller Codebase)

### 1. Orphaned ES Storage Module — `scheduler/src/storage/elasticsearch.rs` (437 lines)

**Status:** Module declared in `storage/mod.rs` line 2, but **never imported or used** anywhere. All functions are unreachable dead code.

**Fatal compile errors if enabled:**
- References `crate::job::Job` — module doesn't exist
- References `crate::round::RoundSummary` — module doesn't exist
- References `self.controller_ip` — not a field on any active struct

| Lines | Function | What it does | Why it matters |
|---|---|---|---|
| 11–186 | `index_telemetry_batch()` | **Richest telemetry indexer** — parses `payload` JSON, handles `typed_event` variants (Trace→`seq/file/line/func/ts_us`, Coverage→`total_bbs/bb_ids/hit_counts/bitmap_b64`, Checkpoint→`checkpoint_name/ts_us`), prefixes payload fields with `payload_`, converts pointer/address values to hex strings, handles numbers > i64::MAX | This is the **intended design** for telemetry indexing — exactly what the active code is missing |
| 188–262 | `store_run_result()` | Indexes `StatusReport` to `runs-*` with dual-URL fallback (controller IP then localhost) | Has more fields than active version but still missing `detected` |
| 264–291 | `try_index_to_es()` | Helper for ES URL fallback | Utility |
| 293–334 | `create_jobs_index_template()` | Creates `jobs-*` index template with proper mappings | **No active equivalent** — index templates are never created |
| 336–373 | `create_rounds_index_template()` | Creates `rounds-*` index template | **No active equivalent** |
| 375–403 | `index_job()` | Indexes job metadata | Uses dead `crate::job::Job` type |
| 405–436 | `index_round()` | Indexes round summaries | Matches current `RoundSummary` struct shape |

### 2. Three Duplicate Telemetry Indexers (Comparison)

The codebase has **three separate telemetry indexing implementations** with different field coverage:

| Implementation | Location | Status | Fields indexed | Payload extraction |
|---|---|---|---|---|
| **Active (api)** | `api/mod.rs:62–94` | Compiled, called | 4 fields: `event_type`, `timestamp`, `job_id`, `metadata` | **None** — payload bytes discarded |
| **Active (inline)** | `orchestrator.rs:451–484` | Compiled, called | Same 4 fields (copy-paste of above) | **None** |
| **Dead (storage)** | `storage/elasticsearch.rs:11–186` | Not compiled | ~15+ fields per event type, all payload fields with `payload_` prefix, typed_event extraction | **Full** — the only implementation that actually parses payloads |

**Impact:** The only code that can produce the `payload_func`, `payload_api_name`, `payload_provider`, `payload_event_id` fields needed for token extraction is the dead one.

### 3. Silent Bug — `compare_runs()` Reads Field Never Written

| Path | File | Line | What happens |
|---|---|---|---|
| **Write** | `api/mod.rs` | 135–165 | `store_run_result()` indexes 9 fields — `detected` is **NOT** among them |
| **Read** | `api/job.rs` | 480 | `compare_runs()` reads `source["detected"].as_bool().unwrap_or(false)` |

**Result:** `compare_runs()` always sees `detected = false` for every run, making differential protocol queries return garbage. The two-run differential comparison (Section 5 of CLAUDE.md) cannot work.

### 4. TODO Stubs in Active Code (21 instances)

#### Critical (block ES schema / feedback loop)

| File | Line | TODO | Impact |
|---|---|---|---|
| `orchestrator.rs` | 263 | `// TODO: Index round to ES` | `rounds-*` never written — all round data lost |
| `orchestrator.rs` | 281 | `// TODO: Update job status in ES` | Jobs forever show `"queued"` |
| `job_worker.rs` | 276 | `mutations: vec![]` — TODO integrate selector | **Every round dispatched with empty mutations** |
| `job_worker.rs` | 512 | `// TODO: Report to mutation selector for feedback` | Feedback loop never closes |

#### High

| File | Line | TODO |
|---|---|---|
| `api/job.rs` | 417 | `// TODO: GetRound implementation is incomplete` |
| `api/job.rs` | 422–426 | Multiple field parsing TODOs (mutations, runs, behavior_match all return empty/null) |
| `api/artifact.rs` | 71 | `// TODO Index artifact metadata to Elasticsearch` |
| `dispatch/vm_executor.rs` | 252 | `// TODO use result.success (evaded?)` — outcome detection unused |

#### Medium

| File | Line | TODO |
|---|---|---|
| `api/utility.rs` | 27–30 | `submit_triage` stub |
| `api/utility.rs` | 50 | Index triage to ES |
| `api/utility.rs` | 67–71 | `query_results` stub |
| `api/utility.rs` | 78 | ES query filters unimplemented |
| `api/worker.rs` | 384–385 | `total_rounds_completed: 0` — never tracked |
| `dispatch/job_worker.rs` | 456 | Round ID routing unclear |
| `dispatch/vm_executor.rs` | 128 | VM cleanup after job |

### 5. Standalone Sub-Crates (Disconnected from Scheduler)

| Sub-crate | Path | Lines | Status | What exists | What's missing |
|---|---|---|---|---|---|
| **selector** | `controller/selector/` | ~344 | **Working but disconnected** | Epsilon-greedy gRPC service, mutation pool, outcome tracking, success rate updates | No ES integration, no real feedback, `filter_avoid_features()` only does string matching, never called by scheduler |
| **triage-engine** | `controller/triage-engine/` | ~67 (lib) + ~80 (main) | **Pure stub** | Empty `analyze()`, TODO comments for 5-step pipeline (fetch→extract→classify→hypothesize→update) | Everything — surrogate classifier, hypothesis generation, feature extraction, ES queries |
| **differential-analyzer** | `controller/differential-analyzer/` | ~34 | **Pure stub** | `analyze()` returns empty `Vec<DifferentialResult>` | All analysis logic |
| **rule-manager** | `controller/rule-manager/` | ~34 | **Pure stub** | `export_sigma()` returns hardcoded string, `import_from_elastic()` returns `Ok(())` | All real logic |

### 6. Architecture Mismatch Summary

```
WHAT'S COMPILED AND RUNNING (incomplete):

  api/mod.rs ──────────────── 104 lines, 4-field telemetry, no payload
  orchestrator.rs (inline) ── copy-paste of above, same 4 fields
  store_run_result ─────────── missing "detected", "round_id", "run_type"
  index_job ────────────────── hardcoded status:"queued", never updated

WHAT'S DEAD BUT SOPHISTICATED (437 lines, not compiled):

  storage/elasticsearch.rs
  ├── index_telemetry_batch ─── payload extraction, typed_event handling
  ├── store_run_result ──────── dual-URL fallback
  ├── create_jobs_index_template ─── proper ES mappings
  ├── create_rounds_index_template ── proper ES mappings
  ├── index_job ─────────────── uses dead types
  └── index_round ───────────── matches RoundSummary shape

WHAT'S STANDALONE BUT NEVER CALLED:

  selector/ ──── epsilon-greedy (works, disconnected)
  triage-engine/ ── pure stub
  differential-analyzer/ ── pure stub
  rule-manager/ ── pure stub
```

### 7. Recommended Revival Path

1. **Fix 3 compile errors** in `storage/elasticsearch.rs`:
   - Replace `crate::job::Job` → `crate::types::JobSession`
   - Replace `crate::round::RoundSummary` → `crate::types::RoundSummary`
   - Replace `self.controller_ip` → constructor parameter or config field
2. **Wire module into `lib.rs`** — add `pub mod storage;`
3. **Replace active minimal indexers** (`api/mod.rs:62–94`, `orchestrator.rs:451–484`) with calls to the revived storage module
4. **Add correlation keys** (`run_id`, `round_id`, `vm_id`) to telemetry documents — use `RunEnvelope` lookup from `active_runs` map
5. **Implement the two critical TODOs** — round indexing (orchestrator:263) and job status update (orchestrator:281)
6. **Add `detected` field** to run documents — derive from `exit_code == -2`
7. **Connect selector** to scheduler's `produce_round()` instead of hardcoding empty mutations