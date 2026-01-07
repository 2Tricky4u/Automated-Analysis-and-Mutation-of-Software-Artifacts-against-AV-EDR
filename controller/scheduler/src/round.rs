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

/// Feedback for next round selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    /// Was artifact detected?
    pub detected: bool,

    /// Features to avoid (from triage analysis)
    pub avoid_features: Vec<String>,

    /// Features to seek (coverage-based)
    pub seek_features: Vec<String>,

    /// Evasion score (0.0 = detected, 1.0 = not detected)
    pub evasion_score: f64,
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

use crate::job::MutationSpec;

/// Complete round with dual-run protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    /// Round identifier (e.g., "round-1")
    pub round_id: String,

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

    /// Feedback from triage/differential analysis
    pub feedback: Option<Feedback>,

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
            round_id: format!("round-{}", round_number),
            job_id,
            round_number,
            mutations: Vec::new(),
            status: RoundStatus::InProgress,
            behavior_match: None,
            feedback: None,
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
            round_id: self.round_id.clone(),
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
mod tests {
    use super::*;

    #[test]
    fn test_round_status_display() {
        assert_eq!(RoundStatus::InProgress.to_string(), "in_progress");
        assert_eq!(RoundStatus::BaselineComplete.to_string(), "baseline_complete");
        assert_eq!(RoundStatus::ComparisonInProgress.to_string(), "comparison_in_progress");
        assert_eq!(RoundStatus::Completed.to_string(), "completed");
        assert_eq!(RoundStatus::Failed.to_string(), "failed");
        assert_eq!(RoundStatus::BehaviorMismatch.to_string(), "behavior_mismatch");
    }

    #[test]
    fn test_run_type_display() {
        assert_eq!(RunType::Baseline.to_string(), "baseline");
        assert_eq!(RunType::Instrumented.to_string(), "instrumented");
        assert_eq!(RunType::Baseline.as_str(), "baseline");
        assert_eq!(RunType::Instrumented.as_str(), "instrumented");
    }

    #[test]
    fn test_round_creation() {
        let round = Round::new("job-000001".to_string(), 1);

        assert_eq!(round.round_id, "round-1");
        assert_eq!(round.job_id, "job-000001");
        assert_eq!(round.round_number, 1);
        assert_eq!(round.status, RoundStatus::InProgress);
        assert!(round.mutations.is_empty());
        assert!(round.behavior_match.is_none());
        assert!(round.feedback.is_none());
        assert!(round.completed_at.is_none());
        assert!(round.error.is_none());
    }

    #[test]
    fn test_round_mark_completed() {
        let mut round = Round::new("job-000001".to_string(), 1);

        round.mark_completed();

        assert_eq!(round.status, RoundStatus::Completed);
        assert!(round.completed_at.is_some());
    }

    #[test]
    fn test_round_mark_failed() {
        let mut round = Round::new("job-000001".to_string(), 1);

        round.mark_failed("Test error".to_string());

        assert_eq!(round.status, RoundStatus::Failed);
        assert_eq!(round.error, Some("Test error".to_string()));
        assert!(round.completed_at.is_some());
    }

    #[test]
    fn test_round_to_summary_no_feedback() {
        let round = Round::new("job-000001".to_string(), 1);

        let summary = round.to_summary();

        assert_eq!(summary.round_id, "round-1");
        assert_eq!(summary.round_number, 1);
        assert!(summary.mutations.is_empty());
        assert!(!summary.detected); // default false
        assert!(!summary.behavior_match); // default false
        assert_eq!(summary.evasion_score, 0.0); // default 0.0
    }

    #[test]
    fn test_round_to_summary_with_feedback() {
        let mut round = Round::new("job-000001".to_string(), 1);

        round.feedback = Some(Feedback {
            detected: true,
            avoid_features: vec!["mem.rwx".to_string()],
            seek_features: vec!["benign.preamble".to_string()],
            evasion_score: 0.3,
        });

        round.behavior_match = Some(BehaviorComparison {
            outcome_match: true,
            baseline_detected: true,
            baseline_exit_code: 0,
            instrumented_detected: true,
            instrumented_exit_code: 0,
            differences: vec![],
            confidence: 0.95,
        });

        round.mutations = vec![MutationSpec {
            id: "ast.import_reshape".to_string(),
            params: None,
        }];

        let summary = round.to_summary();

        assert_eq!(summary.round_id, "round-1");
        assert_eq!(summary.round_number, 1);
        assert_eq!(summary.mutations, vec!["ast.import_reshape"]);
        assert!(summary.detected);
        assert!(summary.behavior_match);
        assert_eq!(summary.evasion_score, 0.3);
    }

    #[test]
    fn test_behavior_comparison_creation() {
        let comparison = BehaviorComparison {
            outcome_match: true,
            baseline_detected: false,
            baseline_exit_code: 0,
            instrumented_detected: false,
            instrumented_exit_code: 0,
            differences: vec![],
            confidence: 1.0,
        };

        assert!(comparison.outcome_match);
        assert!(!comparison.baseline_detected);
        assert!(!comparison.instrumented_detected);
        assert_eq!(comparison.confidence, 1.0);
    }

    #[test]
    fn test_behavior_comparison_with_mismatch() {
        let comparison = BehaviorComparison {
            outcome_match: false,
            baseline_detected: false,
            baseline_exit_code: 0,
            instrumented_detected: true,
            instrumented_exit_code: -1,
            differences: vec![
                "Instrumentation caused detection".to_string(),
                "Exit code mismatch".to_string(),
            ],
            confidence: 0.0,
        };

        assert!(!comparison.outcome_match);
        assert_eq!(comparison.differences.len(), 2);
        assert_eq!(comparison.confidence, 0.0);
    }

    #[test]
    fn test_feedback_creation() {
        let feedback = Feedback {
            detected: true,
            avoid_features: vec!["mem.rwx".to_string(), "thread.start.anon".to_string()],
            seek_features: vec!["benign.preamble".to_string()],
            evasion_score: 0.25,
        };

        assert!(feedback.detected);
        assert_eq!(feedback.avoid_features.len(), 2);
        assert_eq!(feedback.seek_features.len(), 1);
        assert_eq!(feedback.evasion_score, 0.25);
    }

    #[test]
    fn test_round_with_mutations() {
        let mut round = Round::new("job-000001".to_string(), 1);

        round.mutations = vec![
            MutationSpec {
                id: "ast.import_reshape".to_string(),
                params: Some(serde_json::json!({"delay_load": true})),
            },
            MutationSpec {
                id: "beh.preamble.fs".to_string(),
                params: None,
            },
        ];

        assert_eq!(round.mutations.len(), 2);

        let summary = round.to_summary();
        assert_eq!(summary.mutations, vec!["ast.import_reshape", "beh.preamble.fs"]);
    }
}
