//! Orchestrator - manages pool groups and routes jobs.
//!
//! Responsibilities:
//! - Create and manage pool groups by capabilities
//! - Route jobs to compatible pool groups
//! - Handle worker connect/disconnect
//! - Forward pool events

use super::channels::{OrchestratorEvent, WorkerEvent};
use super::group_id::GroupId;
use super::pool_group::{PoolEvent, PoolGroup};
use super::types::{JobSession, WorkerId, WorkerInfo};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// ============================================================================
// Worker Handle
// ============================================================================

/// Handle to a connected worker.
pub struct WorkerHandle {
    pub info: WorkerInfo,
    pub group_id: GroupId,
}

// ============================================================================
// Orchestrator
// ============================================================================

use super::pool_group::PoolGroupRegistry;

pub struct Orchestrator {
    /// Shared pool group registry (same as TargetManager uses)
    pool_registry: Arc<PoolGroupRegistry>,

    /// Connected workers
    workers: HashMap<WorkerId, WorkerHandle>,

    /// Jobs waiting for a compatible pool group
    pending_jobs: VecDeque<JobSession>,

    /// Channel for orchestrator events (worker connect/disconnect)
    orchestrator_rx: mpsc::Receiver<OrchestratorEvent>,

    /// Channel for job submissions
    job_submit_rx: mpsc::Receiver<JobSession>,

    /// Channel for pool events
    pool_event_rx: mpsc::Receiver<PoolEvent>,

    /// Aggregated channel for worker events (all workers send to this)
    worker_event_rx: mpsc::Receiver<WorkerEvent>,
}

impl Orchestrator {
    pub fn new(
        orchestrator_rx: mpsc::Receiver<OrchestratorEvent>,
        job_submit_rx: mpsc::Receiver<JobSession>,
        pool_registry: Arc<PoolGroupRegistry>,
        pool_event_rx: mpsc::Receiver<PoolEvent>,
        worker_event_rx: mpsc::Receiver<WorkerEvent>,
    ) -> Self {
        Self {
            pool_registry,
            workers: HashMap::new(),
            pending_jobs: VecDeque::new(),
            orchestrator_rx,
            job_submit_rx,
            pool_event_rx,
            worker_event_rx,
        }
    }

    /// Main orchestrator loop.
    /// Event-driven: wakes only when there's actual work to do.
    pub async fn run(mut self) {
        info!("Orchestrator started");

        loop {
            tokio::select! {
                biased;

                // Job submissions
                Some(job) = self.job_submit_rx.recv() => {
                    self.on_job_submitted(job).await;
                }

                // Orchestrator events (worker connect/disconnect)
                Some(event) = self.orchestrator_rx.recv() => {
                    match event {
                        OrchestratorEvent::WorkerConnected { worker_id, info } => {
                            self.on_worker_connected(worker_id, info).await;
                        }
                        OrchestratorEvent::WorkerDisconnected { worker_id, reason } => {
                            self.on_worker_disconnected(&worker_id, &reason).await;
                        }
                    }
                }

                // Pool events
                Some(event) = self.pool_event_rx.recv() => {
                    self.on_pool_event(event).await;
                }

                // Worker events (aggregated bus - O(1) receive per event)
                Some(event) = self.worker_event_rx.recv() => {
                    self.on_worker_event(event).await;
                }
            }
        }
    }

    // ========================================================================
    // Pool Group Management
    // ========================================================================

    /// Find a pool group that can run the given job.
    async fn find_compatible_pool(&self, job: &JobSession) -> Option<Arc<PoolGroup>> {
        self.pool_registry
            .find_compatible(job.target_os.as_deref(), &job.required_capabilities)
            .await
    }

    /// Get the pool registry.
    pub fn pool_registry(&self) -> Arc<PoolGroupRegistry> {
        Arc::clone(&self.pool_registry)
    }

    // ========================================================================
    // Worker Management
    // ========================================================================

    async fn on_worker_connected(&mut self, worker_id: WorkerId, info: WorkerInfo) {
        let group_id = GroupId::from_worker_info(&info);

        info!(
            "Worker {} connected (os={}, caps={:?}, group={})",
            worker_id, info.os, info.capabilities, group_id
        );

        // Pool group is already created by TargetManager when Worker was spawned
        // Just store the worker handle
        let handle = WorkerHandle {
            info,
            group_id: group_id.clone(),
        };
        self.workers.insert(worker_id.clone(), handle);

        // Try to assign pending jobs to this group
        self.try_assign_pending_jobs(&group_id).await;
    }

    async fn on_worker_disconnected(&mut self, worker_id: &WorkerId, reason: &str) {
        info!("Worker {} disconnected: {}", worker_id, reason);

        if let Some(_handle) = self.workers.remove(worker_id) {
            // Worker removed, pool group remains (other workers may still use it)
            // TODO: Consider cleaning up empty pool groups
        }
    }

    // ========================================================================
    // Job Management
    // ========================================================================

    async fn on_job_submitted(&mut self, job: JobSession) {
        info!(
            "Job {} submitted (os={:?}, caps={:?})",
            job.id, job.target_os, job.required_capabilities
        );

        // Find compatible pool group
        match self.find_compatible_pool(&job).await {
            Some(pool) => {
                info!("Assigning job {} to pool {}", job.id, pool.group_id());
                pool.assign_job(job).await;
            }
            None => {
                debug!(
                    "No compatible pool for job {}, queueing",
                    job.id
                );
                self.pending_jobs.push_back(job);
            }
        }
    }

    /// Try to assign pending jobs to a specific pool group.
    async fn try_assign_pending_jobs(&mut self, group_id: &GroupId) {
        let pool = match self.pool_registry.get(group_id).await {
            Some(p) => p,
            None => return,
        };

        // Find jobs compatible with this group
        let mut assigned = Vec::new();
        for (idx, job) in self.pending_jobs.iter().enumerate() {
            if group_id.satisfies(job.target_os.as_deref(), &job.required_capabilities) {
                assigned.push(idx);
            }
        }

        // Assign jobs (reverse order to maintain indices)
        for idx in assigned.into_iter().rev() {
            if let Some(job) = self.pending_jobs.remove(idx) {
                info!("Assigning queued job {} to pool {}", job.id, group_id);
                pool.assign_job(job).await;
            }
        }
    }

    // ========================================================================
    // Event Handling
    // ========================================================================

    async fn on_pool_event(&mut self, event: PoolEvent) {
        match event {
            PoolEvent::RunDispatched { pool_id, run_id, worker_id } => {
                debug!(
                    "Run {} dispatched to worker {} (pool={})",
                    run_id, worker_id, pool_id
                );
            }
            PoolEvent::RunCompleted { pool_id, run_id, outcome } => {
                debug!(
                    "Run {} completed: detected={} (pool={})",
                    run_id, outcome.detected, pool_id
                );
            }
            PoolEvent::RoundCompleted { pool_id, round_id, summary } => {
                info!(
                    "Round {} completed: detected={}, evasion={:.2} (pool={})",
                    round_id, summary.detected, summary.evasion_score, pool_id
                );
                // TODO: Index to ES
            }
            PoolEvent::JobCompleted { pool_id, job_id, outcome } => {
                info!(
                    "Job {} completed: {:?} (pool={})",
                    job_id, outcome, pool_id
                );
                // TODO: Index to ES
            }
        }
    }

    /// Handle worker event from aggregated bus.
    async fn on_worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Available { worker_id } => {
                debug!("Worker {} available", worker_id);
                // Workers now pull from pool, no action needed
            }
            WorkerEvent::JobCompleted { worker_id, job_id, outcome } => {
                // This shouldn't happen in new architecture (jobs complete in pool)
                warn!(
                    "Received JobCompleted from worker {} (job={}, outcome={:?}) - unexpected",
                    worker_id, job_id, outcome
                );
            }
            WorkerEvent::RunCompleted { worker_id, run_id, outcome } => {
                debug!(
                    "Run {} completed on worker {}: detected={}",
                    run_id, worker_id, outcome.detected
                );
                // Observability only, aggregation happens in pool
            }
        }
    }

    // ========================================================================
    // Stats (public API for monitoring/management)
    // TODO: Expose these via gRPC service (ListWorkers, GetStats endpoints)
    // ========================================================================

    #[allow(dead_code)]
    pub fn pending_count(&self) -> usize {
        self.pending_jobs.len()
    }

    #[allow(dead_code)]
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    #[allow(dead_code)]
    pub async fn pool_count(&self) -> usize {
        self.pool_registry.count().await
    }

    #[allow(dead_code)]
    pub async fn pool_group_ids(&self) -> Vec<GroupId> {
        self.pool_registry.list_ids().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{ModularBuildSpec, ModuleSelectionSpec};
    use std::path::PathBuf;

    fn test_build_spec() -> ModularBuildSpec {
        ModularBuildSpec {
            modules: ModuleSelectionSpec::default(),
            payload_path: PathBuf::from("test.bin"),
            encoding: "xor".to_string(),
        }
    }

    #[test]
    fn test_group_id_satisfies() {
        let group = GroupId::new("win10-defender+rededr");

        // No requirements
        assert!(group.satisfies(None, &[]));

        // OS only
        assert!(group.satisfies(Some("win10"), &[]));
        assert!(!group.satisfies(Some("win11"), &[]));

        // Capabilities only
        assert!(group.satisfies(None, &["defender".into()]));
        assert!(group.satisfies(None, &["defender".into(), "rededr".into()]));
        assert!(!group.satisfies(None, &["cortex".into()]));
    }
}
