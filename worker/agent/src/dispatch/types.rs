use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::automutate::common::{SampleResponse, TelemetryData};
use automutate_config::WorkerConfig;

// ============================================================================
// Synthetic exit codes assigned by the engine (never from Windows).
// Real Windows exit codes are >= 0 and passed through verbatim.
// ============================================================================

/// OS error on child.wait()
pub const EXIT_WAIT_FAILED: i32 = -1;
/// Process exited but status.code() == None (externally terminated)
pub const EXIT_NO_CODE: i32 = -2;
/// Timeout expired, process killed
pub const EXIT_TIMEOUT: i32 = -3;
/// Never reached execution (spawn/setup failure)
pub const EXIT_INFRA: i32 = -4;

/// Typed request for executing an artifact run
pub struct RunRequest {
    pub job_id: String,
    pub artifact_id: String,
    pub timeout_seconds: u32,
    /// Resolved run_id (from controller's request_id or generated UUID)
    pub run_id: String,
}

/// Context for a run (worker-level state, not per-request)
pub struct RunContext {
    pub worker_id: String,
    pub config: WorkerConfig,
    pub telemetry_dir: PathBuf,
    pub artifact_path: PathBuf,
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

/// Outcome of a completed run
pub struct RunOutcome {
    pub exit_code: i32,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub telemetry_events: Vec<TelemetryData>,
    pub elapsed: Duration,
    pub phase_timings: RunPhaseTimings,
    /// Fine-grained classifier verdict string (e.g. "killed_pre_payload")
    pub detection_verdict: String,
    /// Last checkpoint reached before exit (e.g. "Launching")
    pub last_checkpoint: String,
}

/// Timing breakdown for each execution phase
#[derive(Debug, Default)]
pub struct RunPhaseTimings {
    pub rededr_setup_ms: u64,
    pub process_spawn_ms: u64,
    pub process_wait_ms: u64,
    pub telemetry_collect_ms: u64,
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

/// Build a SampleResponse for a failed execution.
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

/// Build a SampleResponse from a completed RunOutcome.
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

/// Resolve run_id from optional controller-provided value
pub fn resolve_run_id(requested: Option<&str>) -> String {
    requested
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}
