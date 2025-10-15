# AutoMutate++ Development Guide
## Phase 1: Basic Loader with Minimal Mutations & Telemetry Collection

---

## Overview

This guide walks you through setting up a **minimal working system** that can:
1. Build a simple Windows loader with basic mutations
2. Deploy it to a sandboxed Windows VM
3. Collect ETW telemetry
4. Verify end-to-end communication between Controller (Linux) and Worker (Windows VM)

**Goal:** Prove the architecture works before adding complexity.

---

## Architecture Summary (Minimal Setup)

```
┌─────────────────────────────────────────────┐
│ Linux Host (Controller)                     │
│  - Scheduler (queue jobs)                   │
│  - Queue (simple FIFO)                      │
│  - Selector (random mutation picker)        │
│  - Mutator (2-3 basic transforms)           │
│  - Emitter (build Rust loader)              │
│  - Collector (receive telemetry)            │
│  - Elasticsearch (store events)             │
└──────────────┬──────────────────────────────┘
               │ gRPC over network
┌──────────────▼──────────────────────────────┐
│ Windows VM (Worker)                         │
│  - Agent (gRPC server)                      │
│  - Harness (execute + timeout)              │
│  - Monitor (label outcome)                  │
│  - ETW Collector (minimal providers)        │
└─────────────────────────────────────────────┘
```

---

## Part 1: Infrastructure Setup

### 1.1 Linux Host (Controller Machine)

**Prerequisites:**
- Ubuntu 22.04+ or similar
- Rust toolchain (stable + nightly for LLVM IR experiments later)
- Docker or Podman (for Elasticsearch)
- Network access to Windows VM

**Steps:**

1. **Install Rust & Tools**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   rustup toolchain install nightly
   rustup component add llvm-tools-preview
   cargo install cargo-watch
   ```

2. **Install Protobuf Compiler**
   ```bash
   sudo apt update
   sudo apt install -y protobuf-compiler libprotobuf-dev
   protoc --version  # Should be 3.12+
   ```

3. **Setup Elasticsearch (Docker)**
   - Create `docker-compose.yml`:
     ```yaml
     version: '3.8'
     services:
       elasticsearch:
         image: docker.elastic.co/elasticsearch/elasticsearch:8.11.0
         environment:
           - discovery.type=single-node
           - xpack.security.enabled=false
           - "ES_JAVA_OPTS=-Xms2g -Xmx2g"
         ports:
           - "9200:9200"
         volumes:
           - esdata:/usr/share/elasticsearch/data

       kibana:
         image: docker.elastic.co/kibana/kibana:8.11.0
         ports:
           - "5601:5601"
         environment:
           - ELASTICSEARCH_HOSTS=http://elasticsearch:9200
         depends_on:
           - elasticsearch

     volumes:
       esdata:
     ```
   - Start: `docker-compose up -d`
   - Verify: `curl http://localhost:9200` should return cluster info
   - Access Kibana at `http://localhost:5601`

4. **Network Configuration**
   - Ensure firewall allows gRPC traffic (default ports: 50051 for Controller, 50052 for Worker)
   - If using VirtualBox/VMware, configure **Host-Only** or **Bridged** networking so VM can reach host

---

### 1.2 Windows VM (Worker Machine)

**VM Specifications:**
- Windows 10 22H2 or Windows 11 (Pro or Enterprise for better telemetry)
- 4 GB RAM minimum (8 GB recommended)
- 2 vCPUs
- Snapshot capability (restore to clean state between runs)

**Setup Steps:**

1. **Install Windows**
   - Use evaluation ISO from Microsoft
   - Disable Windows Update during initial setup (you want reproducible state)
   - Set static IP or note DHCP-assigned IP

2. **Install Rust Toolchain (x86_64-pc-windows-msvc)**
   ```powershell
   # Download rustup-init.exe from https://rustup.rs
   rustup-init.exe
   rustup default stable-x86_64-pc-windows-msvc
   rustup target add x86_64-pc-windows-msvc
   ```

3. **Install Visual Studio Build Tools**
   - Download from https://visualstudio.microsoft.com/downloads/
   - Select "Desktop development with C++"
   - Required for linking Rust binaries on Windows

4. **Install Protobuf Compiler**
   - Download pre-built `protoc.exe` from https://github.com/protocolbuffers/protobuf/releases
   - Extract to `C:\protoc\`
   - Add `C:\protoc\bin` to PATH

5. **Windows Defender Configuration**
   - **DO NOT DISABLE** Defender (you want to test against it)
   - Enable real-time protection
   - Enable cloud-delivered protection
   - Note: For initial testing, you may need to exclude your build directory, but remove exclusions once telemetry collection works

6. **Enable ETW Providers (Check Availability)**
   ```powershell
   # List available ETW providers
   logman query providers > etw_providers.txt

   # Verify key providers exist:
   # - Microsoft-Windows-Kernel-Process
   # - Microsoft-Windows-Kernel-File
   # - Microsoft-Windows-Kernel-Network
   # - Microsoft-Windows-Threat-Intelligence (may require admin)
   ```

7. **Network Configuration**
   - Test connectivity to Linux host: `Test-NetConnection <linux-ip> -Port 50051`
   - Configure Windows Firewall to allow inbound gRPC (port 50052)

8. **Snapshot the VM**
   - Create a "clean baseline" snapshot
   - You'll restore to this after each test run

---

## Part 2: Codebase Setup & Module Responsibilities

### 2.1 Project Structure Overview

```
Automated-Analysis-and-Mutation-of-Software-Artifacts-against-AV-EDR/
├── controller/           # Linux-side orchestration
│   ├── scheduler/        # Job queue manager
│   ├── queue/            # Priority queue & corpus
│   ├── selector/         # Mutation selection logic
│   ├── mutator/          # AST/IR/binary transforms
│   ├── rule-manager/     # Detection rule storage (future)
│   ├── differential-analyzer/  # Token→detection mapping (future)
│   ├── triage-engine/    # Hypothesis generation (future)
│   ├── triage-client/    # CLI for manual triage (future)
│   └── proto/            # Protobuf definitions
│
├── build/
│   └── emitter/          # Deterministic builds (Rust → Windows PE)
│
├── worker/               # Windows VM components
│   ├── agent/            # gRPC server (WorkerAgent service)
│   ├── harness/          # Execute artifacts with timeout
│   ├── harness-ipc/      # IPC between agent & harness
│   └── monitor/          # Outcome labeling
│
├── telemetry/
│   └── collector/        # ETW → Elasticsearch pipeline
│
├── ui/
│   └── backend/          # REST API (future)
│
├── config/               # Shared configuration structs
└── artifacts/            # Your test loaders & templates
    └── loaders/
        └── basic-loader/ # Minimal Rust loader
```

---

### 2.2 Module-by-Module Guide (What to Implement)

---

#### **A. Protobuf Definitions** (`controller/proto/`)

**Files:**
- `common.proto` - Already defined (JobId, ArtifactId, RunResult, Mutation)
- `controller.proto` - Controller, Selector, Triage services
- `worker.proto` - WorkerAgent, Harness services

**What to do:**
- ✅ **Already complete** based on your existing files
- **Action:** Just verify they compile:
  ```bash
  cd controller
  cargo build --release
  ```

**Why it matters:**
These define the contract between Controller and Worker. If protobuf compiles, gRPC communication will work.

---

#### **B. Scheduler** (`controller/scheduler/src/main.rs`)

**Purpose:**
Accept job requests, assign job IDs, enqueue to Queue.

**What to implement:**

1. **gRPC Server Setup**
   - Implement `Controller` service from `controller.proto`
   - Bind to `0.0.0.0:50051` (allow VM to connect)

2. **`ScheduleJob` RPC**
   - Generate `JobId` (format: `job-NNNNNN`, use incrementing counter or UUID)
   - Validate `JobRequest` (check `artifact_type` is "exe", `source` path exists)
   - Send job to Queue via internal channel or shared state (e.g., `Arc<Mutex<VecDeque<Job>>>`)
   - Return `JobResponse` with `accepted: true` and estimated duration

3. **`GetJobStatus` RPC**
   - Query job state from Queue
   - Return status: `queued`, `building`, `running`, `completed`, `failed`

4. **Minimal State Management**
   - Use a simple `HashMap<JobId, JobStatus>` wrapped in `Arc<RwLock<...>>`
   - Update status as jobs progress through pipeline

**Files to modify:**
- `controller/scheduler/src/main.rs` - gRPC server + routing
- `controller/scheduler/src/state.rs` (create) - Job state storage

**Configuration:**
- Read from `config/scheduler.toml`:
  ```toml
  [server]
  bind_addr = "0.0.0.0:50051"
  max_jobs = 100

  [queue]
  addr = "127.0.0.1:50053"  # Internal communication
  ```

**Testing:**
- Use `grpcurl` to test:
  ```bash
  grpcurl -plaintext -d '{"name":"test","artifact_type":"exe","source":"./test.rs"}' \
    localhost:50051 edr.controller.Controller/ScheduleJob
  ```

---

#### **C. Queue** (`controller/queue/src/main.rs`)

**Purpose:**
Store jobs in priority order, provide next job to Selector.

**What to implement:**

1. **Simple FIFO Queue**
   - For MVP, just use `VecDeque<Job>` (no fancy prioritization yet)
   - Expose via internal gRPC or shared memory (since Scheduler/Queue are on same host)

2. **Job Storage**
   - Fields: `job_id`, `artifact_type`, `source_path`, `mutations`, `priority`, `status`
   - Persist to disk (optional for MVP, but recommended): use `sled` or `rocksdb` crate

3. **APIs (Internal gRPC or Function Calls):**
   - `enqueue(job: Job)` - Add to queue
   - `dequeue() -> Option<Job>` - Get next job (called by Selector)
   - `update_status(job_id, status)` - Update job state

**Files to modify:**
- `controller/queue/src/main.rs` - Queue logic
- `controller/queue/src/storage.rs` (create) - Persistent storage

**Configuration:**
- `config/queue.toml`:
  ```toml
  [storage]
  path = "/var/lib/edr-queue"

  [queue]
  max_size = 1000
  ```

**Testing:**
- Unit test: enqueue 10 jobs, dequeue in order
- Integration test: Scheduler → Queue → Selector chain

---

#### **D. Selector** (`controller/selector/src/main.rs`)

**Purpose:**
Pick next job from Queue, choose mutations (initially random, later feedback-driven).

**What to implement:**

1. **gRPC Server** (`Selector` service)
   - `SelectMutation` RPC:
     - Input: `JobId`, `avoid_features` (empty list for MVP)
     - Output: List of `Mutation` messages (e.g., `[{id: "ast.string_encrypt", params: {}}]`)

2. **Mutation Selection Logic (MVP):**
   - For now, **randomly pick 1-2 mutations** from a hardcoded list:
     ```rust
     const AVAILABLE_MUTATIONS: &[&str] = &[
         "ast.string_encrypt",
         "ast.import_hash",
         "beh.sleep_before",
     ];
     ```
   - Return as `Mutation` proto messages

3. **Feedback Loop (Stub for Now):**
   - `ReportOutcome` RPC:
     - Accept `RunResult` from Worker
     - For MVP, just log outcome (detected/not_detected)
     - Later: update probabilities based on success rate

**Files to modify:**
- `controller/selector/src/main.rs` - gRPC + selection logic
- `controller/selector/src/mutations.rs` (create) - Mutation registry

**Configuration:**
- `config/selector.toml`:
  ```toml
  [mutations]
  exploration_rate = 0.3  # 30% random, 70% exploit (future)
  ```

**Testing:**
- Call `SelectMutation` 100 times, verify random distribution
- Ensure no crashes if `avoid_features` list is populated

---

#### **E. Mutator** (`controller/mutator/src/`)

**Purpose:**
Apply AST/IR/binary transforms to source code or binaries.

**What to implement (MVP - 3 basic mutations):**

1. **`ast.string_encrypt`**
   - **Input:** Rust source code with plaintext strings
   - **Transform:** Replace string literals with XOR-encoded bytes + decode stub
   - **Example:**
     ```rust
     // Before:
     let msg = "Hello";

     // After:
     let msg = decode_xor(&[0x48^0xAA, 0x65^0xAA, ...], 0xAA);
     ```
   - **Implementation:** Use `syn` crate to parse Rust AST, find `LitStr` nodes, replace

2. **`ast.import_hash`**
   - **Input:** Rust code with direct Windows API imports
   - **Transform:** Replace static imports with `GetProcAddress` + API hashing
   - **Example:**
     ```rust
     // Before:
     use winapi::um::processthreadsapi::GetCurrentProcess;

     // After:
     let get_current_process = resolve_api_hash(0x12345678); // hash of "GetCurrentProcess"
     ```

3. **`beh.sleep_before`**
   - **Input:** Rust loader main function
   - **Transform:** Insert `std::thread::sleep(Duration::from_secs(5))` at start
   - **Purpose:** Benign deconditioning (delay sandbox detection)

**Files to create:**
- `controller/mutator/src/lib.rs` - Public API
- `controller/mutator/src/ast/string_encrypt.rs`
- `controller/mutator/src/ast/import_hash.rs`
- `controller/mutator/src/behavioral/sleep.rs`

**Key Functions:**
```rust
pub fn apply_mutation(
    source: &str,
    mutation: &Mutation
) -> Result<String, MutationError> {
    match mutation.id.as_str() {
        "ast.string_encrypt" => ast::string_encrypt(source, &mutation.params),
        "ast.import_hash" => ast::import_hash(source, &mutation.params),
        "beh.sleep_before" => behavioral::sleep(source, &mutation.params),
        _ => Err(MutationError::UnknownMutation),
    }
}
```

**Dependencies:**
- `syn` - Rust AST parsing
- `quote` - Code generation
- `proc-macro2` - Token manipulation

**Testing:**
- Unit test each mutation with sample input
- Verify mutated code compiles with `rustc`

---

#### **F. Emitter** (`build/emitter/src/`)

**Purpose:**
Take mutated source code, compile to Windows PE with deterministic settings.

**What to implement:**

1. **Build Pipeline**
   - Accept: mutated Rust source + `BuildRequest` proto
   - Output: Windows `.exe` in `artifacts/builds/<artifact_id>/`

2. **Deterministic Build Settings**
   - Pin Rust toolchain version (e.g., `1.75.0`)
   - Use `RUSTFLAGS="-C link-arg=-Wl,--build-id=none"` (deterministic linking)
   - Set `SOURCE_DATE_EPOCH` environment variable (for reproducible timestamps)
   - Disable debug info: `--release` + `strip = true` in Cargo.toml

3. **Cross-Compilation to Windows**
   - If running on Linux, use `x86_64-pc-windows-gnu` target:
     ```bash
     rustup target add x86_64-pc-windows-gnu
     sudo apt install gcc-mingw-w64
     ```
   - Or build on Windows VM directly (simpler for MVP)

4. **Artifact ID Generation**
   - After build, compute SHA256 of PE file
   - Store in `artifacts/builds/<sha256>/loader.exe`
   - Return `ArtifactId` in `BuildResponse`

**Files to modify:**
- `build/emitter/src/main.rs` - Build orchestration
- `build/emitter/src/compiler.rs` (create) - Invoke `cargo build`
- `build/emitter/src/artifact.rs` (create) - SHA256 + storage

**Configuration:**
- `config/emitter.toml`:
  ```toml
  [build]
  toolchain = "1.75.0"
  target = "x86_64-pc-windows-gnu"
  output_dir = "/var/lib/edr-artifacts"

  [reproducibility]
  source_date_epoch = 1609459200  # 2021-01-01
  ```

**Testing:**
- Build same source twice, verify byte-identical outputs (determinism test)
- Build with mutation, verify it runs on Windows

---

#### **G. Worker Agent** (`worker/agent/src/main.rs`)

**Purpose:**
Windows gRPC server that receives build/execution requests from Controller.

**What to implement:**

1. **gRPC Server Setup**
   - Implement `WorkerAgent` service from `worker.proto`
   - Bind to `0.0.0.0:50052`
   - Use Tonic (Rust gRPC framework)

2. **`ExecuteBuild` RPC (Optional for MVP)**
   - If building on Windows VM instead of cross-compiling:
     - Receive source code in `BuildRequest`
     - Write to temp directory
     - Invoke `cargo build --release --target x86_64-pc-windows-msvc`
     - Return `BuildResponse` with artifact path

3. **`RunSample` RPC**
   - Receive artifact path from Controller
   - Invoke Harness (separate process)
   - Return execution outcome

4. **`HealthCheck` RPC**
   - Return VM status (CPU%, memory%, active jobs)
   - Use `sysinfo` crate

5. **`StreamTelemetry` RPC (Bidirectional Stream)**
   - Receive telemetry from Harness
   - Forward to Collector on Linux host

**Files to modify:**
- `worker/agent/src/main.rs` - gRPC server
- `worker/agent/src/executor.rs` (create) - Spawn Harness process
- `worker/agent/src/health.rs` (create) - System metrics

**Configuration:**
- `config/worker.toml`:
  ```toml
  [server]
  bind_addr = "0.0.0.0:50052"

  [controller]
  addr = "192.168.1.100:50051"  # Linux host IP

  [harness]
  path = "C:\\edr-worker\\harness.exe"
  timeout_seconds = 30
  ```

**Testing:**
- From Linux: `grpcurl -plaintext <vm-ip>:50052 edr.worker.WorkerAgent/HealthCheck`

---

#### **H. Harness** (`worker/harness/src/main.rs`)

**Purpose:**
Execute artifact in isolated process with timeout, capture basic-block traces.

**What to implement:**

1. **Artifact Execution**
   - Spawn artifact as child process with `std::process::Command`
   - Set timeout (e.g., 30 seconds)
   - Capture stdout/stderr

2. **Basic-Block Tracing (Simple MVP)**
   - For MVP, skip hardware breakpoints (complex)
   - Instead: instrument loader with print statements
   - Or use Windows ETW to capture module loads / thread starts

3. **Timeout Enforcement**
   - Use `tokio::time::timeout` or Windows Job Objects
   - If timeout, kill process tree

4. **Outcome Detection**
   - Monitor for:
     - Defender alert (poll `Get-MpThreatDetection` via PowerShell)
     - Process crash (exit code != 0)
     - Successful execution (exit code 0, no alerts)

**Files to modify:**
- `worker/harness/src/main.rs` - Execution logic
- `worker/harness/src/timeout.rs` (create) - Job object wrapper
- `worker/harness/src/defender.rs` (create) - Query Defender status

**IPC with Agent:**
- Use named pipes or gRPC (`Harness` service)
- Send `MonitorEvent` messages (started, running, completed)

**Testing:**
- Run benign exe (calc.exe), verify it executes and exits
- Run with timeout, verify termination after 30 seconds

---

#### **I. Monitor** (`worker/monitor/src/`)

**Purpose:**
Label execution outcome as `detected | not_detected | noisy | crash`.

**What to implement:**

1. **Outcome Labeling Logic**
   ```rust
   fn label_outcome(
       exit_code: i32,
       defender_alerts: Vec<Alert>,
       execution_time: Duration,
   ) -> RunStatus {
       if !defender_alerts.is_empty() {
           return RunStatus::Detected;
       }
       if exit_code != 0 {
           return RunStatus::Crash;
       }
       if execution_time < Duration::from_secs(1) {
           return RunStatus::Noisy; // Too fast, suspicious
       }
       RunStatus::NotDetected
   }
   ```

2. **Generate `RunResult` Proto**
   - Populate all fields from `common.proto`
   - Include timing, exit code, alert level

3. **Send to Selector**
   - Call `Selector::ReportOutcome` gRPC

**Files to modify:**
- `worker/monitor/src/lib.rs` - Labeling logic
- `worker/monitor/src/defender.rs` (create) - Parse Defender alerts

**Testing:**
- Mock scenarios: clean exit, crash, Defender block
- Verify correct labels

---

#### **J. Telemetry Collector** (`telemetry/collector/src/main.rs`)

**Purpose:**
Collect ETW events from Windows VM, normalize, send to Elasticsearch.

**What to implement:**

1. **ETW Event Capture (Windows-side component)**
   - Use `krabs` crate (Rust ETW library) or `krabsetw` bindings
   - Subscribe to providers:
     ```rust
     let providers = vec![
         "Microsoft-Windows-Kernel-Process",
         "Microsoft-Windows-Kernel-File",
         "Microsoft-Windows-Threat-Intelligence", // Requires admin
     ];
     ```
   - Parse events into normalized schema

2. **Event Normalization**
   - Convert ETW binary format to JSON:
     ```json
     {
       "run_id": "uuid",
       "artifact_id": "sha256",
       "pid": 1234,
       "provider": "Kernel-Process",
       "event_id": 1,
       "ts": "2025-01-15T10:30:00Z",
       "fields": {
         "image_name": "loader.exe",
         "parent_pid": 5678
       }
     }
     ```

3. **Send to Elasticsearch**
   - Use `elasticsearch` crate
   - Index to `etw-*` (e.g., `etw-2025.01.15`)
   - Bulk insert for performance

4. **Collector Runs on Windows VM**
   - Start before Harness executes artifact
   - Stop after execution + 5 second grace period

**Files to create:**
- `telemetry/collector/src/main.rs` - ETW capture
- `telemetry/collector/src/etw.rs` - Provider setup
- `telemetry/collector/src/elastic.rs` - Indexing logic

**Configuration:**
- `config/collector.toml`:
  ```toml
  [etw]
  buffer_size_kb = 1024
  providers = [
    "Microsoft-Windows-Kernel-Process",
    "Microsoft-Windows-Kernel-File",
  ]

  [elasticsearch]
  url = "http://192.168.1.100:9200"
  index_prefix = "etw-"
  ```

**Privileges:**
- Run as Administrator (required for ETW Threat-Intelligence provider)

**Testing:**
- Run collector, execute `notepad.exe`, verify events in Elasticsearch:
  ```bash
  curl "http://localhost:9200/etw-*/_search?q=image_name:notepad.exe"
  ```

---

#### **K. Basic Loader** (`artifacts/loaders/basic-loader/src/main.rs`)

**Purpose:**
Minimal Rust loader to test mutations and telemetry. NOT a real payload, just a sandbox test.

**What to implement:**

1. **Main Function**
   ```rust
   use std::thread;
   use std::time::Duration;

   fn main() {
       println!("Loader started");

       // Benign operations (generate telemetry)
       thread::sleep(Duration::from_secs(2));

       let _ = std::fs::read_to_string("C:\\Windows\\System32\\drivers\\etc\\hosts");

       println!("Loader finished");
   }
   ```

2. **Mutation Targets**
   - String literal `"Loader started"` - target for `ast.string_encrypt`
   - File read - generates File ETW events
   - Sleep - target for `beh.sleep_before` mutation

3. **No Malicious Behavior**
   - DO NOT include process injection, shellcode execution, etc.
   - This is purely for testing telemetry and mutations

**Files to create:**
- `artifacts/loaders/basic-loader/src/main.rs`
- `artifacts/loaders/basic-loader/Cargo.toml`

**Testing:**
- Build and run on Windows manually
- Verify it generates ETW events
- Verify Defender does NOT flag it (it's benign)

---

## Part 3: Integration & Testing Flow

### 3.1 End-to-End Test Scenario

**Objective:** Submit a job, mutate loader, build, execute on VM, collect telemetry.

**Steps:**

1. **Start Infrastructure**
   ```bash
   # Linux Host
   docker-compose up -d  # Elasticsearch + Kibana

   cd controller/scheduler
   cargo run --release &

   cd controller/queue
   cargo run --release &

   cd controller/selector
   cargo run --release &

   cd telemetry/collector
   cargo run --release &  # If collector runs on Linux side
   ```

2. **Start Worker on Windows VM**
   ```powershell
   cd worker\agent
   cargo run --release
   ```

3. **Submit Test Job**
   ```bash
   grpcurl -plaintext -d '{
     "name": "test-loader-mutation",
     "artifact_type": "exe",
     "source": "./artifacts/loaders/basic-loader/src/main.rs",
     "mutation_strategies": ["ast.string_encrypt", "beh.sleep_before"],
     "priority": 1
   }' localhost:50051 edr.controller.Controller/ScheduleJob
   ```

4. **Watch Logs**
   - Scheduler: Job accepted, enqueued
   - Selector: Mutations chosen
   - Mutator: Source transformed
   - Emitter: Build succeeded
   - Worker Agent: Build/artifact received
   - Harness: Execution started
   - Collector: ETW events captured

5. **Query Elasticsearch**
   ```bash
   curl "http://localhost:9200/etw-*/_search?pretty" \
     -H 'Content-Type: application/json' \
     -d '{"query":{"match":{"fields.image_name":"loader.exe"}}}'
   ```

6. **Verify RunResult**
   - Check Kibana dashboard for:
     - Process start event
     - File read event
     - Process exit event
   - Confirm `status: not_detected` (benign loader)

---

### 3.2 Failure Scenarios to Test

1. **Build Failure**
   - Submit invalid Rust code
   - Verify `BuildResponse.success = false`

2. **Timeout**
   - Modify loader to loop infinitely
   - Verify Harness kills after 30 seconds
   - Verify `RunResult.status = "crash"` or timeout label

3. **Network Failure**
   - Stop Collector
   - Verify Worker retries or queues telemetry

4. **Defender Detection**
   - Intentionally add EICAR test string to loader
   - Verify `RunResult.status = "detected"`
   - Verify Defender alert in telemetry

---

### 3.3 Validation Checklist

- [ ] Scheduler accepts job and returns `job_id`
- [ ] Selector picks mutations from available list
- [ ] Mutator transforms source code (verify with `diff`)
- [ ] Emitter produces Windows PE (verify with `file` command)
- [ ] Worker Agent receives and executes artifact
- [ ] Harness enforces 30-second timeout
- [ ] Collector captures ETW events (at least Process/File)
- [ ] Elasticsearch contains indexed events with correct `run_id`
- [ ] Monitor labels outcome correctly (not_detected for benign loader)
- [ ] Selector receives `OutcomeReport` (check logs)

---

## Part 4: Configuration Files

### 4.1 Controller Configuration (`config/controller.toml`)

```toml
[scheduler]
bind_addr = "0.0.0.0:50051"
max_concurrent_jobs = 10

[queue]
storage_path = "/var/lib/edr-queue"
max_queue_size = 1000

[selector]
bind_addr = "127.0.0.1:50053"
exploration_rate = 0.3

[mutator]
max_transforms_per_job = 3

[emitter]
build_dir = "/var/lib/edr-artifacts"
toolchain = "1.75.0"
target = "x86_64-pc-windows-gnu"

[collector]
elasticsearch_url = "http://localhost:9200"
batch_size = 100
flush_interval_ms = 5000
```

### 4.2 Worker Configuration (`config/worker.toml`)

```toml
[agent]
bind_addr = "0.0.0.0:50052"
controller_addr = "192.168.1.100:50051"  # Replace with Linux host IP

[harness]
path = "C:\\edr-worker\\harness.exe"
timeout_seconds = 30
kill_on_timeout = true

[etw]
buffer_size_kb = 1024
providers = [
  "Microsoft-Windows-Kernel-Process",
  "Microsoft-Windows-Kernel-File",
]

[collector]
elasticsearch_url = "http://192.168.1.100:9200"
index_prefix = "etw-"
```

---

## Part 5: Debugging & Troubleshooting

### 5.1 Common Issues

**Issue:** gRPC connection refused
**Fix:**
- Verify firewall rules (Linux & Windows)
- Test with `telnet <host> <port>`
- Check `bind_addr` is `0.0.0.0` (not `127.0.0.1`)

**Issue:** ETW events not appearing in Elasticsearch
**Fix:**
- Run Collector as Administrator
- Verify providers exist: `logman query providers | findstr Process`
- Check Elasticsearch is reachable from VM
- Use Wireshark to confirm HTTP traffic to Elasticsearch

**Issue:** Build fails on Linux (cross-compilation)
**Fix:**
- Install MinGW: `sudo apt install gcc-mingw-w64`
- Or build directly on Windows VM (simpler for MVP)

**Issue:** Harness doesn't enforce timeout
**Fix:**
- Use Windows Job Objects (not just `process::kill()`)
- Test with infinite loop: `loop {}`

**Issue:** Mutations break compilation
**Fix:**
- Add regression tests for each mutation
- Use `cargo check` before full build
- Validate AST transformations with `syn::parse_file`

---

### 5.2 Logging Strategy

**Controller (Linux):**
- Use `tracing` crate with `INFO` level
- Log to stdout + file: `/var/log/edr-controller/`
- Key events:
  - Job received
  - Mutations selected
  - Build started/completed
  - Telemetry received

**Worker (Windows):**
- Use `tracing` with `DEBUG` level (more verbose)
- Log to: `C:\edr-worker\logs\`
- Key events:
  - Agent started
  - Artifact received
  - Execution started/finished
  - ETW events captured (count every 1000 events)

**Log Rotation:**
- Use `tracing-appender` with daily rotation
- Keep last 7 days

---

## Part 6: Next Steps After MVP

Once the basic system works, expand in this order:

### Phase 2: Advanced Mutations
- LLVM IR transforms (requires nightly Rust)
- Binary patching with `goblin` crate
- Import table manipulation

### Phase 3: Feedback Loop
- Implement Triage Engine (surrogate classifier)
- Use scikit-learn via PyO3 bindings
- Generate feature-avoid lists

### Phase 4: Differential Analysis
- Submit artifacts to scan-time Defender (via `MpCmdRun.exe`)
- Compare scan-time vs. runtime signals
- Build token→detection probability map

### Phase 5: Scale Worker Pool
- Add 2-3 more VMs
- Implement load balancing in Scheduler
- Parallel execution

### Phase 6: UI & Visualization
- Build REST API (Axum framework)
- Create Kibana dashboards for telemetry
- Real-time job status page

---

## Part 7: Safety & Ethics Reminders

1. **Air-gapped Network**
   - VMs should NOT have internet access
   - Use isolated network for Controller ↔ Worker communication

2. **No Real Payloads**
   - All loaders are benign test cases
   - Do not load shellcode or exploit code

3. **Data Sanitization**
   - Before sharing telemetry data, remove:
     - Usernames
     - Host identifiers
     - File paths (replace with placeholders)

4. **Responsible Disclosure**
   - If you discover EDR bypass technique, report to vendor first
   - Wait 90 days before public disclosure

---

## Summary

This guide gives you a **concrete roadmap** to build Phase 1:

1. **Infrastructure:** Linux Controller + Windows VM + Elasticsearch
2. **Modules:** Scheduler, Queue, Selector, Mutator, Emitter, Worker Agent, Harness, Monitor, Collector
3. **Artifacts:** Basic benign loader with 3 simple mutations
4. **Telemetry:** ETW (Process + File events) → Elasticsearch
5. **Testing:** End-to-end job submission → execution → telemetry collection

**Definition of Done:**
- Submit a job via gRPC
- See mutated loader execute on Windows VM
- Query ETW events in Kibana
- Verify outcome label (not_detected)

Once this works, you have a **solid foundation** to add advanced mutations, feedback loops, and differential analysis.
