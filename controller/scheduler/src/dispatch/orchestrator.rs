//! Orchestrator - routes jobs to compatible workers.
//!
//! Responsibilities:
//! - Receives job submissions
//! - Routes jobs to compatible workers (OS + capabilities)
//! - Holds pending_jobs queue for unassigned jobs
//! - Listens to worker events

use super::channels::{OrchestratorEvent, WorkerCommand, WorkerEvent};
use super::types::{JobId, JobOutcome, JobSession, WorkerId, WorkerInfo};
use std::collections::{HashMap, VecDeque};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// ============================================================================
// Worker Handle
// ============================================================================

pub struct WorkerHandle {
    pub info: WorkerInfo,
    pub cmd_tx: mpsc::Sender<WorkerCommand>,
    pub event_rx: mpsc::Receiver<WorkerEvent>,
    pub has_active_job: bool,
}

impl WorkerHandle {
    pub fn is_idle(&self) -> bool {
        !self.has_active_job
    }
}

// ============================================================================
// Orchestrator
// ============================================================================

pub struct Orchestrator {
    pending_jobs: VecDeque<JobSession>,
    workers: HashMap<WorkerId, WorkerHandle>,
    orchestrator_rx: mpsc::Receiver<OrchestratorEvent>,
    job_submit_rx: mpsc::Receiver<JobSession>,
}

impl Orchestrator {
    pub fn new(
        orchestrator_rx: mpsc::Receiver<OrchestratorEvent>,
        job_submit_rx: mpsc::Receiver<JobSession>,
    ) -> Self {
        Self {
            pending_jobs: VecDeque::new(),
            workers: HashMap::new(),
            orchestrator_rx,
            job_submit_rx,
        }
    }

    /// Main orchestrator loop
    pub async fn run(mut self) {
        info!("[Orchestrator] Started");

        loop {
            // Build list of worker event receivers for select
            let worker_ids: Vec<WorkerId> = self.workers.keys().cloned().collect();

            tokio::select! {
                // Job submissions
                Some(job) = self.job_submit_rx.recv() => {
                    self.on_job_submitted(job).await;
                }

                // Orchestrator events (worker connect/disconnect)
                Some(event) = self.orchestrator_rx.recv() => {
                    match event {
                        OrchestratorEvent::WorkerConnected { worker_id, info, cmd_tx, event_rx } => {
                            self.register_worker(worker_id, info, cmd_tx, event_rx).await;
                        }
                        OrchestratorEvent::WorkerDisconnected { worker_id, reason } => {
                            self.unregister_worker(&worker_id, &reason).await;
                        }
                    }
                }

                // Poll worker events (using a helper)
                result = poll_worker_events(&mut self.workers, &worker_ids) => {
                    if let Some((worker_id, event)) = result {
                        self.on_worker_event(worker_id, event).await;
                    }
                }
            }
        }
    }

    // ========================================================================
    // Worker Management
    // ========================================================================

    async fn register_worker(
        &mut self,
        worker_id: WorkerId,
        info: WorkerInfo,
        cmd_tx: mpsc::Sender<WorkerCommand>,
        event_rx: mpsc::Receiver<WorkerEvent>,
    ) {
        info!("[Orchestrator] Worker {} connected (os={}, caps={:?})",
            worker_id, info.os, info.capabilities);

        let handle = WorkerHandle {
            info,
            cmd_tx,
            event_rx,
            has_active_job: false,
        };

        self.workers.insert(worker_id.clone(), handle);

        // Try to assign pending job
        self.try_assign_pending(&worker_id).await;
    }

    async fn unregister_worker(&mut self, worker_id: &WorkerId, reason: &str) {
        info!("[Orchestrator] Worker {} disconnected: {}", worker_id, reason);

        if let Some(handle) = self.workers.remove(worker_id) {
            if handle.has_active_job {
                warn!("[Orchestrator] Worker {} had active job, job lost", worker_id);
                // Could implement job recovery here
            }
        }
    }

    // ========================================================================
    // Job Assignment
    // ========================================================================

    async fn on_job_submitted(&mut self, job: JobSession) {
        info!("[Orchestrator] Job {} submitted (os={:?}, caps={:?})",
            job.id, job.target_os, job.required_capabilities);

        // Find compatible idle worker
        let compatible = self.find_compatible_worker(&job);

        match compatible {
            Some(worker_id) => {
                self.assign_job(&worker_id, job).await;
            }
            None => {
                debug!("[Orchestrator] No compatible worker, queueing job {}", job.id);
                self.pending_jobs.push_back(job);
            }
        }
    }

    fn find_compatible_worker(&self, job: &JobSession) -> Option<WorkerId> {
        for (id, handle) in &self.workers {
            if handle.is_idle() && is_compatible(&handle.info, job) {
                return Some(id.clone());
            }
        }
        None
    }

    async fn assign_job(&mut self, worker_id: &WorkerId, job: JobSession) {
        let handle = match self.workers.get_mut(worker_id) {
            Some(h) => h,
            None => {
                warn!("[Orchestrator] Worker {} not found, requeueing job", worker_id);
                self.pending_jobs.push_back(job);
                return;
            }
        };

        info!("[Orchestrator] Assigning job {} to worker {}", job.id, worker_id);

        handle.has_active_job = true;

        if let Err(e) = handle.cmd_tx.send(WorkerCommand::AssignJob(job)).await {
            error!("[Orchestrator] Failed to send job to worker {}: {}", worker_id, e);
            handle.has_active_job = false;
        }
    }

    async fn try_assign_pending(&mut self, worker_id: &WorkerId) {
        let handle = match self.workers.get(worker_id) {
            Some(h) if h.is_idle() => h,
            _ => return,
        };

        // Find first compatible pending job
        let job_idx = self.pending_jobs.iter()
            .position(|job| is_compatible(&handle.info, job));

        if let Some(idx) = job_idx {
            let job = self.pending_jobs.remove(idx).unwrap();
            self.assign_job(worker_id, job).await;
        }
    }

    // ========================================================================
    // Worker Events
    // ========================================================================

    async fn on_worker_event(&mut self, worker_id: WorkerId, event: WorkerEvent) {
        match event {
            WorkerEvent::Available { worker_id } => {
                debug!("[Orchestrator] Worker {} available", worker_id);
                if let Some(handle) = self.workers.get_mut(&worker_id) {
                    handle.has_active_job = false;
                }
                self.try_assign_pending(&worker_id).await;
            }

            WorkerEvent::JobCompleted { worker_id, job_id, outcome } => {
                info!("[Orchestrator] Job {} completed on worker {}: {:?}",
                    job_id, worker_id, outcome);

                if let Some(handle) = self.workers.get_mut(&worker_id) {
                    handle.has_active_job = false;
                }

                // TODO: Index to ES

                self.try_assign_pending(&worker_id).await;
            }

            WorkerEvent::RunCompleted { worker_id, run_id, outcome } => {
                debug!("[Orchestrator] Run {} completed on worker {}: detected={}",
                    run_id, worker_id, outcome.detected);
                // Observability only
            }
        }
    }

    // ========================================================================
    // Stats
    // ========================================================================

    pub fn pending_count(&self) -> usize {
        self.pending_jobs.len()
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn idle_worker_count(&self) -> usize {
        self.workers.values().filter(|h| h.is_idle()).count()
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Check if worker is compatible with job requirements
fn is_compatible(worker: &WorkerInfo, job: &JobSession) -> bool {
    // OS match (if job specifies target_os)
    let os_ok = match &job.target_os {
        None => true,
        Some(required_os) => worker.os.eq_ignore_ascii_case(required_os),
    };

    if !os_ok {
        return false;
    }

    // Capabilities: worker must have ALL required capabilities
    job.required_capabilities.iter().all(|req| {
        worker.capabilities.iter().any(|cap| cap.eq_ignore_ascii_case(req))
    })
}

/// Poll events from any worker
async fn poll_worker_events(
    workers: &mut HashMap<WorkerId, WorkerHandle>,
    worker_ids: &[WorkerId],
) -> Option<(WorkerId, WorkerEvent)> {
    // Simple round-robin poll
    for id in worker_ids {
        if let Some(handle) = workers.get_mut(id) {
            match handle.event_rx.try_recv() {
                Ok(event) => return Some((id.clone(), event)),
                Err(_) => continue,
            }
        }
    }

    // No events ready, yield briefly
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compatibility_any_os() {
        let worker = WorkerInfo {
            id: WorkerId("w1".into()),
            os: "win10".into(),
            capabilities: vec!["mde".into()],
        };

        let job = JobSession::new("j1", 5);
        assert!(is_compatible(&worker, &job));
    }

    #[test]
    fn test_compatibility_specific_os() {
        let worker = WorkerInfo {
            id: WorkerId("w1".into()),
            os: "win10".into(),
            capabilities: vec!["mde".into()],
        };

        let mut job = JobSession::new("j1", 5);
        job.target_os = Some("win10".into());
        assert!(is_compatible(&worker, &job));

        job.target_os = Some("win11".into());
        assert!(!is_compatible(&worker, &job));
    }

    #[test]
    fn test_compatibility_capabilities() {
        let worker = WorkerInfo {
            id: WorkerId("w1".into()),
            os: "win10".into(),
            capabilities: vec!["mde".into(), "rededr".into()],
        };

        let mut job = JobSession::new("j1", 5);
        job.required_capabilities = vec!["mde".into()];
        assert!(is_compatible(&worker, &job));

        job.required_capabilities = vec!["mde".into(), "rededr".into()];
        assert!(is_compatible(&worker, &job));

        job.required_capabilities = vec!["cortex".into()];
        assert!(!is_compatible(&worker, &job));
    }
}
