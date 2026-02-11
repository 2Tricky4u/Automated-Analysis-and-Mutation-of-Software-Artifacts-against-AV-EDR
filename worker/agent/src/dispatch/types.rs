use std::path::PathBuf;
use std::time::Duration;

use crate::automutate::common::TelemetryData;
use edr_config::WorkerConfig;

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

/// Outcome of a completed run
pub struct RunOutcome {
    pub exit_code: i32,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub telemetry_events: Vec<TelemetryData>,
    pub elapsed: Duration,
    pub phase_timings: RunPhaseTimings,
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

/// Resolve run_id from optional controller-provided value
pub fn resolve_run_id(requested: Option<&str>) -> String {
    requested
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}
