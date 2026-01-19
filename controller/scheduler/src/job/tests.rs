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
        10, // max_rounds
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
        10, // max_rounds
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
        10, // max_rounds
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
        5, // max_rounds
    );

    // Should continue when current_round < max_rounds
    assert!(job.should_continue());

    // Should not continue when reaching max_rounds
    job.current_round = 5;
    assert!(!job.should_continue());
}

#[test]
fn test_stop_on_evasion() {
    let mut job = Job::new(
        "job-000001".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        10,
    );

    job.stop_on_evasion = true;

    // Should continue before any rounds
    assert!(job.should_continue());

    // Add detected round
    let detected_round = RoundSummary {
        round_id: "round-1".to_string(),
        round_number: 1,
        mutations: vec![],
        detected: true,
        behavior_match: true,
        evasion_score: 0.0,
        completed_at: SystemTime::now(),
    };
    job.complete_round(detected_round);

    // Should continue after detected round
    assert!(job.should_continue());

    // Add evasion round (not_detected)
    let evasion_round = RoundSummary {
        round_id: "round-2".to_string(),
        round_number: 2,
        mutations: vec![],
        detected: false,
        behavior_match: true,
        evasion_score: 1.0,
        completed_at: SystemTime::now(),
    };
    job.complete_round(evasion_round);

    // Should NOT continue after evasion (stop_on_evasion = true)
    assert!(!job.should_continue());
}

#[test]
fn test_stop_on_detection() {
    let mut job = Job::new(
        "job-000001".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        10,
    );

    job.stop_on_detection = true;

    // Add not_detected round
    let evasion_round = RoundSummary {
        round_id: "round-1".to_string(),
        round_number: 1,
        mutations: vec![],
        detected: false,
        behavior_match: true,
        evasion_score: 1.0,
        completed_at: SystemTime::now(),
    };
    job.complete_round(evasion_round);

    // Should continue after evasion round
    assert!(job.should_continue());

    // Add detected round
    let detected_round = RoundSummary {
        round_id: "round-2".to_string(),
        round_number: 2,
        mutations: vec![],
        detected: true,
        behavior_match: true,
        evasion_score: 0.0,
        completed_at: SystemTime::now(),
    };
    job.complete_round(detected_round);

    // Should NOT continue after detection (stop_on_detection = true)
    assert!(!job.should_continue());
}

#[test]
fn test_start_round() {
    let mut job = Job::new(
        "job-000001".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        10,
    );

    assert_eq!(job.current_round, 0);
    assert_eq!(job.status, JobStatus::Queued);

    // First round should transition to Running
    job.start_round();
    assert_eq!(job.current_round, 1);
    assert_eq!(job.status, JobStatus::Running);

    // Subsequent rounds should stay Running
    job.start_round();
    assert_eq!(job.current_round, 2);
    assert_eq!(job.status, JobStatus::Running);
}

#[test]
fn test_complete_round() {
    let mut job = Job::new(
        "job-000001".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        10,
    );

    assert_eq!(job.rounds.len(), 0);

    let round1 = RoundSummary {
        round_id: "round-1".to_string(),
        round_number: 1,
        mutations: vec!["ast.import_reshape".to_string()],
        detected: true,
        behavior_match: true,
        evasion_score: 0.2,
        completed_at: SystemTime::now(),
    };

    job.complete_round(round1);
    assert_eq!(job.rounds.len(), 1);
    assert_eq!(job.rounds[0].round_number, 1);
    assert!(job.rounds[0].detected);
    assert_eq!(job.rounds[0].evasion_score, 0.2);
}

#[test]
fn test_progress_percent() {
    let mut job = Job::new(
        "job-000001".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        10,
    );

    assert_eq!(job.progress_percent(), 0);

    job.current_round = 5;
    assert_eq!(job.progress_percent(), 50);

    job.current_round = 10;
    assert_eq!(job.progress_percent(), 100);
}

#[test]
fn test_progress_percent_zero_max() {
    let job = Job::new(
        "job-000001".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        0, // max_rounds = 0
    );

    assert_eq!(job.progress_percent(), 0);
}

#[test]
fn test_mark_stopped() {
    let mut job = Job::new(
        "job-000001".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        10,
    );

    job.start_running();
    job.mark_stopped();

    assert_eq!(job.status, JobStatus::Stopped);
    assert!(job.completed_at.is_some());
    assert!(job.is_terminal());
}

#[test]
fn test_elapsed_seconds() {
    let job = Job::new(
        "job-000001".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        10,
    );

    // Just created, should be close to 0
    let elapsed = job.elapsed_seconds();
    assert!(elapsed < 2);
}

#[test]
fn test_job_status_display() {
    assert_eq!(JobStatus::Queued.to_string(), "queued");
    assert_eq!(JobStatus::Running.to_string(), "running");
    assert_eq!(JobStatus::Completed.to_string(), "completed");
    assert_eq!(JobStatus::Failed.to_string(), "failed");
    assert_eq!(JobStatus::Stopped.to_string(), "stopped");
}

#[test]
fn test_mutation_spec_creation() {
    let mutation = MutationSpec {
        id: "ast.import_reshape".to_string(),
        params: Some(serde_json::json!({"delay_load": true})),
    };

    assert_eq!(mutation.id, "ast.import_reshape");
    assert!(mutation.params.is_some());
}
