// Job structure and state machine for scheduler queue system
// Phase 1: Basic implementation with in-memory storage

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use crate::round::RoundSummary;

/// Job status for iterative mutation loop
/// Queued -> Running -> Completed/Failed/Stopped
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    /// Job is waiting in queue
    Queued,
    /// Job is actively running mutation rounds
    Running,
    /// All rounds completed successfully
    Completed,
    /// Job failed (unrecoverable error)
    Failed,
    /// User manually stopped job
    Stopped,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Queued => write!(f, "queued"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Failed => write!(f, "failed"),
            JobStatus::Stopped => write!(f, "stopped"),
        }
    }
}

/// Mutation specification to apply at build stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationSpec {
    /// Mutation ID (e.g., "ast.import_reshape")
    pub id: String,
    /// Mutation parameters (optional)
    pub params: Option<serde_json::Value>,
}

/// Job structure containing all information about a build/execute task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique job identifier (e.g., "job-000001")
    pub id: String,

    /// Current job status
    pub status: JobStatus,

    /// Template name (e.g., "rwx_direct", "eicar")
    pub template_name: String,

    /// Source file path (e.g., "rwx_direct.c")
    pub source_file: String,

    /// Mutations to apply at build stage
    pub mutations: Vec<MutationSpec>,

    /// Trace mode ("api+bb", "lines", "all", etc.)
    pub trace_mode: String,

    /// Priority (higher = earlier execution)
    pub priority: i32,

    /// Current round number (1-indexed, 0 means not started)
    pub current_round: u32,

    /// Maximum rounds to execute (stop condition)
    pub max_rounds: u32,

    /// Stop condition: evasion goal
    pub stop_on_evasion: bool,  // If true, stop when not_detected

    /// Stop condition: detection goal (for testing)
    pub stop_on_detection: bool,  // If true, stop when detected

    /// History of completed rounds
    pub rounds: Vec<RoundSummary>,

    /// Assigned worker ID (None if not assigned yet)
    pub worker_id: Option<String>,

    /// Artifact ID after build (SHA256)
    pub artifact_id: Option<String>,

    /// Run ID during execution (UUID)
    pub run_id: Option<String>,

    /// Job creation timestamp
    pub created_at: SystemTime,

    /// Job start timestamp (when building begins)
    pub started_at: Option<SystemTime>,

    /// Job completion timestamp
    pub completed_at: Option<SystemTime>,

    /// Error message if failed
    pub error: Option<String>,
}

impl Job {
    /// Create a new job with given parameters
    pub fn new(
        id: String,
        template_name: String,
        source_file: String,
        mutations: Vec<MutationSpec>,
        trace_mode: String,
        priority: i32,
        max_rounds: u32,  // NEW parameter
    ) -> Self {
        Job {
            id,
            status: JobStatus::Queued,
            template_name,
            source_file,
            mutations,
            trace_mode,
            priority,
            current_round: 0,           // NEW
            max_rounds,                  // NEW
            stop_on_evasion: false,      // NEW
            stop_on_detection: false,    // NEW
            rounds: Vec::new(),          // NEW
            worker_id: None,
            artifact_id: None,
            run_id: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error: None,
        }
    }

    /// Start job execution (transition to Running)
    pub fn start_running(&mut self) {
        self.status = JobStatus::Running;
        if self.started_at.is_none() {
            self.started_at = Some(SystemTime::now());
        }
    }

    /// Transition job to Completed state
    pub fn mark_completed(&mut self) {
        self.status = JobStatus::Completed;
        self.completed_at = Some(SystemTime::now());
    }

    /// Transition job to Failed state with error message
    pub fn mark_failed(&mut self, error: String) {
        self.status = JobStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(SystemTime::now());
    }

    /// Transition job to Stopped state (user request)
    pub fn mark_stopped(&mut self) {
        self.status = JobStatus::Stopped;
        self.completed_at = Some(SystemTime::now());
    }

    /// Check if job is in a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Stopped
        )
    }

    /// Determine if job should continue with next round
    pub fn should_continue(&self) -> bool {
        // Check max rounds limit
        if self.current_round >= self.max_rounds {
            return false;
        }

        // Check stop conditions based on last round results
        if let Some(last_round) = self.rounds.last() {
            // Stop if evasion achieved and flag is set
            if self.stop_on_evasion && !last_round.detected {
                return false;
            }

            // Stop if detection occurred and flag is set (for testing)
            if self.stop_on_detection && last_round.detected {
                return false;
            }
        }

        true
    }

    /// Start a new round
    pub fn start_round(&mut self) {
        self.current_round += 1;
        if self.status == JobStatus::Queued {
            self.status = JobStatus::Running;
        }
    }

    /// Complete a round and store summary
    pub fn complete_round(&mut self, round_summary: RoundSummary) {
        self.rounds.push(round_summary);
    }

    /// Get progress percentage (0-100)
    pub fn progress_percent(&self) -> u32 {
        if self.max_rounds == 0 {
            return 0;
        }
        ((self.current_round as f32 / self.max_rounds as f32) * 100.0) as u32
    }

    /// Get elapsed time since job creation
    pub fn elapsed_seconds(&self) -> u64 {
        self.created_at
            .elapsed()
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_creation() {
        let job = Job::new(
            "job-000001".to_string(),
            "rwx_direct".to_string(),
            "rwx_direct.c".to_string(),
            vec![],
            "api+bb".to_string(),
            0,
            10,  // max_rounds
        );

        assert_eq!(job.id, "job-000001");
        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.priority, 0);
        assert_eq!(job.current_round, 0);
        assert_eq!(job.max_rounds, 10);
        assert!(job.worker_id.is_none());
        assert!(job.rounds.is_empty());
    }

    #[test]
    fn test_job_state_transitions() {
        let mut job = Job::new(
            "job-000001".to_string(),
            "test".to_string(),
            "test.c".to_string(),
            vec![],
            "api+bb".to_string(),
            0,
            10,  // max_rounds
        );

        // Queued -> Running
        job.start_running();
        assert_eq!(job.status, JobStatus::Running);
        assert!(job.started_at.is_some());

        // Running -> Completed
        job.mark_completed();
        assert_eq!(job.status, JobStatus::Completed);
        assert!(job.is_terminal());
    }

    #[test]
    fn test_job_failure() {
        let mut job = Job::new(
            "job-000001".to_string(),
            "test".to_string(),
            "test.c".to_string(),
            vec![],
            "api+bb".to_string(),
            0,
            10,  // max_rounds
        );

        job.mark_failed("Build error".to_string());
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.error, Some("Build error".to_string()));
        assert!(job.is_terminal());
    }

    #[test]
    fn test_should_continue_max_rounds() {
        let mut job = Job::new(
            "job-000001".to_string(),
            "test".to_string(),
            "test.c".to_string(),
            vec![],
            "api+bb".to_string(),
            0,
            5,  // max_rounds
        );

        // Should continue when current_round < max_rounds
        assert!(job.should_continue());

        // Should not continue when reaching max_rounds
        job.current_round = 5;
        assert!(!job.should_continue());
    }
}
