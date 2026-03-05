# UI Layer — Architecture & Module Reference

## Role in the Global Project

The UI layer is the **human-facing control plane** for AutoMutate++. It sits between the operator and the Controller gRPC service, translating experiment configuration into job submissions and rendering results (rounds, traces, tokens, coverage) for analysis.

```
 Operator
    │
    ▼
┌──────────────────────────┐
│  Frontend (vanilla HTML)  │  index.html · source.html
│  Fetch API → JSON/HTTP    │
└──────────┬───────────────┘
           │ :3000
┌──────────▼───────────────┐
│  Backend  (Axum/Rust)     │  REST → gRPC translation
│  + optional ES queries    │
└──────────┬───────────────┘
           │ :50051 gRPC        │ :9200 ES (optional)
┌──────────▼───────────────┐  ┌─▼──────────────┐
│  Controller               │  │  ElasticSearch  │
│  (job_worker, orchestrator│  │  tokens-*, runs │
│   vm_executor, triage)    │  └─────────────────┘
└───────────────────────────┘
```

**Key point:** The backend has no business logic — it is a thin REST-to-gRPC adapter. All scheduling, building, execution, and triage happens in the Controller. The one exception is direct ElasticSearch queries for triage tokens, which bypass gRPC for efficiency.

---

## Directory Structure

```
ui/
├── backend/                            Rust REST API (Axum 0.7)
│   ├── Cargo.toml                      Dependencies
│   ├── build.rs                        Proto compilation (tonic-build)
│   ├── README.md                       API endpoint reference
│   └── src/
│       ├── main.rs                     Server bootstrap, route table, static file serving
│       ├── grpc_client.rs              Pooled gRPC client wrapper (544 LOC)
│       ├── api/
│       │   ├── mod.rs                  Shared error/response types (81 LOC)
│       │   ├── jobs.rs                 Job lifecycle + round details (597 LOC)
│       │   ├── workers.rs              Worker pool management (413 LOC)
│       │   ├── query.rs                ES query + triage submission (152 LOC)
│       │   └── tokens.rs               Triage token endpoints (210 LOC)
│       └── generated/
│           └── mod.rs                  Auto-generated protobuf/gRPC stubs
├── frontend/
│   ├── index.html                      Main dashboard SPA (1939 LOC)
│   └── source.html                     Source viewer with trace overlay (1017 LOC)
└── kibana-dashboards/
    └── edr-dashboard.ndjson            Pre-built Kibana visualizations (4 objects)
```

**Total:** ~4,700 lines across backend + frontend.

---

## Backend (Rust / Axum)

### Tech Stack

| Dependency | Version | Purpose |
|---|---|---|
| axum | 0.7 | HTTP framework (async, extractors) |
| tonic / prost | — | gRPC client (protobuf codegen) |
| tokio | full | Async runtime |
| elasticsearch | 8.5.0-alpha.1 | Direct ES queries for tokens |
| serde / serde_json | — | JSON serialization |
| tracing | — | Structured logging |
| tower-http (cors) | — | CORS middleware (permissive, lab-only) |

### Configuration (environment variables)

| Variable | Default | Description |
|---|---|---|
| `CONTROLLER_ADDR` | `http://10.200.200.1:50051` | Controller gRPC endpoint |
| `LISTEN_ADDR` | `0.0.0.0:3000` | HTTP listen address |
| `FRONTEND_DIR` | `../frontend` | Static file directory |
| `ELASTICSEARCH_URL` | `http://localhost:9200` | ES endpoint (optional — degrades gracefully) |
| `RUST_LOG` | `debug` | Log level filter |

---

### Module: `main.rs` — Server Bootstrap

Initializes the Axum router with all API routes and shared state.

**Shared state:**
- `Arc<ControllerGrpcClient>` — pooled gRPC connection (passed as Axum `State`)
- `Option<Arc<Elasticsearch>>` — optional ES client (passed as Axum `Extension`)

**Route table:**

| Method | Path | Handler | Module |
|---|---|---|---|
| GET | `/health` | Simple ping | main |
| GET | `/api/health` | Controller connectivity check | main |
| POST | `/api/jobs` | Submit new job | jobs |
| GET | `/api/jobs/:id` | Job status | jobs |
| GET | `/api/jobs/:id/progress` | Rounds table with coverage | jobs |
| POST | `/api/jobs/:id/stop` | Stop running job | jobs |
| GET | `/api/jobs/:job_id/rounds/:round_id` | Full round detail | jobs |
| GET | `/api/runs/:run_id/trace` | Execution trace lines | jobs |
| GET | `/api/runs/compare` | Baseline vs instrumented diff | jobs |
| GET | `/api/workers` | Worker list with status counts | workers |
| GET | `/api/workers/metadata` | Enhanced metadata (health, tools) | workers |
| GET | `/api/workers/available` | Filter by OS/capabilities | workers |
| GET | `/api/orchestrator/status` | Aggregated queue metrics | workers |
| POST | `/api/workers/:id/ping` | Ping specific worker | workers |
| POST | `/api/workers/:id/disconnect` | Disconnect with reason | workers |
| POST | `/api/workers/disconnect-all` | Bulk disconnect | workers |
| POST | `/api/query` | Execute ES query | query |
| POST | `/api/triage` | Submit triage result | query |
| GET | `/api/jobs/:job_id/rounds/:round_id/tokens` | Round triage tokens | tokens |
| GET | `/api/tokens/compare` | Token set comparison | tokens |

Static files (frontend HTML/JS/CSS) are served as a fallback via `tower_http::services::ServeDir`.

---

### Module: `grpc_client.rs` — Controller Connection

`ControllerGrpcClient` wraps all Controller gRPC RPCs with:

- **Lazy connection:** First call dials the controller; subsequent calls reuse `RwLock<Option<Client>>`
- **Reconnection:** On error, clears cached client so next call retries
- **Typed wrappers:** Each gRPC method has a corresponding Rust method returning domain types

**RPC groups:**

| Category | Methods |
|---|---|
| Health | `ping()`, `is_healthy()` |
| Jobs | `schedule_job()`, `get_job_status()`, `get_job_progress()`, `stop_job()`, `get_round()` |
| Artifacts | `build_artifact()`, `deploy_artifact()` |
| Runs | `get_trace_lines()`, `compare_runs()`, `compare_tokens()` |
| Workers | `list_workers()`, `get_available_workers()`, `ping_worker()`, `disconnect_worker()`, `disconnect_all_workers()` |
| Query | `query_results()`, `submit_triage()` |

---

### Module: `api/mod.rs` — Shared Types

Defines the response envelope used by all endpoints:

```rust
// Success
{ "data": { ... } }

// Error
{ "error": "message", "code": "ERROR_CODE" }
```

**Error codes:** `NOT_FOUND` (404), `BAD_REQUEST` (400), `SERVICE_UNAVAILABLE` (503), `INTERNAL_ERROR` (500).

---

### Module: `api/jobs.rs` — Job Lifecycle (597 LOC)

The largest API module. Handles the full experiment lifecycle.

**Key request type — `SubmitJobRequest`:**

```json
{
  "modules": {
    "carrier": "change_rw_rx",
    "decoder": "xor",
    "antiemulation": "none",
    "deconditioner": "none",
    "guardrail": "env",
    "virtualprotect": "standard",
    "decoy": "winexec"
  },
  "encoding": "xor",
  "payload_path": "/path/to/shellcode.bin",
  "trace_mode": "lines",
  "max_rounds": 50,
  "selector_type": "coverage",
  "variation_strategy": "mutation_only",
  "variable_categories": ["carrier", "decoder"],
  "mutation_targets": ["ast.string_xor", "llvm.nop_insert"],
  "fixed_mutations": ["binary.rich_header"],
  "cache_payload": true,
  "msvc_compat": false,
  "sc_checkpoint_count": 5
}
```

**Key response type — `RoundDetailResponse`:**

Contains everything about a single round: baseline/instrumented run outcomes, modules used, mutations applied, function-level coverage, assembled C source (for source viewer), and cutoff line.

**Round progress rows (`RoundSummaryInfo`):**

| Field | Description |
|---|---|
| `round_number` | Sequential round index |
| `round_id` | Unique ID (`{job_id}-round-{N}`) |
| `detected` | Boolean — was the baseline detected? |
| `differential_category` | `real_detection`, `instrumentation_artifact`, `flaky`, `consistent_evasion` |
| `coverage_pct` | Source line coverage percentage |
| `mutation_count` | Number of mutations applied |
| `evasion_score` | Composite evasion metric |

---

### Module: `api/workers.rs` — Worker Pool (413 LOC)

Manages the distributed Windows VM pool.

**`WorkerInfo`** — basic worker status (id, address, os_version, capabilities, status, current_job).

**`WorkerMetadataInfo`** — extended with health score, installed tools (clang version, xwin path), timestamps (connected_at, last_seen).

**`OrchestratorStatusResponse`** — aggregated metrics: pending_jobs, active_pools, total/available/busy workers, list of active jobs with their assigned worker pools.

---

### Module: `api/query.rs` — ElasticSearch & Triage (152 LOC)

**`POST /api/query`** — Flexible ES query with job_id filtering and date range.

**`POST /api/triage`** — Submit a triage verdict back to the controller for storage. Returns a triage_id for tracking.

---

### Module: `api/tokens.rs` — Triage Tokens (210 LOC)

**`GET /api/jobs/:job_id/rounds/:round_id/tokens`** — Queries ES `tokens-*` index directly (bypasses gRPC for performance). Returns tokens grouped by category prefix.

**`GET /api/tokens/compare`** — Compares token sets between two runs via gRPC `compare_tokens`. Returns `only_in_a`, `only_in_b`, `common`, per-mutation deltas, and Jaccard distance.

**Token categories:** `etw:`, `api:`, `api_arg:`, `seq2:`, `seq3:`, `dt:`, `trunc:`, `coverage:`

---

## Frontend (Vanilla HTML + JavaScript)

No build tools, no framework, no dependencies. Pure HTML5 + CSS3 + ES6 JavaScript served as static files.

### `index.html` — Main Dashboard (1939 LOC)

**Layout sections (top to bottom):**

```
┌─────────────────────────────────────────────────┐
│  Header: Status dot + Controller connectivity    │
├─────────────────────────────────────────────────┤
│  Stat Grid: Workers (total/available/busy/pending│
├─────────────────────────────────────────────────┤
│  Active Jobs Table: progress bars, stop buttons  │
├────────────┬────────────┬───────────────────────┤
│ Submit Job │  Workers   │  Job Status Lookup     │
│ (full form)│ (metadata  │  (real-time progress   │
│            │  table)    │   table)               │
├────────────┴────────────┴───────────────────────┤
│  Round Details  │  Compare Runs  │  Triage Tokens│
│  (modules,      │  (baseline vs  │  (categorized │
│   mutations,    │   instrumented)│   pills)      │
│   coverage)     │                │               │
└─────────────────┴────────────────┴───────────────┘
```

**Key JavaScript functions:**

| Function | Purpose |
|---|---|
| `checkHealth()` | Polls `/api/health`, updates status dot (green/yellow/red) |
| `refreshWorkers()` | Fetches `/api/workers/metadata`, renders table with health dots |
| `submitJob()` | Collects form fields → `POST /api/jobs` |
| `getJobProgress(id)` | Fetches rounds table, renders detection badges + coverage bars |
| `getRound(roundId)` | Parses `{job}-round-{N}` format, renders collapsible detail sections |
| `lookupTokens()` | Fetches tokens from ES, groups by category, renders as colored pills |
| `navigateToRound(roundId)` | Auto-fills round input + triggers detail fetch |

**Job submission form details:**

The form exposes the full `SubmitJobRequest` surface:
- 7 module dropdowns (carrier, decoder, antiemulation, deconditioner, guardrail, virtualprotect, decoy)
- Encoding type, trace mode, max rounds
- Selector algorithm: `coverage` (epsilon-greedy), `fuzzer` (genetic), `token` (triage-guided), `random`
- Variation strategy: `mutation_only` vs `full` (modules + mutations)
- Variable categories (checkboxes): which module categories the selector can mutate
- Mutation targets / fixed mutations (text lists)
- Flags: `cache_payload`, `msvc_compat`, `sc_checkpoint_count`

**Badge color conventions:**

| Badge | Color | Meaning |
|---|---|---|
| `detected` | Red | Artifact was detected |
| `not-detected` | Green | Artifact evaded |
| `real_detection` | Red | Both runs detected (confirmed) |
| `instrumentation_artifact` | Yellow | Only instrumented run detected |
| `flaky` | Orange | Inconsistent results |
| `consistent_evasion` | Green | Both runs evaded |
| `static_detection` | Dark red | Defender static scan flagged before execution |

---

### `source.html` — Source Code Viewer (1017 LOC)

Interactive C source viewer with execution trace overlay. Linked from the round detail view.

**Features:**

| Feature | Implementation |
|---|---|
| Syntax highlighting | Client-side regex: keywords (red), types (blue), preprocessor (purple), strings (cyan), comments (gray). Recognizes Windows API types (DWORD, HANDLE, LPVOID, etc.) |
| Execution trace overlay | Green highlight on executed lines (from trace data). Red highlight on cutoff line (last executed before detection/timeout) |
| Cutoff marker | Contextual label: "EXECUTION CONTINUED INTO SHELLCODE" vs "EXECUTION STOPPED AT CARRIER LAUNCH" based on `last_checkpoint` |
| Function navigation | Dropdown listing all C functions with per-function coverage %. Jump-to-function scrolls and highlights. Fold/collapse to hide function bodies |
| Deep linking | URL params: `job_id`, `round_id`, `run_id`. Hash: `#L{lineNum}` for line-level linking |

**Data flow:**
1. Fetch `/api/jobs/{job_id}/rounds/{round_id}` → extract `assembled_source`
2. Optionally fetch `/api/runs/{run_id}/trace?last=16000` → trace line data
3. Parse C functions (regex-based, tracks brace depth)
4. Render with highlighting + coverage overlay

---

## Kibana Dashboards

`kibana-dashboards/edr-dashboard.ndjson` contains 4 exported Kibana saved objects:

| Object | Type | Description |
|---|---|---|
| Process Events | Visualization | Histogram of process create/terminate over time |
| Network Events | Visualization | Histogram of network connections |
| Event Timeline | Visualization | Table view of events by type |
| EDR Dashboard | Dashboard | Composite layout combining all three visualizations |

All reference the `edr-telemetry-*` index pattern in ElasticSearch.

---

## Data Flow: End-to-End Example

**Operator submits a job:**

```
index.html submitJob()
  → POST /api/jobs  (JSON)
  → backend jobs.rs submit_job()
  → grpc_client.schedule_job()
  → Controller gRPC ScheduleJob
  → JobWorker starts producing rounds
```

**Operator monitors progress:**

```
index.html getJobProgress()
  → GET /api/jobs/{id}/progress
  → grpc_client.get_job_progress()
  → Controller returns round summaries
  → Frontend renders rounds table with badges
```

**Operator inspects a round:**

```
index.html getRound()
  → GET /api/jobs/{job_id}/rounds/{round_id}
  → grpc_client.get_round()
  → Controller returns full round detail
  → Frontend renders modules, mutations, coverage, outcomes
```

**Operator views source with trace:**

```
index.html → opens source.html?job_id=X&round_id=Y&run_id=Z
  → GET /api/jobs/{job_id}/rounds/{round_id}  (assembled source)
  → GET /api/runs/{run_id}/trace?last=16000    (trace lines)
  → source.html renders highlighted source with coverage overlay
```

**Operator queries triage tokens:**

```
index.html lookupTokens()
  → GET /api/jobs/{job_id}/rounds/{round_id}/tokens
  → backend tokens.rs → direct ES query to tokens-* index
  → Frontend groups tokens by category, renders as pills
```

---

## Design Decisions

| Decision | Rationale |
|---|---|
| Vanilla HTML (no React/Vue) | Lab tool — minimal complexity, no build step, easy to modify |
| REST adapter over gRPC-web | Simpler than gRPC-web in browser; Axum already exists in the workspace |
| Optional ElasticSearch | Backend degrades gracefully if ES is unavailable — token endpoints return 503 but all other endpoints work normally |
| No authentication | Lab-only deployment behind VPN/firewall |
| Static file serving from backend | Single deployment unit — `cargo run` serves both API and frontend |
| Direct ES queries for tokens | Avoids round-tripping through Controller for read-heavy token visualization |
