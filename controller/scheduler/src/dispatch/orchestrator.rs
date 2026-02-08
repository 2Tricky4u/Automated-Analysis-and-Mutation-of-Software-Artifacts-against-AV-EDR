//! Orchestrator - central coordinator for jobs, VMs, and telemetry.
//!
//! Responsibilities:
//! - Spawns JobWorkers for job submissions
//! - Handles VM lifecycle (connect/disconnect) via TargetEvent
//! - Routes telemetry to ES indexing
//! - Manages shared RunPool

use super::channels::{JobControlCommand, JobWorkerEvent};
use super::job_worker::JobWorker;
use super::run_pool::RunPool;
use super::types::{JobId, JobOutcome, JobSession, WorkerId, WorkerInfo};
use crate::vm::{TargetEvent, TargetManager};
use elasticsearch::Elasticsearch;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub struct Orchestrator {
    /// Shared run pool
    run_pool: Arc<RunPool>,

    /// Target manager for VM state
    targets: Arc<TargetManager>,

    /// ES client for telemetry indexing
    es_client: Elasticsearch,

    /// Active job workers: JobId -> CancellationToken
    job_workers: HashMap<JobId, CancellationToken>,

    /// Sender for job worker events (given to each JobWorker)
    job_event_tx: mpsc::Sender<JobWorkerEvent>,

    /// Receiver for job worker events
    job_event_rx: mpsc::Receiver<JobWorkerEvent>,

    /// Connected VMs: WorkerId -> WorkerInfo
    vms: HashMap<WorkerId, WorkerInfo>,

    /// Channel for target events (VM lifecycle + telemetry)
    events_rx: mpsc::Receiver<TargetEvent>,

    /// Channel for job submissions
    job_submit_rx: mpsc::Receiver<JobSession>,

    /// Channel for job control commands (stop, etc.)
    job_control_rx: mpsc::Receiver<JobControlCommand>,
}

impl Orchestrator {
    pub fn new(
        events_rx: mpsc::Receiver<TargetEvent>,
        job_submit_rx: mpsc::Receiver<JobSession>,
        job_control_rx: mpsc::Receiver<JobControlCommand>,
        run_pool: Arc<RunPool>,
        targets: Arc<TargetManager>,
        es_client: Elasticsearch,
    ) -> Self {
        let (job_event_tx, job_event_rx) = mpsc::channel(256);
        Self {
            run_pool,
            targets,
            es_client,
            job_workers: HashMap::new(),
            job_event_tx,
            job_event_rx,
            vms: HashMap::new(),
            events_rx,
            job_submit_rx,
            job_control_rx,
        }
    }

    /// Main orchestrator loop.
    pub async fn run(mut self) {
        info!("Orchestrator started");

        loop {
            tokio::select! {
                biased;

                // Job control commands (stop, etc.) - highest priority
                Some(cmd) = self.job_control_rx.recv() => {
                    self.on_job_control(cmd);
                }

                // Job submissions -> spawn JobWorker
                Some(job) = self.job_submit_rx.recv() => {
                    self.spawn_job_worker(job).await;
                }

                // JobWorker events (round/job completion)
                Some(event) = self.job_event_rx.recv() => {
                    self.on_job_worker_event(event).await;
                }

                // Target events (VM lifecycle + telemetry)
                Some(event) = self.events_rx.recv() => {
                    self.on_target_event(event).await;
                }
            }
        }
    }

    // ========================================================================
    // Job Control
    // ========================================================================

    fn on_job_control(&self, cmd: JobControlCommand) {
        match cmd {
            JobControlCommand::Stop { job_id } => {
                self.shutdown_job(&job_id);
            }
        }
    }

    // ========================================================================
    // Job Management
    // ========================================================================

    async fn spawn_job_worker(&mut self, mut job: JobSession) {
        let job_id = job.id.clone();

        // Resolve missing constraints from available targets
        let needs_os = job.target_os.is_none();
        let needs_caps = job.required_capabilities.is_empty();

        if needs_os || needs_caps {
            if let Some((resolved_os, resolved_caps)) = self.resolve_job_constraints(
                job.target_os.as_deref(),
                &job.required_capabilities,
            ) {
                if needs_os {
                    debug!(
                        "[Orchestrator] Job {} auto-assigned OS: {}",
                        job_id, resolved_os
                    );
                    job.target_os = Some(resolved_os);
                }
                if needs_caps {
                    debug!(
                        "[Orchestrator] Job {} auto-assigned capabilities: {:?}",
                        job_id, resolved_caps
                    );
                    job.required_capabilities = resolved_caps;
                }
            } else {
                warn!(
                    "[Orchestrator] Job {} has no matching targets, using defaults",
                    job_id
                );
                if needs_os {
                    job.target_os = Some("win10".to_string());
                    job.required_capabilities = vec!["rededr".to_string(),"mde".to_string()];
                }
            }
        }

        info!(
            "[Orchestrator] Spawning JobWorker for job {} (max_rounds={}, os={:?}, caps={:?})",
            job_id, job.max_rounds, job.target_os, job.required_capabilities
        );

        let worker = JobWorker::new(
            job,
            Arc::clone(&self.run_pool),
            self.job_event_tx.clone(),
        );

        let shutdown_token = worker.cancellation_token();
        tokio::spawn(worker.run());
        self.job_workers.insert(job_id, shutdown_token);
    }

    // ========================================================================
    // Constraint Resolution
    // ========================================================================

    /// Resolve missing job constraints from available targets.
    ///
    /// Given optional OS and capabilities constraints, finds a suitable target
    /// and returns (os, capabilities) to fill in any missing fields.
    ///
    /// Priority:
    /// 1. Available (not busy) targets matching any provided constraints
    /// 2. Any existing (possibly busy) target matching constraints
    /// 3. None if no targets exist
    fn resolve_job_constraints(
        &self,
        requested_os: Option<&str>,
        requested_caps: &[String],
    ) -> Option<(String, Vec<String>)> {
        use crate::vm::TargetStatus;

        let all_targets = self.targets.list_all();

        // Collect candidates: (os, capabilities, is_available)
        let mut candidates: Vec<(String, Vec<String>, bool)> = Vec::new();

        for t in &all_targets {
            if !t.enabled {
                continue;
            }

            // Check OS match if requested
            if let Some(os) = requested_os {
                if !t.os_version.eq_ignore_ascii_case(os) {
                    continue;
                }
            }

            // Check capabilities match if requested
            if !requested_caps.is_empty() {
                let has_all = requested_caps
                    .iter()
                    .all(|req| t.capabilities.iter().any(|cap| cap.eq_ignore_ascii_case(req)));
                if !has_all {
                    continue;
                }
            }

            let is_available = t.status == TargetStatus::Available;
            candidates.push((t.os_version.clone(), t.capabilities.clone(), is_available));
        }

        if candidates.is_empty() {
            // No matching targets - try any enabled target as fallback
            for t in &all_targets {
                if t.enabled {
                    let is_available = t.status == TargetStatus::Available;
                    candidates.push((t.os_version.clone(), t.capabilities.clone(), is_available));
                }
            }
        }

        if candidates.is_empty() {
            return None;
        }

        // Sort: available targets first, then by OS for determinism
        candidates.sort_by(|a, b| {
            match (a.2, b.2) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.0.cmp(&b.0),
            }
        });

        // Pick the best candidate
        let best = &candidates[0];
        Some((best.0.clone(), best.1.clone()))
    }

    async fn on_job_worker_event(&mut self, event: JobWorkerEvent) {
        match event {
            JobWorkerEvent::RoundCompleted {
                job_id,
                round_id,
                summary,
            } => {
                info!(
                    "[Orchestrator] Round {} completed for job {}: detected={}, evasion={:.2}",
                    round_id, job_id, summary.detected, summary.evasion_score
                );
                // TODO: Index round to ES
            }
            JobWorkerEvent::JobCompleted { job_id, outcome } => {
                match &outcome {
                    JobOutcome::Completed { rounds_completed } => {
                        info!(
                            "[Orchestrator] Job {} completed: {} rounds finished",
                            job_id, rounds_completed
                        );
                    }
                    JobOutcome::Stopped { reason } => {
                        warn!(
                            "[Orchestrator] Job {} stopped: {}",
                            job_id, reason
                        );
                    }
                    JobOutcome::Failed { error } => {
                        error!(
                            "[Orchestrator] Job {} failed: {}",
                            job_id, error
                        );
                    }
                }
                self.job_workers.remove(&job_id);
                // TODO: Update job status in ES
            }
        }
    }

    // ========================================================================
    // Target Event Handling (VM lifecycle + telemetry)
    // ========================================================================

    async fn on_target_event(&mut self, event: TargetEvent) {
        match event {
            TargetEvent::Connected {
                target_id,
                os_version,
                capabilities,
            } => {
                debug!(
                    "[Orchestrator] VM {} connected (os={}, caps={:?})",
                    target_id, os_version, capabilities
                );

                // Update target state
                let _ = self.targets.mark_connected(&target_id);

                // Track in local map
                let worker_id = WorkerId(target_id.0.clone());
                let info = WorkerInfo {
                    id: worker_id.clone(),
                    os: os_version,
                    capabilities,
                };
                self.vms.insert(worker_id, info);
            }

            TargetEvent::Disconnected { target_id, reason } => {
                info!("[Orchestrator] VM {} disconnected: {}", target_id, reason);

                // Update target state
                let _ = self.targets.mark_offline(&target_id);

                // Remove from local tracking
                self.vms.remove(&WorkerId(target_id.0.clone()));
            }

            TargetEvent::Message { target_id, msg } => {
                self.handle_worker_message(target_id.as_str(), msg).await;
            }
        }
    }

    async fn handle_worker_message(
        &self,
        target_id: &str,
        msg: crate::automutate::common::WorkerMessage,
    ) {
        use crate::automutate::common::worker_message;

        match msg.payload {
            Some(worker_message::Payload::Registration(reg)) => {
                debug!(
                    "[Orchestrator] Registration: {} - OS: {}, Caps: {:?}",
                    target_id, reg.os_version, reg.capabilities
                );

                let tools = if let Some(tv) = reg.tools {
                    let mut m = HashMap::new();
                    if !tv.rededr_version.is_empty() {
                        m.insert("rededr".to_string(), tv.rededr_version);
                    }
                    if !tv.defender_version.is_empty() {
                        m.insert("defender".to_string(), tv.defender_version);
                    }
                    m
                } else {
                    HashMap::new()
                };

                let _ = self.targets.register_with_metadata(
                    target_id.to_string(),
                    reg.ip_address,
                    reg.os_version,
                    reg.capabilities,
                    reg.metadata,
                    tools,
                );
            }

            Some(worker_message::Payload::Status(status)) => {
                debug!(
                    "[Orchestrator] Status: {} - CPU: {}%, Jobs: {}",
                    target_id, status.cpu_percent, status.active_jobs
                );
                let _ = self.targets.update_health(target_id);
            }

            Some(worker_message::Payload::Telemetry(batch)) => {
                let count = batch.events.len();
                debug!(
                    "[Orchestrator] Telemetry: {} - {} events (run: {})",
                    target_id, count, batch.run_id
                );

                if !batch.events.is_empty() {
                    let es = self.es_client.clone();
                    let events = batch.events;
                    tokio::spawn(async move {
                        if let Err(e) = index_telemetry(&es, &events).await {
                            error!("Failed to index telemetry: {}", e);
                        }
                    });
                }
            }

            Some(worker_message::Payload::SampleResponse(response)) => {
                debug!(
                    "[Orchestrator] SampleResponse: {} - success={}, exit={}",
                    target_id, response.success, response.exit_code
                );
                // Release is handled by VMExecutor when it receives the result
            }

            Some(worker_message::Payload::ExecutionStatus(status)) => {
                debug!(
                    "[Orchestrator] ExecStatus: {} - {} (job: {})",
                    target_id, status.event_type, status.job_id
                );
            }

            Some(worker_message::Payload::Ack(ack)) => {
                debug!("[Orchestrator] Ack: {} - req: {}", target_id, ack.request_id);
            }

            _ => {}
        }
    }

    // ========================================================================
    // Public API
    // ========================================================================

    pub fn shutdown_job(&self, job_id: &JobId) {
        if let Some(token) = self.job_workers.get(job_id) {
            warn!("[Orchestrator] Shutting down job {}", job_id);
            token.cancel();
        }
    }

    pub fn shutdown_all_jobs(&self) {
        warn!("[Orchestrator] Shutting down all jobs");
        for token in self.job_workers.values() {
            token.cancel();
        }
    }

    pub fn active_job_count(&self) -> usize {
        self.job_workers.len()
    }

    pub fn vm_count(&self) -> usize {
        self.vms.len()
    }
}

// ============================================================================
// Telemetry Indexing
// ============================================================================

async fn index_telemetry(
    es: &Elasticsearch,
    events: &[crate::automutate::common::TelemetryData],
) -> anyhow::Result<()> {
    use elasticsearch::IndexParts;
    use serde_json::json;

    if events.is_empty() {
        return Ok(());
    }

    let index_name = format!("telemetry-{}", chrono::Utc::now().format("%Y.%m"));

    for event in events {
        let doc = json!({
            "event_type": event.event_type,
            "timestamp": event.timestamp,
            "job_id": event.job_id,
            "metadata": event.metadata,
        });

        let response = es
            .index(IndexParts::Index(&index_name))
            .body(doc)
            .send()
            .await?;

        if !response.status_code().is_success() {
            return Err(anyhow::anyhow!("Index failed: {}", response.status_code()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use elasticsearch::http::transport::Transport;

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let (_events_tx, events_rx) = mpsc::channel(10);
        let (_job_tx, job_rx) = mpsc::channel(10);
        let (_job_control_tx, job_control_rx) = mpsc::channel(10);
        let run_pool = Arc::new(RunPool::new());

        // Create minimal TargetManager for test
        let (target_events_tx, _) = mpsc::channel(10);
        let targets = Arc::new(TargetManager::new(30, target_events_tx, Arc::clone(&run_pool)));

        // Create ES client (won't actually connect in test)
        let transport = Transport::single_node("http://localhost:9200").unwrap();
        let es_client = Elasticsearch::new(transport);

        let orchestrator = Orchestrator::new(events_rx, job_rx, job_control_rx, run_pool, targets, es_client);

        assert_eq!(orchestrator.active_job_count(), 0);
        assert_eq!(orchestrator.vm_count(), 0);
    }
}
