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

    assert_eq!(round.id, "round-1");
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