# Storage Module — Deep Analysis

> **Scope:** `controller/src/storage/` — 9 files, ~1,788 lines of Rust
> **Generated from source code only** (no other `.md` referenced)

---

## 1. Overview

The `storage` module is the **single data-access layer** between the controller and Elasticsearch. Every document that enters or leaves ES — jobs, rounds, runs, telemetry events, artifacts, tokens — passes through this module. It owns:

- **Schema definition** (index templates with typed mappings)
- **Write path** (indexing and updating documents)
- **Read path** (query helpers returning raw `serde_json::Value`)
- **Data normalization** (payload flattening, numeric conversion, timestamp formatting)

The module is intentionally **proto-agnostic on the read side**: queries return raw JSON `Value`, leaving protobuf mapping to the API handlers. On the write side, it accepts typed Rust structs (`JobSession`, `RoundSummary`, `RunOutcome`, `TelemetryData`) and converts them to ES documents internally.

---

## 2. File Inventory

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | 219 | `EsStorage` facade struct + `TelemetryContext` + re-exports |
| `helpers.rs` | 133 | Index naming, timestamps, `update_doc_by_id` with retry, response checking |
| `telemetry.rs` | 188 | Bulk telemetry indexing with payload flattening and typed_event handling |
| `jobs.rs` | 137 | Job document lifecycle (create, started, progress, status+outcome) |
| `runs.rs` | 139 | Run result indexing with detection outcome derivation (2 paths) |
| `rounds.rs` | 167 | Round summary indexing + coverage/evasion score updates |
| `templates.rs` | 321 | Index template bootstrap (5 templates, created in parallel) |
| `artifacts.rs` | 18 | Artifact build metadata indexing |
| `queries.rs` | 466 | All read-side queries (14 query functions + 2 internal helpers) |

**Total: ~1,788 lines**

---

## 3. Per-File Deep Analysis

### 3.1 `mod.rs` — EsStorage Facade (219 lines)

#### Purpose
Central entry point. Wraps a single `Elasticsearch` client and exposes **typed methods** for every storage operation. All other modules in the controller import `EsStorage` (via `Arc<EsStorage>`) and call its methods — they never touch the ES client directly.

#### Key Types

```rust
pub struct TelemetryContext {
    pub run_id: Option<String>,
    pub round_id: Option<String>,
    pub vm_id: String,
}
```
Correlation context attached to every telemetry document. `run_id` and `round_id` are optional because telemetry can arrive before a run is fully assigned (e.g., during VM registration).

```rust
pub struct EsStorage {
    client: Elasticsearch,
}
```
Single field — the ES client. `Clone` is derived (the `Elasticsearch` client is internally `Arc`-based), enabling cheap sharing via `Arc<EsStorage>`.

#### Method Categories

| Category | Methods | Delegates To |
|----------|---------|-------------|
| **Telemetry** | `index_telemetry_batch` | `telemetry::index_telemetry_batch` |
| **Jobs** | `index_job`, `update_job_started`, `update_job_progress`, `update_job_status` | `jobs::*` |
| **Rounds** | `index_round`, `update_round_coverage`, `update_round_evasion_score` | `rounds::*` |
| **Runs** | `index_run_result`, `index_run_status` | `runs::*` |
| **Artifacts** | `index_artifact` | `artifacts::index_artifact` |
| **Queries** | `query_job`, `query_rounds`, `query_round`, `query_runs_by_ids`, `update_job_field`, `query_trace_lines`, `query_trace_content`, `query_analysis_results`, `query_api_telemetry`, `query_checkpoint_events`, `query_token_sets`, `query_token_set_by_round_id` | `queries::*` |
| **Tokens** | `index_token_set` | Inline (direct ES index call with `Refresh::WaitFor`) |
| **Bootstrap** | `ensure_templates` | `templates::ensure_templates` |

**Re-exports:** `RoundIndexParams` and `RunIndexParams` are re-exported from `mod.rs` so callers can construct parameter structs without importing submodules.

#### Architecture Notes
- Pure **facade pattern**: every method is a 1-line delegation to the corresponding submodule function, passing `&self.client` as the first argument.
- Exception: `index_token_set` is implemented inline (6 lines) rather than delegated — likely because it's simple enough to not warrant its own function in `queries.rs`.
- All write methods return `anyhow::Result<()>`. All read methods return `Option<Value>` or `Vec<Value>` and silently swallow errors (returning `None`/empty).

---

### 3.2 `helpers.rs` — Shared Utilities (133 lines)

#### Purpose
Eliminates boilerplate that was previously copy-pasted across storage submodules. Provides 7 functions used by every other file in the module.

#### Functions

**Index Naming:**
```
es_index_name("jobs")      → "jobs-2026.03"       (monthly)
es_index_name_daily("telemetry") → "telemetry-2026.03.04" (daily)
```
Monthly indices for jobs/rounds/runs/tokens/artifacts; daily indices for telemetry (high volume).

**Timestamp Helpers:**
- `system_time_to_rfc3339(SystemTime)` — used for `created_at`, `completed_at` from Rust `SystemTime`
- `now_rfc3339()` — used for `updated_at`, `indexed_at`, `timestamp` fields
- `now_unix_secs()` — returns `i64`, used for TTL/age comparisons

**Document Update (`update_doc_by_id`):**
The most complex helper (50 lines). Implements a **find-then-update** pattern:
1. Calls `queries::find_index()` to resolve the concrete index name and ES `_id` (required because index patterns like `jobs-*` span multiple monthly indices)
2. Sends an ES Update request with `Refresh::WaitFor`
3. **Retry logic**: on HTTP 409 (version conflict), retries up to 3 times with exponential backoff (50ms, 100ms, 150ms)
4. Returns `Ok(true)` on success, `Ok(false)` if document not found, or `Err` on transport failure

Why this is needed: ES documents are distributed across monthly indices (`jobs-2026.01`, `jobs-2026.02`, etc.). A simple `update by ID` requires knowing which concrete index holds the document. The `find_index` lookup resolves this.

**Response Checking (`check_index_response`):**
Validates that an ES index response has a success status code. Returns `Err(anyhow)` with the response body on failure. Used after every `es.index()` call.

**Optional Field Insertion (`insert_optional_field`):**
Conditionally inserts a `&str` field into a JSON object — no-op if `None`. Uses `let-chain` syntax (`if let Some(v) = value && let Some(obj) = doc.as_object_mut()`).

---

### 3.3 `telemetry.rs` — Telemetry Indexing (188 lines)

#### Purpose
Indexes batches of `TelemetryData` protobuf messages into daily ES indices (`telemetry-YYYY.MM.DD`). Handles three distinct data shapes: raw JSON payloads, typed proto events, and correlation context enrichment.

#### Index Pattern
`telemetry-YYYY.MM.DD` (daily) — high-volume index (thousands of events per run)

#### Processing Pipeline

**Step 1: Payload Extraction**
```
event.payload (bytes) → serde_json::from_slice → payload_fields (Map)
```
Each `TelemetryData` carries a `payload` bytes field containing JSON. This is deserialized into a flat key-value map.

**Step 2: Typed Event Handling**
If the protobuf `typed_event` oneof is present, its fields are merged into `payload_fields`:

| TypedEvent Variant | Fields Extracted |
|-------------------|-----------------|
| `Trace` | `seq`, `file`, `line`, `func`, `ts_us` |
| `Coverage` | `total_bbs`, `bb_ids`, `hit_counts`, `bitmap_size`, `bitmap_b64` (base64-encoded bitmap) |
| `Checkpoint` | `checkpoint_name`, `ts_us` |

Coverage bitmaps are base64-encoded before storage to avoid ES field explosion from large arrays.

**Step 3: Document Construction**
Core fields from the event:
```json
{
    "job_id": "...",
    "event_type": "dll|kernel|etw|trace_log|checkpoint",
    "source": "worker",
    "timestamp": "...",
    "metadata": { ... },
    "indexed_at": "RFC3339",
    "vm_id": "..."
}
```
Plus optional `run_id` and `round_id` from `TelemetryContext`.

**Step 4: Smart Numeric Conversion**
Payload fields are merged with intelligent type handling:

| Condition | Conversion |
|-----------|-----------|
| Pointer-like field name (address, pointer, stack, base, limit, rva, offset) | Number → `"0x{:X}"` hex string |
| Value > `i64::MAX` | Number → `"0x{:X}"` hex string (prevents ES long overflow) |
| Numeric field name (addr, port, pid, tid, size) with unparseable string | → `null` |
| Normal number | Preserved as-is |

All payload fields are prefixed with `payload_` when merged into the document (e.g., `func` → `payload_func`).

**Step 5: Bulk Indexing**
Uses ES Bulk API for efficiency. After sending:
- Parses bulk response to count individual item successes/failures
- Logs first 3 failures for debugging
- Never returns an error (graceful degradation) — only logs warnings

---

### 3.4 `jobs.rs` — Job Lifecycle (137 lines)

#### Purpose
Manages the lifecycle of job documents in `jobs-YYYY.MM` indices. A job document is created once and updated multiple times as it progresses through states.

#### Index Pattern
`jobs-YYYY.MM` (monthly), document ID = `job_id`

#### Functions

**`index_job(es, job: &JobSession)`** — Creates the initial job document on submission.

Document fields:
```json
{
    "job_id": "job-20260304-...",
    "status": "queued",
    "template_name": "modular_template",
    "source_file": "/path/to/payload.bin",
    "trace_mode": "lines",
    "encoding": "xor",
    "priority": 0,
    "current_round": 0,
    "max_rounds": 10,
    "stop_on_evasion": true,
    "stop_on_detection": false,
    "target_os": "win10",
    "required_capabilities": ["defender"],
    "modules": {
        "carrier": "alloc_rw_rx",
        "decoder": "xor",
        "antiemulation": "none",
        ...
    },
    "sc_checkpoint_count": 5,
    "created_at": "RFC3339",
    "updated_at": "RFC3339"
}
```
Uses `IndexParts::IndexId` to set the ES document `_id` to `job_id` for deterministic lookups.

**`update_job_started(es, job_id)`** — Sets `status: "running"` and `started_at`.

**`update_job_progress(es, job_id, current_round)`** — Increments `current_round` counter after each round completes.

**`update_job_status(es, job_id, status, outcome)`** — Terminal update. Sets `status` to `"completed"`, `"stopped"`, or `"failed"`, plus `completed_at`. If `outcome` is provided:

| JobOutcome Variant | Outcome Fields |
|-------------------|---------------|
| `Completed { rounds_completed }` | `total_rounds`, `detection_outcome: "completed"` |
| `Stopped { reason }` | `reason` |
| `Failed { error }` | `error` |

All updates use `helpers::update_doc_by_id` with the find-then-update pattern to resolve the concrete monthly index.

#### Job State Machine
```
queued → running → completed | stopped | failed
```

---

### 3.5 `runs.rs` — Run Indexing (139 lines)

#### Purpose
Indexes individual run results into `runs-YYYY.MM`. Provides two indexing paths for different data sources.

#### Index Pattern
`runs-YYYY.MM` (monthly)

#### Parameter Struct

```rust
pub struct RunIndexParams<'a> {
    pub job_id: &'a str,
    pub round_id: &'a str,
    pub run_id: &'a str,
    pub run_type: &'a str,     // "baseline" | "instrumented" | "dryrun"
    pub outcome: &'a RunOutcome,
    pub mutations: &'a [String],
    pub vm_id: &'a str,
}
```

#### Path 1: `index_run_result` (Primary)
Called from the `RoundCompleted` event in the Orchestrator. Has full context:

- **Detection derivation**: First checks `DetectionVerdict::from_verdict_str()` (structured verdict from worker agent). Falls back to `outcome.detected` boolean, then to `exit_code != 0` for legacy workers.
- **Trace mode derivation**: `trace_mode_from_run_type()` maps: `baseline → off`, `instrumented → lines`, `dryrun → off`.
- **Error handling**: If `outcome.error` is `Some`, adds structured error object with `class: "execution_error"`.
- Document ID = `run_id` (deterministic).
- Uses `Refresh::WaitFor` for immediate visibility.

Document fields:
```json
{
    "run_id": "...", "job_id": "...", "round_id": "...",
    "run_type": "baseline",
    "mutation_chain": ["mod.carrier.alloc_rw_rx"],
    "mutations": ["mod.carrier.alloc_rw_rx"],
    "vm_id": "win10-vm-1", "worker_id": "win10-vm-1",
    "status": "completed",
    "detected": true,
    "exit_code": 1,
    "success": false,
    "elapsed_ms": 3200,
    "detection_verdict": "MUTATION_FAILED",
    "last_checkpoint": "sc_decode_complete",
    "trace_mode": "off",
    "finished_at": "RFC3339",
    "timestamp": "RFC3339"
}
```

#### Path 2: `index_run_status` (Legacy)
Called from the `StatusReport` gRPC handler. Has worker metadata but no structured exit_code/detected:

```json
{
    "run_id": "...", "job_id": "...",
    "worker_id": "...", "worker_ip": "10.0.0.5",
    "artifact_name": "loader.exe",
    "pid": 1234,
    "status": "started|completed|error",
    "elapsed_seconds": 12.5,
    "telemetry_events_count": 847,
    "details": "...",
    "timestamp": "RFC3339"
}
```
Uses auto-generated ES `_id` (not deterministic). No `Refresh::WaitFor` (fire-and-forget).

---

### 3.6 `rounds.rs` — Round Indexing (167 lines)

#### Purpose
Indexes round summaries and provides two asynchronous update operations for coverage and evasion score data that arrives after the initial round document is created.

#### Index Pattern
`rounds-YYYY.MM` (monthly), document ID = `{job_id}/{round_id}` (composite)

#### Parameter Struct

```rust
pub struct RoundIndexParams<'a> {
    pub job_id: &'a str,
    pub summary: &'a RoundSummary,
    pub mutation_specs: &'a [MutationSpec],
    pub baseline_run_id: &'a str,
    pub instrumented_run_id: &'a str,
    pub started_at: Option<&'a str>,
    pub modules: Option<&'a ModuleSelectionSpec>,
    pub assembled_source: Option<&'a str>,  // Pre-instrumentation C source for line trace resolution
    pub dry_run_exit_code: Option<i32>,
    pub has_dryrun: bool,
    pub dryrun_run_id: Option<&'a str>,
}
```

#### Functions

**`index_round(es, params)`** — Creates the initial round document.

Key document fields:
- `mutation_recipe`: Array of `{ id, params }` objects (full mutation specs with parameters)
- `modules`: 7-slot module selection (carrier, decoder, antiemulation, etc.)
- `assembled_source`: Full C source text stored as non-indexed text field — used later for line trace resolution
- `differential_category`: String from `DifferentialCategory::as_str()` (e.g., `"real_detection"`, `"evasion"`, `"instrumentation_artifact"`)
- Dryrun fields: `dry_run_exit_code`, `has_dryrun`, `dryrun_run_id`

**`update_round_coverage(es, job_id, round_id, coverage)`** — Asynchronous update after trace data is processed. Adds:
```json
{
    "coverage_total_lines": 342,
    "coverage_executable_lines": 187,
    "coverage_executed_lines": 143,
    "coverage_percent": 76.5,
    "cutoff_line": 298,
    "cutoff_func": "execute_payload",
    "function_coverage": [
        { "name": "main", "total": 45, "executed": 42, "percent": 93.3 },
        { "name": "decode_payload", "total": 28, "executed": 28, "percent": 100.0 }
    ]
}
```
Coverage percentages are rounded to 1 decimal place (`(x * 10.0).round() / 10.0`).

**`update_round_evasion_score(es, job_id, round_id, blended_score)`** — Updates the evasion score with the blended value (70% coverage + 30% time). Rounds to 3 decimal places. Sets `evasion_score_blended: true` flag to distinguish from the initial non-blended score.

Both update functions use `update_doc_by_id` to find the document across monthly indices.

---

### 3.7 `templates.rs` — Index Template Bootstrap (321 lines)

#### Purpose
Creates/updates Elasticsearch index templates on controller startup. Templates define field mappings and settings for all 5 index families. Uses `_meta.version` for tracking schema evolution.

#### Bootstrap Flow
`ensure_templates()` runs all 5 template creation functions in parallel via `tokio::join!`. Template creation failures are **logged as warnings but do not crash** the controller — this allows the system to operate even if ES is temporarily unavailable.

#### Template Definitions

**`jobs-template`** (version 3)
- Pattern: `jobs-*`
- Settings: 1 shard, 0 replicas
- 17 mapped fields including nested `modules` (7 keyword fields) and `outcome` (4 fields)
- All dates use `date` type for range queries

**`rounds-template`** (version 6)
- Pattern: `rounds-*`
- 16+ mapped fields
- `mutation_recipe.params`: `{ "type": "object", "enabled": false }` — stored but not indexed (arbitrary structure)
- `assembled_source`: `{ "type": "text", "index": false }` — stored for retrieval but not searchable (large text)
- `evasion_score`: `float` type

**`runs-template`** (version 4)
- Pattern: `runs-*`
- 21 mapped fields
- `worker_ip`: `ip` type (ES IP address type for CIDR queries)
- `error`: nested object with `class` (keyword) and `message` (text)
- `detection_verdict`: keyword (for aggregations)

**`telemetry-template`** (version 3)
- Pattern: `telemetry-*`
- Uses **dynamic templates** for `payload_*` fields:
  - String payload fields → `keyword` (for exact match)
  - Long payload fields → `long` (for numeric queries)
- 12 explicitly mapped fields
- `metadata`: `{ "type": "object", "enabled": false }` — opaque storage
- `payload_bitmap_b64`: `{ "type": "keyword", "index": false }` — stored but not searchable

**`tokens-template`** (version 1)
- Pattern: `tokens-*`
- 10 mapped fields
- `tokens`: keyword array (for terms aggregation)
- `token_count`: integer
- Nested `modules` (same 7-slot structure as jobs/rounds)

#### Schema Design Principles
- **Monthly indices** for structured data (jobs, rounds, runs, tokens) — manageable index sizes
- **Daily indices** for telemetry — high volume, time-bounded retention
- **0 replicas** — lab environment, no HA needed
- **1 shard** — single-node ES deployment assumed
- **`Refresh::WaitFor`** on writes — ensures immediate read-after-write consistency (critical for the round/run lifecycle)
- **Dynamic templates** for telemetry — handles arbitrary payload fields from different ETW providers without mapping explosion

---

### 3.8 `artifacts.rs` — Artifact Indexing (18 lines)

#### Purpose
Minimal module — indexes artifact build metadata documents to `artifacts-YYYY.MM`. The document is a pre-built `serde_json::Value` constructed by the API artifact handler (not shaped here).

#### Index Pattern
`artifacts-YYYY.MM` (monthly), auto-generated ES `_id`

Uses `Refresh::WaitFor` for immediate visibility.

---

### 3.9 `queries.rs` — Read-Side Queries (466 lines)

#### Purpose
All read-side ES queries live here. Returns raw `serde_json::Value` — the API handlers perform protobuf conversion. This separation keeps the storage layer proto-agnostic.

#### Query Functions (14 total)

**Job Queries:**

| Function | Index | Query | Returns |
|----------|-------|-------|---------|
| `query_job(job_id)` | `jobs-*` | `term: job_id` | `Option<Value>` — single job doc |
| `update_job_field(job_id, field, value)` | `jobs-*` | Update via `update_doc_by_id` | `Result<()>` |

**Round Queries:**

| Function | Index | Query | Returns |
|----------|-------|-------|---------|
| `query_rounds(job_id)` | `rounds-*` | `term: job_id`, sort `round_number asc`, limit 100 | `Vec<Value>` |
| `query_round(job_id, round_id)` | `rounds-*` | `bool: must [term job_id, term round_id]` | `Option<Value>` |

**Run Queries:**

| Function | Index | Query | Returns |
|----------|-------|-------|---------|
| `query_runs_by_ids(run_ids)` | `runs-*` | `terms: run_id`, size = `run_ids.len()` | `Vec<Value>` |

**Trace Queries:**

| Function | Index | Query | Returns |
|----------|-------|-------|---------|
| `query_trace_lines(run_id, last_n)` | `telemetry-*` | `match_phrase: run_id + event_type=trace_log` | `(Vec<Value>, u64)` — (lines, total_count) |
| `query_trace_content(run_id)` | `telemetry-*` | Same as above, returns raw JSONL string | `Option<String>` |

**`query_trace_lines` detailed flow:**
1. Fetches the single `trace_log` document for a run (contains JSONL blob in `payload_content`)
2. Uses `match_phrase` instead of `term` — works on indices created before keyword templates were applied (text fields with hyphens fail `term` queries)
3. Parses JSONL content line-by-line into `Value` objects
4. Sorts by `seq` descending (most recent first)
5. Truncates to `last_n` entries (defaults to 50 if 0)
6. Remaps field names: `seq → payload_seq`, `file → payload_file`, etc.
7. Returns both the page of results and total line count

**Analysis Queries:**

| Function | Index | Query | Returns |
|----------|-------|-------|---------|
| `query_analysis_results(job_ids, date_from, date_to)` | `runs-*` | Optional `terms: job_id` + optional `range: timestamp`, sorted by `timestamp desc`, limit 100 | `Vec<Value>` |

Dynamic filter construction: only adds filters that are non-empty. Falls back to `match_all` if no filters.

**Triage/Token Queries:**

| Function | Index | Query | Returns |
|----------|-------|-------|---------|
| `query_api_telemetry(run_id)` | `telemetry-*` | `match_phrase: run_id`, `must_not: event_type=trace_log`, sorted by `payload_id asc`, limit 5000 | `Vec<Value>` |
| `query_checkpoint_events(run_id)` | `telemetry-*` | `filter: [run_id, event_type=checkpoint]`, sorted by `payload_ts_us asc`, limit 500 | `Vec<Value>` |
| `query_token_sets(job_id)` | `tokens-*` | `term: job_id`, sorted by `timestamp asc`, limit 500 | `Vec<Value>` |
| `query_token_set_by_round_id(job_id, round_id)` | `tokens-*` | `bool: must [term job_id, term round_id]` | `Option<Value>` |

`query_api_telemetry` uses `_source` filtering to only return the fields needed for token extraction:
```
payload_func, payload_id, payload_protect, payload_size, payload_alloc_type,
payload_free_type, payload_event, payload_etw_provider_name, payload_event_id,
payload_image, event_type, payload_type
```

#### Internal Helpers

**`extract_sources(response)`** — Unwraps an ES search response into a `Vec<Value>` of `_source` documents. Handles both transport errors and JSON parse errors gracefully (returns empty vec).

**`find_index(es, pattern, id_field, id_value)`** — Resolves a field-based ID to the concrete `(index_name, _id)` pair needed for ES updates. Uses `_source: false` to minimize transfer. Called by `helpers::update_doc_by_id`.

---

## 4. Architecture

### 4.1 Module Dependency Graph

```
                   ┌───────────────────────────────────────────┐
                   │          EsStorage (mod.rs)                │
                   │  Facade: wraps Elasticsearch client        │
                   │  Exposes typed methods per index           │
                   └──────────────┬────────────────────────────┘
                                  │ delegates to
         ┌──────────┬─────────────┼────────────┬────────────┬──────────┐
         ▼          ▼             ▼            ▼            ▼          ▼
    ┌─────────┐ ┌────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────┐
    │ jobs.rs │ │runs.rs │ │rounds.rs │ │telemetry │ │artifacts │ │queries  │
    │         │ │        │ │          │ │  .rs     │ │  .rs     │ │  .rs    │
    │ Create  │ │ 2-path │ │ Index +  │ │ Bulk +   │ │ Simple   │ │ 14 read │
    │ + 3     │ │ index  │ │ 2 update │ │ flatten  │ │ index    │ │ queries │
    │ updates │ │        │ │          │ │ + typed  │ │          │ │         │
    └────┬────┘ └───┬────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬────┘
         │          │           │             │            │            │
         └──────────┴───────────┴──────┬──────┴────────────┘            │
                                       │                                │
                               ┌───────▼────────┐              ┌───────▼────────┐
                               │  helpers.rs    │◄─────────────│  helpers.rs    │
                               │ Index naming   │  find_index  │  (also used    │
                               │ Timestamps     │◄─────────────│   by queries)  │
                               │ update_doc     │              │                │
                               │ check_response │              │                │
                               └────────────────┘              └────────────────┘
```

Internal dependency: `helpers.rs` calls `queries::find_index()` for the update_doc_by_id pattern. This is the only cross-module dependency within storage.

### 4.2 Index Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Elasticsearch Cluster                         │
│                                                                  │
│  ┌──────────────────────┐  ┌──────────────────────┐             │
│  │  jobs-2026.03        │  │  rounds-2026.03       │  Monthly   │
│  │  (doc_id = job_id)   │  │  (doc_id = job/round) │  indices   │
│  └──────────────────────┘  └──────────────────────┘             │
│                                                                  │
│  ┌──────────────────────┐  ┌──────────────────────┐             │
│  │  runs-2026.03        │  │  tokens-2026.03       │  Monthly   │
│  │  (doc_id = run_id)   │  │  (doc_id = auto)      │  indices   │
│  └──────────────────────┘  └──────────────────────┘             │
│                                                                  │
│  ┌──────────────────────┐  ┌──────────────────────┐             │
│  │  artifacts-2026.03   │  │  telemetry-2026.03.04 │  Monthly / │
│  │  (doc_id = auto)     │  │  (doc_id = auto)      │  Daily     │
│  └──────────────────────┘  └──────────────────────┘             │
└─────────────────────────────────────────────────────────────────┘
```

### 4.3 Write vs Read Path Separation

| Aspect | Write Path | Read Path |
|--------|-----------|-----------|
| **Location** | `jobs.rs`, `runs.rs`, `rounds.rs`, `telemetry.rs`, `artifacts.rs` | `queries.rs` |
| **Input types** | Typed Rust structs (`JobSession`, `RunOutcome`, `RoundSummary`, `TelemetryData`) | String IDs (`job_id`, `run_id`, `round_id`) |
| **Output types** | `anyhow::Result<()>` | `Option<Value>` / `Vec<Value>` |
| **Error handling** | Returns errors to caller (caller decides severity) | Swallows errors, returns empty (graceful degradation) |
| **Refresh** | `Refresh::WaitFor` (most paths) | N/A |
| **Proto dependency** | Yes (imports `TelemetryData`, `StatusReport`) | No (returns raw `Value`) |

### 4.4 Data Flow Through the System

```
Job Submission        Round Execution              Post-Round
─────────────        ─────────────────            ───────────
schedule_job()        VM runs artifact             Orchestrator processes
    │                     │                            │
    ▼                     ▼                            ▼
index_job()          stream_telemetry()         index_round()
status:"queued"      → index_telemetry_batch()  index_run_result() (×2-3)
    │                     │                            │
    ▼                     ▼                            ▼
update_job_started() Telemetry to ES             update_round_coverage()
status:"running"     (daily index)              update_round_evasion_score()
    │                                            update_job_progress()
    │                                                  │
    ▼                                                  ▼
update_job_status()                             index_token_set()
status:"completed"                              (triage extraction)
```

---

## 5. Concurrency Model

### 5.1 Thread Safety
`EsStorage` is `Clone + Send + Sync` (derived). Shared via `Arc<EsStorage>` across:
- All gRPC handler tasks (via `SchedulerService`)
- Orchestrator background task
- JobWorker tasks (indirect, via event channels)

The underlying `Elasticsearch` client is internally `Arc`-based with connection pooling — safe for concurrent access.

### 5.2 Consistency Guarantees
- **`Refresh::WaitFor`** on all primary write paths ensures read-after-write consistency. Critical for the round lifecycle where `index_round` must be visible before `update_round_coverage`.
- **Version conflict retry** in `update_doc_by_id` handles concurrent updates to the same document (e.g., `update_job_progress` racing with `update_job_status`).
- **Fire-and-forget** for `index_run_status` (legacy path) — no `WaitFor`, no retry.

### 5.3 Error Handling Strategy
- **Write-side**: Errors propagated via `anyhow::Result`. Callers (Orchestrator, API handlers) decide whether to retry or log.
- **Read-side**: All errors silently swallowed. `query_*` functions return `None`/`Vec::new()` on failure. This prevents ES transient failures from crashing gRPC handlers.
- **Template bootstrap**: Failures logged as warnings, never crash. Controller can operate with degraded mappings.
- **Telemetry bulk**: Per-item failures counted and logged, but the overall operation returns `Ok(())`.

---

## 6. Role in the Global Project

### 6.1 Position in Architecture
The storage module sits between the **dispatch layer** (producer of experimental data) and the **API/triage layers** (consumers of that data). It is the **single source of truth** for all experimental results.

```
Build → Dispatch → VM Execution → Storage → { API, Triage, UI }
                                     ▲
                              (this module)
```

### 6.2 Data Domains Served

| Consumer | Data Needed | Query Functions Used |
|----------|-------------|---------------------|
| **API: Job handlers** | Job status, rounds, runs | `query_job`, `query_rounds`, `query_round`, `query_runs_by_ids` |
| **API: Trace handlers** | Line trace data | `query_trace_lines`, `query_trace_content` |
| **API: Worker handlers** | Run results for UI | `query_analysis_results` |
| **Triage: Token extractor** | Raw telemetry events | `query_api_telemetry`, `query_checkpoint_events` |
| **Triage: Scoring** | Historical token sets | `query_token_sets`, `query_token_set_by_round_id` |
| **Orchestrator** | Round/run indexing, coverage | `index_round`, `index_run_result`, `update_round_coverage` |
| **JobWorker** | Job progress updates | `update_job_progress` |

### 6.3 Feedback Loop Support
The storage module is the **persistence backbone** for the token-driven feedback loop:
1. **Execution data in**: `index_run_result`, `index_telemetry_batch`, `index_round`
2. **Token extraction reads**: `query_api_telemetry`, `query_checkpoint_events` → triage extractor builds tokens
3. **Token data stored**: `index_token_set`
4. **Token history read**: `query_token_sets` → triage scorer computes lift/confidence
5. **Guidance fed back**: Token scores guide the Selector's mutation choices for the next round

Without this module, the feedback loop has no memory — mutations would be random rather than evidence-driven.

---

## 7. Summary Statistics

| Metric | Value |
|--------|-------|
| Total files | 9 |
| Total lines | ~1,788 |
| ES indices managed | 6 (jobs, rounds, runs, telemetry, artifacts, tokens) |
| Index templates | 5 (no template for artifacts) |
| Write functions | 13 |
| Read functions | 14 |
| Helper functions | 7 |
| Concurrency retries | 3 (update_doc_by_id version conflict) |
| Bulk operations | 1 (telemetry batch) |
