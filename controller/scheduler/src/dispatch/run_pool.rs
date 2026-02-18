//! RunPool - sharded run queue for all jobs.
//!
//! Architecture:
//! - Sharded by OS: each OS has its own queue (reduces contention)
//! - DashMap for lock-free run storage
//! - Per-OS Mutex for queue ordering
//! - JobWorkers put runs into the pool via add_runs()
//! - VMExecutors take runs via take_run(os, caps) - locks only their OS queue
//! - Results are routed back to originating JobWorker via route_result()

use dashmap::DashMap;
use std::collections::{HashMap, VecDeque};
use tokio::sync::{Mutex, Notify, RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::channels::JobRunResult;
use super::types::{JobId, JobInfo, JobOutcome, JobSession, JobStatus, RunEnvelope, RunId};

// ============================================================================
// Configuration
// ============================================================================

/// Maximum runs to buffer per OS queue before warning
const MAX_QUEUE_SIZE: usize = 50;

// ============================================================================
// RunPool
// ============================================================================

/// Sharded run pool for all jobs.
///
/// Internally sharded by OS to reduce lock contention.
/// VMExecutors only lock their own OS queue when taking runs.
pub struct RunPool {
    /// Run storage (lock-free access)
    pending: DashMap<RunId, RunEnvelope>,

    /// Per-OS run queues (only the relevant OS queue is locked)
    by_os: DashMap<String, Mutex<VecDeque<RunId>>>,

    /// Signal when runs are available
    runs_available: Notify,

    /// Route results back to JobWorkers
    result_routers: RwLock<HashMap<JobId, mpsc::Sender<JobRunResult>>>,

    /// Job info registry for API visibility
    job_registry: DashMap<JobId, JobInfo>,

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
            pending: DashMap::new(),
            by_os: DashMap::new(),
            runs_available: Notify::new(),
            result_routers: RwLock::new(HashMap::new()),
            job_registry: DashMap::new(),
            shutdown_token: CancellationToken::new(),
            metrics: Mutex::new(RunPoolMetrics::default()),
        }
    }

    // ========================================================================
    // Job Registration (JobWorker calls these)
    // ========================================================================

    /// Register a job's result channel and track its info.
    pub async fn register_job(&self, job: &JobSession, result_tx: mpsc::Sender<JobRunResult>) {
        let job_id = job.id.clone();

        // Store job info for API visibility
        self.job_registry
            .insert(job_id.clone(), job.to_info(JobStatus::Running));

        // Register result router
        let mut routers = self.result_routers.write().await;
        routers.insert(job_id.clone(), result_tx);
        self.metrics.lock().await.active_jobs = routers.len();
        debug!("[RunPool] Job {} registered", job_id);
    }

    /// Update job progress (called by JobWorker after each round).
    pub fn update_job_progress(&self, job: &JobSession) {
        if let Some(mut info) = self.job_registry.get_mut(&job.id) {
            info.current_round = job.current_round;
        }
    }

    /// Mark job as completed with outcome.
    pub fn complete_job(&self, job_id: &JobId, outcome: &JobOutcome) {
        if let Some(mut info) = self.job_registry.get_mut(job_id) {
            info.status = outcome.to_status();
        }
    }

    /// Unregister a job and remove all its pending runs.
    /// Note: Job info is kept in registry for API visibility.
    pub async fn unregister_job(&self, job_id: &JobId) {
        // Remove from result routers
        let mut routers = self.result_routers.write().await;
        routers.remove(job_id);
        self.metrics.lock().await.active_jobs = routers.len();

        // Remove all pending runs for this job
        let removed = self.remove_runs_for_job(job_id).await;
        if removed > 0 {
            debug!(
                "[RunPool] Job {} unregistered, removed {} pending runs",
                job_id, removed
            );
        } else {
            debug!("[RunPool] Job {} unregistered", job_id);
        }
    }

    // ========================================================================
    // Job Registry Queries (for API)
    // ========================================================================

    // TODO: wire list_jobs/get_job_info into admin gRPC handlers
    // TODO: wire shutdown/is_shutdown into Orchestrator::shutdown_all_jobs()

    /// List all jobs (running and completed).
    #[allow(dead_code)]
    pub fn list_jobs(&self) -> Vec<JobInfo> {
        self.job_registry
            .iter()
            .map(|r| r.value().clone())
            .collect()
    }

    /// List only running jobs.
    pub fn list_running_jobs(&self) -> Vec<JobInfo> {
        self.job_registry
            .iter()
            .filter(|r| r.value().status == JobStatus::Running)
            .map(|r| r.value().clone())
            .collect()
    }

    /// Get info for a specific job.
    #[allow(dead_code)]
    pub fn get_job_info(&self, job_id: &JobId) -> Option<JobInfo> {
        self.job_registry.get(job_id).map(|r| r.value().clone())
    }

    /// Remove all pending runs for a specific job.
    /// Returns the number of runs removed.
    pub async fn remove_runs_for_job(&self, job_id: &JobId) -> usize {
        let mut removed_count = 0;

        // Find all run_ids belonging to this job
        let run_ids_to_remove: Vec<RunId> = self
            .pending
            .iter()
            .filter(|entry| &entry.value().job_id == job_id)
            .map(|entry| entry.key().clone())
            .collect();

        // Remove from pending map
        for run_id in &run_ids_to_remove {
            if self.pending.remove(run_id).is_some() {
                removed_count += 1;
            }
        }

        // Note: We don't need to remove from by_os queues because take_run()
        // already handles missing entries gracefully (continues to next)

        removed_count
    }

    // ========================================================================
    // Run Production (JobWorker calls these)
    // ========================================================================

    /// Add runs to the pool.
    /// Runs are sharded into per-OS queues.
    pub async fn add_runs(&self, runs: Vec<RunEnvelope>) {
        if runs.is_empty() {
            return;
        }

        let count = runs.len();

        for run in runs {
            let os = run.required_os.clone();
            let run_id = run.run_id.clone();

            // Store run in pending map (lock-free)
            self.pending.insert(run_id.clone(), run);

            // Add run_id to the OS queue (short lock)
            let queue = self
                .by_os
                .entry(os.clone())
                .or_insert_with(|| Mutex::new(VecDeque::new()));
            let mut q = queue.lock().await;

            if q.len() >= MAX_QUEUE_SIZE {
                warn!(
                    "[RunPool] OS queue '{}' near capacity ({}/{})",
                    os,
                    q.len(),
                    MAX_QUEUE_SIZE
                );
            }

            q.push_back(run_id);
        }

        self.metrics.lock().await.total_runs_added += count as u64;
        debug!("[RunPool] Added {} runs to pool", count);

        // Wake ALL waiting VMExecutors - they compete for runs
        self.runs_available.notify_waiters();
    }

    // ========================================================================
    // Run Consumption (VMExecutor calls these)
    // ========================================================================

    /// Take a run from the pool that matches VM's OS and capabilities.
    ///
    /// Only locks the queue for the given OS, not the entire pool.
    /// Returns None if no compatible run found.
    pub async fn take_run(&self, vm_os: &str, vm_capabilities: &[String]) -> Option<RunEnvelope> {
        // Get the queue for this OS
        let queue = self.by_os.get(vm_os)?;
        let mut q = queue.lock().await;

        // Bounded scan to avoid infinite rotation
        let n = q.len();
        for _ in 0..n {
            let run_id = match q.pop_front() {
                Some(id) => id,
                None => return None,
            };

            // Check if run exists and matches capabilities
            if let Some(run_ref) = self.pending.get(&run_id) {
                let caps_match = run_ref.required_capabilities.iter().all(|required| {
                    vm_capabilities
                        .iter()
                        .any(|cap| cap.eq_ignore_ascii_case(required))
                });

                if caps_match {
                    // Remove from pending and return
                    drop(run_ref); // Release read lock before remove
                    if let Some((_, run)) = self.pending.remove(&run_id) {
                        debug!(
                            "[RunPool] Run {} taken (job={}, os={}, caps={:?})",
                            run.run_id, run.job_id, vm_os, run.required_capabilities
                        );
                        drop(q); // Release queue lock before metrics update
                        self.metrics.lock().await.total_runs_taken += 1;
                        return Some(run);
                    }
                } else {
                    // Capabilities don't match - push back to queue
                    q.push_back(run_id);
                }
            }
            // Run not in pending (already taken or removed) - continue to next
        }

        None
    }

    /// Wait for runs to become available.
    pub async fn wait_for_runs(&self) {
        self.runs_available.notified().await
    }

    // ========================================================================
    // Result Routing (VMExecutor calls this)
    // ========================================================================

    /// Route a result back to the originating JobWorker.
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
    #[allow(dead_code)]
    pub fn is_shutdown(&self) -> bool {
        self.shutdown_token.is_cancelled()
    }

    /// Signal graceful shutdown to all VMExecutors.
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        warn!("[RunPool] Shutdown requested");
        self.shutdown_token.cancel();
        self.runs_available.notify_waiters();
    }

    // ========================================================================
    // Monitoring
    // ========================================================================

    /// Get total pending runs across all OS queues.
    pub async fn pool_size(&self) -> usize {
        self.pending.len()
    }

    /// Get pending runs per OS.
    #[allow(dead_code)]
    pub async fn pool_size_by_os(&self) -> HashMap<String, usize> {
        let mut sizes = HashMap::new();
        for entry in self.by_os.iter() {
            let os = entry.key().clone();
            let q = entry.value().lock().await;
            sizes.insert(os, q.len());
        }
        sizes
    }

    /// Get current metrics.
    pub async fn get_metrics(&self) -> RunPoolMetrics {
        self.metrics.lock().await.clone()
    }

    /// Get count of registered jobs.
    pub async fn job_count(&self) -> usize {
        self.result_routers.read().await.len()
    }

    /// Get count of pending runs for a specific job.
    pub fn pending_runs_for_job(&self, job_id: &JobId) -> usize {
        self.pending
            .iter()
            .filter(|entry| &entry.value().job_id == job_id)
            .count()
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
            .field("pending_count", &self.pending.len())
            .field("os_queues", &self.by_os.len())
            .field("shutdown", &self.shutdown_token.is_cancelled())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{
        ArtifactRef, ModularBuildSpec, ModuleSelectionSpec, RoundId, RunType,
    };
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_job(job_id: &str) -> JobSession {
        JobSession::new(
            job_id,
            5,
            ModularBuildSpec {
                modules: ModuleSelectionSpec::default(),
                payload_path: PathBuf::from("test.bin"),
                encoding: "xor".to_string(),
            },
        )
    }

    fn test_run(job_id: &str, run_id: &str, os: &str, caps: Vec<String>) -> RunEnvelope {
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
            required_os: os.to_string(),
            required_capabilities: caps,
        }
    }

    #[tokio::test]
    async fn test_add_and_take_runs() {
        let pool = RunPool::new();

        // Add runs for different OS
        pool.add_runs(vec![
            test_run("job-1", "run-1", "windows", vec![]),
            test_run("job-1", "run-2", "windows", vec![]),
            test_run("job-1", "run-3", "linux", vec![]),
        ])
        .await;

        assert_eq!(pool.pool_size().await, 3);

        // Take Windows runs
        let run1 = pool.take_run("windows", &[]).await;
        assert!(run1.is_some());
        assert_eq!(run1.unwrap().run_id.0, "run-1");

        let run2 = pool.take_run("windows", &[]).await;
        assert!(run2.is_some());
        assert_eq!(run2.unwrap().run_id.0, "run-2");

        // No more Windows runs
        assert!(pool.take_run("windows", &[]).await.is_none());

        // Linux run still there
        let run3 = pool.take_run("linux", &[]).await;
        assert!(run3.is_some());
        assert_eq!(run3.unwrap().run_id.0, "run-3");

        assert_eq!(pool.pool_size().await, 0);
    }

    #[tokio::test]
    async fn test_capability_filtering() {
        let pool = RunPool::new();

        pool.add_runs(vec![
            test_run("job-1", "run-1", "windows", vec!["defender".to_string()]),
            test_run("job-1", "run-2", "windows", vec!["rededr".to_string()]),
            test_run(
                "job-1",
                "run-3",
                "windows",
                vec!["defender".to_string(), "rededr".to_string()],
            ),
        ])
        .await;

        // VM with only defender can take run-1
        let run = pool.take_run("windows", &["defender".to_string()]).await;
        assert!(run.is_some());
        assert_eq!(run.unwrap().run_id.0, "run-1");

        // VM with only rededr can take run-2
        let run = pool.take_run("windows", &["rededr".to_string()]).await;
        assert!(run.is_some());
        assert_eq!(run.unwrap().run_id.0, "run-2");

        // VM with both can take run-3
        let run = pool
            .take_run("windows", &["defender".to_string(), "rededr".to_string()])
            .await;
        assert!(run.is_some());
        assert_eq!(run.unwrap().run_id.0, "run-3");
    }

    #[tokio::test]
    async fn test_result_routing() {
        let pool = Arc::new(RunPool::new());
        let (tx, mut rx) = mpsc::channel(10);

        let job = test_job("job-1");
        pool.register_job(&job, tx).await;

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

        let received = rx.recv().await;
        assert!(received.is_some());
        assert_eq!(received.unwrap().run_id.0, "run-1");
    }

    #[tokio::test]
    async fn test_job_registration() {
        let pool = RunPool::new();
        let (tx1, _) = mpsc::channel(10);
        let (tx2, _) = mpsc::channel(10);

        let job1 = test_job("job-1");
        let job2 = test_job("job-2");

        pool.register_job(&job1, tx1).await;
        pool.register_job(&job2, tx2).await;

        assert_eq!(pool.job_count().await, 2);

        // Check job info is tracked
        let jobs = pool.list_running_jobs();
        assert_eq!(jobs.len(), 2);

        pool.unregister_job(&JobId("job-1".to_string())).await;
        assert_eq!(pool.job_count().await, 1);
    }
}
