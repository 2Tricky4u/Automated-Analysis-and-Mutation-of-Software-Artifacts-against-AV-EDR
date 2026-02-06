//! RunPool - shared run queue for all jobs.
//!
//! Architecture:
//! - JobWorkers put runs into the pool via add_runs()
//! - VMExecutors take runs via take_run() (capability-filtered)
//! - Results are routed back to originating JobWorker via route_result()
//!
//! This is the central coordination point between jobs and VMs.
//! Multiple jobs can run in parallel, all sharing the same pool of VMs.

use std::collections::{HashMap, VecDeque};
use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::channels::JobRunResult;
use super::types::{JobId, RunEnvelope};

// ============================================================================
// Configuration
// ============================================================================

/// Maximum runs to buffer in pool before backpressure
const MAX_POOL_SIZE: usize = 100;

// ============================================================================
// RunPool
// ============================================================================

/// Shared run pool for all jobs.
///
/// JobWorkers produce runs into this pool, VMExecutors consume them.
/// Results are routed back to the originating JobWorker.
pub struct RunPool {
    /// All pending runs (from all jobs)
    runs: Mutex<VecDeque<RunEnvelope>>,

    /// Signal when runs are available
    runs_available: Notify,

    /// Route results back to JobWorkers
    /// JobId -> Sender for that job's results
    result_routers: RwLock<HashMap<JobId, mpsc::Sender<JobRunResult>>>,

    /// Global shutdown token
    shutdown_token: CancellationToken,

    /// Pool metrics
    metrics: Mutex<RunPoolMetrics>,
}

/// Metrics for the run pool
#[derive(Debug, Default, Clone)]
pub struct RunPoolMetrics {
    pub total_runs_added: u64,
    pub total_runs_taken: u64,
    pub total_results_routed: u64,
    pub active_jobs: usize,
}

impl RunPool {
    /// Create a new run pool.
    pub fn new() -> Self {
        Self {
            runs: Mutex::new(VecDeque::new()),
            runs_available: Notify::new(),
            result_routers: RwLock::new(HashMap::new()),
            shutdown_token: CancellationToken::new(),
            metrics: Mutex::new(RunPoolMetrics::default()),
        }
    }

    // ========================================================================
    // Job Registration (JobWorker calls these)
    // ========================================================================

    /// Register a job's result channel.
    /// Called by JobWorker on start to receive results for its runs.
    pub async fn register_job(&self, job_id: JobId, result_tx: mpsc::Sender<JobRunResult>) {
        let mut routers = self.result_routers.write().await;
        routers.insert(job_id.clone(), result_tx);
        self.metrics.lock().await.active_jobs = routers.len();
        debug!("[RunPool] Job {} registered", job_id);
    }

    /// Unregister a job.
    /// Called by JobWorker on completion.
    pub async fn unregister_job(&self, job_id: &JobId) {
        let mut routers = self.result_routers.write().await;
        routers.remove(job_id);
        self.metrics.lock().await.active_jobs = routers.len();
        debug!("[RunPool] Job {} unregistered", job_id);
    }

    // ========================================================================
    // Run Production (JobWorker calls these)
    // ========================================================================

    /// Add runs to the pool.
    /// Called by JobWorker when producing a round.
    pub async fn add_runs(&self, runs: Vec<RunEnvelope>) {
        if runs.is_empty() {
            return;
        }

        let count = runs.len();
        {
            let mut pool = self.runs.lock().await;

            // Backpressure: don't exceed max pool size
            if pool.len() + count > MAX_POOL_SIZE {
                warn!(
                    "[RunPool] Pool near capacity ({}/{}), adding {} runs anyway",
                    pool.len(),
                    MAX_POOL_SIZE,
                    count
                );
            }

            pool.extend(runs);
            self.metrics.lock().await.total_runs_added += count as u64;
        }

        debug!("[RunPool] Added {} runs to pool", count);

        // Wake ALL waiting VMExecutors - they compete for runs
        self.runs_available.notify_waiters();
    }

    // ========================================================================
    // Run Consumption (VMExecutor calls these)
    // ========================================================================

    /// Try to take a run from the pool (non-blocking).
    /// Returns None if pool is empty or no compatible run found.
    ///
    /// VMExecutors call this after being signaled or after completing a run.
    pub async fn take_run(&self, capabilities: &[String]) -> Option<RunEnvelope> {
        let mut pool = self.runs.lock().await;

        // For now, take any run (capability filtering can be added later)
        // In future: find first run where run.required_caps is subset of capabilities
        let run = pool.pop_front();

        if let Some(ref r) = run {
            debug!("[RunPool] Run {} taken (job={})", r.run_id, r.job_id);
            drop(pool); // Release lock before async metrics update
            self.metrics.lock().await.total_runs_taken += 1;
        }

        run
    }

    /// Take a run with capability filtering.
    /// Returns the first run that matches the given capabilities.
    #[allow(dead_code)]
    pub async fn take_run_filtered(&self, capabilities: &[String]) -> Option<RunEnvelope> {
        let mut pool = self.runs.lock().await;

        // Find first compatible run
        // For now, all runs are compatible (capability filtering on job submission)
        // In future: check run.required_capabilities against VM capabilities
        let _ = capabilities; // Suppress warning for now

        let run = pool.pop_front();

        if let Some(ref r) = run {
            debug!("[RunPool] Run {} taken (job={}, filtered)", r.run_id, r.job_id);
            drop(pool);
            self.metrics.lock().await.total_runs_taken += 1;
        }

        run
    }

    /// Wait for runs to become available.
    /// Use in tokio::select! to efficiently wait for work.
    pub async fn wait_for_runs(&self) {
        self.runs_available.notified().await
    }

    // ========================================================================
    // Result Routing (VMExecutor calls this)
    // ========================================================================

    /// Route a result back to the originating JobWorker.
    /// Called by VMExecutor when a run completes.
    pub async fn route_result(&self, result: JobRunResult) {
        let routers = self.result_routers.read().await;

        if let Some(tx) = routers.get(&result.job_id) {
            if let Err(e) = tx.send(result.clone()).await {
                warn!(
                    "[RunPool] Failed to route result for run {}: channel closed ({})",
                    result.run_id, e
                );
            } else {
                debug!(
                    "[RunPool] Routed result for run {} to job {}",
                    result.run_id, result.job_id
                );
                drop(routers);
                self.metrics.lock().await.total_results_routed += 1;
            }
        } else {
            warn!(
                "[RunPool] No router for job {} (run {}), job may have completed",
                result.job_id, result.run_id
            );
        }
    }

    // ========================================================================
    // Lifecycle
    // ========================================================================

    /// Get cancellation token for VMExecutors to monitor shutdown.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Check if shutdown has been requested.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown_token.is_cancelled()
    }

    /// Signal graceful shutdown to all VMExecutors.
    pub fn shutdown(&self) {
        warn!("[RunPool] Shutdown requested");
        self.shutdown_token.cancel();
        // Wake any waiting VMExecutors so they can check cancellation
        self.runs_available.notify_waiters();
    }

    // ========================================================================
    // Monitoring
    // ========================================================================

    /// Get current pool size.
    pub async fn pool_size(&self) -> usize {
        self.runs.lock().await.len()
    }

    /// Get current metrics.
    pub async fn get_metrics(&self) -> RunPoolMetrics {
        self.metrics.lock().await.clone()
    }

    /// Get count of registered jobs.
    pub async fn job_count(&self) -> usize {
        self.result_routers.read().await.len()
    }
}

impl Default for RunPool {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RunPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunPool")
            .field("shutdown", &self.shutdown_token.is_cancelled())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{ArtifactRef, RoundId, RunId, RunType};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_run(job_id: &str, run_id: &str) -> RunEnvelope {
        RunEnvelope {
            run_id: RunId(run_id.to_string()),
            job_id: JobId(job_id.to_string()),
            round_id: RoundId("round-1".to_string()),
            round_number: 1,
            run_type: RunType::Baseline,
            artifact: ArtifactRef {
                path: PathBuf::from("test.exe"),
                sha256: Some("abc123".to_string()),
            },
            mutations: vec![],
            timeout_seconds: 120,
        }
    }

    #[tokio::test]
    async fn test_add_and_take_runs() {
        let pool = RunPool::new();

        // Add runs
        pool.add_runs(vec![test_run("job-1", "run-1"), test_run("job-1", "run-2")])
            .await;

        assert_eq!(pool.pool_size().await, 2);

        // Take runs
        let run1 = pool.take_run(&[]).await;
        assert!(run1.is_some());
        assert_eq!(run1.unwrap().run_id.0, "run-1");

        let run2 = pool.take_run(&[]).await;
        assert!(run2.is_some());
        assert_eq!(run2.unwrap().run_id.0, "run-2");

        // Pool empty
        assert!(pool.take_run(&[]).await.is_none());
        assert_eq!(pool.pool_size().await, 0);
    }

    #[tokio::test]
    async fn test_result_routing() {
        let pool = Arc::new(RunPool::new());
        let (tx, mut rx) = mpsc::channel(10);

        // Register job
        pool.register_job(JobId("job-1".to_string()), tx).await;

        // Route result
        let result = JobRunResult {
            run_id: RunId("run-1".to_string()),
            job_id: JobId("job-1".to_string()),
            round_id: RoundId("round-1".to_string()),
            outcome: crate::dispatch::types::RunOutcome {
                detected: false,
                exit_code: 0,
                error: None,
            },
            vm_id: "vm-1".to_string(),
        };
        pool.route_result(result).await;

        // Should receive result
        let received = rx.recv().await;
        assert!(received.is_some());
        assert_eq!(received.unwrap().run_id.0, "run-1");
    }

    #[tokio::test]
    async fn test_job_registration() {
        let pool = RunPool::new();
        let (tx1, _) = mpsc::channel(10);
        let (tx2, _) = mpsc::channel(10);

        pool.register_job(JobId("job-1".to_string()), tx1).await;
        pool.register_job(JobId("job-2".to_string()), tx2).await;

        assert_eq!(pool.job_count().await, 2);

        pool.unregister_job(&JobId("job-1".to_string())).await;
        assert_eq!(pool.job_count().await, 1);
    }
}
