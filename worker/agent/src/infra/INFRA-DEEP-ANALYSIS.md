# Infra Module — Deep Analysis

Deep analysis of `worker/agent/src/infra/` — the OS-level utility layer for process management, system metrics, filesystem operations, and time.

---

## 1. Overview

### Purpose

The `infra/` folder is the **pluggable OS boundary** for the worker agent. It isolates all direct operating system interactions — spawning processes, killing process trees, checking if a PID is alive, reading CPU/memory metrics, managing telemetry directories, and getting timestamps — into small, pure-function modules with no business logic.

### Role in the Global Project

Every component in the worker agent that needs to touch the OS goes through `infra/`:

- `execution/engine.rs` calls `process::spawn_artifact()`, `process::kill_process_tree()`, `process::capture_stream()`, `system::prepare_telemetry_dir()`, `system::cleanup_run_artifacts()`
- `execution/monitor.rs` calls `process::is_process_alive()` for process liveness checks
- `api/info.rs` calls `system::collect_system_metrics()` for health reporting and `time::now_unix_secs()` for timestamps

This separation serves three goals:
1. **Testability** — business logic in `execution/` and `api/` can be reasoned about without OS side effects
2. **Platform portability** — Windows-specific operations are `#[cfg(target_os = "windows")]` gated with non-Windows stubs
3. **Single responsibility** — each function does exactly one OS operation, making failure modes explicit

```
┌──────────────────────────────────────────┐
│  execution/engine.rs   api/info.rs       │
│  execution/monitor.rs  api/run.rs        │
│  session/stream_handler.rs               │
└──────────────┬───────────────────────────┘
               │ calls
               ▼
┌──────────────────────────────────────────┐
│              infra/ (this folder)          │
│                                            │
│  process.rs    system.rs      time.rs      │
│  ─────────    ──────────     ────────      │
│  spawn        metrics        timestamp     │
│  kill         cleanup                      │
│  alive?       prepare dir                  │
│  capture                                   │
└──────────────────────────────────────────┘
               │
               ▼
      OS: Windows API, filesystem, sysinfo, chrono
```

---

## 2. File Inventory

| File | Lines | Functions | Purpose |
|------|-------|-----------|---------|
| `mod.rs` | 6 | 0 | Module declarations |
| `process.rs` | 81 | 4 | Process spawn, kill, alive check, stream capture |
| `system.rs` | 46 | 3 | CPU/memory metrics, telemetry dir management, artifact cleanup |
| `time.rs` | 4 | 1 | Unix timestamp |
| **Total** | **137** | **8** | — |

---

## 3. Per-Module Deep Analysis

### 3.1 `process.rs` — Process Lifecycle Operations (81 lines)

Handles every phase of a child process's life: creation, monitoring, termination, and output capture.

#### 3.1.1 `spawn_artifact()`

```rust
pub fn spawn_artifact(
    artifact_path: &Path,
    working_dir: &Path,
) -> std::io::Result<tokio::process::Child>
```

Spawns the artifact PE as a child process.

| Configuration | Value | Why |
|--------------|-------|-----|
| `current_dir` | `working_dir` | Sets CWD so runtime files (trace.log, coverage.bin, checkpoints.log) are written to the telemetry directory |
| `stdin` | `Stdio::null()` | Artifact is non-interactive |
| `stdout` | `Stdio::piped()` | Captured for diagnostic logging |
| `stderr` | `Stdio::piped()` | Captured for error reporting |

**Callers:**
- `engine::execute_run()` — passes `telemetry_dir` as working directory so instrumentation runtime output lands in the right place
- `engine::execute_dryrun()` — passes artifact's parent directory (no telemetry directory for dryruns)

**Returns:** `tokio::process::Child` which is immediately wrapped in a `ProcessGuard` by the engine.

#### 3.1.2 `kill_process_tree()`

```rust
pub async fn kill_process_tree(child: &mut tokio::process::Child, _pid: Option<u32>)
```

Two-stage process termination for reliable cleanup on timeout:

```
Stage 1 (Windows only):
    taskkill /F /T /PID {pid}
    │
    │  /F = force kill (no graceful shutdown)
    │  /T = kill entire process tree (child processes too)
    │
Stage 2 (all platforms):
    child.kill().await
    │
    │  Tokio-level kill as fallback
    │
    sleep(500ms)
    │  Brief pause to let OS reclaim resources
```

**Why two stages:** The artifact may have spawned child processes (e.g., via `CreateProcess` for staged execution). `child.kill()` only kills the immediate child, not its descendants. `taskkill /T` walks the process tree and kills everything.

**Platform behavior:**
- Windows: runs `taskkill` first, then `child.kill()` as fallback
- Non-Windows: only `child.kill()` (the `#[cfg(target_os = "windows")]` block is skipped)

#### 3.1.3 `is_process_alive()`

```rust
#[cfg(target_os = "windows")]
pub fn is_process_alive(pid: u32) -> bool

#[cfg(not(target_os = "windows"))]
pub fn is_process_alive(_pid: u32) -> bool  // always returns false
```

Checks whether a process still exists using the Windows API.

**Windows implementation:**
```
OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
├── Ok(handle) && !handle.is_invalid() → process exists → CloseHandle → true
└── Err or invalid handle → process does not exist → false
```

**Why `PROCESS_QUERY_LIMITED_INFORMATION`:** This is the minimum privilege needed to check if a PID exists. Using `PROCESS_ALL_ACCESS` would fail for processes owned by other users (like SYSTEM processes).

**Callers:**
- `execution/monitor.rs` — called every 3 seconds to detect process termination
- `execution/engine.rs` — called after `kill_process_tree()` to verify the process actually died

**Non-Windows stub:** Returns `false` unconditionally. This allows the crate to compile on Linux/macOS for development/testing, even though the worker agent only runs on Windows VMs in production.

#### 3.1.4 `capture_stream()`

```rust
pub fn capture_stream<R>(stream: Option<R>) -> JoinHandle<String>
where
    R: AsyncRead + Unpin + Send + 'static,
```

Spawns an async task that reads an entire stream into a `String`. Used to capture stdout and stderr from the child process.

**Flow:**
```
Option<R> ─── Some(stream) ───► BufReader ──► read_to_string() ──► String
           └── None ───────────────────────────────────────────► ""
```

**Why async + spawned:** Stdout and stderr must be consumed concurrently with the process wait. If not consumed, the pipe buffers fill up and the child process blocks on write. Spawning separate tasks for stdout and stderr prevents this deadlock.

**Callers:**
- `engine::execute_run()` — captures both `child.stdout.take()` and `child.stderr.take()` before entering the wait loop

---

### 3.2 `system.rs` — System Metrics & Filesystem Operations (46 lines)

Three utility functions for OS-level system information and filesystem management.

#### 3.2.1 `collect_system_metrics()`

```rust
pub fn collect_system_metrics(sys: &sysinfo::System) -> (i32, i32)
```

Extracts CPU and memory percentages from an already-refreshed `sysinfo::System` object.

| Return | Computation | Guard |
|--------|------------|-------|
| `cpu_percent` | `sys.global_cpu_usage() as i32` | None (sysinfo returns 0.0 if not refreshed) |
| `memory_percent` | `(used_memory / total_memory) * 100` | Div-by-zero: returns 0 if `total_memory == 0` |

**Why `global_cpu_usage()`:** Uses the system-wide CPU average for consistency across all call sites (health check, worker info, status report). Per-core or per-process metrics are handled separately by the `ExecutionMonitor`.

**Callers:**
- `api/info.rs::health_check()` — periodic health reporting
- `api/info.rs::get_worker_info()` — capability response with live metrics

**Important:** The caller must refresh the `System` object before calling this function. The function does NOT call `refresh_*()` itself — this is intentional to allow callers to control refresh scope and frequency.

#### 3.2.2 `cleanup_run_artifacts()`

```rust
pub fn cleanup_run_artifacts(artifact_path: &Path, telemetry_dir: &Path)
```

Removes the artifact binary and its telemetry directory after a completed run.

| Operation | Target | On Failure |
|-----------|--------|-----------|
| `remove_file()` | `{artifacts_path}/{artifact_id}.exe` | Warning logged, continues |
| `remove_dir_all()` | `{artifacts_path}/telemetry_{artifact_id}/` | Warning logged, continues |

**Why non-fatal cleanup:** Failed cleanup (e.g., file locked by antivirus scan) should not fail the overall run. The engine has already collected all telemetry. Leftover files are a minor disk space issue, not a correctness issue.

**Caller:**
- `engine::execute_run()` — Phase 10 (final phase, after all telemetry is collected and streamed)

#### 3.2.3 `prepare_telemetry_dir()`

```rust
pub fn prepare_telemetry_dir(dir: &Path) -> std::io::Result<()>
```

Creates a clean telemetry directory for an execution run.

**Flow:**
```
dir exists? ─── yes ──► remove_dir_all() (silently ignore errors)
             └── no ──┐
                      ▼
               create_dir_all(dir)
```

**Why remove then recreate:** If the directory exists from a previous run (e.g., the previous run's cleanup failed), stale files (trace.log, coverage.bin) would be mixed with new data. Removing first guarantees a clean slate.

**Caller:**
- `engine::execute_run()` — Phase 3, before spawning the process. The telemetry directory becomes the process's working directory, so it must exist before spawn.

---

### 3.3 `time.rs` — Timestamp Utility (4 lines)

```rust
pub fn now_unix_secs() -> i64 {
    chrono::Utc::now().timestamp()
}
```

Returns the current UTC time as Unix seconds (epoch). One-liner wrapper around `chrono`.

**Why a wrapper:** Centralizes the timestamp source so all worker components use the same clock. If the project ever needs to switch to a monotonic clock or inject time for testing, only this function changes.

**Callers:**
- `api/info.rs::ping()` — response timestamp
- `api/info.rs::get_worker_info()` — uptime calculation baseline

---

## 4. Platform Portability Strategy

The `infra/` module uses `#[cfg(target_os = "windows")]` conditional compilation to support both the production environment (Windows VMs) and the development environment (Linux/macOS).

| Function | Windows | Non-Windows |
|----------|---------|-------------|
| `spawn_artifact()` | Full behavior | Full behavior (tokio::process works cross-platform) |
| `kill_process_tree()` | `taskkill /F /T` + `child.kill()` | `child.kill()` only |
| `is_process_alive()` | `OpenProcess` API | Always returns `false` |
| `collect_system_metrics()` | Full behavior | Full behavior (sysinfo is cross-platform) |
| `cleanup_run_artifacts()` | Full behavior | Full behavior |
| `prepare_telemetry_dir()` | Full behavior | Full behavior |
| `now_unix_secs()` | Full behavior | Full behavior |

Only `kill_process_tree()` and `is_process_alive()` have platform-specific behavior. Everything else uses cross-platform Rust standard library or crate APIs.

---

## 5. Dependency Map

```
infra/process.rs
├── tokio::process::Command, Child       (async process spawn/kill)
├── tokio::io::{AsyncRead, BufReader}    (stdout/stderr capture)
├── std::process::{Command, Stdio}       (taskkill, pipe config)
├── windows::Win32::System::Threading    (OpenProcess — Windows only)
└── windows::Win32::Foundation           (CloseHandle — Windows only)

infra/system.rs
├── sysinfo::System                      (CPU/memory metrics)
├── std::fs                              (remove_file, remove_dir_all, create_dir_all)
└── tracing                              (info/warn logging)

infra/time.rs
└── chrono::Utc                          (UTC timestamps)
```

---

## 6. Callers Map

| Function | Called By |
|----------|----------|
| `process::spawn_artifact()` | `execution/engine.rs` (execute_run, execute_dryrun) |
| `process::kill_process_tree()` | `execution/engine.rs` (timeout handling) |
| `process::is_process_alive()` | `execution/monitor.rs` (every poll), `execution/engine.rs` (post-kill check) |
| `process::capture_stream()` | `execution/engine.rs` (stdout/stderr capture) |
| `system::collect_system_metrics()` | `api/info.rs` (health_check, get_worker_info) |
| `system::cleanup_run_artifacts()` | `execution/engine.rs` (Phase 10) |
| `system::prepare_telemetry_dir()` | `execution/engine.rs` (Phase 3) |
| `time::now_unix_secs()` | `api/info.rs` (ping, get_worker_info) |

---

## 7. Summary Statistics

| Metric | Value |
|--------|-------|
| Files | 4 |
| Total lines | 137 |
| Public functions | 8 |
| Platform-conditional functions | 2 (`is_process_alive`, partial `kill_process_tree`) |
| External crate dependencies | tokio, sysinfo, chrono, windows (Windows only) |
| Business logic | 0 (pure OS operations) |
