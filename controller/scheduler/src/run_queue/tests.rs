use super::*;

#[test]
fn test_submit_and_pop() {
    let queue = RunQueue::new();

    // Submit run for win10
    let (run_id, _rx) = queue.submit_run(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "off".to_string(),
        "win10".to_string(),
    );

    assert_eq!(queue.pending_count(), 1);
    assert_eq!(queue.pending_count_for_os("win10"), 1);

    // Pop for win10 should return the run
    let pending = queue.pop_for_os("win10").unwrap();
    assert_eq!(pending.run_id, run_id);
    assert_eq!(pending.required_os, "win10");

    assert_eq!(queue.pending_count(), 0);
}

#[test]
fn test_os_matching() {
    let queue = RunQueue::new();

    // Submit runs for different OSes
    queue.submit_run(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "off".to_string(),
        "win10".to_string(),
    );

    queue.submit_run(
        "job-000002".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "off".to_string(),
        "win11".to_string(),
    );

    assert_eq!(queue.pending_count(), 2);
    assert_eq!(queue.pending_count_for_os("win10"), 1);
    assert_eq!(queue.pending_count_for_os("win11"), 1);

    // Pop for win11 should only return win11 run
    let pending = queue.pop_for_os("win11").unwrap();
    assert_eq!(pending.required_os, "win11");

    assert_eq!(queue.pending_count(), 1);
    assert_eq!(queue.pending_count_for_os("win10"), 1);
    assert_eq!(queue.pending_count_for_os("win11"), 0);
}

#[test]
fn test_fifo_per_os() {
    let queue = RunQueue::new();

    // Submit 3 runs for win10
    let (run_id1, _) = queue.submit_run(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "test1".to_string(),
        "test1.c".to_string(),
        vec![],
        "off".to_string(),
        "win10".to_string(),
    );

    let (run_id2, _) = queue.submit_run(
        "job-000002".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "test2".to_string(),
        "test2.c".to_string(),
        vec![],
        "off".to_string(),
        "win10".to_string(),
    );

    // Pop should return FIFO order
    let first = queue.pop_for_os("win10").unwrap();
    assert_eq!(first.run_id, run_id1);

    let second = queue.pop_for_os("win10").unwrap();
    assert_eq!(second.run_id, run_id2);
}

#[tokio::test]
async fn test_async_result() {
    let queue = RunQueue::new();

    // Submit run
    let (run_id, rx) = queue.submit_run(
        "job-000001".to_string(),
        "round-1".to_string(),
        RunType::Baseline,
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "off".to_string(),
        "win10".to_string(),
    );

    // Pop run (simulating worker)
    let mut pending = queue.pop_for_os("win10").unwrap();

    // Complete run with result (send via the PendingRun's oneshot channel)
    let result = RunResult {
        run_id: run_id.clone(),
        success: true,
        detected: false,
        exit_code: Some(0),
        error: None,
    };

    // Send result through the PendingRun's oneshot sender
    if let Some(tx) = pending.result_tx.take() {
        tx.send(result).unwrap();
    }

    // Receiver should get the result
    let received = rx.await.unwrap();
    assert_eq!(received.run_id, run_id);
    assert_eq!(received.success, true);
    assert_eq!(received.detected, false);
}