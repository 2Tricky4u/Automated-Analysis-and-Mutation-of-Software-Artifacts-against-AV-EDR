# Integrating RedEDR with AutoMutate++ EDR Triage Framework

This guide explains how to integrate RedEDR telemetry collection into the AutoMutate++ project architecture, specifically for the `telemetry/collector` module and `worker/agent` coordination.

## Quick Start (TL;DR)

**Chosen Method:** JSON File Export + REST API Control

1. **Worker Agent** (`worker/agent`) → REST API calls to RedEDR for lifecycle (reset, kill, save)
2. **RedEDR** → Writes events to `C:\RedEDR\Data\*.events.json`
3. **Telemetry Collector** (`telemetry/collector`) → Watches files, parses, ships to Elasticsearch
4. **Triage Engine** (`controller/triage-engine`) → Queries ES, extracts features, generates avoid-list
5. **Selector** (`controller/selector`) → Uses avoid-list to pick next mutations

**Why this approach:** Decoupled, reliable, debuggable, aligns with existing architecture (see Summary section for full rationale).

**Implementation:** See code snippets in "Implementation Guide" section below.

---

## Table of Contents

1. [Architecture Context](#architecture-context) - System overview and integration strategy
2. [Deployment Configuration](#deployment-configuration) - RedEDR setup on Worker VMs
3. [Integration Method](#integration-method-1-rest-api-control--json-file-export) - REST API workflow
4. [REST API Endpoints](#rest-api-endpoints) - RedEDR HTTP API reference
5. [JSON File Export](#method-2-json-file-export) - File format and access methods
6. [Implementation Guide](#implementation-guide-for-automutate) - Rust code for all modules
7. [Event Types](#event-types-and-fields) - RedEDR JSON schema documentation
8. [Pipeline Mapping](#how-rededr-telemetry-maps-to-automutate-pipeline) - How events feed triage/selector
9. [Performance Optimization](#performance-optimization) - Tuning RedEDR for low overhead
10. [Troubleshooting](#troubleshooting) - Common issues and solutions
11. [Summary](#summary-chosen-integration-strategy) - Decision rationale and checklist

---

## Architecture Context

AutoMutate++ uses a **Controller-Worker** architecture:
- **Controller** (Linux): Orchestrates mutation, triage, and analysis via gRPC
- **Worker** (Windows VM): Executes artifacts and collects telemetry
- **Telemetry Collector**: Ships events from Workers to Elasticsearch
- **RedEDR**: Runs on Worker VMs to capture ETW/kernel/DLL hook events

```
┌──────────────────────────────────────────────────────────────────┐
│                    Controller (Linux)                            │
│  ┌──────────┐   ┌─────────┐   ┌──────────────┐   ┌──────────┐  │
│  │Scheduler │──▶│ Mutator │──▶│   Selector   │──▶│  Triage  │  │
│  └──────────┘   └─────────┘   └──────────────┘   └──────────┘  │
│                                      │                ▲          │
└──────────────────────────────────────┼────────────────┼──────────┘
                                       │ gRPC           │
                                       ▼                │
┌──────────────────────────────────────────────────────┼──────────┐
│                    Worker VM (Windows)                │          │
│  ┌────────────┐    ┌──────────┐    ┌────────────────▼───────┐  │
│  │Worker Agent│───▶│ Harness  │    │ Telemetry Collector    │  │
│  │  (gRPC)    │    │(Execute) │    │  (Ship to ES)          │  │
│  └────────────┘    └────┬─────┘    └──────▲─────────────────┘  │
│                         │                   │                    │
│                         ▼                   │                    │
│                   ┌──────────┐       ┌─────┴──────┐             │
│                   │ Artifact │──────▶│  RedEDR    │             │
│                   │ (PE/EXE) │ hooks │ (C++ ETW)  │             │
│                   └──────────┘       └────────────┘             │
└──────────────────────────────────────────────────────────────────┘
                                              │
                                              ▼
                                       ┌─────────────┐
                                       │Elasticsearch│
                                       │  (Indexing) │
                                       └─────────────┘
```

## Integration Strategy: JSON File Export + REST API (Recommended)

**Why this approach:**
1. **Decoupling**: RedEDR runs independently of Worker Agent (survives crashes)
2. **Performance**: Async file watching avoids blocking Worker execution
3. **REST API for control**: Use REST endpoints for lifecycle management (reset, lock)
4. **Simplicity**: No complex IPC, no pipe handling, easy debugging
5. **Alignment**: Matches project architecture (`telemetry/collector` already designed for file watching)

---

## Deployment Configuration

### Worker VM Setup (Windows)

1. **Install RedEDR as a service** (runs continuously in background)
2. **Configure output directory** for JSON exports
3. **Enable REST API** for lifecycle control
4. **Configure file watching** for `telemetry/collector` Rust service

### RedEDR Configuration

```powershell
# Start RedEDR with web server + JSON export + hide console
PS C:\RedEDR> .\RedEdr.exe --all --web --hide --output "C:\RedEDR\Data"

# RedEDR will:
# - Listen on http://0.0.0.0:8080 (REST API)
# - Write JSON events to C:\RedEDR\Data\<process_name>_<timestamp>.events.json
# - Automatically save on process exit or /api/save trigger
```

**Important**: Set `enable_remote_exec = true` in RedEDR `config.h` (allows REST-based process execution).

---

## Integration Method 1: REST API Control + JSON File Export

**Used for:** Worker lifecycle management (process execution, cleanup)
**Implemented in:** `worker/agent/src/rededr_client.rs` (to be created)

### Workflow

```
worker/agent (gRPC server)
    │
    │ 1. Receive RunSample RPC from Controller
    │
    ├──[POST http://localhost:8080/api/trace/reset]──▶ RedEDR
    │   (Clear event buffer from previous run)
    │
    ├──[Spawn harness.exe with artifact]──▶ worker/harness
    │                                         │
    │                                         ├─[Execute artifact.exe]
    │                                         │         │
    │                                         │         └──[ETW/hooks]──▶ RedEDR
    │                                         │                            │
    │                                         ├─[Enforce timeout]         │
    │                                         │                            │
    │                                         └─[Return exit code]        │
    │                                                                      │
    ├──[POST http://localhost:8080/api/execute/kill]──▶ RedEDR           │
    │   (Ensure process terminated)                                       │
    │                                                                      │
    ├──[GET http://localhost:8080/api/save]──▶ RedEDR                    │
    │   (Trigger JSON file flush to disk)                                 │
    │                                                                      │
    └──[Return SampleResponse with run_id]                                │
                                                                           │
telemetry/collector (Rust service, async file watcher)                   │
    │                                                                      │
    ├──[Detect new JSON file in C:\RedEDR\Data\]◄────────────────────────┘
    │   (notify crate: watches filesystem events)
    │
    ├──[Parse JSON events]
    │   (Extract: run_id, artifact_id, pid, event type, fields)
    │
    ├──[Normalize to Elasticsearch schema]
    │   (Convert RedEDR format → project RunResult/TelemetryEvent schema)
    │
    ├──[Extract features]──▶ feature_extractor.rs
    │   (Compute: rwx_short_window, anon_thread_start, etc.)
    │
    └──[Bulk insert to Elasticsearch]──▶ etw-* and rededr-* indices
        (Batch 100 events or 5 seconds)
```

### Starting RedEdr with REST API

```powershell
# In the VM, start RedEdr with web server enabled
PS C:\RedEdr> .\RedEdr.exe --all --web --hide --trace yourapp.exe

# RedEdr will listen on http://0.0.0.0:8080
```

**Important**: Enable remote execution mode by setting `enable_remote_exec` in config (see `config.h`).

### REST API Endpoints

#### 1. **Execute Your Target Binary**

```http
POST http://VM_IP:8080/api/execute/exec
Content-Type: multipart/form-data

Fields:
  - file: (binary data) - Your fuzzer test case
  - filename: (string) - e.g., "test.exe"
  - fileargs: (string, optional) - Command line arguments
  - path: (string, optional) - Destination path (default: C:\RedEdr\data\)
  - use_additional_etw: (string, optional) - "true" for extra ETW providers

Response:
{
  "status": "ok",
  "pid": 12345
}
```

**Implementation**: `webserver.cpp:336-448`

#### 2. **Kill the Target Process**

```http
POST http://VM_IP:8080/api/execute/kill

Response: 200 OK (or error)
```

**Implementation**: `webserver.cpp:450-465`

#### 3. **Retrieve Collected Events (Primary)**

```http
GET http://VM_IP:8080/api/logs/rededr

Response: JSON array of events
[
  {
    "date": "2025-07-20-10-36-24",
    "type": "syscall",
    "syscall": "NtAllocateVirtualMemory",
    "pid": 12345,
    "tid": 5678,
    "protection": "RWX",
    "size": 4096,
    "detection": "rwx_allocation",
    ...
  },
  {
    "type": "etw",
    "event": "ImageLoad",
    "image": "C:\\Windows\\System32\\kernel32.dll",
    ...
  },
  ...
]
```

**Source**: `webserver.cpp:172-205`
**Data**: Aggregated from all sources (ETW, ETW-TI, Kernel, DLL hooks)

#### 4. **Get Event Statistics**

```http
GET http://VM_IP:8080/api/stats

Response:
{
  "events_count": 1234,
  "num_kernel": 100,
  "num_etw": 500,
  "num_etwti": 300,
  "num_dll": 334,
  "num_process_cache": 5
}
```

**Implementation**: `webserver.cpp:160-170`

#### 5. **Reset Event Buffer**

```http
POST http://VM_IP:8080/api/trace/reset

Response: 200 OK
```

Clears all accumulated events for the next fuzzing iteration.
**Implementation**: `webserver.cpp:301-304`

#### 6. **Resource Locking** (Multi-fuzzer coordination)

```http
# Acquire exclusive access
POST http://VM_IP:8080/api/lock/acquire
Response: 200 OK (or 409 if locked)

# Release lock
POST http://VM_IP:8080/api/lock/release

# Check lock status
GET http://VM_IP:8080/api/lock/status
Response: { "in_use": false }
```

**Implementation**: `webserver.cpp:306-333`

---

## Method 2: JSON File Export

RedEdr can save events to JSON files on disk.

### Usage

```http
# Trigger save
GET http://VM_IP:8080/api/save

# Files are written to:
C:\RedEdr\Data\<process_name>_<timestamp>.events.json
```

### Accessing Files

1. **Via SMB Share**: Share `C:\RedEdr\Data` and mount from fuzzer
2. **Via VM Tools**: VMware shared folders, Hyper-V file sharing
3. **Via SCP/FTP**: Install OpenSSH server in VM

### File Format

```json
[
  {
    "date": "2025-07-20-10-36-24",
    "type": "dll",
    "syscall": "NtProtectVirtualMemory",
    "pid": 4444,
    "tid": 5555,
    "base_address": "0x00007FFE12340000",
    "size": 4096,
    "old_protection": "RW",
    "new_protection": "RWX",
    "callstack": ["kernel32!VirtualProtect", "..."],
    "detection": "rw_to_rwx"
  },
  ...
]
```

**See**: `Data/` directory in RedEdr repo for examples

---

## Alternative: Direct Pipe Connection (Not Recommended)

**Note:** This method is documented for completeness but **NOT recommended** for AutoMutate++.

RedEDR uses named pipes (`\\.\pipe\RedEdrDllCom`, `\\.\pipe\RedEdrKrnCom`, etc.) for internal communication between its components. While you could tap into these pipes directly, this approach has significant drawbacks:

- **Tight coupling**: Requires Worker and RedEDR in same process space
- **No persistence**: Events lost on crash (can't replay/debug)
- **Complex error handling**: Pipe disconnects, buffer overruns
- **Windows-only IPC**: Doesn't work across VM boundaries

**If you still need pipes** (e.g., for low-latency streaming), see RedEDR source: `RedEdrShared/piping.cpp`

---

## Implementation Guide for AutoMutate++

### Module 1: `worker/agent/src/rededr_client.rs` (To Create)

**Purpose:** REST client for RedEDR lifecycle management
**Used by:** `worker/agent` during `RunSample` RPC handling

```rust
// worker/agent/src/rededr_client.rs

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

#[derive(Debug, Serialize, Deserialize)]
pub struct RedEdrStats {
    pub events_count: u32,
    pub num_kernel: u32,
    pub num_etw: u32,
    pub num_etwti: u32,
    pub num_dll: u32,
}

pub struct RedEdrClient {
    client: Client,
    base_url: String, // e.g., "http://localhost:8080"
}

impl RedEdrClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// Reset event buffer before new run
    pub async fn reset_trace(&self) -> Result<()> {
        let url = format!("{}/api/trace/reset", self.base_url);
        self.client
            .post(&url)
            .send()
            .await
            .context("Failed to reset RedEDR trace")?
            .error_for_status()
            .context("RedEDR reset returned error")?;
        info!("RedEDR trace buffer reset");
        Ok(())
    }

    /// Kill target process (safety fallback)
    pub async fn kill_process(&self) -> Result<()> {
        let url = format!("{}/api/execute/kill", self.base_url);
        let resp = self.client.post(&url).send().await?;

        if resp.status().is_success() {
            info!("RedEDR killed target process");
            Ok(())
        } else {
            warn!("RedEDR kill request failed (may already be dead)");
            Ok(()) // Non-fatal
        }
    }

    /// Trigger JSON file save to disk
    pub async fn save_events(&self) -> Result<()> {
        let url = format!("{}/api/save", self.base_url);
        self.client
            .get(&url)
            .send()
            .await
            .context("Failed to trigger RedEDR save")?
            .error_for_status()
            .context("RedEDR save returned error")?;
        info!("RedEDR events saved to disk");
        Ok(())
    }

    /// Get event statistics (optional, for debugging)
    pub async fn get_stats(&self) -> Result<RedEdrStats> {
        let url = format!("{}/api/stats", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let stats: RedEdrStats = resp.json().await?;
        Ok(stats)
    }

    /// Acquire lock (multi-worker coordination)
    pub async fn acquire_lock(&self) -> Result<bool> {
        let url = format!("{}/api/lock/acquire", self.base_url);
        let resp = self.client.post(&url).send().await?;

        match resp.status().as_u16() {
            200 => Ok(true),
            409 => Ok(false), // Locked by another worker
            _ => Err(anyhow::anyhow!("Unexpected lock response: {}", resp.status())),
        }
    }

    /// Release lock
    pub async fn release_lock(&self) -> Result<()> {
        let url = format!("{}/api/lock/release", self.base_url);
        self.client.post(&url).send().await?.error_for_status()?;
        Ok(())
    }
}
```

### Module 2: `worker/agent` Integration

Modify `worker/agent/src/main.rs` to use `RedEdrClient`:

```rust
// worker/agent/src/main.rs (excerpt)

use rededr_client::RedEdrClient;

impl WorkerAgent for MyWorkerAgent {
    async fn run_sample(
        &self,
        request: Request<SampleRequest>,
    ) -> Result<Response<SampleResponse>, Status> {
        let req = request.into_inner();
        let run_id = uuid::Uuid::new_v4().to_string();

        // 1. Initialize RedEDR client
        let rededr = RedEdrClient::new("http://localhost:8080".to_string());

        // 2. Reset trace buffer
        rededr.reset_trace().await
            .map_err(|e| Status::internal(format!("RedEDR reset failed: {}", e)))?;

        // 3. Spawn harness with artifact
        let harness_result = self.execute_harness(
            &req.artifact_path,
            req.timeout_seconds,
        ).await?;

        // 4. Kill process (safety)
        rededr.kill_process().await.ok(); // Non-fatal

        // 5. Trigger JSON save
        rededr.save_events().await
            .map_err(|e| Status::internal(format!("RedEDR save failed: {}", e)))?;

        // 6. Return response (telemetry collector will pick up JSON file)
        Ok(Response::new(SampleResponse {
            job_id: req.job_id,
            success: harness_result.success,
            exit_code: harness_result.exit_code,
            output: harness_result.output,
            telemetry_ids: vec![run_id], // Used to correlate with JSON file
        }))
    }
}
```

### Module 3: `telemetry/collector` File Watcher

**Purpose:** Watch `C:\RedEDR\Data\` for new JSON files, parse, and ship to Elasticsearch

```rust
// telemetry/collector/src/main.rs

use notify::{Watcher, RecursiveMode, Result as NotifyResult, Event, EventKind};
use std::path::PathBuf;
use tokio::fs;
use tracing::{info, error, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let watch_dir = PathBuf::from(r"C:\RedEDR\Data");
    info!("Starting telemetry collector watching: {:?}", watch_dir);

    // Create async watcher
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let mut watcher = notify::recommended_watcher(move |res: NotifyResult<Event>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                for path in event.paths {
                    if path.extension().and_then(|s| s.to_str()) == Some("json") {
                        let _ = tx.blocking_send(path);
                    }
                }
            }
        }
    })?;

    watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;

    // Process file events
    while let Some(json_file) = rx.recv().await {
        info!("Detected new telemetry file: {:?}", json_file);

        match process_telemetry_file(&json_file).await {
            Ok(_) => info!("Successfully processed {:?}", json_file),
            Err(e) => error!("Failed to process {:?}: {}", json_file, e),
        }
    }

    Ok(())
}

async fn process_telemetry_file(path: &PathBuf) -> anyhow::Result<()> {
    // 1. Read JSON file
    let contents = fs::read_to_string(path).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&contents)?;

    info!("Parsed {} events from {:?}", events.len(), path);

    // 2. Normalize events to project schema
    let normalized = rededr::normalize_events(events)?;

    // 3. Extract features
    let features = feature_extractor::extract_features(&normalized)?;

    // 4. Bulk insert to Elasticsearch
    // TODO: Implement elastic.rs bulk insert
    // elastic::bulk_insert(&normalized).await?;

    // 5. Archive processed file
    let archive_path = path.with_extension("json.processed");
    fs::rename(path, &archive_path).await?;

    Ok(())
}
```

### Module 4: Schema Normalization (`telemetry/collector/src/rededr.rs`)

```rust
// telemetry/collector/src/rededr.rs

use serde::{Deserialize, Serialize};
use anyhow::Result;

#[derive(Debug, Deserialize)]
pub struct RedEdrEvent {
    pub date: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub pid: Option<u32>,
    pub tid: Option<u32>,
    pub syscall: Option<String>,
    pub detection: Option<String>,
    // ... other fields from RedEDR JSON
}

#[derive(Debug, Serialize)]
pub struct NormalizedEvent {
    pub run_id: String,
    pub artifact_id: String,
    pub ts: String,
    pub provider: String, // "rededr-dll", "rededr-etw", etc.
    pub event_id: u32,
    pub pid: u32,
    pub fields: serde_json::Value, // Flexible nested fields
}

pub fn normalize_events(rededr_events: Vec<serde_json::Value>) -> Result<Vec<NormalizedEvent>> {
    let mut normalized = Vec::new();

    for event in rededr_events {
        let re: RedEdrEvent = serde_json::from_value(event.clone())?;

        normalized.push(NormalizedEvent {
            run_id: extract_run_id_from_filename()?, // Extract from JSON filename
            artifact_id: "TODO".to_string(), // Correlate with worker metadata
            ts: re.date,
            provider: format!("rededr-{}", re.event_type),
            event_id: hash_event_type(&re.event_type),
            pid: re.pid.unwrap_or(0),
            fields: event, // Keep original fields for flexibility
        });
    }

    Ok(normalized)
}

fn extract_run_id_from_filename() -> Result<String> {
    // Parse filename: <process_name>_<timestamp>.events.json
    // Map timestamp to run_id (requires correlation with worker metadata)
    Ok("run-placeholder".to_string())
}

fn hash_event_type(event_type: &str) -> u32 {
    // Simple hash for event_id
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    event_type.hash(&mut hasher);
    hasher.finish() as u32
}
```

---

## Event Types and Fields

### Common Fields (All Events)

```json
{
  "date": "2025-07-20-10-36-24",
  "type": "dll|etw|etwti|kernel|meta",
  "pid": 1234,
  "tid": 5678
}
```

### DLL Hook Events (`type: "dll"`)

```json
{
  "type": "dll",
  "syscall": "NtAllocateVirtualMemory",
  "base_address": "0x00007FFE12340000",
  "size": 4096,
  "protection": "RWX",
  "callstack": ["kernel32!VirtualAlloc+0x50", "myapp!main+0x100"],
  "detection": "rwx_allocation"
}
```

### ETW Events (`type: "etw"`)

```json
{
  "type": "etw",
  "event": "ProcessCreate|ThreadCreate|ImageLoad|...",
  "image": "C:\\Windows\\System32\\kernel32.dll",
  "parent_pid": 1000
}
```

### Kernel Events (`type: "kernel"`)

```json
{
  "type": "kernel",
  "event": "ProcessCreate|ThreadCreate|ImageLoad",
  "image": "C:\\malware.exe"
}
```

### Detection Fields

Events may include a `detection` field:
- `"rwx_allocation"` - RWX memory allocated
- `"rw_to_rwx"` - Memory protection changed from RW to RWX
- `"callstack_non_image"` - Callstack originates from non-image memory

**See**: `Doc/captured_events.md` in RedEdr repo for complete event documentation

---

## How RedEDR Telemetry Maps to AutoMutate++ Pipeline

RedEDR events are used by different components of the AutoMutate++ feedback loop:

### 1. **Triage Engine** (`controller/triage-engine`)
Extracts features from RedEDR events for surrogate classifier training:

| RedEDR Event | Extracted Feature | Purpose |
|--------------|-------------------|---------|
| `detection: "rwx_allocation"` | `rwx_short_window` (bool) | Flags suspicious memory allocations |
| `type: "dll"`, `callstack: [...]` | `callstack_non_image` (bool) | Detects execution from anonymous memory |
| `type: "etw"`, `event: "ImageLoad"` | `unsigned_child_of_signed` (bool) | Provenance anomaly detection |
| Memory protection transitions | `write_to_execute_ms` (int) | Timing-based heuristic |

**See:** `telemetry/collector/src/feature_extractor.rs` for full feature extraction logic

### 2. **Selector** (`controller/selector`)
Uses triage feedback to select mutations that avoid detected features:

```
RedEDR Detection → Triage Analysis → Feature Importance → Avoid-List → Mutation Selection
```

Example:
- If `rwx_short_window` has high importance → Selector picks `beh.sleep_before` mutation
- If `callstack_non_image` triggered → Selector picks `ast.stack_pivot_avoid` mutation

**See:** CLAUDE.md Section 8 for mutation selection algorithm

### 3. **Monitor** (`worker/monitor`)
Labels execution outcomes based on RedEDR detections:

| RedEDR Signal | RunStatus Label | Rationale |
|---------------|-----------------|-----------|
| `detection` field present | `detected` | RedEDR flagged suspicious behavior |
| No detections, exit_code=0 | `not_detected` | Clean execution |
| Process crash before events | `crash` | Artifact failed before telemetry |
| No telemetry, exit in <1s | `noisy` | Likely sandbox evasion |

**See:** `worker/monitor/src/lib.rs` for labeling logic

---

## Performance Optimization

### 1. **Minimize Event Processing Overhead**
```powershell
# Disable real-time output with --hide
.\RedEdr.exe --all --web --hide --trace target.exe
```

### 2. **Selective Event Collection**
Choose only needed telemetry sources:
```powershell
# Only kernel + DLL (fastest)
.\RedEdr.exe --kernel --inject --web --hide --trace target.exe

# Only ETW (medium)
.\RedEdr.exe --etw --web --hide --trace target.exe
```

### 3. **Batch Processing**
Process multiple test cases before retrieving events:
```python
# Queue multiple executions
for test_case in batch:
    execute_test_case(test_case)
    time.sleep(1)

# Retrieve all events at once
events = requests.get(f"{REDEDR_URL}/api/logs/rededr").json()
```

---

## Troubleshooting

### Events not appearing
- Verify target process name matches `--trace` argument
- Check RedEdr logs: `C:\RedEdr\rededr.log`
- Ensure PPL service is running (for ETW-TI): `Get-Service RedEdrPplService`

### Connection refused
- Verify firewall allows port 8080
- Check RedEdr is running with `--web` flag
- Test locally in VM: `curl http://localhost:8080/api/stats`

### High overhead
- Disable callstack collection (set `do_dllinjection_ucallstack = false`)
- Use only kernel events (fastest)
- Reduce target process complexity

### Missing events
- ETW-TI requires PPL service and ELAM driver
- Some events require SYSTEM privileges
- Check group policy settings for audit events

---

## References

- **REST API Implementation**: `RedEdr/webserver.cpp`
- **Event Aggregation**: `RedEdr/event_aggregator.cpp:51-59`
- **Event Processing**: `RedEdr/event_processor.cpp`
- **Example Events**: `Data/` directory in RedEdr repository
- **Captured Events Documentation**: `Doc/captured_events.md`

---

## Summary: Chosen Integration Strategy

### Decision: **JSON File Export + REST API Control**

**Rationale for AutoMutate++ project:**

1. **Architectural Alignment**
   - `telemetry/collector` already designed as file watcher (CLAUDE.md Section 5)
   - Decouples RedEDR (C++) from Worker Agent (Rust) via filesystem
   - No need for complex IPC or pipe handling

2. **Reliability & Debuggability**
   - JSON files persist on disk (can inspect/replay later)
   - Worker crashes don't lose telemetry (RedEDR continues running)
   - Easy to validate schema (just read the JSON)

3. **Performance**
   - Async file watching (no blocking)
   - Bulk Elasticsearch inserts (batch 100 events)
   - RedEDR runs independently (no gRPC overhead)

4. **Simplicity**
   - REST API = standard HTTP (no custom protocols)
   - `notify` crate = mature file watching
   - `reqwest` = battle-tested HTTP client

5. **Scalability**
   - Multiple workers can share one Elasticsearch cluster
   - Telemetry collector can run on Controller or Workers
   - JSON files can be archived for long-term analysis

**Why NOT direct pipe connection:**
- Requires Worker and RedEDR in same process space (tight coupling)
- Complex error handling (pipe disconnects)
- No persistence (events lost on crash)
- Harder to debug (can't inspect intermediate data)

**Why NOT REST API for events:**
- Polling `/api/logs/rededr` wastes bandwidth
- Events must fit in HTTP response (memory limits)
- No disk persistence (must process immediately)

### Implementation Checklist

- [ ] **Phase 1: Worker Agent**
  - [ ] Create `worker/agent/src/rededr_client.rs` (REST client)
  - [ ] Integrate into `RunSample` RPC handler
  - [ ] Test lifecycle: reset → execute → kill → save

- [ ] **Phase 2: Telemetry Collector**
  - [ ] Implement file watcher in `telemetry/collector/src/main.rs`
  - [ ] Add schema normalization in `rededr.rs`
  - [ ] Implement Elasticsearch bulk insert in `elastic.rs`
  - [ ] Add feature extraction in `feature_extractor.rs`

- [ ] **Phase 3: Configuration**
  - [ ] Add RedEDR settings to `config/worker.toml`
    ```toml
    [rededr]
    api_url = "http://localhost:8080"
    data_dir = "C:\\RedEDR\\Data"
    timeout_ms = 5000
    ```
  - [ ] Add Elasticsearch settings to `config/collector.toml`
    ```toml
    [elasticsearch]
    url = "http://controller:9200"
    index_prefix = "etw-"
    bulk_size = 100
    bulk_timeout_ms = 5000
    ```

- [ ] **Phase 4: VM Setup**
  - [ ] Install RedEDR on Worker VM as Windows service
  - [ ] Configure firewall (allow port 8080 from Worker Agent)
  - [ ] Test manual workflow: `POST /reset` → run artifact → `GET /save` → check JSON

- [ ] **Phase 5: End-to-End Test**
  - [ ] Controller schedules job → Worker executes → Collector indexes → Query ES
  - [ ] Verify `run_id` correlation across all components
  - [ ] Test failure scenarios (RedEDR crash, network partition, etc.)

### Key Files Modified/Created

| File | Action | Purpose |
|------|--------|---------|
| `worker/agent/src/rededr_client.rs` | **Create** | REST client for RedEDR lifecycle |
| `worker/agent/src/main.rs` | **Modify** | Integrate `RedEdrClient` into `RunSample` |
| `telemetry/collector/src/main.rs` | **Modify** | Add file watcher + processing loop |
| `telemetry/collector/src/rededr.rs` | **Modify** | Add schema normalization |
| `telemetry/collector/src/elastic.rs` | **Create** | Elasticsearch bulk insert client |
| `telemetry/collector/Cargo.toml` | **Modify** | Add deps: `notify`, `elasticsearch`, `tokio-fs` |
| `config/worker.toml` | **Create** | RedEDR configuration |
| `config/collector.toml` | **Create** | Elasticsearch configuration |

### Expected Data Flow

```
Controller (gRPC)
    │
    └──[RunSample RPC]──▶ Worker Agent
                             │
                             ├──[POST /api/trace/reset]──▶ RedEDR
                             ├──[Spawn harness.exe]──▶ Harness
                             │                           │
                             │                           └──[Execute artifact]──▶ RedEDR (capture ETW/hooks)
                             │                                                      │
                             ├──[POST /api/execute/kill]──▶ RedEDR               │
                             ├──[GET /api/save]──▶ RedEDR                        │
                             │                       │                             │
                             │                       └──[Write JSON]──▶ C:\RedEDR\Data\<process>.events.json
                             │                                                      │
                             └──[Return SampleResponse]                            │
                                                                                    │
Telemetry Collector (file watcher)                                                │
    │                                                                              │
    ├──[Detect new JSON file]◄───────────────────────────────────────────────────┘
    ├──[Parse JSON]
    ├──[Normalize to project schema]
    ├──[Extract features: rwx_short_window, etc.]
    └──[Bulk insert to Elasticsearch]──▶ etw-*, rededr-* indices
                                            │
                                            └──[Query]◄──Triage Engine (CLAUDE.md Section 7)
```

### Testing Strategy

1. **Unit tests:**
   - `RedEdrClient`: mock HTTP responses with `wiremock`
   - `normalize_events`: test RedEDR JSON → project schema mapping
   - `extract_features`: verify feature computation (rwx_short_window, etc.)

2. **Integration tests:**
   - Spawn RedEDR in test mode → generate fake events → verify collector picks them up
   - Test Elasticsearch indexing with `testcontainers-rs`

3. **End-to-end tests:**
   - Deploy full stack (Controller + Worker + RedEDR + ES)
   - Schedule test job → verify telemetry appears in Kibana

---

## Next Steps

1. **Read** RedEDR documentation: `Doc/captured_events.md` (understand full event schema)
2. **Create** `worker/agent/src/rededr_client.rs` (copy from code above)
3. **Test** RedEDR REST API manually: `curl http://localhost:8080/api/stats`
4. **Implement** file watcher in `telemetry/collector` (copy from code above)
5. **Deploy** to test VM and run first end-to-end experiment

This integration gives you the same telemetry visibility that commercial EDR products (Defender, CrowdStrike, SentinelOne) use for detection, while maintaining the clean architecture of AutoMutate++.
