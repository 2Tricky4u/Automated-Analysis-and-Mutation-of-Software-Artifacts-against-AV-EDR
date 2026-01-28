use super::*;

#[tokio::test]
async fn test_worker_registration() {
    let pool = WorkerPool::new(30);

    pool.register_worker(
        "worker-01".to_string(),
        "10.200.200.11:50052".to_string(),
        true,
    )
        .await
        .unwrap();

    assert_eq!(pool.total_count().await, 1);

    let worker = pool.get_worker("worker-01").await.unwrap();
    assert_eq!(worker.status, WorkerStatus::Available);
}