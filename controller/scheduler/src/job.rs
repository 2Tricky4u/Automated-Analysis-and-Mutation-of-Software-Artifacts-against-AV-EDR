// Job structure and state machine for scheduler queue system
// Phase 1: Basic implementation with in-memory storage

use crate::round::RoundSummary;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

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

/// Job structure containing all information about a build/execute task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique job identifier (e.g., "job-000001")
    pub id: String,

    /// Current job status
    pub status: JobStatus,

    /// Target OS for execution (e.g., "win10", "win11", "ubuntu")
    /// If None, scheduler will auto-assign based on first available worker
    pub target_os: Option<String>,

    /// Required capabilities for worker selection (e.g., ["mde"], ["cortex"])
    /// Workers must have ALL listed capabilities to be selected
    /// If empty, no capability filtering is applied
    pub required_capabilities: Vec<String>,

    /// Current round number (1-indexed, 0 means not started)
    pub current_round: u32,

    /// Maximum rounds to execute (stop condition)
    pub max_rounds: u32,

    /// Stop condition: evasion goal
    pub stop_on_evasion: bool, // If true, stop when not_detected
    
    /// History of completed rounds
    pub rounds: Vec<RoundSummary>,
    
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
    /// Create a new job with given parameters (legacy SourceFile mode)
    pub fn new(
        id: String,
        max_rounds: u32,
    ) -> Self {
        Job {
            id,
            status: JobStatus::Queued,
            target_os: None,
            required_capabilities: Vec::new(),
            current_round: 0,
            max_rounds,
            stop_on_evasion: false,
            rounds: Vec::new(),
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error: None,
        }
    }
    
    /// Start job execution 
    /// Marks running and set started time
    pub fn start_running(&mut self) {
        self.status = JobStatus::Running;
        if self.started_at.is_none() {
            self.started_at = Some(SystemTime::now());
        }
    }

    /// Transition job to Completed state
    /// Marks completed and set completed_at
    pub fn mark_completed(&mut self) {
        self.status = JobStatus::Completed;
        self.completed_at = Some(SystemTime::now());
    }

    /// Transition job to Failed state with error message
    /// Marks failed and set completed_at
    pub fn mark_failed(&mut self, error: String) {
        self.status = JobStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(SystemTime::now());
    }

    /// Transition job to Stopped state (user request)
    /// Marks Stopped and set completed_at
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
        }

        true
    }

    /// Start a new round
    /// Increase current round and put in running state
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
        self.created_at.elapsed().map(|d| d.as_secs()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests;
