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

#[test]
fn test_run_outcome_display() {
    assert_eq!(RunOutcome::NotDetected.to_string(), "not_detected");
    assert_eq!(RunOutcome::Detected.to_string(), "detected");
    assert_eq!(RunOutcome::Timeout.to_string(), "timeout");
    assert_eq!(RunOutcome::Crashed.to_string(), "crashed");
    assert_eq!(RunOutcome::Error.to_string(), "error");
}

#[test]
fn test_timeout_priority() {
    // Timeout takes priority over detection
    let outcome = RunOutcome::from_status(true, 0, true);
    assert_eq!(outcome, RunOutcome::Timeout);

    // Timeout takes priority over crash
    let outcome = RunOutcome::from_status(false, 1, true);
    assert_eq!(outcome, RunOutcome::Timeout);
}

#[test]
fn test_detection_priority_over_crash() {
    // Detection takes priority over crash (non-zero exit)
    let outcome = RunOutcome::from_status(true, 1, false);
    assert_eq!(outcome, RunOutcome::Detected);
}

#[test]
fn test_baseline_vs_instrumented_run_id() {
    let baseline = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "abc123".to_string(),
        vec![],
    );

    let instrumented = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Instrumented,
        "abc123".to_string(),
        vec![],
    );

    assert_eq!(baseline.run_id, "job-000001/round-1/baseline");
    assert_eq!(instrumented.run_id, "job-000001/round-1/instrumented");
}

#[test]
fn test_mutations_preserved() {
    let result = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "abc123".to_string(),
        vec![
            "ast.import_reshape".to_string(),
            "beh.preamble.fs".to_string(),
        ],
    );

    assert_eq!(result.mutations.len(), 2);
    assert_eq!(result.mutations[0], "ast.import_reshape");
    assert_eq!(result.mutations[1], "beh.preamble.fs");
}

#[test]
fn test_update_result_not_detected() {
    let mut result = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "abc123".to_string(),
        vec![],
    );

    result.update_result(false, 0, 5, 0, false);

    assert!(!result.detected);
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.outcome, RunOutcome::NotDetected);
}

#[test]
fn test_update_result_timeout() {
    let mut result = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Instrumented,
        "abc123".to_string(),
        vec![],
    );

    result.update_result(false, 0, 30, 100, true);

    assert_eq!(result.elapsed_seconds, 30);
    assert_eq!(result.outcome, RunOutcome::Timeout);
}

#[test]
fn test_update_result_crashed() {
    let mut result = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "abc123".to_string(),
        vec![],
    );

    result.update_result(false, -1, 2, 0, false);

    assert_eq!(result.exit_code, -1);
    assert_eq!(result.outcome, RunOutcome::Crashed);
}

#[test]
fn test_telemetry_count_baseline_vs_instrumented() {
    let mut baseline = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "abc123".to_string(),
        vec![],
    );

    let mut instrumented = RunResult::new(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Instrumented,
        "abc123".to_string(),
        vec![],
    );

    // Baseline typically has 0 telemetry events
    baseline.update_result(false, 0, 5, 0, false);
    assert_eq!(baseline.telemetry_events_count, 0);

    // Instrumented has many events
    instrumented.update_result(false, 0, 5, 1234, false);
    assert_eq!(instrumented.telemetry_events_count, 1234);
}