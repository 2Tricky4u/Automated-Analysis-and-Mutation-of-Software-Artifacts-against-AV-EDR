use crate::round::RunType;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Run outcome
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunOutcome {
    /// Artifact executed successfully without detection
    NotDetected,
    /// Artifact was detected by EDR
    Detected,
    /// Execution timed out
    Timeout,
    /// Artifact crashed during execution
    Crashed,
    /// Error occurred (build, deploy, or execution error)
    Error,
}

impl std::fmt::Display for RunOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunOutcome::NotDetected => write!(f, "not_detected"),
            RunOutcome::Detected => write!(f, "detected"),
            RunOutcome::Timeout => write!(f, "timeout"),
            RunOutcome::Crashed => write!(f, "crashed"),
            RunOutcome::Error => write!(f, "error"),
        }
    }
}

impl RunOutcome {
    /// Create outcome from detection status and exit code
    pub fn from_status(detected: bool, exit_code: i32, timed_out: bool) -> Self {
        if timed_out {
            RunOutcome::Timeout
        } else if detected {
            RunOutcome::Detected
        } else if exit_code != 0 {
            RunOutcome::Crashed
        } else {
            RunOutcome::NotDetected
        }
    }
}

/// Complete run result (baseline or instrumented)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    /// Full run ID: {job_id}/{round_id}/{run_type}
    pub run_id: String, // e.g., "job-000001/round-3/baseline"

    /// Parent job ID
    pub job_id: String,

    /// Parent round ID
    pub round_id: String,

    /// Run type (baseline or instrumented)
    pub run_type: RunType,

    /// Artifact ID (SHA256)
    pub artifact_id: String,

    /// Mutations applied (mutation IDs)
    pub mutations: Vec<String>,

    /// Run outcome
    pub outcome: RunOutcome,

    /// Was artifact detected by EDR?
    pub detected: bool,

    /// Exit code
    pub exit_code: i32,

    /// Worker that executed this run
    pub worker_id: String,

    /// Execution duration in seconds
    pub elapsed_seconds: u64,

    /// Number of telemetry events collected (0 for baseline)
    pub telemetry_events_count: u64,

    /// Run start timestamp
    pub started_at: SystemTime,

    /// Run completion timestamp
    pub completed_at: SystemTime,
}

impl RunResult {
    /// Create a new run result
    pub fn new(
        job_id: String,
        round_id: String,
        run_type: RunType,
        artifact_id: String,
        mutations: Vec<String>,
    ) -> Self {
        let run_id = format!("{}/{}/{}", job_id, round_id, run_type.as_str());

        RunResult {
            run_id,
            job_id,
            round_id,
            run_type,
            artifact_id,
            mutations,
            outcome: RunOutcome::NotDetected,
            detected: false,
            exit_code: 0,
            worker_id: String::new(),
            elapsed_seconds: 0,
            telemetry_events_count: 0,
            started_at: SystemTime::now(),
            completed_at: SystemTime::now(),
        }
    }

    /// Update with execution result
    pub fn update_result(
        &mut self,
        detected: bool,
        exit_code: i32,
        elapsed_seconds: u64,
        telemetry_count: u64,
        timed_out: bool,
    ) {
        self.detected = detected;
        self.exit_code = exit_code;
        self.elapsed_seconds = elapsed_seconds;
        self.telemetry_events_count = telemetry_count;
        self.outcome = RunOutcome::from_status(detected, exit_code, timed_out);
        self.completed_at = SystemTime::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_result_creation() {
        let result = RunResult::new(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "abc123".to_string(),
            vec![],
        );

        assert_eq!(result.run_id, "job-000001/round-1/baseline");
        assert_eq!(result.job_id, "job-000001");
        assert_eq!(result.round_id, "round-1");
        assert_eq!(result.run_type, RunType::Baseline);
        assert_eq!(result.outcome, RunOutcome::NotDetected);
    }

    #[test]
    fn test_run_outcome_from_status() {
        // Not detected, clean exit
        let outcome = RunOutcome::from_status(false, 0, false);
        assert_eq!(outcome, RunOutcome::NotDetected);

        // Detected
        let outcome = RunOutcome::from_status(true, 0, false);
        assert_eq!(outcome, RunOutcome::Detected);

        // Crashed (non-zero exit)
        let outcome = RunOutcome::from_status(false, 1, false);
        assert_eq!(outcome, RunOutcome::Crashed);

        // Timeout
        let outcome = RunOutcome::from_status(false, 0, true);
        assert_eq!(outcome, RunOutcome::Timeout);
    }

    #[test]
    fn test_update_result() {
        let mut result = RunResult::new(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Instrumented,
            "abc123".to_string(),
            vec!["ast.string_xor".to_string()],
        );

        result.update_result(true, 1, 60, 342, false);

        assert_eq!(result.detected, true);
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.elapsed_seconds, 60);
        assert_eq!(result.telemetry_events_count, 342);
        assert_eq!(result.outcome, RunOutcome::Detected);
    }
}
