// Job structure and state machine for scheduler queue system
// Phase 1: Basic implementation with in-memory storage

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Job status states following the state machine:
/// Queued -> Building -> Deployed -> Running -> Completed/Failed/Timeout
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    /// Job is waiting in queue
    Queued,
    /// Build/emitter is running
    Building,
    /// Artifact sent to worker
    Deployed,
    /// Worker is executing artifact
    Running,
    /// Finished successfully
    Completed,
    /// Error occurred during any phase
    Failed,
    /// Execution timed out
    Timeout,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Queued => write!(f, "queued"),
            JobStatus::Building => write!(f, "building"),
            JobStatus::Deployed => write!(f, "deployed"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Failed => write!(f, "failed"),
            JobStatus::Timeout => write!(f, "timeout"),
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
    ) -> Self {
        Job {
            id,
            status: JobStatus::Queued,
            template_name,
            source_file,
            mutations,
            trace_mode,
            priority,
            worker_id: None,
            artifact_id: None,
            run_id: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error: None,
        }
    }

    /// Transition job to Building state
    pub fn start_building(&mut self) {
        self.status = JobStatus::Building;
        if self.started_at.is_none() {
            self.started_at = Some(SystemTime::now());
        }
    }

    /// Transition job to Deployed state with artifact ID
    pub fn mark_deployed(&mut self, artifact_id: String) {
        self.status = JobStatus::Deployed;
        self.artifact_id = Some(artifact_id);
    }

    /// Transition job to Running state with worker and run IDs
    pub fn mark_running(&mut self, worker_id: String, run_id: String) {
        self.status = JobStatus::Running;
        self.worker_id = Some(worker_id);
        self.run_id = Some(run_id);
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

    /// Transition job to Timeout state
    pub fn mark_timeout(&mut self) {
        self.status = JobStatus::Timeout;
        self.completed_at = Some(SystemTime::now());
    }

    /// Check if job is in a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Timeout
        )
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
        );

        assert_eq!(job.id, "job-000001");
        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.priority, 0);
        assert!(job.worker_id.is_none());
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
        );

        // Queued -> Building
        job.start_building();
        assert_eq!(job.status, JobStatus::Building);
        assert!(job.started_at.is_some());

        // Building -> Deployed
        job.mark_deployed("abc123".to_string());
        assert_eq!(job.status, JobStatus::Deployed);
        assert_eq!(job.artifact_id, Some("abc123".to_string()));

        // Deployed -> Running
        job.mark_running("worker-01".to_string(), "run-uuid".to_string());
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.worker_id, Some("worker-01".to_string()));

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
        );

        job.mark_failed("Build error".to_string());
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.error, Some("Build error".to_string()));
        assert!(job.is_terminal());
    }
}
