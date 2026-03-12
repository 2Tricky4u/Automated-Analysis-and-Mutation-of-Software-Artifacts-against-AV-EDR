//! Shared types for the execution subsystem.
//!
//! Defines [`RunRequest`], [`RunContext`], [`RunOutcome`], and [`RunPhaseTimings`] —
//! the value objects that flow through the execution engine pipeline. Also defines
//! synthetic exit code constants and [`SampleResponse`]
//! builder functions used by both the unary RPC and stream handler paths.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::automutate::common::{SampleResponse, TelemetryData};
use automutate_config::WorkerConfig;

// ============================================================================
// Synthetic exit codes — re-exported from automutate_common.
// Values in the -100_00x range — unreachable from Windows ExitProcess(UINT).
// ============================================================================

pub use automutate_common::{EXIT_INFRA, EXIT_NO_CODE, EXIT_TIMEOUT, EXIT_WAIT_FAILED};

/// Typed request for executing an artifact run.
///
/// Passed to [`execute_run`](crate::execution::engine::execute_run) and
/// [`execute_dryrun`](crate::execution::engine::execute_dryrun).
pub struct RunRequest {
    /// Controller-assigned job identifier.
    pub job_id: String,
    /// Artifact identifier (used to derive the `.exe` filename).
    pub artifact_id: String,
    /// Maximum execution time before the process is killed.
    pub timeout_seconds: u32,
    /// Resolved run_id (from controller's `request_id` or generated UUID).
    pub run_id: String,
}

/// Context for a run (worker-level state, not per-request).
pub struct RunContext {
    /// Identity of this worker (used for status reports).
    pub worker_id: String,
    /// Worker configuration (paths, telemetry settings, etc.).
    pub config: WorkerConfig,
    /// Directory where telemetry files are written during the run.
    pub telemetry_dir: PathBuf,
    /// Full filesystem path to the artifact `.exe` on disk.
    pub artifact_path: PathBuf,
    /// Filename of the artifact (e.g. `"artifact-001.exe"`).
    pub artifact_name: String,
}

impl RunContext {
    /// Build a RunContext from an artifact ID, deriving all paths from config.
    pub fn new(artifact_id: &str, worker_id: String, config: WorkerConfig) -> Self {
        let artifact_name = format!("{}.exe", artifact_id);
        let artifacts_base = std::path::Path::new(&config.storage.artifacts_path);
        Self {
            artifact_path: artifacts_base.join(&artifact_name),
            telemetry_dir: artifacts_base.join(format!("telemetry_{}", artifact_id)),
            artifact_name,
            worker_id,
            config,
        }
    }
}

/// Outcome of a completed run.
///
/// Returned by [`execute_run`](crate::execution::engine::execute_run) and
/// converted into a [`SampleResponse`]
/// via [`sample_response_ok`].
pub struct RunOutcome {
    /// Process exit code (may be a synthetic constant like [`EXIT_INFRA`]).
    pub exit_code: i32,
    /// `true` if the process was killed because the timeout expired.
    pub timed_out: bool,
    /// Captured standard output of the artifact process.
    pub stdout: String,
    /// Captured standard error of the artifact process.
    pub stderr: String,
    /// All telemetry events collected during the run.
    pub telemetry_events: Vec<TelemetryData>,
    /// Wall-clock time from spawn to process exit.
    pub elapsed: Duration,
    /// Per-phase timing breakdown for observability.
    pub phase_timings: RunPhaseTimings,
    /// Fine-grained [`DetectionVerdict`](automutate_common::DetectionVerdict) string
    /// (e.g. `"detected"`, `"evasion"`).
    pub detection_verdict: String,
    /// Last checkpoint reached before exit (e.g. `"Launching"`).
    pub last_checkpoint: String,
}

/// Timing breakdown for each execution phase.
#[derive(Debug, Default)]
pub struct RunPhaseTimings {
    /// Time spent configuring RedEDR (contamination check, start tracing).
    pub rededr_setup_ms: u64,
    /// Time spent spawning the artifact process.
    pub process_spawn_ms: u64,
    /// Time spent waiting for the process to exit (or timeout).
    pub process_wait_ms: u64,
    /// Time spent collecting telemetry after process exit.
    pub telemetry_collect_ms: u64,
    /// Time spent resetting RedEDR state for the next run.
    pub rededr_reset_ms: u64,
}

impl RunPhaseTimings {
    /// Convert to metadata HashMap for telemetry event inclusion.
    pub fn to_metadata(&self) -> HashMap<String, String> {
        [
            ("rededr_setup_ms", self.rededr_setup_ms),
            ("process_spawn_ms", self.process_spawn_ms),
            ("process_wait_ms", self.process_wait_ms),
            ("telemetry_collect_ms", self.telemetry_collect_ms),
            ("rededr_reset_ms", self.rededr_reset_ms),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }
}

/// Build a [`SampleResponse`] for a failed execution.
///
/// Sets `exit_code` to [`EXIT_INFRA`] and `detection_verdict` to `"infra_error"`.
pub fn sample_response_error(
    job_id: &str,
    run_id: &str,
    error: &dyn std::fmt::Display,
) -> SampleResponse {
    SampleResponse {
        job_id: job_id.to_string(),
        success: false,
        exit_code: EXIT_INFRA,
        output: format!("Execution error: {}", error),
        telemetry_ids: vec![],
        run_id: run_id.to_string(),
        detected: false,
        error: error.to_string(),
        elapsed_ms: 0.0,
        detection_verdict: "infra_error".to_string(),
        last_checkpoint: String::new(),
    }
}

/// Build a [`SampleResponse`] from a completed [`RunOutcome`].
///
/// Uses the [`DetectionVerdict`](automutate_common::DetectionVerdict) from the
/// outcome when available, falling back to legacy exit-code logic.
pub fn sample_response_ok(
    job_id: &str,
    run_id: &str,
    outcome: &RunOutcome,
    output: String,
) -> SampleResponse {
    // Use classifier verdict for detected flag when available, fall back to legacy logic
    let detected = if !outcome.detection_verdict.is_empty() {
        automutate_common::DetectionVerdict::from_verdict_str(&outcome.detection_verdict)
            .map(|v| v.is_detected())
            .unwrap_or(outcome.exit_code != 0 && outcome.exit_code > 0)
    } else {
        match outcome.exit_code {
            EXIT_NO_CODE => true,
            0 | EXIT_WAIT_FAILED | EXIT_INFRA | EXIT_TIMEOUT => false,
            _ => true,
        }
    };

    SampleResponse {
        job_id: job_id.to_string(),
        success: !outcome.timed_out && outcome.exit_code == 0,
        exit_code: outcome.exit_code,
        output,
        telemetry_ids: vec![run_id.to_string()],
        run_id: run_id.to_string(),
        detected,
        error: String::new(),
        elapsed_ms: outcome.elapsed.as_secs_f64() * 1000.0,
        detection_verdict: outcome.detection_verdict.clone(),
        last_checkpoint: outcome.last_checkpoint.clone(),
    }
}

/// Resolve `run_id` from an optional controller-provided value.
///
/// Returns the provided string when non-empty, otherwise generates a new UUID v4.
pub fn resolve_run_id(requested: Option<&str>) -> String {
    requested
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

// ============================================================================
// Output formatting helpers (used by both api/run.rs and session/stream_handler.rs)
// ============================================================================

/// Format a human-readable output string from a [`RunOutcome`].
pub fn format_output(outcome: &RunOutcome, timeout_seconds: u32) -> String {
    if outcome.timed_out {
        format!("Execution timed out after {}s", timeout_seconds)
    } else if outcome.exit_code == 0 {
        if !outcome.stdout.is_empty() {
            format!(
                "Execution completed in {:.2}s\nOutput:\n{}",
                outcome.elapsed.as_secs_f64(),
                outcome.stdout
            )
        } else {
            format!(
                "Execution completed in {:.2}s",
                outcome.elapsed.as_secs_f64()
            )
        }
    } else {
        let error_type = describe_exit(outcome.exit_code);
        let mut output = format!(
            "Execution failed with exit code {} ({}), elapsed: {:.2}s",
            outcome.exit_code,
            error_type,
            outcome.elapsed.as_secs_f64()
        );

        if !outcome.stderr.is_empty() {
            output.push_str(&format!("\nStderr:\n{}", outcome.stderr));
        }

        if !outcome.stdout.is_empty() {
            output.push_str(&format!("\nStdout:\n{}", outcome.stdout));
        }

        output
    }
}

// ============================================================================
// Exit code interpretation helpers
// ============================================================================

fn describe_exit(exit_code: i32) -> String {
    // Synthetic engine exit codes (negative)
    match exit_code {
        EXIT_NO_CODE => return "Externally terminated (no exit code)".to_string(),
        EXIT_WAIT_FAILED => return "wait() failed".to_string(),
        EXIT_TIMEOUT => return "Timeout (process killed)".to_string(),
        EXIT_INFRA => return "Infrastructure error (never executed)".to_string(),
        _ => {}
    }

    let code_u32 = exit_code as u32;

    if code_u32 == 0 {
        return "Success".to_string();
    }

    // Loader namespaced ranges
    match exit_code {
        10..=19 => return "Guardrail failed".to_string(),
        30 => return "Carrier: VirtualAlloc failed".to_string(),
        31 => return "Carrier: VirtualProtect failed".to_string(),
        32 => return "Carrier: PEB module resolution failed".to_string(),
        33 => return "Carrier: PEB export resolution failed".to_string(),
        34..=39 => return "Carrier: unknown error".to_string(),
        _ => {}
    }

    if looks_like_ntstatus(code_u32) {
        if let Some(msg) = ntstatus_to_message(code_u32) {
            return format!("NTSTATUS 0x{code_u32:08X}: {msg}");
        }
        return format!("NTSTATUS 0x{code_u32:08X}");
    }

    format!("Exit code {code_u32} (0x{code_u32:08X})")
}

#[cfg(target_os = "windows")]
fn ntstatus_to_message(status: u32) -> Option<String> {
    use windows::Win32::Foundation::{
        GetLastError, HLOCAL, LocalFree, NTSTATUS, RtlNtStatusToDosError,
    };
    use windows::Win32::System::Diagnostics::Debug::{
        FORMAT_MESSAGE_ALLOCATE_BUFFER, FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS,
        FormatMessageW,
    };
    use windows::core::PWSTR;

    let dos: u32 = unsafe { RtlNtStatusToDosError(NTSTATUS(status as i32)) };
    if dos == 0 {
        return None;
    }

    let flags =
        FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS | FORMAT_MESSAGE_ALLOCATE_BUFFER;

    let buf: PWSTR = PWSTR::null();

    let len = unsafe { FormatMessageW(flags, None, dos, 0, PWSTR(buf.0), 0, None) };

    if len == 0 || buf.is_null() {
        let _ = unsafe { GetLastError() };
        return None;
    }

    let slice = unsafe { std::slice::from_raw_parts(buf.0, len as usize) };
    let s = String::from_utf16_lossy(slice).trim().to_string();

    unsafe {
        let _ = LocalFree(Option::from(HLOCAL(buf.0 as _)));
    }

    Some(s)
}

#[cfg(not(target_os = "windows"))]
fn ntstatus_to_message(_status: u32) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn looks_like_ntstatus(code: u32) -> bool {
    (code & 0x8000_0000) != 0
}

#[cfg(not(target_os = "windows"))]
fn looks_like_ntstatus(_code: u32) -> bool {
    false
}
