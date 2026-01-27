//! Signal-based dispatch coordination
//!
//! Coordinates run submission and worker availability using async signals
//! instead of polling loops.

use crate::run_queue::RunQueue;
use crate::worker_pool::WorkerPool;
use tokio::sync::Notify;
use tracing::trace;

/// Coordinates dispatch between RunQueue and WorkerPool using signals
pub struct DispatchCoordinator {
    /// Signal: new run submitted to queue
    run_submitted: Notify,

    /// Signal: worker became available
    worker_available: Notify,

    /// Signal: new job submitted to job queue
    job_submitted: Notify,

    /// Pending runs waiting for workers (owned)
    run_queue: RunQueue,

    /// Available workers (shared reference via clone)
    worker_pool: WorkerPool,
}

impl DispatchCoordinator {
    /// Create new coordinator with queue and pool
    pub fn new(run_queue: RunQueue, worker_pool: WorkerPool) -> Self {
        Self {
            run_submitted: Notify::new(),
            worker_available: Notify::new(),
            job_submitted: Notify::new(),
            run_queue,
            worker_pool,
        }
    }

    /// Signal that a new run was submitted to the queue
    pub fn signal_run_submitted(&self) {
        trace!("Signal: run_submitted");
        self.run_submitted.notify_one();
    }

    /// Signal that a worker became available
    pub fn signal_worker_available(&self) {
        trace!("Signal: worker_available");
        self.worker_available.notify_one();
    }

    /// Signal that a new job was submitted to the job queue
    pub fn signal_job_submitted(&self) {
        trace!("Signal: job_submitted");
        self.job_submitted.notify_one();
    }

    /// Wait for job submitted signal (for scheduler core)
    pub async fn wait_for_job(&self) {
        self.job_submitted.notified().await;
        trace!("Scheduler woken: job submitted");
    }

    /// Access run queue for submissions and pops
    pub fn run_queue(&self) -> &RunQueue {
        &self.run_queue
    }

    /// Access worker pool for availability queries
    pub fn worker_pool(&self) -> &WorkerPool {
        &self.worker_pool
    }

    /// Wait for either signal (run submitted OR worker available)
    pub async fn wait_for_signal(&self) {
        tokio::select! {
            _ = self.run_submitted.notified() => {
                trace!("Dispatcher woken: run submitted");
            }
            _ = self.worker_available.notified() => {
                trace!("Dispatcher woken: worker available");
            }
        }
    }

    /// Check if there are pending runs (non-blocking)
    pub fn has_pending_runs(&self) -> bool {
        self.run_queue.pending_count() > 0
    }

    /// Check if there are available workers (non-blocking, async for pool access)
    pub async fn has_available_workers(&self) -> bool {
        !self.worker_pool.get_available_workers().await.is_empty()
    }

    /// Get counts for monitoring
    pub async fn get_stats(&self) -> CoordinatorStats {
        CoordinatorStats {
            pending_runs: self.run_queue.pending_count(),
            available_workers: self.worker_pool.get_available_workers().await.len(),
            total_workers: self.worker_pool.total_count().await,
        }
    }
}

/// Statistics for monitoring
#[derive(Debug, Clone)]
pub struct CoordinatorStats {
    pub pending_runs: usize,
    pub available_workers: usize,
    pub total_workers: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn test_run_signal_wakes_waiter() {
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        let coord_clone = Arc::clone(&coordinator);
        let waiter = tokio::spawn(async move {
            coord_clone.wait_for_signal().await;
            true
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        coordinator.signal_run_submitted();

        let result = timeout(Duration::from_millis(100), waiter).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_worker_signal_wakes_waiter() {
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        let coord_clone = Arc::clone(&coordinator);
        let waiter = tokio::spawn(async move {
            coord_clone.wait_for_signal().await;
            true
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        coordinator.signal_worker_available();

        let result = timeout(Duration::from_millis(100), waiter).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_job_signal_wakes_waiter() {
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        let coord_clone = Arc::clone(&coordinator);
        let waiter = tokio::spawn(async move {
            coord_clone.wait_for_job().await;
            true
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        coordinator.signal_job_submitted();

        let result = timeout(Duration::from_millis(100), waiter).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_coordinator_stats() {
        use crate::round::RunType;
        use std::collections::HashMap;

        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);

        // Register a worker
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

        let coordinator = DispatchCoordinator::new(run_queue, worker_pool);

        // Initial stats
        let stats = coordinator.get_stats().await;
        assert_eq!(stats.pending_runs, 0);
        assert_eq!(stats.available_workers, 1);
        assert_eq!(stats.total_workers, 1);

        // Submit a run
        coordinator.run_queue().submit_run(
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

        // Stats should reflect pending run
        let stats = coordinator.get_stats().await;
        assert_eq!(stats.pending_runs, 1);
        assert_eq!(stats.available_workers, 1);
    }

    #[tokio::test]
    async fn test_has_pending_runs() {
        use crate::round::RunType;

        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = DispatchCoordinator::new(run_queue, worker_pool);

        // Initially no pending runs
        assert!(!coordinator.has_pending_runs());

        // Submit a run
        coordinator.run_queue().submit_run(
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

        // Now has pending runs
        assert!(coordinator.has_pending_runs());
    }

    #[tokio::test]
    async fn test_has_available_workers() {
        use std::collections::HashMap;

        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = DispatchCoordinator::new(run_queue, worker_pool);

        // Initially no workers
        assert!(!coordinator.has_available_workers().await);

        // Register a worker
        coordinator.worker_pool().register_worker_with_metadata(
            "worker-1".to_string(),
            "192.168.1.1:50051".to_string(),
            true,
            "win10".to_string(),
            vec![],
            HashMap::new(),
            HashMap::new(),
        ).await.unwrap();

        // Mark worker as connected
        coordinator.worker_pool().mark_connected("worker-1").await.unwrap();

        // Now has available workers
        assert!(coordinator.has_available_workers().await);

        // Reserve the worker
        coordinator.worker_pool().reserve_worker("worker-1").await.unwrap();

        // No longer has available workers
        assert!(!coordinator.has_available_workers().await);
    }

    #[tokio::test]
    async fn test_multiple_job_signals() {
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        // Send multiple job signals
        coordinator.signal_job_submitted();
        coordinator.signal_job_submitted();
        coordinator.signal_job_submitted();

        // First wait should succeed immediately
        let coord_clone = Arc::clone(&coordinator);
        let result = timeout(Duration::from_millis(50), async move {
            coord_clone.wait_for_job().await;
            true
        }).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_signal_before_wait() {
        let run_queue = RunQueue::new();
        let worker_pool = WorkerPool::new(30);
        let coordinator = Arc::new(DispatchCoordinator::new(run_queue, worker_pool));

        // Signal BEFORE wait
        coordinator.signal_job_submitted();

        // Wait should still succeed (Notify stores pending notification)
        let coord_clone = Arc::clone(&coordinator);
        let result = timeout(Duration::from_millis(50), async move {
            coord_clone.wait_for_job().await;
            true
        }).await;
        assert!(result.is_ok());
    }
}
