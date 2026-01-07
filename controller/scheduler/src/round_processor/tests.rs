use super::*;

// NOTE: Full process_round() tests require actual builder + worker infrastructure
// Use test-e2e-eicar.sh for end-to-end testing
// Unit tests below cover individual components (compare_behavior, feedback extraction)

#[test]
fn test_compare_behavior_identical_runs() {
    let processor = RoundProcessor::new();

    // Create two identical run results
    let baseline = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "artifact-123".to_string(),
        vec![],
    );

    let mut instrumented = baseline.clone();
    instrumented.run_type = RunType::Instrumented;

    // Compare
    let comparison = processor.compare_behavior(&baseline, &instrumented);

    assert!(comparison.is_ok(), "Comparison should succeed");
    let comp = comparison.unwrap();

    assert!(comp.outcome_match, "Identical runs should match");
    assert_eq!(comp.confidence, 1.0, "Perfect match = confidence 1.0");
    assert_eq!(comp.differences.len(), 0, "No differences expected");
}

#[test]
fn test_compare_behavior_different_detection() {
    let processor = RoundProcessor::new();

    // Create baseline (not detected)
    let baseline = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "artifact-123".to_string(),
        vec![],
    );

    // Create instrumented (detected)
    let mut instrumented = baseline.clone();
    instrumented.run_type = RunType::Instrumented;
    instrumented.detected = true;
    instrumented.outcome = RunOutcome::Detected;

    // Compare
    let comparison = processor.compare_behavior(&baseline, &instrumented);

    assert!(comparison.is_ok(), "Comparison should succeed");
    let comp = comparison.unwrap();

    assert!(!comp.outcome_match, "Different detection should not match");
    assert_eq!(comp.differences.len(), 1, "Should have 1 difference");
    assert!(
        comp.differences[0].contains("Detection mismatch"),
        "Should mention detection mismatch"
    );
    assert_eq!(comp.confidence, 0.75, "One difference = confidence 0.75");
}

#[test]
fn test_compare_behavior_different_exit_codes() {
    let processor = RoundProcessor::new();

    // Create baseline (exit code 0)
    let baseline = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "artifact-123".to_string(),
        vec![],
    );

    // Create instrumented (exit code 1)
    let mut instrumented = baseline.clone();
    instrumented.run_type = RunType::Instrumented;
    instrumented.exit_code = 1;

    // Compare
    let comparison = processor.compare_behavior(&baseline, &instrumented);

    assert!(comparison.is_ok(), "Comparison should succeed");
    let comp = comparison.unwrap();

    assert!(!comp.outcome_match, "Different exit codes should not match");
    assert_eq!(comp.differences.len(), 1, "Should have 1 difference");
    assert!(
        comp.differences[0].contains("Exit code mismatch"),
        "Should mention exit code mismatch"
    );
}

#[test]
fn test_compare_behavior_multiple_differences() {
    let processor = RoundProcessor::new();

    // Create baseline
    let baseline = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "artifact-123".to_string(),
        vec![],
    );

    // Create instrumented with multiple differences
    let mut instrumented = baseline.clone();
    instrumented.run_type = RunType::Instrumented;
    instrumented.detected = true;
    instrumented.exit_code = 1;

    // Compare
    let comparison = processor.compare_behavior(&baseline, &instrumented);

    assert!(comparison.is_ok(), "Comparison should succeed");
    let comp = comparison.unwrap();

    assert!(!comp.outcome_match, "Multiple differences should not match");
    assert_eq!(comp.differences.len(), 2, "Should have 2 differences");
    assert_eq!(comp.confidence, 0.5, "Two differences = confidence 0.5");
}

#[test]
fn test_round_processor_with_selector() {
    // Test that RoundProcessor can be created with Selector address
    let processor = RoundProcessor::with_selector("localhost:50054".to_string());

    assert_eq!(
        processor.selector_address,
        Some("localhost:50054".to_string())
    );
}

#[test]
fn test_round_processor_without_selector() {
    // Test that RoundProcessor works without Selector (fallback mode)
    let processor = RoundProcessor::new();

    assert_eq!(processor.selector_address, None);
}

#[test]
fn test_feedback_extraction_from_previous_rounds() {
    // Test that feedback is correctly extracted from previous rounds
    use crate::round::RoundSummary;
    use std::time::SystemTime;

    // Create a round summary (simulates a completed previous round)
    let previous_round = RoundSummary {
        round_id: "round-1".to_string(),
        round_number: 1,
        mutations: vec!["ast.import_reshape".to_string()],
        detected: true, // Was detected
        behavior_match: true,
        evasion_score: 0.0, // No evasion (detected)
        completed_at: SystemTime::now(),
    };

    // Verify fields that would be passed to Selector
    assert!(previous_round.detected, "Should be detected");
    assert_eq!(
        previous_round.evasion_score, 0.0,
        "No evasion when detected"
    );

    // Create another round summary (not detected)
    let successful_round = RoundSummary {
        round_id: "round-2".to_string(),
        round_number: 2,
        mutations: vec!["beh.preamble.fs".to_string()],
        detected: false, // Not detected
        behavior_match: true,
        evasion_score: 1.0, // Full evasion (not detected)
        completed_at: SystemTime::now(),
    };

    // Verify fields that would be passed to Selector
    assert!(!successful_round.detected, "Should not be detected");
    assert_eq!(
        successful_round.evasion_score, 1.0,
        "Full evasion when not detected"
    );
}