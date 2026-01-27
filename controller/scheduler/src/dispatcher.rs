//! Signal-based run dispatcher
//!
//! Matches pending runs with available workers using event-driven signals.
//! Spawns execution tasks and handles completion feedback.

use crate::dispatch_coordinator::DispatchCoordinator;
use crate::executor::Executor;
use crate::run_queue::{PendingRun, RunResult};
use crate::worker_manager::WorkerManager;
use crate::worker_pool::WorkerState;
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

/// Run the dispatcher loop (signal-based, no polling)
///
/// # Arguments
/// * `coordinator` - Shared coordinator for signals and queues
/// * `worker_manager` - Worker communication (artifact transfer, execution)
/// * `executor` - Build/deploy/execute implementation
///
/// # Behavior
/// - Waits for either `run_submitted` or `worker_available` signal
/// - On wake: tries to match all pending runs with available workers
/// - Spawns execution tasks for matches
/// - Tasks release workers and signal on completion
pub async fn run_dispatcher(
    coordinator: Arc<DispatchCoordinator>,
    worker_manager: Arc<WorkerManager>,
    executor: Arc<dyn Executor>,
) {
    info!("Dispatcher started (signal-based)");

    loop {
        coordinator.wait_for_signal().await;

        dispatch_all_matches(&coordinator, &worker_manager, &executor).await;
    }
}

/// Try to match all pending runs with available workers
///
/// # Algorithm
/// 1. Get snapshot of available workers
/// 2. For each worker, try to find a matching run (by OS + capabilities)
/// 3. If match found:
///    a. Reserve worker (mark busy)
///    b. Spawn execution task
///    c. Task will release worker and signal on completion
async fn dispatch_all_matches(
    coordinator: &Arc<DispatchCoordinator>,
    worker_manager: &Arc<WorkerManager>,
    executor: &Arc<dyn Executor>,
) {
    let pool = coordinator.worker_pool();
    let queue = coordinator.run_queue();

    let available_workers = pool.get_all_available().await;

    if available_workers.is_empty() {
        trace!("No available workers for dispatch");
        return;
    }

    trace!(
        "Dispatch attempt: {} workers available",
        available_workers.len()
    );

    for worker in available_workers {
        let pending_run = match queue.pop_for_worker(&worker.os_version, &worker.capabilities) {
            Some(run) => run,
            None => {
                trace!(
                    "No matching run for worker {} (OS: {})",
                    worker.id,
                    worker.os_version
                );
                continue;
            }
        };

        info!(
            "[DISPATCH] Run {} (job: {}) → Worker {} (OS: {}, caps: {:?})",
            pending_run.run_id,
            pending_run.job_id,
            worker.id,
            worker.os_version,
            worker.capabilities
        );

        if let Err(e) = pool.reserve_worker(&worker.id).await {
            warn!(
                "Failed to reserve worker {} for run {}: {}",
                worker.id, pending_run.run_id, e
            );
            queue.requeue(pending_run);
            continue;
        }

        let run_id = pending_run.run_id.clone();
        let worker_id = worker.id.clone();

        let coord: Arc<DispatchCoordinator> = Arc::clone(coordinator);
        let wm: Arc<WorkerManager> = Arc::clone(worker_manager);
        let exec: Arc<dyn Executor> = Arc::clone(executor);
        let worker_clone = worker.clone();

        tokio::spawn(async move {
            let result = execute_run(pending_run, &worker_clone, &wm, exec.as_ref()).await;

            match &result {
                Ok(r) => {
                    info!(
                        "[DISPATCH] Run {} complete: success={}, detected={}",
                        run_id, r.success, r.detected
                    );
                }
                Err(e) => {
                    error!("[DISPATCH] Run {} failed: {}", run_id, e);
                }
            }

            let run_result = match result {
                Ok(r) => r,
                Err(e) => RunResult {
                    run_id: run_id.clone(),
                    success: false,
                    detected: false,
                    exit_code: None,
                    error: Some(e.to_string()),
                },
            };

            coord.run_queue().complete_run(&run_id, run_result).ok();

            if let Err(e) = coord.worker_pool().release_worker(&worker_id).await {
                error!("Failed to release worker {}: {}", worker_id, e);
            }

            coord.signal_worker_available();
        });
    }
}

/// Execute a single run on a worker
///
/// # Steps
/// 1. Build artifact (via Executor)
/// 2. Deploy artifact to worker (via Executor)
/// 3. Execute on worker (via Executor)
/// 4. Return result
async fn execute_run(
    run: PendingRun,
    worker: &WorkerState,
    worker_manager: &WorkerManager,
    executor: &dyn Executor,
) -> anyhow::Result<RunResult> {
    let run_id = run.run_id.clone();

    debug!("[{}] Starting execution on worker {}", run_id, worker.id);

    debug!("[{}] Building artifact...", run_id);
    let artifact_id = executor.build_artifact(&run).await.map_err(|e| {
        error!("[{}] Build failed: {}", run_id, e);
        e
    })?;

    debug!("[{}] Build complete: artifact_id={}", run_id, artifact_id);

    debug!("[{}] Deploying to worker {}...", run_id, worker.id);
    executor
        .deploy_artifact(&artifact_id, worker, worker_manager)
        .await
        .map_err(|e| {
            error!("[{}] Deploy failed: {}", run_id, e);
            e
        })?;

    debug!("[{}] Deploy complete", run_id);

    debug!("[{}] Executing on worker {}...", run_id, worker.id);
    let result = executor
        .execute_on_worker(&run, &artifact_id, worker, worker_manager)
        .await
        .map_err(|e| {
            error!("[{}] Execution failed: {}", run_id, e);
            e
        })?;

    debug!(
        "[{}] Execution complete: success={}, detected={}, exit_code={:?}",
        run_id, result.success, result.detected, result.exit_code
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::RunType;
    use crate::run_queue::RunQueue;
    use crate::worker_pool::WorkerPool;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn test_dispatcher_wakes_on_signal() {
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        let coord_clone = Arc::clone(&coordinator);
        let wake_test = tokio::spawn(async move {
            coord_clone.wait_for_signal().await;
            true
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        coordinator.signal_run_submitted();

        let result = timeout(Duration::from_millis(100), wake_test).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dispatcher_wakes_on_worker_available() {
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        let coord_clone = Arc::clone(&coordinator);
        let wake_test = tokio::spawn(async move {
            coord_clone.wait_for_signal().await;
            true
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        coordinator.signal_worker_available();

        let result = timeout(Duration::from_millis(100), wake_test).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_submission_with_available_worker() {
        use std::collections::HashMap;

        // Setup coordinator with worker pool
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);

        // Register a worker with metadata
        worker_pool.register_worker_with_metadata(
            "worker-1".to_string(),
            "192.168.1.1:50051".to_string(),
            true,
            "win10".to_string(),
            vec![],
            HashMap::new(),
            HashMap::new(),
        ).await.unwrap();

        // Mark worker as connected (simulates gRPC stream established)
        worker_pool.mark_connected("worker-1").await.unwrap();

        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        // Submit a run
        let (_run_id, _rx) = coordinator.run_queue().submit_run(
            "job-001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test".to_string(),
            "test.c".to_string(),
            None,
            vec![],
            "off".to_string(),
            "win10".to_string(),
            vec![],
        );

        // Verify run is pending
        assert_eq!(coordinator.run_queue().pending_count(), 1);

        // Verify worker is available
        let available = coordinator.worker_pool().get_all_available().await;
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].id, "worker-1");
    }

    #[tokio::test]
    async fn test_capability_based_matching() {
        use std::collections::HashMap;

        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);

        // Register worker with MDE capability
        worker_pool.register_worker_with_metadata(
            "worker-mde".to_string(),
            "192.168.1.1:50051".to_string(),
            true,
            "win10".to_string(),
            vec!["mde".to_string()],
            HashMap::new(),
            HashMap::new(),
        ).await.unwrap();

        // Register worker without capabilities
        worker_pool.register_worker_with_metadata(
            "worker-plain".to_string(),
            "192.168.1.2:50051".to_string(),
            true,
            "win10".to_string(),
            vec![],
            HashMap::new(),
            HashMap::new(),
        ).await.unwrap();

        // Mark workers as connected
        worker_pool.mark_connected("worker-mde").await.unwrap();
        worker_pool.mark_connected("worker-plain").await.unwrap();

        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        // Submit run requiring MDE
        let (run_id_mde, _) = coordinator.run_queue().submit_run(
            "job-mde".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test".to_string(),
            "test.c".to_string(),
            None,
            vec![],
            "off".to_string(),
            "win10".to_string(),
            vec!["mde".to_string()],
        );

        // Submit run without requirements
        let (run_id_any, _) = coordinator.run_queue().submit_run(
            "job-any".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test".to_string(),
            "test.c".to_string(),
            None,
            vec![],
            "off".to_string(),
            "win10".to_string(),
            vec![],
        );

        // Plain worker should only match non-MDE run
        let plain_run = coordinator.run_queue().pop_for_worker("win10", &[]);
        assert!(plain_run.is_some());
        assert_eq!(plain_run.unwrap().run_id, run_id_any);

        // MDE worker should match MDE run
        let mde_run = coordinator.run_queue().pop_for_worker("win10", &["mde".to_string()]);
        assert!(mde_run.is_some());
        assert_eq!(mde_run.unwrap().run_id, run_id_mde);

        // Queue should be empty
        assert_eq!(coordinator.run_queue().pending_count(), 0);
    }

    #[tokio::test]
    async fn test_multiple_signals_coalesce() {
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        // Send multiple signals rapidly
        coordinator.signal_run_submitted();
        coordinator.signal_run_submitted();
        coordinator.signal_worker_available();

        // Single wait should consume at least one signal
        let coord_clone = Arc::clone(&coordinator);
        let result = timeout(Duration::from_millis(50), async move {
            coord_clone.wait_for_signal().await;
            true
        }).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_os_based_matching() {
        use std::collections::HashMap;

        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);

        // Register win10 worker
        worker_pool.register_worker_with_metadata(
            "worker-win10".to_string(),
            "192.168.1.1:50051".to_string(),
            true,
            "win10".to_string(),
            vec![],
            HashMap::new(),
            HashMap::new(),
        ).await.unwrap();

        // Register win11 worker
        worker_pool.register_worker_with_metadata(
            "worker-win11".to_string(),
            "192.168.1.2:50051".to_string(),
            true,
            "win11".to_string(),
            vec![],
            HashMap::new(),
            HashMap::new(),
        ).await.unwrap();

        // Mark workers as connected
        worker_pool.mark_connected("worker-win10").await.unwrap();
        worker_pool.mark_connected("worker-win11").await.unwrap();

        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        // Submit win11 run
        let (run_id, _) = coordinator.run_queue().submit_run(
            "job-001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test".to_string(),
            "test.c".to_string(),
            None,
            vec![],
            "off".to_string(),
            "win11".to_string(),
            vec![],
        );

        // win10 worker should NOT match
        let win10_run = coordinator.run_queue().pop_for_worker("win10", &[]);
        assert!(win10_run.is_none());

        // win11 worker SHOULD match
        let win11_run = coordinator.run_queue().pop_for_worker("win11", &[]);
        assert!(win11_run.is_some());
        assert_eq!(win11_run.unwrap().run_id, run_id);
    }

    #[tokio::test]
    async fn test_worker_reserve_release_cycle() {
        use std::collections::HashMap;

        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);

        // Register worker
        worker_pool.register_worker_with_metadata(
            "worker-1".to_string(),
            "192.168.1.1:50051".to_string(),
            true,
            "win10".to_string(),
            vec![],
            HashMap::new(),
            HashMap::new(),
        ).await.unwrap();

        // Mark worker as connected
        worker_pool.mark_connected("worker-1").await.unwrap();

        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        // Worker should be available
        let available = coordinator.worker_pool().get_all_available().await;
        assert_eq!(available.len(), 1);

        // Reserve worker
        coordinator.worker_pool().reserve_worker("worker-1").await.unwrap();

        // Worker should NOT be available
        let available = coordinator.worker_pool().get_all_available().await;
        assert_eq!(available.len(), 0);

        // Release worker
        coordinator.worker_pool().release_worker("worker-1").await.unwrap();

        // Worker should be available again
        let available = coordinator.worker_pool().get_all_available().await;
        assert_eq!(available.len(), 1);
    }
}
