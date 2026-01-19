use super::*;

#[tokio::test]
async fn test_add_remove_worker() {
    let (events_tx, _events_rx) = mpsc::channel(10);
    let manager = WorkerManager::new(30, events_tx);

    let config = WorkerConfig {
        id: "win10-worker-01".to_string(),
        address: "10.200.200.11:50052".to_string(),
        enabled: true,
    };

    manager.add_worker(config).unwrap();

    let workers = manager.list_workers();
    assert_eq!(workers.len(), 1);
    assert!(workers.contains(&"win10-worker-01".to_string()));

    manager.remove_worker("win10-worker-01").await.unwrap();

    let workers = manager.list_workers();
    assert_eq!(workers.len(), 0);
}

#[tokio::test]
async fn test_worker_stats() {
    let (events_tx, _events_rx) = mpsc::channel(10);
    let manager = WorkerManager::new(30, events_tx);

    let config = WorkerConfig {
        id: "win10-worker-01".to_string(),
        address: "10.200.200.11:50052".to_string(),
        enabled: true,
    };

    manager.add_worker(config).unwrap();

    let stats = manager.get_worker_stats("win10-worker-01");
    assert!(stats.is_some());

    let (connected, retry_count) = stats.unwrap();
    assert_eq!(connected, false); // Not connected yet
    assert_eq!(retry_count, 0);
}

// Test event emission
#[tokio::test]
async fn test_event_emission() {
    let (events_tx, mut events_rx) = mpsc::channel(10);
    let _manager = WorkerManager::new(30, events_tx.clone());

    // Simulate Connected event
    let _ = events_tx.send(WorkerEvent::connected(
        "test-worker",
        "windows10",
        vec!["rededr".to_string(), "defender".to_string()],
    )).await;

    // Receive event
    let event = tokio::time::timeout(
        Duration::from_secs(1),
        events_rx.recv()
    ).await.expect("Timeout waiting for event").expect("Channel closed");

    match event {
        WorkerEvent::Connected { worker_id, os_version, capabilities } => {
            assert_eq!(worker_id, "test-worker");
            assert_eq!(os_version, "windows10");
            assert_eq!(capabilities.len(), 2);
        }
        _ => panic!("Expected Connected event"),
    }
}

// Test Disconnected event
#[tokio::test]
async fn test_disconnected_event() {
    let (events_tx, mut events_rx) = mpsc::channel(10);
    let _manager = WorkerManager::new(30, events_tx.clone());

    // Simulate Disconnected event
    let _ = events_tx.send(WorkerEvent::disconnected(
        "test-worker",
        "Stream closed"
    )).await;

    // Receive event
    let event = tokio::time::timeout(
        Duration::from_secs(1),
        events_rx.recv()
    ).await.expect("Timeout").expect("Channel closed");

    match event {
        WorkerEvent::Disconnected { worker_id, reason } => {
            assert_eq!(worker_id, "test-worker");
            assert_eq!(reason, "Stream closed");
        }
        _ => panic!("Expected Disconnected event"),
    }
}

// Test session staleness
#[tokio::test]
async fn test_session_staleness() {
    let (tx, _rx) = mpsc::channel(10);
    let mut session = SessionHandle::new("test-worker".to_string(), tx);

    // Fresh session should not be stale
    assert!(!session.is_stale(5));

    // Simulate old session
    session.last_seen = std::time::SystemTime::now() - Duration::from_secs(10);
    assert!(session.is_stale(5));  // 10 seconds > 5 second timeout

    // Touch should refresh
    session.touch();
    assert!(!session.is_stale(5));
}

// Test broadcast (simulated)
#[tokio::test]
async fn test_broadcast_logic() {
    let (events_tx, _events_rx) = mpsc::channel(10);
    let manager = WorkerManager::new(30, events_tx);

    // Create mock sessions
    let (tx1, _rx1) = mpsc::channel(10);
    let (tx2, _rx2) = mpsc::channel(10);

    manager.sessions.insert("worker-01".to_string(), SessionHandle::new(
        "worker-01".to_string(),
        tx1,
    ));
    manager.sessions.insert("worker-02".to_string(), SessionHandle::new(
        "worker-02".to_string(),
        tx2,
    ));

    // Verify both sessions exist
    assert_eq!(manager.sessions.len(), 2);
    assert!(manager.sessions.contains_key("worker-01"));
    assert!(manager.sessions.contains_key("worker-02"));
}

// Test list_workers with both sessions and legacy
#[tokio::test]
async fn test_list_workers_hybrid() {
    let (events_tx, _events_rx) = mpsc::channel(10);
    let manager = WorkerManager::new(30, events_tx);

    // Add legacy connection
    let config = WorkerConfig {
        id: "legacy-worker".to_string(),
        address: "10.0.0.1:50052".to_string(),
        enabled: true,
    };
    manager.add_worker(config).unwrap();

    // Add session
    let (tx, _rx) = mpsc::channel(10);
    manager.sessions.insert("streaming-worker".to_string(), SessionHandle::new(
        "streaming-worker".to_string(),
        tx,
    ));

    // List should include both
    let workers = manager.list_workers();
    assert_eq!(workers.len(), 2);
    assert!(workers.contains(&"legacy-worker".to_string()));
    assert!(workers.contains(&"streaming-worker".to_string()));
}

// Test is_worker_connected with sessions
#[tokio::test]
async fn test_is_worker_connected_with_session() {
    let (events_tx, _events_rx) = mpsc::channel(10);
    let manager = WorkerManager::new(30, events_tx);

    // Worker not added yet
    assert!(!manager.is_worker_connected("test-worker"));

    // Add session
    let (tx, _rx) = mpsc::channel(10);
    manager.sessions.insert("test-worker".to_string(), SessionHandle::new(
        "test-worker".to_string(),
        tx,
    ));

    // Should be connected (fresh session)
    assert!(manager.is_worker_connected("test-worker"));

    // Make session stale
    if let Some(mut session) = manager.sessions.get_mut("test-worker") {
        session.last_seen = std::time::SystemTime::now() - Duration::from_secs(400);
    }

    // Should be disconnected (stale session > 5 min)
    assert!(!manager.is_worker_connected("test-worker"));
}