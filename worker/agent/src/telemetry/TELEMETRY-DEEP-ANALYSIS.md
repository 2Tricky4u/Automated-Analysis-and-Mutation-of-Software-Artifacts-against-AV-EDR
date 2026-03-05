# Telemetry Module — Deep Analysis

Deep analysis of `worker/agent/src/telemetry/` — the telemetry collection, packaging, and compression layer for the worker agent.

---

## 1. Overview

### Purpose

The `telemetry/` folder is the **data acquisition layer** for the worker agent. It collects raw telemetry from multiple sources — ETW events via RedEDR's HTTP API, line-level execution traces via Windows named pipes, binary protocol trace logs, API checkpoints, and basic-block coverage — and packages them into protobuf `TelemetryData` messages ready for gRPC transport to the controller.

### Role in the Global Project

Telemetry is the foundation of AutoMutate++'s learning loop. Every piece of data used for triage token extraction, differential analysis, and mutation selection originates from this module:

- **RedEDR collector** → behavioral events (ETW syscalls, API calls, stack traces) → triage tokens (`api:VirtualProtect`, `etw:provider/event_id`)
- **Trace collector** → line-level execution traces → coverage tokens (`trunc:loader.c:143`), sequence tokens
- **Pipeline packager** → deduplicated trace logs, checkpoints, BB coverage → controller storage → differential analysis

```
┌──────────────────────────────────────────────────┐
│              execution/engine.rs                  │
│              (Phase 4-9 of run)                   │
└──────────┬──────────────────────┬────────────────┘
           │                      │
           ▼                      ▼
┌─────────────────────┐  ┌──────────────────────────┐
│  collectors/         │  │  pipeline.rs              │
│                      │  │                           │
│  rededr.rs           │  │  package_trace_log()      │
│  ├─ HTTP poll loop   │  │  collect_trace_log_binary │
│  ├─ start_trace()    │  │  collect_bb_coverage()    │
│  ├─ collect_all()    │  │  collect_api_checkpoints() │
│  ├─ reset()          │  │                           │
│  └─ lock/unlock      │  │  deduplicate_trace_jsonl()│
│                      │  │  (loop compression)       │
│  trace.rs            │  │                           │
│  ├─ named pipe       │  └───────────┬───────────────┘
│  ├─ binary protocol  │              │
│  └─ Base64 protocol  │              │
└──────────┬───────────┘              │
           │                          │
           ▼                          ▼
    mpsc::channel              Vec<TelemetryData>
    (real-time streaming)      (batch collection)
           │                          │
           ▼                          ▼
┌──────────────────────────────────────────────────┐
│          gRPC stream / unary RPC → controller     │
└──────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────┐
│  trace_compressor.rs (NOT INTEGRATED)             │
│  CLP + Matrix Profile + Sequitur grammar          │
│  Three-stage structural compression               │
└──────────────────────────────────────────────────┘
```

---

## 2. File Inventory

| File | Lines | Functions | Purpose |
|------|-------|-----------|---------|
| `mod.rs` | 11 | 0 | Module declarations |
| `pipeline.rs` | 421 | 5 | Telemetry packaging: trace dedup, binary trace parsing, BB coverage, API checkpoints |
| `trace_compressor.rs` | 533 | 5 (+3 tests) | Advanced three-stage trace compression (not integrated) |
| `collectors/mod.rs` | 11 | 0 | Collector module declarations |
| `collectors/rededr.rs` | 412 | 10 (+1 test) | RedEDR HTTP API collector with polling, dedup, transform |
| `collectors/trace.rs` | 547 | 7 (+3 tests) | Named pipe trace collector with binary/Base64 auto-detection |
| **Total** | **1935** | **27** | — |

---

## 3. Per-Module Deep Analysis

### 3.1 `mod.rs` — Module Declarations (11 lines)

```rust
pub mod collectors;
pub mod pipeline;
#[allow(dead_code)]
pub mod trace_compressor;
```

Three submodules:
- `collectors` — real-time telemetry collectors (RedEDR HTTP, named pipe trace)
- `pipeline` — batch telemetry packaging (trace log, coverage, checkpoints)
- `trace_compressor` — experimental structural compression (`#[allow(dead_code)]`, not integrated)

The TODO comment documents the integration gap: `trace_compressor` output needs to be wired into the pipeline and converted to a format the controller can parse.

---

### 3.2 `pipeline.rs` — Telemetry Packaging Pipeline (421 lines)

The batch-mode telemetry packager. Called by `execution/engine.rs` after artifact execution completes to collect all disk-based telemetry and package it into `Vec<TelemetryData>` for transport.

#### 3.2.1 `deduplicate_trace_jsonl()` (private)

```rust
fn deduplicate_trace_jsonl(raw: &str) -> (String, usize, usize)
```

Collapses repeated trace lines (from loops) into single entries with hit counts.

**Deduplication key:** `(file, line, func)` — the source location triple.

**Algorithm:**

```
For each JSONL line:
    Parse → extract (file, line, func) + seq
    If key seen before:
        Increment count
        Keep entry with highest seq (most recent execution)
    Else:
        Insert new entry

Sort all entries by seq ascending (execution order)

For entries with count > 1:
    Add "count": N field to JSON object

Return (deduplicated_jsonl, raw_count, unique_count)
```

**Why keep highest seq:** The last time a line was executed is the most relevant for truncation analysis. If the artifact was killed at line 143, the highest seq for that line tells you it was the last thing executed.

**Why add count field:** Loop iteration counts are valuable for triage — `count: 500` on a timing check line suggests an anti-emulation loop, which is a behavioral signal.

#### 3.2.2 `package_trace_log()`

```rust
pub fn package_trace_log(
    trace_events_file: &Path,
    job_id: &str,
    telemetry_events: &mut Vec<TelemetryData>,
)
```

Reads the `trace_events.jsonl` file written by the named pipe collector, deduplicates it, and packages it as a single `trace_log` telemetry event.

**Flow:**

```
trace_events.jsonl (disk)
    │
    ▼
Read file to string
    │
    ▼
deduplicate_trace_jsonl()
    │  (file,line,func) dedup, keep highest seq, add count
    │
    ▼
Size check: serialized JSON ≤ MAX_SERIALIZED_PAYLOAD (3.5MB)?
    ├── yes → ship full content
    └── no  → progressive tail truncation
                │
                ▼
              Binary search by halving:
                Cut slice in half → advance to next \n
                Re-serialize → check size
                Repeat until fits
                │
                ▼
              Ship tail (most recent lines)
```

**Truncation strategy:** Keeps the **tail** (most recent lines), not the head. This is critical because the detection-relevant behavior happens near the end of execution (where the payload executes and the artifact gets killed).

**Metadata fields:**

| Field | Value |
|-------|-------|
| `trace_file` | Path to source file |
| `event_count` | Total lines in original file |
| `original_size_bytes` | Raw file size |
| `raw_event_count` | Lines before dedup |
| `unique_lines` | Lines after dedup |
| `compression` | `"none"` or `"truncated_tail"` |
| `sent_lines` | Lines actually sent (if truncated) |
| `final_size_bytes` | Serialized payload size |

**Output TelemetryData:**

| Field | Value |
|-------|-------|
| `event_type` | `"trace_log"` |
| `payload` | JSON-serialized `{"content": "<jsonl>"}` as bytes |
| `typed_event` | `None` (uses generic payload) |
| `timestamp` | Current UTC seconds |

#### 3.2.3 `collect_trace_log_binary()`

```rust
pub fn collect_trace_log_binary(
    trace_log_path: &Path,
    job_id: &str,
    telemetry_events: &mut Vec<TelemetryData>,
)
```

Parses the binary-format `trace.log` file written by the instrumentation runtime and extracts individual line trace events.

**Binary protocol format:**

```
┌─────────────────────────────────────────┐
│  Header (32 bytes)                       │
│  ┌────────────┬────────────────────────┐ │
│  │ magic      │ 0x49535452 ('ISTR')    │ │
│  │ version    │ u16                    │ │
│  │ event_type │ u16                    │ │
│  │ thread_id  │ u32                    │ │
│  │ seq_no     │ u64                    │ │
│  │ ts_us      │ u64                    │ │
│  │ payload_len│ u32                    │ │
│  └────────────┴────────────────────────┘ │
│  Payload (payload_len bytes)             │
│  "file:line:func" UTF-8 string           │
├─────────────────────────────────────────┤
│  Next record...                          │
└─────────────────────────────────────────┘
```

**Event type handling:**

| event_type | Action |
|-----------|--------|
| 1 (line trace) | Parse payload, create `trace_line` TelemetryData |
| 2-4 (artifact status) | Warn and skip — these belong in `checkpoints.log` |
| other | Debug log, skip |

**Output:** One `TelemetryData` per line trace event with:
- `event_type`: `"trace_line"`
- `payload`: raw payload bytes
- `timestamp`: milliseconds (current time, not from trace)

#### 3.2.4 `collect_bb_coverage()`

```rust
pub async fn collect_bb_coverage(
    _bitmap_path: &Path,
    metadata_path: &Path,
    job_id: &str,
) -> Result<TelemetryData, Box<dyn Error + Send + Sync>>
```

Reads basic-block coverage metadata written by the instrumentation runtime and packages it as a typed `CoverageEvent`.

**Input format** (`coverage_bbs.txt`):

```
# BB_ID HIT_COUNT
0 1
1 3
2 0
5 12
```

**Output:** Single `TelemetryData` with:

| Field | Value |
|-------|-------|
| `event_type` | `"coverage"` |
| `payload` | empty (data in typed_event) |
| `typed_event` | `CoverageEvent { bitmap: [], bb_ids, hit_counts, total_bbs }` |

**Note:** The `_bitmap_path` parameter is unused — the binary bitmap is for AFL-style fuzzing and is not sent to the controller. Only the human-readable text metadata is parsed.

#### 3.2.5 `collect_api_checkpoints()`

```rust
pub async fn collect_api_checkpoints(
    checkpoints_path: &Path,
    job_id: &str,
) -> Result<Vec<TelemetryData>, Box<dyn Error + Send + Sync>>
```

Reads the `checkpoints.log` JSONL file written by the artifact's instrumentation runtime at key execution milestones.

**Input format** (`checkpoints.log`):

```json
{"ts_us":1234567,"checkpoint":"alloc_memory"}
{"ts_us":1234600,"checkpoint":"decode_payload","type":"success"}
{"ts_us":1234700,"checkpoint":"execute_payload","type":"failure","error_code":-1073741819}
```

**Checkpoint type handling:**

| `type` field | `event_type` | Action |
|-------------|-------------|--------|
| `"success"` | `artifact_success` | Log success |
| `"failure"` | `artifact_failure` | Log error, include `error_code` in metadata |
| other / missing | `checkpoint` | Standard checkpoint |

**Output:** One `TelemetryData` per checkpoint line with:

| Field | Value |
|-------|-------|
| `event_type` | `"checkpoint"`, `"artifact_success"`, or `"artifact_failure"` |
| `payload` | empty |
| `typed_event` | `CheckpointEvent { name, ts_us }` |
| `timestamp` | `ts_us / 1000` (converted from microseconds to milliseconds) |
| `metadata.status_type` | Original type field value (for non-checkpoint types) |

---

### 3.3 `trace_compressor.rs` — Advanced Structural Compression (533 lines)

**Status: NOT INTEGRATED.** The module compiles and tests pass, but it is not wired into the telemetry pipeline. The `#[allow(dead_code)]` annotation and detailed TODO comment in `mod.rs` document the three blocking issues.

This module implements a three-stage trace compression pipeline inspired by CLP (Compact Log Processing), Matrix Profile time-series analysis, and Sequitur grammar induction.

#### 3.3.1 Three-Stage Architecture

```
Stage 1: CLP Columnar Decomposition
    JSONL trace events
        │
        ▼
    ColumnarTrace
    ├── line_sequence: Vec<u32>    (dense integer array)
    ├── file_dict: Vec<String>     (string dictionary)
    ├── func_dict: Vec<String>     (string dictionary)
    ├── file_indices: Vec<usize>   (event → dict index)
    ├── func_indices: Vec<usize>   (event → dict index)
    ├── thread_ids: Vec<u32>
    └── timestamps: Vec<u64>

Stage 2: Matrix Profile Pattern Detection
    line_sequence: [10, 11, 12, 10, 11, 12, 10, 11, 12, 20]
        │
        ▼
    MatrixProfile::compute(min_window=2, max_window=50, min_occurrences=3)
        │
        ▼
    Vec<Motif> sorted by compression benefit (occurrences × length)
    e.g., Motif { pattern: [10,11,12], occurrences: [0,3,6], length: 3 }

Stage 3: Sequitur Grammar Induction
    line_sequence + motifs
        │
        ▼
    Grammar
    ├── rules: [GrammarRule { id, expansion, usage_count }]
    └── start_rule: Vec<Symbol>  (Terminal(line) | NonTerminal(rule_id))
        │
        ▼
    Compressed text:
        RULE_0 (used 3 times): L10 L11 L12
        @RULE_0 @RULE_0 @RULE_0 L20
```

#### 3.3.2 Stage 1: `ColumnarTrace::from_jsonl()`

Parses JSONL trace events into a columnar representation. Separates the dense integer data (line numbers) from string data (file names, function names) using dictionary encoding.

**Why columnar:** Line numbers are the primary axis for pattern detection. Dictionary encoding eliminates redundant string storage (e.g., `"loader_template.c"` stored once instead of 10,000 times).

#### 3.3.3 Stage 2: `MatrixProfile::compute()`

Finds recurring subsequences (motifs) in the line number sequence using sliding window pattern matching.

**Algorithm:**

```
For window_size in min_window..=max_window:
    For each position i:
        Extract pattern = line_sequence[i..i+window_size]
        Hash into pattern_map
    Filter: keep patterns with ≥ min_occurrences hits
    Score: benefit = occurrences.len() × pattern.length

Sort all motifs by compression benefit descending
```

**Known performance issue (documented in module header):** Creates `HashMap<Vec<u32>>` for every window size — O(n × max_window) entries. The `occurrences.contains(&i)` lookup in grammar induction is O(n) per position. The TODO suggests using `HashSet<usize>` and capping input at 50K events.

#### 3.3.4 Stage 3: `Grammar::from_sequence_and_motifs()`

Converts motifs into a hierarchical grammar using greedy non-overlapping selection.

**Algorithm:**

```
For each motif (sorted by benefit):
    Find valid_occurrences (positions not yet covered)
    If ≥ 2 valid occurrences:
        Create GrammarRule with Terminal symbols
        Mark all occurrence positions as covered

Build start_rule:
    For each position in line_sequence:
        If position starts a rule occurrence → emit NonTerminal
        Else → emit Terminal
```

#### 3.3.5 `compress_trace_log()`

```rust
pub fn compress_trace_log(content: &str, min_loop_iterations: usize) -> CompressedTrace
```

Main entry point. Orchestrates all three stages.

**Early exit:** Traces with < 10 lines bypass compression (not worth the overhead).

**Fallback:** If JSONL parsing fails, returns content unchanged with ratio 1.0.

**Output:** `CompressedTrace` with human-readable text containing:
- File/function dictionaries
- Grammar rules with usage counts
- Compressed start sequence using rule references

#### 3.3.6 `gzip_compress()`

```rust
pub fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, std::io::Error>
```

Standalone gzip compression utility using `flate2`. Available for use but not called by any current code path.

#### 3.3.7 Integration Blockers (from module header)

| Blocker | Description | Impact |
|---------|-------------|--------|
| Output format | Emits human-readable text, controller expects JSONL | Controller cannot parse compressed output |
| Information loss | Discards `seq`, `ts_us`, `thread_id` | Source viewer needs `seq` for cutoff line detection |
| Performance | O(n × max_window) HashMap entries, O(n) linear scan | Unusable for traces > 50K events |

The pipeline currently uses `deduplicate_trace_jsonl()` instead — simpler, preserves all fields, and produces controller-compatible JSONL.

---

### 3.4 `collectors/mod.rs` — Collector Declarations (11 lines)

```rust
pub mod rededr;
pub mod trace;
```

Two collectors implemented, three planned (ETW, Event Logs, Defender alerts noted as future).

---

### 3.5 `collectors/rededr.rs` — RedEDR HTTP API Collector (412 lines)

The primary behavioral telemetry collector. Interfaces with RedEDR — an ETW-based monitoring tool that runs on the Windows VM and exposes collected events via an HTTP REST API.

#### 3.5.1 Data Types

**`RedEdrEvent`** — Deserialized RedEDR event:

| Field | Type | Description |
|-------|------|-------------|
| `date` | `Option<String>` | Event timestamp string |
| `type` | `Option<String>` | Event category (e.g., `"etw"`, `"dll"`, `"kernel"`) |
| `trace_id` | `Option<u64>` | Unique event identifier for deduplication |
| `target` | `Option<String>` | Target process name |
| `func` | `Option<String>` | API function name (e.g., `"NtAllocateVirtualMemory"`) |
| `pid` | `Option<u32>` | Process ID |
| `tid` | `Option<u32>` | Thread ID |
| `provider` | `Option<String>` | ETW provider name |
| `event_id` | `Option<u32>` | ETW event ID |
| `callstack` | `Option<Value>` | Flexible callstack (array of strings or objects) |
| `stack_trace` | `Option<Vec<StackTraceEntry>>` | Structured stack trace |
| `targets` | `Option<Vec<String>>` | Multiple target processes |
| `extra` | `Map<String, Value>` | `#[serde(flatten)]` catch-all for unknown fields |

**Why all fields `Option`:** RedEDR events are heterogeneous — different event types have different fields. An ETW event has `provider` and `event_id`, a DLL load event has `target` and `func`. The `#[serde(flatten)]` `extra` map captures any fields not explicitly modeled.

**`StackTraceEntry`:**

| Field | Type |
|-------|------|
| `addr` | `Option<u64>` |
| `addr_info` | `Option<String>` |
| `idx` | `Option<u32>` |

**`RedEdrCollectorConfig`:**

| Field | Type | Purpose |
|-------|------|---------|
| `base_url` | `String` | RedEDR HTTP API base URL (e.g., `http://localhost:8081`) |
| `flush_interval_ms` | `u64` | Polling interval (default 1000ms) |
| `job_id` | `String` | Job identifier for TelemetryData tagging |
| `run_id` | `String` | Run identifier |

#### 3.5.2 `RedEdrCollector` Struct

```rust
pub struct RedEdrCollector {
    config: RedEdrCollectorConfig,
    client: reqwest::Client,          // HTTP client (5s timeout)
    seen_trace_ids: HashSet<u64>,     // Deduplication set
}
```

#### 3.5.3 Core Methods

**`start()` — Real-time polling loop:**

```rust
pub async fn start(mut self, tx: Sender<TelemetryData>) -> Result<()>
```

Consumes `self` and runs an infinite polling loop:

```
loop {
    fetch_events() → Vec<RedEdrEvent>
        │
        ▼
    Filter: remove events with trace_id ∈ seen_trace_ids
        │
        ▼
    For each new event:
        seen_trace_ids.insert(trace_id)
        transform_event() → TelemetryData
        tx.try_send(telemetry)  ← non-blocking
        │
        ▼
    sleep(flush_interval_ms)
}
```

**Why `try_send` not `send`:** The collector should never block waiting for the consumer. If the channel is full (consumer slow), skip the event rather than stalling the poll loop. This prevents backpressure from the gRPC stream from slowing telemetry collection.

**`fetch_events()` — HTTP API call:**

```rust
async fn fetch_events(&self) -> Result<Vec<RedEdrEvent>>
```

| Step | Detail |
|------|--------|
| URL | `{base_url}/api/logs/rededr` |
| Method | GET |
| Timeout | 5s (client-level) |
| Empty response | Returns empty Vec (RedEDR may return empty string instead of `[]`) |
| Parse error | Logs first 500 chars of response body + line/column for debugging |

**`start_trace()` — Activate tracing:**

```rust
pub async fn start_trace(&self, targets: Vec<String>) -> Result<()>
```

| Step | Detail |
|------|--------|
| URL | `{base_url}/api/trace/start` |
| Method | POST |
| Body | `{"trace": ["artifact.exe"]}` |
| Purpose | Tell RedEDR which process names to monitor |

**`collect_all()` — Batch collection:**

```rust
pub async fn collect_all(&self, job_id: &str) -> Result<Vec<TelemetryData>>
```

One-shot collection of all events. Used after execution completes (as opposed to `start()` which is real-time). Does NOT filter by `seen_trace_ids` — collects everything.

**`reset()` — Clear state:**

```rust
pub async fn reset(&self) -> Result<()>
```

| Step | Detail |
|------|--------|
| URL | `{base_url}/api/trace/reset` |
| Method | POST |
| Timeout | 30s (separate client — reset can be slow) |
| Purpose | Clear RedEDR's internal state between runs |

**`acquire_lock()` / `release_lock()` — Exclusive access:**

```rust
pub async fn acquire_lock(&self) -> Result<()>
pub async fn release_lock(&self) -> Result<()>
```

| Method | URL |
|--------|-----|
| `acquire_lock` | POST `{base_url}/api/lock/acquire` |
| `release_lock` | POST `{base_url}/api/lock/release` |

**Why locking:** The worker agent uses a single-execution lock (`ExecutionState` in `execution/state.rs`) to prevent concurrent runs. RedEDR's lock serves the same purpose at the telemetry layer — prevents ETW event cross-contamination between concurrent artifact executions on the same VM.

#### 3.5.4 `transform_event()` / `transform_event_with_job()`

```rust
fn transform_event(&self, event: &RedEdrEvent) -> TelemetryData
fn transform_event_with_job(&self, job_id: &str, event: &RedEdrEvent) -> TelemetryData
```

Converts a `RedEdrEvent` into a protobuf `TelemetryData`:

| TelemetryData Field | Value |
|---------------------|-------|
| `job_id` | From config or parameter |
| `event_type` | `event.type` or `"unknown"` |
| `timestamp` | Current UTC milliseconds |
| `payload` | Full event serialized as JSON bytes |
| `typed_event` | `None` (RedEDR events use generic payload) |
| `metadata.source` | `"rededr"` (always) |
| `metadata.event_type` | Event type string |
| `metadata.pid` | Process ID |
| `metadata.tid` | Thread ID |
| `metadata.provider` | ETW provider name |
| `metadata.trace_id` | Unique event ID |

**Why full JSON in payload:** RedEDR events are heterogeneous with variable fields. Storing the complete JSON preserves all information without needing to model every possible field in protobuf. The `metadata` map extracts the most commonly queried fields for fast filtering.

---

### 3.6 `collectors/trace.rs` — Named Pipe Trace Collector (547 lines)

The line-level execution trace collector. Receives trace events from instrumented artifacts via a Windows named pipe, supporting both a legacy Base64 text protocol and a newer binary protocol with automatic detection.

#### 3.6.1 Binary Protocol Header

```rust
#[repr(C, packed)]
struct InstRecordHeader {
    magic: u32,      // 0x49535452 ('ISTR')
    version: u16,
    event_type: u16, // 1=line_trace, 2-4=status (deprecated here)
    thread_id: u32,
    seq_no: u64,
    ts_us: u64,
    payload_len: u32,
}
```

Total size: 32 bytes. Matches the C runtime's `InstRecordHeader` with `#pragma pack(1)`.

**Why `#[repr(C, packed)]`:** The C instrumentation runtime uses a packed struct to minimize pipe traffic. The Rust side must match the exact memory layout. All field reads use `std::ptr::read_unaligned` because packed structs may have fields at unaligned addresses.

#### 3.6.2 `TraceEvent` — Parsed Output

```rust
pub struct TraceEvent {
    pub seq: u32,        // Sequence number (execution order)
    pub thread_id: u32,  // OS thread ID
    pub file: String,    // Source file name
    pub line: u32,       // Line number
    pub func: String,    // Function name
    pub ts_us: u64,      // Timestamp in microseconds
}
```

This is the canonical trace event format used internally. Both protocols (binary and Base64) are parsed into this structure.

#### 3.6.3 `TraceCollector` Struct

```rust
pub struct TraceCollector {
    pipe_name: String,                              // "\\.\pipe\rededr_trace"
    event_tx: mpsc::Sender<TraceEvent>,             // Channel to consumer
    sequence_counter: Arc<AtomicU32>,               // For Base64 protocol (no seq in wire format)
}
```

#### 3.6.4 `start_server()` — Named Pipe Server

```rust
#[cfg(windows)]
pub async fn start_server(&self) -> Result<()>
```

Fully async named pipe server using `tokio::net::windows::named_pipe`.

**Pipe creation:**

| Configuration | Value | Why |
|--------------|-------|-----|
| Pipe name | `\\.\pipe\rededr_trace` | Convention shared with C runtime |
| `in_buffer_size` | 1MB | Default 4KB too small for high-frequency line tracing (loops) |
| `out_buffer_size` | 1MB | Matching input buffer |
| Retry logic | 5 attempts, 200ms apart | Pipe may still exist from previous run |

**Connection loop:**

```
loop {
    server.connect().await  ← wait for artifact to connect
        │
        ▼
    Read first 4 bytes (with 2s timeout)
        │
        ▼
    Magic == 0x49535452?
    ├── yes → read_binary_stream()
    └── no  → read_text_stream()
        │
        ▼
    server.disconnect()
        │
        ▼
    Loop (accept next connection)
}
```

**Why auto-detection:** The project has two instrumentation backends:
- **Binary protocol:** Used by the newer `instrumentation_runtime.c` — includes thread_id, seq_no, ts_us in the header
- **Base64 text protocol:** Used by the AST-based `line_tracer.c` — simpler format, no binary header

Auto-detection means the same collector works with both backends without configuration.

**Non-Windows stub:** Returns `Err("only supported on Windows")`.

#### 3.6.5 `read_binary_stream()`

```rust
#[cfg(windows)]
async fn read_binary_stream<S>(&self, stream: &mut S, first_bytes: [u8; 4]) -> Result<()>
```

Reads a stream of binary protocol records. The first 4 bytes (magic) were already consumed by the auto-detection peek.

**First record handling:**
1. Read remaining 28 bytes of header
2. Reconstruct full 32-byte header from `first_bytes` + `header_rest`
3. Parse header via `ptr::read_unaligned`
4. Read payload (payload_len bytes)
5. Dispatch to `parse_on_event_type()`

**Subsequent records:**
```
loop {
    Read 32 bytes (full header)
    Validate magic
    Read payload_len bytes
    parse_on_event_type()
}
```

Loop breaks on read error (client disconnected) or invalid magic.

#### 3.6.6 `parse_on_event_type()`

```rust
fn parse_on_event_type(&self, hdr: &InstRecordHeader, event_type: u16, payload: &mut [u8])
```

| event_type | Handler |
|-----------|---------|
| 1 (line trace) | `handle_binary_line_trace()` |
| 2-4 (artifact status) | Warn — these should be in checkpoint pipe |
| other | Debug log |

#### 3.6.7 `handle_binary_line_trace()`

```rust
fn handle_binary_line_trace(&self, hdr: &InstRecordHeader, payload: &[u8]) -> Result<()>
```

Parses binary line trace payload format `"file:line:func"` and combines with header fields:

```
Header:   seq_no=42, thread_id=1234, ts_us=1699000000
Payload:  "loader_template.c:143:main"
                    │
                    ▼
TraceEvent {
    seq: 42,
    thread_id: 1234,
    file: "loader_template.c",
    line: 143,
    func: "main",
    ts_us: 1699000000,
}
```

Sends via `event_tx.try_send()` (non-blocking, same rationale as RedEDR collector).

#### 3.6.8 `read_text_stream()` / `handle_trace_line()`

```rust
async fn read_text_stream<S>(&self, stream: &mut S, first_bytes: [u8; 4]) -> Result<()>
fn handle_trace_line(&self, line: &str) -> Result<()>
```

Processes Base64-encoded line trace events. Two wire formats supported:

| Format | Prefix | Base64 Content | Origin |
|--------|--------|---------------|--------|
| IR format | `b64line:` | `line:file.c:42:main` | LLVM IR instrumentation pass |
| AST format | `YjY0` | `line:file.c:42:func` | AST-based line tracer (`YjY0` = Base64("b64")) |

**Parsing flow:**

```
"b64line:bGluZTp0ZXN0LmM6NDI6bWFpbg=="
    │
    ▼
Strip prefix → "bGluZTp0ZXN0LmM6NDI6bWFpbg=="
    │
    ▼
Base64 decode → "line:test.c:42:main"
    │
    ▼
splitn(4, ':') → ["line", "test.c", "42", "main"]
    │
    ▼
TraceEvent {
    seq: atomic_counter++,    ← No seq in Base64 format, use local counter
    thread_id: 0,             ← No thread_id in Base64 format
    file: "test.c",
    line: 42,
    func: "main",
    ts_us: SystemTime::now(), ← No timestamp in Base64 format
}
```

**Why atomic counter for seq:** The Base64 protocol doesn't include a sequence number in the wire format. The collector assigns monotonically increasing sequence numbers to preserve execution order. This is safe because the named pipe is single-client (one artifact per execution).

---

## 4. Cross-Module Interactions

### 4.1 Execution Engine Integration

The execution engine (`execution/engine.rs`) orchestrates telemetry collection across multiple phases:

```
Phase 3:  prepare_telemetry_dir()
Phase 4:  RedEdrCollector::acquire_lock() + start_trace()
Phase 5:  TraceCollector spawned (named pipe server)
Phase 6:  spawn_artifact() — artifact connects to named pipe
Phase 7:  RedEdrCollector::start() polling loop (via mpsc channel)
Phase 8:  Artifact completes / timeout
Phase 9:  pipeline::package_trace_log()
          pipeline::collect_trace_log_binary()
          pipeline::collect_bb_coverage()
          pipeline::collect_api_checkpoints()
          RedEdrCollector::collect_all()
Phase 10: cleanup + RedEdrCollector::release_lock()
```

### 4.2 Data Flow: Real-time vs Batch

```
Real-time (during execution):
    RedEDR HTTP poll ──→ mpsc::channel ──→ gRPC stream ──→ controller
    Named pipe trace ──→ mpsc::channel ──→ gRPC stream ──→ controller

Batch (after execution):
    trace_events.jsonl ──→ package_trace_log() ──→ Vec<TelemetryData>
    trace.log (binary) ──→ collect_trace_log_binary() ──→ Vec<TelemetryData>
    coverage_bbs.txt   ──→ collect_bb_coverage() ──→ TelemetryData
    checkpoints.log    ──→ collect_api_checkpoints() ──→ Vec<TelemetryData>
        │
        ▼
    All batch results sent via gRPC (unary or stream) → controller
```

### 4.3 Telemetry Source × TelemetryData Mapping

| Source | File | `event_type` | `typed_event` | Transport |
|--------|------|-------------|--------------|-----------|
| RedEDR HTTP | `rededr.rs` | varies (`etw`, `dll`, etc.) | `None` (generic JSON payload) | Real-time stream |
| Named pipe trace | `trace.rs` | — (parsed to `TraceEvent`) | — | Real-time stream |
| Trace events JSONL | `pipeline.rs` | `trace_log` | `None` (JSONL in payload) | Batch |
| Trace binary log | `pipeline.rs` | `trace_line` | `None` (raw bytes) | Batch |
| BB coverage | `pipeline.rs` | `coverage` | `CoverageEvent` | Batch |
| API checkpoints | `pipeline.rs` | `checkpoint` / `artifact_success` / `artifact_failure` | `CheckpointEvent` | Batch |

---

## 5. Dependency Map

```
telemetry/pipeline.rs
├── crate::automutate::common::TelemetryData     (protobuf type)
├── crate::automutate::common::CoverageEvent      (typed event)
├── crate::automutate::common::CheckpointEvent    (typed event)
├── crate::constants::MAX_SERIALIZED_PAYLOAD      (3.5MB limit)
├── serde_json                                     (JSONL parsing, payload serialization)
├── chrono::Utc                                    (timestamps)
├── tokio::fs                                      (async file I/O for coverage/checkpoints)
└── std::collections::HashMap                      (dedup map, metadata)

telemetry/trace_compressor.rs
├── serde::{Serialize, Deserialize}                (TraceEvent parsing)
├── serde_json                                     (JSONL parsing)
├── flate2                                         (gzip compression)
└── std::collections::HashMap                      (pattern map, dictionaries)

telemetry/collectors/rededr.rs
├── crate::automutate::common::TelemetryData       (protobuf type)
├── reqwest::Client                                (HTTP client)
├── serde::{Serialize, Deserialize}                (event parsing)
├── serde_json                                     (JSON serialization)
├── tokio::sync::mpsc::Sender                      (channel for real-time streaming)
├── tokio::time::sleep                             (poll interval)
├── chrono::Utc                                    (timestamps)
└── std::collections::HashSet                      (trace_id dedup)

telemetry/collectors/trace.rs
├── tokio::net::windows::named_pipe                (async pipe server — Windows only)
├── tokio::io::{AsyncReadExt, AsyncBufReadExt}     (stream reading)
├── tokio::sync::mpsc::Sender                      (channel for events)
├── base64::engine::general_purpose                (Base64 decoding)
├── anyhow                                         (error handling)
├── serde::{Serialize, Deserialize}                (TraceEvent)
└── std::sync::atomic::AtomicU32                   (sequence counter)
```

---

## 6. Platform Portability

| Component | Windows | Non-Windows |
|-----------|---------|-------------|
| `pipeline.rs` | Full behavior | Full behavior (pure Rust I/O) |
| `trace_compressor.rs` | Full behavior | Full behavior (pure algorithms) |
| `collectors/rededr.rs` | Full behavior | Full behavior (HTTP is cross-platform) |
| `collectors/trace.rs` | Named pipe server | Compile error stub (`bail!("only supported on Windows")`) |

The trace collector is the only platform-dependent component. It uses `#[cfg(windows)]` for the pipe server and `#[cfg(any(windows, test))]` for Base64 parsing (allowing tests to run on any platform).

---

## 7. Summary Statistics

| Metric | Value |
|--------|-------|
| Files | 6 |
| Total lines | 1935 |
| Public functions | 17 |
| Private functions | 7 |
| Test functions | 7 |
| Telemetry sources | 6 (RedEDR HTTP, named pipe trace, trace JSONL, trace binary, BB coverage, checkpoints) |
| Wire protocols | 3 (HTTP JSON, binary named pipe, Base64 named pipe) |
| Output format | Protobuf `TelemetryData` (all paths) |
| Platform-conditional code | `collectors/trace.rs` (Windows named pipe) |
| Not integrated | `trace_compressor.rs` (3 blocking issues documented) |
| External crate dependencies | reqwest, serde_json, base64, flate2, chrono, tokio, anyhow |
