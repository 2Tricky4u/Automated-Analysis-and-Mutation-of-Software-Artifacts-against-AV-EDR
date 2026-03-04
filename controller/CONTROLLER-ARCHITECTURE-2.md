# Controller Module — Architecture & Goal

## 1. Goal

The controller is the **central orchestration binary** of AutoMutate++. It implements the closed experimental loop described in the project's design: receiving mutation campaign requests, scheduling rounds of artifact builds, dispatching execution to sandboxed Windows VMs, collecting telemetry, performing differential triage, scoring detection-correlated tokens, and feeding those scores back into the next round's mutation selection.

In one sentence: the controller turns a payload and a search-space configuration into a sequence of evidence-driven mutation experiments, all automatically.

---

## 2. Module Map

The controller is a single Rust binary (`main.rs`) backed by a library (`lib.rs`) that exposes five internal modules:

```
controller/
├── build.rs            ← tonic/prost codegen (3 proto files → Rust)
├── src/
│   ├── main.rs         ← Startup: config → ES → channels → Orchestrator → targets → gRPC server
│   ├── lib.rs          ← Re-exports: api, dispatch, storage, triage, vm + protobuf modules
│   ├── api/            ← gRPC ingress layer (thin handlers, 25 RPCs)
│   ├── dispatch/       ← Execution engine (Orchestrator, JobWorker, RunPool, VMExecutor)
│   ├── storage/        ← ElasticSearch data-access layer (6 index families)
│   ├── triage/         ← Intelligence layer (token extraction, scoring, 4 selectors)
│   └── vm/             ← Physical VM connection layer (TargetManager, streams, heartbeat)
```

| Module | Lines | Files | Role |
|--------|------:|------:|------|
| `api/` | ~2,146 | 5 | gRPC boundary — translates proto ↔ domain types |
| `dispatch/` | ~4,938 | 12 | Experiment loop — schedules rounds, dispatches runs, aggregates results |
| `storage/` | ~1,788 | 9 | Persistence — all ES reads/writes, schema templates |
| `triage/` | ~5,721 | 10 | Decision-making — token extraction, scoring, mutation selection |
| `vm/` | ~1,081 | 2 | Transport — VM connections, bidi streams, artifact upload |
| **Total** | **~15,674** | **38** | |

---

## 3. Startup Sequence (`main.rs`)

```
1. Load ControllerConfig from controller.toml
2. Init tracing (console + file, per-crate env filter)
3. Connect to ElasticSearch → EsStorage (Arc)
4. Bootstrap index templates (5 templates, non-fatal on failure)
5. Create channels:
   ┌─ events_tx/rx   (TargetEvent, 4096)    VM → Orchestrator
   ├─ job_tx/rx      (JobSession, 128)       API → Orchestrator
   └─ job_control_tx/rx (JobControlCommand, 64) API → Orchestrator
6. Create RunPool (Arc, OS-sharded queues)
7. Create TargetManager (Arc, DashMap-backed)
8. Spawn Orchestrator (tokio::spawn, long-lived select loop)
9. Discover targets from automation/generated/*.toml → register
10. Query target metadata (OS, capabilities) via unary gRPC
11. Establish bidi streams with all targets (spawns VMExecutors + heartbeats)
12. Spawn reconnect loop (background, configurable interval)
13. Create SchedulerService (gRPC handler struct)
14. Start tonic gRPC server (+ reflection) → serve forever
```

---

## 4. Architecture Overview

```
                         ┌───────────────────────────────────────┐
                         │          External Clients              │
                         │    (UI, CLI, Worker Agents)            │
                         └───────────────┬───────────────────────┘
                                         │ gRPC (tonic)
                                         ▼
                    ┌─────────────────────────────────────────────┐
                    │              API Module                      │
                    │         SchedulerService                     │
                    │   25 RPCs, thin handlers, no business logic  │
                    │   Reads: Arc<EsStorage>, Arc<RunPool>,       │
                    │          Arc<TargetManager>                   │
                    │   Writes: job_tx, job_control_tx (channels)  │
                    └────────┬───────────────────────┬────────────┘
                             │                       │
                   job_tx    │            job_control_tx
                   (submit)  │            (stop)     │
                             ▼                       ▼
                    ┌─────────────────────────────────────────────┐
                    │           Dispatch Module                    │
                    │                                              │
                    │  ┌──────────────────────────────────────┐   │
                    │  │           Orchestrator                │   │
                    │  │  (1 instance, biased tokio::select)  │   │
                    │  │  • Spawns JobWorkers per job          │   │
                    │  │  • Indexes rounds/runs to ES (async)  │   │
                    │  │  • Computes coverage → blended score  │   │
                    │  │  • Handles VM connect/disconnect      │   │
                    │  └─────────┬────────────────────────────┘   │
                    │            │ spawns                          │
                    │            ▼                                 │
                    │  ┌──────────────────────────────────────┐   │
                    │  │           JobWorker                   │   │
                    │  │  (1 per active job, tokio task)       │   │
                    │  │  • Calls Selector → mutations          │   │
                    │  │  • Builds baseline + instrumented      │   │
                    │  │  • Static Defender scan (short-circuit) │  │
                    │  │  • Creates 3 RunEnvelopes → RunPool    │   │
                    │  │  • Aggregates results in RoundAgg      │   │
                    │  │  • Finalizes → differential category   │   │
                    │  │  • Spawns async triage extraction      │   │
                    │  └─────────┬────────────────────────────┘   │
                    │            │ adds runs                      │
                    │            ▼                                 │
                    │  ┌──────────────────────────────────────┐   │
                    │  │           RunPool                     │   │
                    │  │  (shared, OS-sharded DashMap+VecDeque)│   │
                    │  │  • Per-OS queues with Notify signal   │   │
                    │  │  • Capability + dryrun filtering      │   │
                    │  │  • Per-job result routing (mpsc)      │   │
                    │  └─────────┬────────────────────────────┘   │
                    │            │ takes runs                     │
                    │            ▼                                 │
                    │  ┌──────────────────────────────────────┐   │
                    │  │          VMExecutor                   │   │
                    │  │  (1 per connected VM, tokio task)     │   │
                    │  │  • Reserve → upload → command → wait  │   │
                    │  │  • Routes result back via RunPool     │   │
                    │  └──────────────────────────────────────┘   │
                    └─────────────────────────────────────────────┘
                             │                          │
                    ┌────────▼──────────┐    ┌─────────▼─────────┐
                    │  Storage Module    │    │    VM Module       │
                    │  EsStorage facade  │    │  TargetManager     │
                    │                    │    │                    │
                    │  6 index families: │    │  • DashMap<Target> │
                    │  jobs, rounds,     │    │  • Bidi streams    │
                    │  runs, telemetry,  │    │  • Heartbeat (30s) │
                    │  artifacts, tokens │    │  • Artifact upload  │
                    │                    │    │  • Reconnect loop  │
                    │  Write: typed      │    │  • Deferred exec   │
                    │  Read: raw JSON    │    │    spawn (15s reg)  │
                    └────────────────────┘    └────────────────────┘
                                                       │
                                              gRPC bidi stream
                                                       │
                                                       ▼
                                              ┌────────────────────┐
                                              │  Worker Agent      │
                                              │  (Windows VM)      │
                                              │  RedEDR + ETW +    │
                                              │  Defender + Agent   │
                                              └────────────────────┘
```

---

## 5. Module Responsibilities

### 5.1 API — gRPC Ingress (`api/`)

**Goal:** Single entry-point for all external interactions. Pure translation layer — no business logic.

**Design pattern:** Thin handler. Each RPC validates, delegates to storage/channels, maps results to proto, returns.

| Category | RPCs | Handler |
|----------|------|---------|
| Job lifecycle | `ScheduleJob`, `GetJobStatus`, `GetJobProgress`, `StopJob`, `ReportStatus` | `job.rs` |
| Round & analysis | `GetRound`, `CompareRuns`, `CompareTokens`, `GetTraceLines` | `job.rs` |
| Artifact | `BuildArtifact`, `DeployArtifact` | `artifact.rs` |
| Worker/Telemetry | `ListWorkers`, `StreamTelemetry`, `GetWorker`, `GetAvailableWorkers`, `GetWorkerMetadata` | `worker.rs` |
| Monitoring | `GetPoolMetrics`, `GetOrchestratorStatus` | `worker.rs` |
| Admin | `PingWorker`, `DisconnectWorker`, `DisconnectAllWorkers` | `worker.rs` |
| Utility | `Ping`, `SubmitTriage` (legacy), `QueryResults` | `utility.rs` |

**Key invariant:** The API is a **producer only** for channels (`job_tx`, `job_control_tx`). It never blocks on Orchestrator completion. All result observation is through ES queries.

---

### 5.2 Dispatch — Execution Engine (`dispatch/`)

**Goal:** Implement the closed experiment loop: select mutations → build artifacts → dispatch to VMs → aggregate results → finalize differential category → trigger triage.

**Four components, producer-consumer pipeline:**

| Component | Count | Role |
|-----------|-------|------|
| **Orchestrator** | 1 (long-lived) | Receives jobs, spawns workers, indexes to ES, computes coverage |
| **JobWorker** | 1 per active job | Round production, artifact builds, result aggregation, finalization |
| **RunPool** | 1 (shared) | OS-sharded queue, capability filtering, result routing |
| **VMExecutor** | 1 per connected VM | Reserve → upload → execute → release → route result |

**Round production flow:**

```
Selector.select() → mutations + modules
    ↓
ArtifactBuilder.build() × 2 (baseline + instrumented)
    ↓
Static Defender scan (if detected → StaticDetection, skip VM dispatch)
    ↓
3 RunEnvelopes (baseline + instrumented + dryrun) → RunPool
    ↓
VMExecutor takes run → reserve VM → upload artifact → send command → wait
    ↓
Result arrives → RoundAgg aggregates (join 2-3 results)
    ↓
finalize_round() → DifferentialCategory + evasion_score
    ↓
Spawn async triage extraction → TriageGuidance → next round's selector
```

**Differential categories (two-run protocol):**

| Category | Baseline | Instrumented | Meaning |
|----------|----------|-------------|---------|
| `RealDetection` | detected | detected | Genuine EDR detection |
| `InstrumentationArtifact` | clean | detected | Tracing caused detection |
| `Flaky` | detected | clean | Non-reproducible |
| `Evasion` | clean | clean | Full evasion |
| `MutationFailed` | — | — | Dryrun crash (loader broken) |
| `PayloadFailed` | — | — | Dryrun crash (payload broken) |
| `StaticDetection` | — | — | Defender file scan hit |

**Backpressure:** Max 5 in-flight rounds, max 9 pending runs (3 rounds × 3 runs), 5s dryrun grace period.

---

### 5.3 Storage — Data-Access Layer (`storage/`)

**Goal:** Single persistence boundary between the controller and ElasticSearch. Owns schema, writes, reads.

**Six index families:**

| Index Pattern | Rotation | Doc ID | Purpose |
|--------------|----------|--------|---------|
| `jobs-YYYY.MM` | Monthly | `job_id` | Job lifecycle state |
| `rounds-YYYY.MM` | Monthly | `job_id/round_id` | Round summaries, coverage, mutations |
| `runs-YYYY.MM` | Monthly | `run_id` | Per-run outcomes |
| `telemetry-YYYY.MM.DD` | Daily | auto | ETW, DLL hooks, traces, checkpoints |
| `tokens-YYYY.MM` | Monthly | auto | Extracted token sets per round |
| `artifacts-YYYY.MM` | Monthly | auto | Build metadata |

**Design decisions:**
- **Write side:** Accepts typed Rust structs, returns `Result<()>`. Uses `Refresh::WaitFor` for consistency.
- **Read side:** Returns raw `serde_json::Value`. Proto mapping is the API layer's job.
- **Update pattern:** `find_index()` resolves monthly index → `update_doc_by_id()` with 3-retry version conflict handling.
- **Telemetry:** Bulk API, payload flattening, smart numeric conversion (pointer-like fields → hex strings), dynamic templates for arbitrary `payload_*` fields.

---

### 5.4 Triage — Intelligence Layer (`triage/`)

**Goal:** Close the feedback loop. Transform execution results into actionable mutation guidance.

**Pipeline:**

```
Execution results (RoundSummary + telemetry)
    ↓
Token Extraction (extractor.rs)
  • 9 token categories: module, mutation, api, api_arg, seq2, image, etw, etw_event, checkpoint
  • 3 sources: in-memory (modules/mutations), ES telemetry (RedEDR), ES checkpoints
    ↓
Token Scoring (scorer.rs)
  • lift = P(detected|T) / P(detected)
  • confidence = min(1.0, n_observations / 5)
  • importance = lift × confidence
    ↓
Guidance (scorer.rs → TriageGuidance)
  • avoid_tokens: lift > 1.5 AND confidence > 0.3
  • seek_tokens:  lift < 0.667 AND confidence > 0.3
    ↓
Mutation Selection (4 selector strategies)
```

**Four selectors (all implement `Selector` trait):**

| Selector | Algorithm | Learning Signal | Determinism |
|----------|-----------|----------------|-------------|
| **CoverageSelector** (default) | Epsilon-greedy (ε=0.3) | Evasion scores from history | Pseudo (`subsec_nanos`) |
| **FuzzerSelector** | Genetic algorithm (tournament, crossover, perturbation) | Evasion scores via fitness | Full (`SeededRng`) |
| **TokenSelector** | Token-biased epsilon-greedy | Evasion scores + avoid/seek tokens | Pseudo (`subsec_nanos`) |
| **RandomSelector** | Uniform random | None (evaluation baseline) | Full (`SeededRng`) |

**Mutation catalog:** 22 mutations (10 AST + 3 LLVM IR + 9 Binary), each with a defined parameter space (categorical, int-range, float-range) supporting sampling, perturbation, and distance computation.

**Default split:**
- **Fixed mutations** (always applied): 10 (1 LLVM + 9 Binary) — PE normalization
- **Explored mutations** (1 per round, varied by selector): 10 AST — behavioral changes

---

### 5.5 VM — Physical Transport (`vm/`)

**Goal:** Bridge the logical dispatch system to real Windows sandboxes. Manage connections, state, artifact transport, and auto-recovery.

**TargetManager** is the single struct, backed by `DashMap<TargetId, Target>`:

- **Target state machine:** `Offline → Available → Busy → Available` (or any → `Offline`)
- **Two gRPC channels per VM:**
  - Unary channel (10s timeout): `send_artifact()`, `get_worker_info()`
  - Bidi stream channel (no timeout): commands, results, heartbeats
- **Per-VM task ensemble:** 3 concurrent tokio tasks per connected VM:
  - `stream_handler` — reads incoming `WorkerMessage`s
  - `VMExecutor::run()` — takes runs from pool, dispatches to VM
  - `heartbeat` — 30s keepalive
- **Deferred executor spawn:** Waits 15s for `Registration` message before creating VMExecutor (avoids capability mismatch)
- **Auto-reconnection:** Background loop for `Offline + enabled` targets

---

## 6. Data Flow: Complete Round Lifecycle

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ 1. API receives ScheduleJob → creates JobSession → job_tx → Orchestrator    │
│                                                                              │
│ 2. Orchestrator spawns JobWorker(job, selector, run_pool, ...)              │
│                                                                              │
│ 3. JobWorker.produce_round():                                                │
│    a. Selector.select(history, guidance) → Selection{modules, mutations}     │
│    b. ArtifactBuilder.build(baseline, trace=off)                             │
│    c. Static Defender scan → if detected: StaticDetection, skip to (6)       │
│    d. ArtifactBuilder.build(instrumented, trace=lines)                       │
│    e. Create 3 RunEnvelopes → RunPool.add_runs()                             │
│                                                                              │
│ 4. VMExecutor wakes → RunPool.take_run() → reserve VM → upload → execute    │
│    Worker Agent runs artifact, streams telemetry → stream_handler            │
│    stream_handler → result_tx → RunPool.route_result() → JobWorker           │
│                                                                              │
│ 5. JobWorker.on_result() aggregates in RoundAgg (2-3 results)               │
│    When complete: finalize_round()                                           │
│    → DifferentialCategory + evasion_score + detection_verdict                │
│    → Spawn extract_and_score() background task                               │
│    → Emit RoundCompleted to Orchestrator                                     │
│                                                                              │
│ 6. Orchestrator:                                                             │
│    a. index_round_and_runs() → ES (rounds, runs)                            │
│    b. compute_round_coverage() → trace → SourceMap → coverage → ES          │
│       → blended_evasion_score → CoverageCorrection → JobWorker              │
│                                                                              │
│ 7. extract_and_score() (background):                                         │
│    a. Extract tokens (modules + mutations + telemetry + checkpoints)         │
│    b. Index token_set to ES                                                  │
│    c. Score tokens (lift × confidence) from history                          │
│    d. Build TriageGuidance (avoid/seek) → guidance_tx → JobWorker            │
│                                                                              │
│ 8. JobWorker checks should_continue():                                       │
│    → If yes: goto (3) with updated history + guidance                        │
│    → If no: cleanup artifacts, emit JobCompleted                             │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## 7. Concurrency Model

### Shared State

| Resource | Type | Access Pattern |
|----------|------|----------------|
| `RunPool` | `Arc<RunPool>` | DashMap (lock-free reads) + per-OS Mutex (short writes) + Notify |
| `TargetManager` | `Arc<TargetManager>` | DashMap (lock-free reads, per-shard writes) |
| `EsStorage` | `Arc<EsStorage>` | HTTP client (internally Arc, connection-pooled) |
| `JobSession` | Owned by JobWorker | Single-writer, no sharing |
| `RoundAgg` | Owned by JobWorker | Single-writer, no sharing |

### Channel Topology

```
API ──job_tx──────────> Orchestrator.job_submit_rx
API ──job_control_tx──> Orchestrator.job_control_rx
TargetManager ────────> Orchestrator.events_rx           (TargetEvent)
JobWorker ────────────> Orchestrator.job_event_rx         (JobWorkerEvent)
Orchestrator ─────────> JobWorker.correction_rx           (CoverageCorrection)
RunPool ──────────────> JobWorker.result_rx               (JobRunResult)
Triage spawn ─────────> JobWorker.guidance_rx             (TriageGuidance)
stream_handler ───────> VMExecutor.result_rx              (RemoteRunResult)
```

### Cancellation

- **Per-job:** `CancellationToken` in `JobHandle`, triggered by `StopJob` RPC
- **Global:** `CancellationToken` in `RunPool`, triggers all VMExecutors + JobWorkers

---

## 8. gRPC & Protobuf

`build.rs` compiles 3 proto files via `tonic-prost-build`:

| Proto | Package | Generated Module |
|-------|---------|-----------------|
| `common.proto` | `automutate.common` | `automutate::common` |
| `controller.proto` | `automutate.controller` | `automutate::controller` |
| `worker.proto` | `automutate.worker` | `automutate::worker` |

Both server and client stubs are generated. A file descriptor set is emitted for gRPC reflection.

---

## 9. Cross-Module Dependencies

```
                    main.rs
                   /   |   \
                  v    v    v
               api  dispatch  vm
                |   / |  \    |
                v  v  v   v   v
              storage   triage
                  \      /
                   v    v
               ElasticSearch
```

| Producer Module | Consumer Module | Interface |
|----------------|----------------|-----------|
| `api` | `dispatch` | `job_tx` channel (JobSession), `job_control_tx` channel (Stop) |
| `dispatch` | `storage` | `index_round`, `index_run_result`, `update_job_*` |
| `dispatch` | `triage` | `Selector::select()`, `extract_and_score()` spawn |
| `dispatch` | `vm` | `TargetManager::reserve/release`, `send_command`, `send_artifact` |
| `triage` | `storage` | `query_api_telemetry`, `query_checkpoint_events`, `query_token_sets` |
| `triage` | `dispatch` | `TriageGuidance` via mpsc, `CoverageResult` via `SourceMap` |
| `vm` | `dispatch` | `TargetEvent` channel, `RemoteRunResult` via `result_tx` |
| `api` | `storage` | All `query_*` functions for read-only ES access |
| `api` | `vm` | `TargetManager` read queries (list, get, metadata) |
| `api` | `build` (crate) | `ArtifactBuilder::build()` in `BuildArtifact` RPC |

---

## 10. Summary: Position in the Global Project

The controller implements **layers 1 through 6** of the AutoMutate++ experimental loop:

| Project Layer | Controller Component |
|--------------|---------------------|
| 1. Collect Windows telemetry | `vm/` streams → `storage/` indexes to ES |
| 2. Generate controlled mutations | `triage/` selectors → `dispatch/` JobWorker → `build` crate |
| 3. Execute in sandboxed VMs | `dispatch/` RunPool + VMExecutor → `vm/` bidi stream |
| 4. Normalize into triage tokens | `triage/` extractor → 9 token categories |
| 5. Score by correlation with detection | `triage/` scorer → lift × confidence |
| 6. Guide future mutation selection | `triage/` guidance → selector → next round |

What the controller does **not** contain:
- C source compilation and cross-compilation toolchain (`build` crate)
- Worker agent logic running inside VMs (`worker/agent` crate)
- Configuration schema and loading (`config` crate)
- Proto definitions (`proto/` directory)

The controller is the **brain and nervous system** of AutoMutate++: it decides what to mutate, coordinates execution, remembers what happened, learns from outcomes, and decides what to try next.
