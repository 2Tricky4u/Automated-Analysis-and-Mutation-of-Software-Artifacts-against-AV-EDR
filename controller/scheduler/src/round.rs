use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Round status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RoundStatus {
    /// Round is in progress
    InProgress,
    /// Baseline run complete, instrumented run pending
    BaselineComplete,
    /// Both runs complete, comparison in progress
    ComparisonInProgress,
    /// Round completed successfully
    Completed,
    /// Round failed (error during execution)
    Failed,
    /// Behavior mismatch between baseline and instrumented
    BehaviorMismatch,
}

impl std::fmt::Display for RoundStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoundStatus::InProgress => write!(f, "in_progress"),
            RoundStatus::BaselineComplete => write!(f, "baseline_complete"),
            RoundStatus::ComparisonInProgress => write!(f, "comparison_in_progress"),
            RoundStatus::Completed => write!(f, "completed"),
            RoundStatus::Failed => write!(f, "failed"),
            RoundStatus::BehaviorMismatch => write!(f, "behavior_mismatch"),
        }
    }
}

/// Run type identifier
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunType {
    Baseline,     // No instrumentation
    Instrumented, // Full tracing
}

impl RunType {
    pub fn as_str(&self) -> &str {
        match self {
            RunType::Baseline => "baseline",
            RunType::Instrumented => "instrumented",
        }
    }
}

impl std::fmt::Display for RunType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Behavior comparison between baseline and instrumented runs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorComparison {
    /// Do both runs have the same outcome?
    pub outcome_match: bool,

    /// Baseline outcome
    pub baseline_detected: bool,
    pub baseline_exit_code: i32,

    /// Instrumented outcome
    pub instrumented_detected: bool,
    pub instrumented_exit_code: i32,

    /// Differences detected
    pub differences: Vec<String>,

    /// Confidence that behaviors are identical (0.0 to 1.0)
    pub confidence: f64,
}

/// Summary of a completed round (stored in Job.rounds)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundSummary {
    pub round_id: String,
    pub round_number: u32,
    pub mutations: Vec<String>, // Mutation IDs only
    pub detected: bool,         // Was artifact detected?
    pub behavior_match: bool,   // Did baseline and instrumented match?
    pub evasion_score: f64,     // Score for selector feedback (0.0-1.0)
    pub completed_at: SystemTime,
}


/// Mutation specification to apply at build stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationSpec {
    /// Mutation ID (e.g., "ast.import_reshape")
    pub id: String,
    /// Mutation parameters (optional)
    pub params: Option<serde_json::Value>,
}

/// Complete round with dual-run protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    /// Round identifier (e.g., "round-1")
    pub id: String,

    /// Parent job ID
    pub job_id: String,

    /// Round number (1-indexed)
    pub round_number: u32,

    /// Mutations applied in this round
    pub mutations: Vec<MutationSpec>,

    /// Round status
    pub status: RoundStatus,

    /// Behavior comparison result
    pub behavior_match: Option<BehaviorComparison>,

    /// Round start timestamp
    pub started_at: SystemTime,

    /// Round completion timestamp
    pub completed_at: Option<SystemTime>,

    /// Error message if failed
    pub error: Option<String>,
}

impl Round {
    /// Create a new round
    pub fn new(job_id: String, round_number: u32) -> Self {
        Round {
            id: format!("round-{}", round_number),
            job_id,
            round_number,
            modular_build: ModularBuildSpec,
            mutations: Vec::new(),
            status: RoundStatus::InProgress,
            behavior_match: None,
            started_at: SystemTime::now(),
            completed_at: None,
            error: None,
        }
    }

    /// Mark round as completed
    pub fn mark_completed(&mut self) {
        self.status = RoundStatus::Completed;
        self.completed_at = Some(SystemTime::now());
    }

    /// Mark round as failed
    pub fn mark_failed(&mut self, error: String) {
        self.status = RoundStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(SystemTime::now());
    }

    /// Create summary for storage in Job
    pub fn to_summary(&self) -> RoundSummary {
        RoundSummary {
            round_id: self.id.clone(),
            round_number: self.round_number,
            mutations: self.mutations.iter().map(|m| m.id.clone()).collect(),
            detected: self.feedback.as_ref().map(|f| f.detected).unwrap_or(false),
            behavior_match: self
                .behavior_match
                .as_ref()
                .map(|b| b.outcome_match)
                .unwrap_or(false),
            evasion_score: self
                .feedback
                .as_ref()
                .map(|f| f.evasion_score)
                .unwrap_or(0.0),
            completed_at: self.completed_at.unwrap_or(SystemTime::now()),
        }
    }
}

#[cfg(test)]
mod tests;
