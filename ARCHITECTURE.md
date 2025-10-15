# AutoMutate++ Architecture Documentation
## Comprehensive Module Design & File Interaction Map

---

## Table of Contents
1. [System Overview](#system-overview)
2. [Architectural Principles](#architectural-principles)
3. [Module Catalog](#module-catalog)
4. [File-Level Architecture](#file-level-architecture)
5. [Data Flow Diagrams](#data-flow-diagrams)
6. [Interface Contracts](#interface-contracts)

---

## System Overview

AutoMutate++ is a **fuzzer-like EDR evaluation framework** that implements a feedback-driven mutation loop:

```
┌─────────────────────────────────────────────────────────────┐
│                    FEEDBACK LOOP                            │
│                                                             │
│  ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌──────────┐     │
│  │ Mutate  │──▶│  Build  │──▶│ Execute │──▶│ Collect  │     │
│  │         │   │         │   │         │   │ Telemetry│     │
│  └─────────┘   └─────────┘   └─────────┘   └──────────┘     │
│       ▲                                            │        │
│       │                                            ▼        │
│       │                                     ┌──────────┐    │
│       └─────────────────────────────────────│  Triage  │    │
│                                             │ Analyze  │    │
│                                             └──────────┘    │
└─────────────────────────────────────────────────────────────┘
```

**Core Objective:** Discover which code features/behaviors trigger EDR detections, then avoid them in subsequent iterations.

---

## Architectural Principles

### 1. Separation of Concerns
- **Controller** (Linux) = Brain (orchestration, analysis, storage)
- **Worker** (Windows VM) = Hands (execution, telemetry collection)
- **Build System** = Factory (deterministic artifact generation)
- **Telemetry** = Eyes (observe what happens)

### 2. Contract-Based Communication
- All inter-module communication uses **Protobuf-defined gRPC** contracts
- No tight coupling between modules (can swap implementations)
- Defined in `controller/proto/`

### 3. Reproducibility
- Every artifact tagged with `ArtifactId` (SHA256)
- Every run tagged with `RunId` (UUID)
- Deterministic builds (pinned toolchains, fixed timestamps)

### 4. Scalability
- Worker pool can grow independently
- Controller components communicate via internal gRPC (can distribute later)
- Elasticsearch provides horizontal scaling for telemetry

### 5. Explainability
- All decisions (mutation selection, triage hypotheses) are logged with rationale
- Feature importances tracked and exported
- Human-readable reports generated

---

## Module Catalog

### Legend
- 🧠 **Controller Modules** (Linux, orchestration)
- 🖐️ **Worker Modules** (Windows VM, execution)
- 🏭 **Build Modules** (Compilation pipeline)
- 👁️ **Telemetry Modules** (Observation layer)
- 🎨 **UI Modules** (User interface)
- 📚 **Shared Libraries** (Cross-cutting concerns)

---

## File-Level Architecture

---

## 🧠 Controller Layer (Linux Host)

### Purpose
The **Controller** orchestrates the entire analysis pipeline. It lives on a Linux host and manages job scheduling, mutation selection, triage analysis, and result storage.

---

### Module: `controller/scheduler`

**Purpose:** Entry point for all analysis jobs. Acts as the **gRPC gateway** for external clients.

**Files:**

#### `controller/scheduler/src/main.rs`
- **What:** Main binary, starts gRPC server
- **Why:** Single entry point for job submission (eliminates multiple endpoints confusion)
- **Responsibilities:**
  - Bind gRPC server to `0.0.0.0:50051`
  - Implement `Controller` service (from `controller.proto`)
  - Handle `ScheduleJob`, `GetJobStatus`, `SubmitTriage`, `QueryResults` RPCs
  - Validate incoming `JobRequest` (check artifact type, source path exists)
  - Generate unique `JobId` (format: `job-NNNNNN`)
- **Interactions:**
  - **Reads:** `config/scheduler.toml` (server config)
  - **Calls:** `controller/queue` (enqueue job)
  - **Calls:** `config` crate (load configuration)
  - **Returns:** `JobResponse` to client (accepted/rejected + job_id)

#### `controller/scheduler/src/state.rs` *(To Create)*
- **What:** Job state management (in-memory + disk persistence)
- **Why:** Track job status across pipeline (queued → building → running → completed)
- **Data Structures:**
  ```rust
  struct JobState {
      job_id: JobId,
      status: JobStatus, // queued, building, running, completed, failed
      created_at: Timestamp,
      updated_at: Timestamp,
      progress_percent: u8,
      current_phase: String, // "mutating", "building", "executing"
      logs: Vec<String>,
  }
  ```
- **Interactions:**
  - **Read/Write:** `Arc<RwLock<HashMap<JobId, JobState>>>` (shared state)
  - **Optional:** Persist to `sled` database for crash recovery
  - **Used by:** `main.rs` (update status on pipeline events)

**Why This Module Exists:**
Without a centralized scheduler, clients wouldn't know where to submit jobs or how to query status. This is the **single source of truth** for job lifecycle management.

---

### Module: `controller/queue`

**Purpose:** Priority queue and corpus manager. Stores jobs waiting for execution.

**Files:**

#### `controller/queue/src/main.rs`
- **What:** Queue service (can be internal gRPC or shared memory)
- **Why:** Decouple job submission from job execution (buffering layer)
- **Responsibilities:**
  - Maintain priority queue (simple FIFO for MVP, heap-based later)
  - Expose APIs: `enqueue(job)`, `dequeue() -> Option<Job>`, `update_status(job_id, status)`
  - Persist queue to disk (survive restarts)
- **Interactions:**
  - **Called by:** `controller/scheduler` (enqueue new jobs)
  - **Called by:** `controller/selector` (dequeue next job)
  - **Reads:** `config/queue.toml` (storage path, max size)
  - **Writes:** `/var/lib/edr-queue/*.db` (persistent storage)

#### `controller/queue/src/storage.rs` *(To Create)*
- **What:** Persistent queue storage using `sled` or `rocksdb`
- **Why:** Queue state must survive crashes (jobs can't be lost)
- **Operations:**
  - `insert(job)` - Add job to queue + disk
  - `pop() -> Option<Job>` - Remove and return next job
  - `get(job_id) -> Option<Job>` - Query specific job
  - `update(job_id, status)` - Modify job state
- **Data Format:** Serialize `Job` struct to bytes via `bincode` or `protobuf`

#### `controller/queue/src/priority.rs` *(Future)*
- **What:** Priority ranking logic (after MVP)
- **Why:** Prioritize jobs that explore new behavior or target specific features
- **Algorithm:** Combination of:
  - User-specified priority
  - Novelty score (untested mutation combinations)
  - Feedback score (how often past mutations evaded detection)

**Why This Module Exists:**
The queue **decouples producers (scheduler) from consumers (selector)**. This allows:
- Multiple job sources (UI, CLI, API)
- Backpressure handling (don't overload workers)
- Job persistence (restarts don't lose work)

---

### Module: `controller/selector`

**Purpose:** **Intelligent mutation selection** with feedback loop. The "brain" of the fuzzer.

**Files:**

#### `controller/selector/src/main.rs`
- **What:** gRPC server implementing `Selector` service
- **Why:** Central decision point for what mutations to try next
- **Responsibilities:**
  - Implement `SelectMutation` RPC:
    - Input: `JobId`, `avoid_features` (from triage feedback)
    - Output: List of `Mutation` messages (e.g., `[{id: "ast.string_encrypt", params: {}}]`)
  - Implement `ReportOutcome` RPC:
    - Input: `RunResult` (detected/not_detected/crash)
    - Action: Update mutation success statistics
  - Choose mutations using:
    - **Exploration:** Random selection (try new things)
    - **Exploitation:** Pick historically successful mutations
    - **Avoidance:** Exclude mutations that triggered detected features
- **Interactions:**
  - **Called by:** `controller/queue` (poll for next job)
  - **Calls:** `controller/mutator` (apply selected mutations)
  - **Calls:** `controller/triage-engine` (get avoid-list)
  - **Receives:** `worker/monitor` (outcome reports)
  - **Reads:** `config/selector.toml` (exploration rate)

#### `controller/selector/src/mutations.rs` *(To Create)*
- **What:** Mutation registry and metadata
- **Why:** Centralized catalog of all available mutations
- **Data:**
  ```rust
  struct MutationMetadata {
      id: String, // "ast.string_encrypt"
      category: MutationCategory, // AST, Binary, Behavioral
      parameters: Vec<ParamSpec>,
      success_rate: f32, // 0.0 to 1.0 (updated by feedback)
      times_used: u32,
      times_detected: u32,
  }
  ```
- **Operations:**
  - `get_available_mutations() -> Vec<MutationMetadata>`
  - `update_success(mutation_id, detected: bool)`
  - `filter_by_avoid_features(mutations, avoid_features) -> Vec<MutationMetadata>`

#### `controller/selector/src/strategy.rs` *(Future)*
- **What:** Advanced selection strategies (epsilon-greedy, UCB, genetic algorithm)
- **Why:** Optimize exploration vs. exploitation tradeoff
- **Algorithms:**
  - Epsilon-greedy: Random with probability ε, else best mutation
  - UCB (Upper Confidence Bound): Balance uncertainty and reward
  - Genetic: Crossover successful mutation combinations

**Why This Module Exists:**
The selector implements the **feedback loop** that makes this a learning system. Without it, mutations would be random with no improvement over time. This is where the "fuzzer intelligence" lives.

---

### Module: `controller/mutator`

**Purpose:** Apply AST/IR/binary transformations to source code or binaries.

**Files:**

#### `controller/mutator/src/lib.rs`
- **What:** Public API for mutation engine
- **Why:** Single interface for all mutation types
- **Main Function:**
  ```rust
  pub fn apply_mutation(
      source: &str,
      mutation: &Mutation
  ) -> Result<String, MutationError>
  ```
- **Responsibilities:**
  - Route mutation requests to specific handlers
  - Validate mutation parameters
  - Return mutated source code or binary
- **Interactions:**
  - **Called by:** `controller/selector` (after mutation selection)
  - **Calls:** `ast/`, `ir/`, `binary/`, `behavioral/` submodules
  - **Returns:** Mutated code to `build/emitter` (for compilation)

#### `controller/mutator/src/ast/string_encrypt.rs` *(To Create)*
- **What:** Replace string literals with XOR-encoded bytes
- **Why:** Evade static signature detection of plaintext strings
- **Implementation:**
  - Parse Rust AST using `syn` crate
  - Find `LitStr` nodes (string literals)
  - Replace with: `decode_xor(&[encrypted_bytes], key)`
  - Generate decode function stub
- **Example:**
  ```rust
  // Before: let msg = "Hello";
  // After:  let msg = decode_xor(&[0x48^0xAA, 0x65^0xAA, ...], 0xAA);
  ```
- **Dependencies:** `syn`, `quote`, `proc-macro2`

#### `controller/mutator/src/ast/import_hash.rs` *(To Create)*
- **What:** Replace static API imports with dynamic resolution via hash
- **Why:** Evade import table analysis (common EDR heuristic)
- **Implementation:**
  - Parse import statements
  - Replace with `GetProcAddress` + API hash lookup
  - Inject hash resolver stub
- **Example:**
  ```rust
  // Before: use winapi::um::processthreadsapi::GetCurrentProcess;
  // After:  let get_current_process = resolve_api_hash(0x12345678);
  ```

#### `controller/mutator/src/behavioral/sleep.rs` *(To Create)*
- **What:** Insert `sleep` calls at function entry
- **Why:** Benign deconditioning (delay execution, evade sandbox timeouts)
- **Implementation:**
  - Parse AST, find `fn main()` or specified functions
  - Insert `std::thread::sleep(Duration::from_secs(N))` as first statement
- **Parameters:** `duration_secs` (configurable)

#### `controller/mutator/src/ir/` *(Future - Phase 2)*
- **What:** LLVM IR transforms (control-flow flattening, opaque predicates, etc.)
- **Why:** Deeper obfuscation at intermediate representation level
- **Files:** `cfg_flatten.rs`, `opaque_predicate.rs`, `constant_fold.rs`

#### `controller/mutator/src/binary/` *(Future - Phase 3)*
- **What:** PE binary patching (import table rewrite, section splicing)
- **Why:** Post-compilation mutations (when source unavailable)
- **Files:** `import_table.rs`, `section_splice.rs`, `bitflip.rs`

**Why This Module Exists:**
This is the **transformation engine** that generates variants. Without mutations, every run would be identical. The mutator is what creates the "search space" the fuzzer explores.

---

### Module: `controller/triage-engine`

**Purpose:** Analyze telemetry to generate **explainable hypotheses** about why detections occurred.

**Files:**

#### `controller/triage-engine/src/main.rs`
- **What:** gRPC server implementing `Triage` service
- **Why:** Centralize triage analysis (don't duplicate logic)
- **Responsibilities:**
  - Implement `AnalyzeRun` RPC:
    - Query Elasticsearch for telemetry (given `run_id`)
    - Train/run surrogate classifier (logistic regression, decision tree)
    - Extract feature importances
    - Generate ranked hypotheses
  - Implement `GetAvoidList` RPC:
    - Return features correlated with detection (for selector feedback)
- **Interactions:**
  - **Called by:** `controller/selector` (get avoid-list)
  - **Called by:** `controller/triage-client` (manual analysis)
  - **Reads:** Elasticsearch (query `etw-*` and `rededr-*` indices)
  - **Writes:** `runs-*` index (store hypotheses)

#### `controller/triage-engine/src/lib.rs`
- **What:** Core triage algorithms
- **Why:** Reusable logic (can be called from CLI or service)
- **Components:**
  - `classifier.rs` - Surrogate model training
  - `hypothesis.rs` - Hypothesis generation
  - `features.rs` - Feature importance extraction

#### `controller/triage-engine/src/classifier.rs` *(To Create)*
- **What:** Train lightweight classifier on telemetry features
- **Why:** Learn which features predict detection
- **Models:**
  - Logistic Regression (fast, interpretable)
  - Decision Tree (CART, feature splits visible)
  - Random Forest (ensemble, feature importances)
- **Input:** Feature vectors from Elasticsearch (e.g., `[rwx_short_window=1, anon_thread=1, ...]`)
- **Output:** Feature importance scores + model
- **Library:** Use `smartcore` (pure Rust ML) or `linfa` (Rust ML toolkit)

#### `controller/triage-engine/src/hypothesis.rs` *(To Create)*
- **What:** Generate human-readable hypotheses from model
- **Why:** Translate ML output to actionable insights
- **Logic:**
  - Sort features by importance
  - For top-N features, generate text:
    ```
    "Flagged due to short RWX window + thread start in anon region"
    Evidence: mem.write_to_execute_ms<15, thr.start_region=anon
    Confidence: 0.82
    Recommendation: Avoid rwx_short_window; Seek staged RX→RW→RX
    ```
- **Output:** `Hypothesis` proto messages (rank, description, evidence, confidence, recommendation)

#### `controller/triage-engine/src/features.rs` *(To Create)*
- **What:** Feature extraction from raw telemetry
- **Why:** Convert JSON events to ML-ready vectors
- **Features (Examples):**
  - `rwx_short_window` - Boolean (write-to-execute < 15ms)
  - `anon_thread_start` - Boolean (thread started in anonymous memory)
  - `unsigned_child_of_signed` - Boolean (provenance risk)
  - `string_entropy` - Float (0-8, entropy of string literals)
- **Pipeline:**
  - Query Elasticsearch for events
  - Aggregate per run_id
  - Compute boolean/numeric features
  - Return feature vector

**Why This Module Exists:**
Triage provides **explainability** - the "why" behind detections. Without it, the fuzzer is blind (just trial-and-error). Triage enables:
- Targeted mutation selection (avoid detected features)
- Human-readable reports for analysts
- Differential analysis (compare features across runs)

---

### Module: `controller/rule-manager` *(Future - Phase 4)*

**Purpose:** Manage detection rules (Sigma, KQL, EQL) and export for Elasticsearch/SIEM.

**Files:**

#### `controller/rule-manager/src/lib.rs`
- **What:** Rule CRUD operations (create, read, update, delete)
- **Why:** Store/version detection rules discovered during analysis
- **Operations:**
  - `import_rule(rule: SigmaRule)` - Add new rule
  - `export_rule(rule_id) -> String` - Get rule as YAML/JSON
  - `list_rules(filter) -> Vec<Rule>` - Search rules
- **Storage:** Elasticsearch or file-based (YAML in git repo)
- **Interactions:**
  - **Called by:** `ui/backend` (rule management UI)
  - **Writes:** Elasticsearch `rules-*` index or `rules/` directory

**Why This Module Exists:**
Rules discovered through triage should be **reusable**. Export them for:
- Elastic detection engine
- Kibana alerting
- Sharing with blue teams

---

### Module: `controller/differential-analyzer` *(Future - Phase 4)*

**Purpose:** Compare scan-time vs. runtime signals to isolate detection triggers.

**Files:**

#### `controller/differential-analyzer/src/lib.rs`
- **What:** Differential analysis engine
- **Why:** Identify which specific tokens/features cause detection
- **Approach:**
  1. Submit artifact to scan-time Defender (`MpCmdRun.exe`)
  2. Submit same artifact to runtime execution (Harness)
  3. Compare telemetry and detection outcomes
  4. Compute `Δ` (what changed between detected vs. not_detected)
- **Output:** Token→detection probability map
- **Interactions:**
  - **Reads:** Elasticsearch (both scan-time and runtime telemetry)
  - **Writes:** `differential-*` index (token correlation data)

**Why This Module Exists:**
Differential analysis provides **causality** - which specific code changes led to detection. This is the most powerful feedback for mutation selection.

---

### Module: `controller/triage-client`

**Purpose:** CLI tool for manual triage analysis (for analysts, not automated pipeline).

**Files:**

#### `controller/triage-client/src/main.rs`
- **What:** CLI binary, calls `triage-engine` via gRPC
- **Why:** Allow humans to investigate specific runs interactively
- **Commands:**
  - `triage-client analyze <run_id>` - Generate hypotheses
  - `triage-client compare <run_id1> <run_id2>` - Differential view
  - `triage-client export <run_id> --format=json` - Export report
- **Interactions:**
  - **Calls:** `controller/triage-engine` (AnalyzeRun RPC)
  - **Reads:** Elasticsearch (for raw event inspection)
  - **Outputs:** Terminal (formatted tables, JSON, LaTeX)

**Why This Module Exists:**
Automated triage is useful for feedback loops, but humans need **interactive exploration** for deep investigation. This provides a REPL-like interface.

---

## 🖐️ Worker Layer (Windows VM)

### Purpose
Workers are **sandboxed Windows VMs** that execute artifacts, collect telemetry, and report outcomes. They are stateless and disposable (snapshot-restore after each run).

---

### Module: `worker/agent`

**Purpose:** gRPC server on Windows VM, receives execution requests from Controller.

**Files:**

#### `worker/agent/src/main.rs`
- **What:** Main binary, starts gRPC server on `0.0.0.0:50052`
- **Why:** Single entry point for all Worker operations
- **Responsibilities:**
  - Implement `WorkerAgent` service (from `worker.proto`)
  - Handle `ExecuteBuild`, `RunSample`, `HealthCheck`, `StreamTelemetry` RPCs
  - Spawn `worker/harness` as child process for execution
  - Forward telemetry to `telemetry/collector` on Linux host
- **Interactions:**
  - **Called by:** Controller (via gRPC from Linux)
  - **Calls:** `worker/harness` (spawn process)
  - **Calls:** `telemetry/collector` (stream telemetry)
  - **Reads:** `config/worker.toml` (controller address, harness path)

#### `worker/agent/src/executor.rs` *(To Create)*
- **What:** Spawn and manage Harness process
- **Why:** Isolate execution logic from gRPC handling
- **Responsibilities:**
  - Launch `harness.exe` with arguments (artifact path, timeout)
  - Capture stdout/stderr
  - Monitor process exit code
  - Kill process if timeout exceeded
- **Windows API:** Use `CreateProcess` or `std::process::Command`
- **IPC:** Named pipe or gRPC (defined by `worker/harness-ipc`)

#### `worker/agent/src/health.rs` *(To Create)*
- **What:** System health metrics (CPU%, memory%, disk space)
- **Why:** Controller needs to know if VM is healthy before scheduling jobs
- **Metrics:**
  - CPU usage (via `sysinfo` crate)
  - Memory usage
  - Active jobs count
  - Last heartbeat timestamp
- **Interactions:**
  - **Called by:** `main.rs` (HealthCheck RPC)
  - **Returns:** `HealthResponse` proto

**Why This Module Exists:**
Agent is the **interface** to the Worker VM. Without it, Controller can't communicate with VMs. It abstracts:
- Process management (spawn, kill, monitor)
- System metrics
- Telemetry forwarding

---

### Module: `worker/harness`

**Purpose:** Execute artifacts in isolated process, enforce timeout, capture traces.

**Files:**

#### `worker/harness/src/lib.rs`
- **What:** Core execution logic (library, not binary)
- **Why:** Reusable logic for different execution modes
- **Main Function:**
  ```rust
  pub fn execute_artifact(
      artifact_path: &Path,
      timeout: Duration,
      telemetry_providers: &[String],
  ) -> Result<ExecutionResult, HarnessError>
  ```
- **Responsibilities:**
  - Spawn artifact as child process
  - Enforce timeout using Windows Job Objects
  - Capture exit code, stdout, stderr
  - Return outcome (success, timeout, crash)
- **Interactions:**
  - **Called by:** `worker/agent` (via process spawn or IPC)
  - **Calls:** `worker/monitor` (outcome labeling)
  - **Triggers:** Windows ETW events (via artifact execution)

#### `worker/harness/src/timeout.rs` *(To Create)*
- **What:** Timeout enforcement using Windows Job Objects
- **Why:** Reliable process tree termination (kill all child processes)
- **Windows API:**
  - `CreateJobObject`
  - `SetInformationJobObject` (set `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`)
  - `AssignProcessToJobObject` (assign artifact process)
  - When timeout expires, close job handle → kills all processes
- **Fallback:** `TerminateProcess` if Job Objects fail

#### `worker/harness/src/defender.rs` *(To Create)*
- **What:** Query Windows Defender alerts
- **Why:** Detect if artifact was blocked/flagged
- **Implementation:**
  - Call PowerShell: `Get-MpThreatDetection`
  - Parse output (JSON or XML)
  - Filter by timeframe (only alerts during execution window)
- **Interactions:**
  - **Called by:** `lib.rs` (after execution completes)
  - **Returns:** List of alerts (if any)

#### `worker/harness/src/trace.rs` *(Future - Phase 2)*
- **What:** Basic-block/instruction tracing (hardware breakpoints or ETW-TI)
- **Why:** Fine-grained execution profiling (which code paths executed)
- **Implementation:**
  - Use ETW Threat-Intelligence provider (requires admin)
  - Or instrument binary with INT3 breakpoints
  - Capture executed addresses → map to source code

**Why This Module Exists:**
Harness is the **execution sandbox**. It ensures:
- Artifacts can't escape timeout (prevents runaway processes)
- Execution is logged (exit code, timing)
- Defender interactions are captured

---

### Module: `worker/harness-ipc`

**Purpose:** IPC protocol between Agent and Harness (if they run as separate processes).

**Files:**

#### `worker/harness-ipc/src/lib.rs`
- **What:** Shared IPC types and helpers
- **Why:** Decouple Agent and Harness (can run in separate security contexts)
- **IPC Mechanisms:**
  - Named pipes (Windows native)
  - gRPC (if Harness runs as local service)
  - Shared memory (for high-throughput telemetry)
- **Messages:**
  ```rust
  pub enum IpcMessage {
      ExecuteRequest { artifact_path: PathBuf, timeout: Duration },
      ExecuteResponse { exit_code: i32, output: String },
      TelemetryBatch { events: Vec<TelemetryEvent> },
  }
  ```
- **Interactions:**
  - **Used by:** `worker/agent` (send requests)
  - **Used by:** `worker/harness` (receive requests, send responses)

**Why This Module Exists:**
Separating Agent and Harness allows:
- **Security:** Harness runs at lower privilege (Agent handles gRPC auth)
- **Isolation:** Harness crash doesn't kill Agent
- **Performance:** Async IPC for telemetry streaming

---

### Module: `worker/monitor`

**Purpose:** Label execution outcome (detected | not_detected | noisy | crash).

**Files:**

#### `worker/monitor/src/lib.rs`
- **What:** Outcome classification logic
- **Why:** Standardize labeling across all runs (consistency)
- **Main Function:**
  ```rust
  pub fn label_outcome(
      exit_code: i32,
      defender_alerts: Vec<Alert>,
      execution_time: Duration,
      telemetry_events: usize,
  ) -> RunStatus
  ```
- **Labeling Logic:**
  - `detected` - Defender alert present OR process blocked
  - `not_detected` - Clean exit (exit_code=0), no alerts
  - `crash` - Non-zero exit code (exception, assertion failure)
  - `noisy` - Suspicious (e.g., exit in <1s, no telemetry)
- **Interactions:**
  - **Called by:** `worker/harness` (after execution completes)
  - **Calls:** `worker/harness/src/defender.rs` (get alerts)
  - **Returns:** `RunResult` proto (sent to Selector)

#### `worker/monitor/src/defender.rs` *(To Create)*
- **What:** Parse Defender alert data
- **Why:** Extract structured info from PowerShell output
- **Input:** JSON from `Get-MpThreatDetection`
- **Output:**
  ```rust
  pub struct DefenderAlert {
      threat_name: String,
      severity: AlertLevel, // Low, Medium, High, Severe
      category: String, // Trojan, Backdoor, etc.
      timestamp: DateTime<Utc>,
  }
  ```

**Why This Module Exists:**
Consistent labeling is **critical for feedback loops**. If outcomes are mislabeled:
- Selector learns incorrect patterns
- Triage hypotheses are wrong
- Entire system degrades

Monitor ensures **ground truth** is accurate.

---

## 🏭 Build Layer

### Purpose
Deterministic compilation of mutated source code into Windows PE binaries.

---

### Module: `build/emitter`

**Purpose:** Compile Rust source to Windows executable with reproducible builds.

**Files:**

#### `build/emitter/src/lib.rs`
- **What:** Build orchestration API
- **Why:** Encapsulate complex build process (toolchain management, flags, artifact storage)
- **Main Function:**
  ```rust
  pub fn build_artifact(
      source: &str,
      target: BuildTarget, // x86_64-pc-windows-gnu
      options: BuildOptions,
  ) -> Result<ArtifactId, BuildError>
  ```
- **Responsibilities:**
  - Write source to temp directory
  - Invoke `cargo build --release --target <target>`
  - Set deterministic flags (RUSTFLAGS, SOURCE_DATE_EPOCH)
  - Compute SHA256 of output PE
  - Store artifact in `artifacts/builds/<sha256>/`
  - Return `ArtifactId` proto
- **Interactions:**
  - **Called by:** `controller/selector` (after mutations applied)
  - **Reads:** `config/emitter.toml` (toolchain version, output dir)
  - **Writes:** `/var/lib/edr-artifacts/<sha256>/loader.exe`

#### `build/emitter/src/compiler.rs` *(To Create)*
- **What:** Invoke Rust compiler with proper flags
- **Why:** Ensure deterministic builds (byte-identical outputs)
- **Flags:**
  ```bash
  RUSTFLAGS="-C link-arg=-Wl,--build-id=none"
  SOURCE_DATE_EPOCH=1609459200
  cargo +1.75.0 build --release --target x86_64-pc-windows-gnu
  ```
- **Toolchain Management:**
  - Pin specific Rust version (e.g., `1.75.0`)
  - Use `rustup run <toolchain>` to invoke
- **Verification:**
  - Build same source twice → compare SHA256 hashes (must match)

#### `build/emitter/src/artifact.rs` *(To Create)*
- **What:** Artifact storage and metadata
- **Why:** Track which artifacts were built, when, with what mutations
- **Operations:**
  - `store_artifact(path, artifact_id)` - Copy to artifact repository
  - `get_artifact(artifact_id) -> PathBuf` - Retrieve artifact path
  - `compute_artifact_id(path) -> ArtifactId` - SHA256 hash
- **Metadata File:** Store alongside binary (`artifacts/builds/<sha256>/metadata.json`)
  ```json
  {
    "artifact_id": "abc123...",
    "source_hash": "def456...",
    "mutations": ["ast.string_encrypt", "beh.sleep_before"],
    "built_at": "2025-01-15T10:30:00Z",
    "toolchain": "1.75.0",
    "target": "x86_64-pc-windows-gnu"
  }
  ```

**Why This Module Exists:**
Reproducible builds are **essential for differential analysis**. If builds are non-deterministic:
- Can't isolate which code changes caused detection (build variance confounds results)
- Can't reproduce findings (experiments not repeatable)

Emitter ensures **build variance = 0**.

---

## 👁️ Telemetry Layer

### Purpose
Collect, normalize, and index Windows telemetry (ETW, Event Logs, Defender alerts, forensics).

---

### Module: `telemetry/collector`

**Purpose:** Capture Windows telemetry and send to Elasticsearch.

**Files:**

#### `telemetry/collector/src/main.rs`
- **What:** Main telemetry collection daemon (runs on Windows VM as Administrator)
- **Why:** Centralize all telemetry capture (ETW, logs, Defender, forensics)
- **Responsibilities:**
  - Subscribe to ETW providers (Kernel-Process, Kernel-File, Threat-Intelligence, etc.)
  - Parse ETW events into normalized JSON
  - Collect Windows Event Logs (Security, Application, System)
  - Poll Defender alerts
  - Capture forensic artifacts (Prefetch, Amcache, USN Journal diffs)
  - Batch events and send to Elasticsearch (bulk insert)
- **Interactions:**
  - **Triggered by:** `worker/harness` (start before execution, stop after)
  - **Writes:** Elasticsearch `etw-*` and `rededr-*` indices
  - **Reads:** `config/collector.toml` (ES URL, providers, buffer size)

#### `telemetry/collector/src/etw.rs` *(To Create)*
- **What:** ETW event capture and parsing
- **Why:** Raw ETW events are binary blobs (need structured JSON)
- **Implementation:**
  - Use `krabs` crate (Rust ETW bindings) or `krabsetw`
  - Subscribe to providers:
    ```rust
    let providers = vec![
        "Microsoft-Windows-Kernel-Process",
        "Microsoft-Windows-Kernel-File",
        "Microsoft-Windows-Kernel-Network",
        "Microsoft-Windows-Threat-Intelligence",
        "Microsoft-Windows-PowerShell",
        "Microsoft-Windows-WMI-Activity",
    ];
    ```
  - Parse event schema → extract fields
  - Normalize to JSON:
    ```json
    {
      "run_id": "uuid",
      "artifact_id": "sha256",
      "pid": 1234,
      "provider": "Kernel-Process",
      "event_id": 1,
      "ts": "2025-01-15T10:30:00Z",
      "fields": { "image_name": "loader.exe", "parent_pid": 5678 }
    }
    ```

#### `telemetry/collector/src/rededr.rs`
- **What:** Process-centric view (aggregate events by process)
- **Why:** Easier querying for analysts (see all events for one process)
- **RedEDR Schema:**
  - One document per process
  - Aggregate all events (file, network, registry, etc.)
  - Compute derived features (execution duration, memory transitions, etc.)
- **Index:** `rededr-*` (separate from raw ETW)

#### `telemetry/collector/src/feature_extractor.rs`
- **What:** Extract high-level features from raw telemetry
- **Why:** Convert events to ML-ready features (for triage)
- **Features Extracted:**
  - `rwx_short_window` - Time from write to execute < threshold
  - `anon_thread_start` - Thread started in anonymous memory
  - `unsigned_child_of_signed` - Provenance anomaly
  - `string_entropy` - Average entropy of strings in memory
  - `com_clsid_count` - Number of unique COM objects activated
  - `bits_job_count` - BITS transfers initiated
- **Output:** Feature vector per run_id

#### `telemetry/collector/src/slo.rs`
- **What:** Collector SLO (Service Level Objective) tracking
- **Why:** Ensure telemetry quality (lost events = blind spots)
- **Metrics:**
  - `etw.lost_events` - Count of dropped events (buffer overflow)
  - `event_to_record_ms_p95` - Latency (event generated → indexed)
  - `collector.threads` - Number of worker threads
  - `collector.cache_pools` - Memory pool usage
- **Export:** Log to Elasticsearch or Prometheus

#### `telemetry/collector/src/elastic.rs` *(To Create)*
- **What:** Elasticsearch client and bulk indexing
- **Why:** Batch inserts for performance (don't index one-by-one)
- **Implementation:**
  - Use `elasticsearch` crate
  - Buffer events (e.g., 100 events or 5 seconds)
  - Bulk insert to ES
  - Retry on failure (transient network issues)
- **Index Naming:**
  - `etw-2025.01.15` (date-based, daily rotation)
  - `rededr-2025.01.15`
  - `runs-*` (run metadata)

**Why This Module Exists:**
Telemetry is the **observation layer** - without it, you're blind to what artifacts do. Collector ensures:
- Comprehensive coverage (kernel + user mode events)
- Low overhead (efficient ETW buffering)
- Data quality (SLO tracking, no lost events)

---

## 🎨 UI Layer

### Purpose
User interface for job submission, result visualization, and triage exploration.

---

### Module: `ui/backend`

**Purpose:** REST API for web frontend or CLI clients.

**Files:**

#### `ui/backend/src/main.rs`
- **What:** REST API server (Axum or Actix-web framework)
- **Why:** Provide HTTP interface for non-gRPC clients (web browser, curl)
- **Endpoints:**
  - `POST /api/jobs` - Submit job (proxies to Scheduler gRPC)
  - `GET /api/jobs/:id` - Get job status
  - `GET /api/runs/:id` - Get run details
  - `GET /api/triage/:run_id` - Get triage hypotheses
  - `GET /api/artifacts/:id` - Download artifact
  - `POST /api/rules` - Create detection rule
- **Interactions:**
  - **Calls:** `controller/scheduler` (via gRPC)
  - **Calls:** `controller/triage-engine` (via gRPC)
  - **Reads:** Elasticsearch (direct queries for dashboards)

**Why This Module Exists:**
Not all clients can use gRPC (browsers can't, curl is easier with REST). UI backend provides:
- HTTP interface (JSON in/out)
- WebSocket support (for real-time job status updates)
- CORS handling (for SPA frontends)

---

## 📚 Shared Libraries

### Module: `config`

**Purpose:** Centralized configuration management.

**Files:**

#### `config/src/lib.rs`
- **What:** Configuration structs and TOML parsing
- **Why:** All modules share same config format (consistency)
- **Structs:**
  ```rust
  pub struct SchedulerConfig {
      pub server: ServerConfig,
      pub queue: QueueConfig,
  }
  pub struct WorkerConfig {
      pub agent: AgentConfig,
      pub harness: HarnessConfig,
      pub etw: EtwConfig,
  }
  ```
- **Loading:**
  ```rust
  pub fn load_config<T: DeserializeOwned>(path: &Path) -> Result<T, ConfigError>
  ```
- **Interactions:**
  - **Used by:** Every module (read config on startup)
  - **Reads:** `config/*.toml` files

**Why This Module Exists:**
Configuration sprawl is a maintenance nightmare. Centralized config ensures:
- Single source of truth (no conflicting settings)
- Type safety (Rust structs, not raw TOML)
- Validation (catch errors at startup, not runtime)

---

## Data Flow Diagrams

### 1. Job Submission Flow

```
User/Client
    │
    ├──[gRPC: ScheduleJob]──▶ controller/scheduler
    │                            │
    │                            ├──[Validate JobRequest]
    │                            │
    │                            ├──[Generate JobId]
    │                            │
    │                            └──[Enqueue]──▶ controller/queue
    │
    └──[Returns JobResponse]
```

### 2. Mutation Selection Flow

```
controller/queue
    │
    ├──[Dequeue next job]──▶ controller/selector
    │                            │
    │                            ├──[Get avoid-list]──▶ controller/triage-engine
    │                            │                        │
    │                            │                        └──[Query ES for features]
    │                            │
    │                            ├──[Select mutations (exploration/exploitation)]
    │                            │
    │                            └──[Return Mutation list]
    │
    ├──[Apply mutations]──▶ controller/mutator
    │                            │
    │                            └──[Return mutated source]
    │
    └──[Build artifact]──▶ build/emitter
                             │
                             └──[Return ArtifactId]
```

### 3. Execution Flow

```
build/emitter
    │
    ├──[Send artifact]──▶ worker/agent (gRPC: RunSample)
    │                        │
    │                        ├──[Spawn harness]──▶ worker/harness
    │                        │                        │
    │                        │                        ├──[Execute artifact]
    │                        │                        │
    │                        │                        ├──[Timeout enforcement]
    │                        │                        │
    │                        │                        └──[Capture exit code]
    │                        │
    │                        ├──[Start collector]──▶ telemetry/collector
    │                        │                        │
    │                        │                        ├──[Subscribe ETW]
    │                        │                        │
    │                        │                        ├──[Parse events]
    │                        │                        │
    │                        │                        └──[Bulk insert to ES]
    │                        │
    │                        └──[Label outcome]──▶ worker/monitor
    │                                                │
    │                                                └──[Generate RunResult]
    │
    └──[Send RunResult]──▶ controller/selector (ReportOutcome)
```

### 4. Triage Flow

```
controller/selector
    │
    ├──[Request analysis]──▶ controller/triage-engine (AnalyzeRun)
    │                            │
    │                            ├──[Query telemetry]──▶ Elasticsearch
    │                            │                        │
    │                            │                        └──[Return events]
    │                            │
    │                            ├──[Extract features]
    │                            │
    │                            ├──[Train surrogate classifier]
    │                            │
    │                            ├──[Extract feature importances]
    │                            │
    │                            └──[Generate hypotheses]
    │
    └──[Receive AnalysisResponse (hypotheses + avoid-list)]
```

---

## Interface Contracts

### Protobuf Services Summary

#### Controller Layer
- **Controller** (`controller.proto`)
  - `ScheduleJob(JobRequest) -> JobResponse`
  - `GetJobStatus(JobStatusRequest) -> JobStatusResponse`
  - `SubmitTriage(TriageRequest) -> TriageResponse`
  - `QueryResults(QueryRequest) -> QueryResponse`

- **Selector** (`controller.proto`)
  - `SelectMutation(SelectionRequest) -> SelectionResponse`
  - `ReportOutcome(OutcomeReport) -> OutcomeAck`

- **Triage** (`controller.proto`)
  - `AnalyzeRun(AnalysisRequest) -> AnalysisResponse`
  - `GetAvoidList(AvoidListRequest) -> AvoidListResponse`

#### Worker Layer
- **WorkerAgent** (`worker.proto`)
  - `ExecuteBuild(BuildRequest) -> BuildResponse`
  - `RunSample(SampleRequest) -> SampleResponse`
  - `HealthCheck(HealthRequest) -> HealthResponse`
  - `StreamTelemetry(stream TelemetryData) -> TelemetryAck`

- **Harness** (`worker.proto`)
  - `Execute(ExecuteRequest) -> ExecuteResponse`
  - `Monitor(MonitorRequest) -> stream MonitorEvent`

---

## Summary: Why Each File Matters

| File | Purpose | Why It Can't Be Removed |
|------|---------|------------------------|
| **controller/scheduler** | Job entry point | Without it, no way to submit jobs |
| **controller/queue** | Job buffering | Without it, can't handle concurrent requests |
| **controller/selector** | Mutation intelligence | Without it, no feedback loop (random fuzzing only) |
| **controller/mutator** | Code transformation | Without it, no variants (static artifact) |
| **controller/triage-engine** | Explainability | Without it, blind fuzzing (no "why") |
| **build/emitter** | Compilation | Without it, can't execute mutated code |
| **worker/agent** | VM interface | Without it, can't communicate with Workers |
| **worker/harness** | Execution sandbox | Without it, no timeout/safety (runaway processes) |
| **worker/monitor** | Outcome labeling | Without it, no ground truth (feedback loop breaks) |
| **telemetry/collector** | Observation layer | Without it, blind to what artifacts do |
| **config** | Configuration | Without it, hard-coded settings (no flexibility) |

**Core Insight:** Every module serves a **specific role** in the feedback loop. Removing any one breaks the loop:
- No Mutator → no variants
- No Selector → no intelligence
- No Triage → no explainability
- No Collector → no observation
- No Monitor → no ground truth

This architecture is **minimally complete** - every file exists for a reason.
