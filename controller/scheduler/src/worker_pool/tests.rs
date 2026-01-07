use super::*;

#[test]
fn test_worker_registration() {
    let pool = WorkerPool::new(30);

    pool.register_worker(
        "worker-01".to_string(),
        "10.200.200.11:50052".to_string(),
        true,
    )
        .unwrap();

    assert_eq!(pool.total_count(), 1);

    let worker = pool.get_worker("worker-01").unwrap();
    assert_eq!(worker.status, WorkerStatus::Available);
}

#[test]
fn test_worker_assignment() {
    let pool = WorkerPool::new(30);

    pool.register_worker(
        "worker-01".to_string(),
        "10.200.200.11:50052".to_string(),
        true,
    )
        .unwrap();

    // Assign job
    let address = pool.assign_worker("worker-01", "job-000001").unwrap();
    assert_eq!(address, "10.200.200.11:50052");

    let worker = pool.get_worker("worker-01").unwrap();
    assert_eq!(worker.status, WorkerStatus::Busy);
    assert_eq!(worker.current_job, Some("job-000001".to_string()));

    // Cannot assign another job to busy worker
    assert!(pool.assign_worker("worker-01", "job-000002").is_err());

    // Release worker
    pool.release_worker("worker-01").unwrap();

    let worker = pool.get_worker("worker-01").unwrap();
    assert_eq!(worker.status, WorkerStatus::Available);
    assert_eq!(worker.current_job, None);
}

#[test]
fn test_available_workers() {
    let pool = WorkerPool::new(30);

    pool.register_worker(
        "worker-01".to_string(),
        "10.200.200.11:50052".to_string(),
        true,
    )
        .unwrap();
    pool.register_worker(
        "worker-02".to_string(),
        "10.200.200.12:50052".to_string(),
        true,
    )
        .unwrap();
    pool.register_worker(
        "worker-03".to_string(),
        "10.200.200.13:50052".to_string(),
        false, // disabled
    )
        .unwrap();

    // Mark workers as connected (Phase 4: required for availability)
    pool.mark_connected("worker-01").unwrap();
    pool.mark_connected("worker-02").unwrap();
    // worker-03 is disabled, so doesn't matter if connected

    let available = pool.get_available_workers();
    assert_eq!(available.len(), 2);
    assert!(available.contains(&"worker-01".to_string()));
    assert!(available.contains(&"worker-02".to_string()));

    // Assign one worker
    pool.assign_worker("worker-01", "job-000001").unwrap();

    let available = pool.get_available_workers();
    assert_eq!(available.len(), 1);
    assert_eq!(available[0], "worker-02");
}

#[test]
fn test_health_check() {
    let pool = WorkerPool::new(2);

    pool.register_worker(
        "worker-01".to_string(),
        "10.200.200.11:50052".to_string(),
        true,
    )
        .unwrap();

    // Initially available
    assert_eq!(pool.count_by_status(WorkerStatus::Available), 1);

    // Simulate no health check (worker should still be available within timeout)
    pool.check_worker_health();
    assert_eq!(pool.count_by_status(WorkerStatus::Available), 1);

    // Note: In real scenario, would wait 3 seconds and then check_worker_health
    // would mark worker as offline. Can't easily test without sleep in unit test.
}