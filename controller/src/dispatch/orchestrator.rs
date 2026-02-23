//! Orchestrator - central coordinator for jobs, VMs, and telemetry.
//!
//! Responsibilities:
//! - Spawns JobWorkers for job submissions
//! - Handles VM lifecycle (connect/disconnect) via TargetEvent
//! - Routes telemetry to ES indexing
//! - Manages shared RunPool

use super::channels::{JobControlCommand, JobWorkerEvent, RoundCompletedData};
use super::job_worker::JobWorker;
use super::run_pool::RunPool;
use super::types::{JobId, JobOutcome, JobSession, WorkerId, WorkerInfo};
use crate::storage::{EsStorage, RoundIndexParams, RunIndexParams, TelemetryContext};
use crate::triage::coverage_selector::CoverageSelector;
use crate::triage::source_resolver::SourceMap;
use crate::vm::{TargetEvent, TargetManager};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub struct Orchestrator {
    /// Shared run pool
    run_pool: Arc<RunPool>,

    /// Target manager for VM state
    targets: Arc<TargetManager>,

    /// Consolidated ES storage
    storage: Arc<EsStorage>,

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
        storage: Arc<EsStorage>,
    ) -> Self {
        let (job_event_tx, job_event_rx) = mpsc::channel(256);
        Self {
            run_pool,
            targets,
            storage,
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
            if let Some((resolved_os, resolved_caps)) =
                self.resolve_job_constraints(job.target_os.as_deref(), &job.required_capabilities)
            {
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
                    job.required_capabilities = vec!["rededr".to_string(), "mde".to_string()];
                }
            }
        }

        info!(
            "[Orchestrator] Spawning JobWorker for job {} (max_rounds={}, os={:?}, caps={:?})",
            job_id, job.max_rounds, job.target_os, job.required_capabilities
        );

        let selector = Arc::new(CoverageSelector::new());
        let worker = JobWorker::new(
            job,
            Arc::clone(&self.run_pool),
            self.job_event_tx.clone(),
            selector,
        );

        let shutdown_token = worker.cancellation_token();
        tokio::spawn(worker.run());
        self.job_workers.insert(job_id.clone(), shutdown_token);

        // Update job status to "running" in ES
        let storage = self.storage.clone();
        let jid = job_id.0.clone();
        tokio::spawn(async move {
            if let Err(e) = storage.update_job_started(&jid).await {
                warn!("Failed to update job started status: {}", e);
            }
        });
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
            if let Some(os) = requested_os
                && !t.os_version.eq_ignore_ascii_case(os)
            {
                continue;
            }

            // Check capabilities match if requested
            if !requested_caps.is_empty() {
                let has_all = requested_caps.iter().all(|req| {
                    t.capabilities
                        .iter()
                        .any(|cap| cap.eq_ignore_ascii_case(req))
                });
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
        candidates.sort_by(|a, b| match (a.2, b.2) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.0.cmp(&b.0),
        });

        // Pick the best candidate
        let best = &candidates[0];
        Some((best.0.clone(), best.1.clone()))
    }

    async fn on_job_worker_event(&mut self, event: JobWorkerEvent) {
        match event {
            JobWorkerEvent::RoundCompleted(data) => {
                let RoundCompletedData {
                    job_id,
                    round_id,
                    summary,
                    baseline_run_id,
                    instrumented_run_id,
                    baseline_outcome,
                    instrumented_outcome,
                    mutation_specs,
                    mutations,
                    modules,
                    baseline_vm_id,
                    instrumented_vm_id,
                    round_started_at,
                    assembled_source,
                    dryrun_run_id,
                    dryrun_outcome,
                    dryrun_vm_id,
                } = *data;
                info!(
                    "[Orchestrator] Round {} completed for job {}: detected={}, evasion={:.2}",
                    round_id, job_id, summary.detected, summary.evasion_score
                );

                // Convert round_started_at to RFC3339
                let started_at_str =
                    crate::storage::helpers::system_time_to_rfc3339(round_started_at);

                // Index round, both runs, and update job progress in ES
                let storage = self.storage.clone();
                let jid = job_id.0.clone();
                let rid = round_id.0.clone();
                let round_number = summary.round_number;
                let b_run_id = baseline_run_id.0.clone();
                let i_run_id = instrumented_run_id.0.clone();
                let b_vm_id = baseline_vm_id;
                let i_vm_id = instrumented_vm_id;
                let d_run_id = dryrun_run_id.map(|id| id.0.clone());
                let d_outcome = dryrun_outcome;
                let d_vm_id = dryrun_vm_id;
                tokio::spawn(async move {
                    // Update job progress (current_round)
                    if let Err(e) = storage.update_job_progress(&jid, round_number).await {
                        error!("Failed to update job progress: {}", e);
                    }
                    // Index round summary
                    if let Err(e) = storage
                        .index_round(&RoundIndexParams {
                            job_id: &jid,
                            summary: &summary,
                            mutation_specs: &mutation_specs,
                            baseline_run_id: &b_run_id,
                            instrumented_run_id: &i_run_id,
                            started_at: Some(&started_at_str),
                            modules: Some(&modules),
                            assembled_source: assembled_source.as_deref(),
                            dry_run_exit_code: summary.dry_run_exit_code,
                            has_dryrun: summary.has_dryrun,
                            dryrun_run_id: d_run_id.as_deref(),
                        })
                        .await
                    {
                        error!("Failed to index round: {}", e);
                    }
                    // Index baseline run with exit_code, detected, round_id, run_type
                    if let Err(e) = storage
                        .index_run_result(&RunIndexParams {
                            job_id: &jid,
                            round_id: &rid,
                            run_id: &b_run_id,
                            run_type: "baseline",
                            outcome: &baseline_outcome,
                            mutations: &mutations,
                            vm_id: &b_vm_id,
                        })
                        .await
                    {
                        error!("Failed to index baseline run: {}", e);
                    }
                    // Index instrumented run
                    if let Err(e) = storage
                        .index_run_result(&RunIndexParams {
                            job_id: &jid,
                            round_id: &rid,
                            run_id: &i_run_id,
                            run_type: "instrumented",
                            outcome: &instrumented_outcome,
                            mutations: &mutations,
                            vm_id: &i_vm_id,
                        })
                        .await
                    {
                        error!("Failed to index instrumented run: {}", e);
                    }
                    // Index dryrun run (if present)
                    if let (Some(dr_run_id), Some(dr_outcome)) = (&d_run_id, &d_outcome) {
                        if let Err(e) = storage
                            .index_run_result(&RunIndexParams {
                                job_id: &jid,
                                round_id: &rid,
                                run_id: dr_run_id,
                                run_type: "dryrun",
                                outcome: dr_outcome,
                                mutations: &mutations,
                                vm_id: &d_vm_id,
                            })
                            .await
                        {
                            error!("Failed to index dryrun run: {}", e);
                        }
                    }

                    // Compute line coverage from trace data
                    if let Some(ref source) = assembled_source {
                        // Telemetry bulk+refresh should make data immediately visible.
                        // Single defensive retry in case of edge-case latency.
                        let mut trace_content = storage.query_trace_content(&i_run_id).await;
                        if trace_content.is_none() {
                            debug!(
                                "Round {}/{}: trace content not found on first query, retrying in 1s",
                                jid, rid
                            );
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                            trace_content = storage.query_trace_content(&i_run_id).await;
                        }
                        match trace_content {
                            Some(content) => {
                                let executed_lines: HashSet<usize> = content
                                    .lines()
                                    .filter_map(|line| {
                                        serde_json::from_str::<serde_json::Value>(line.trim()).ok()
                                    })
                                    .filter_map(|v| v["line"].as_u64().map(|n| n as usize))
                                    .collect();

                                if !executed_lines.is_empty() {
                                    let sm = SourceMap::new(source);
                                    let coverage = sm.compute_coverage(&executed_lines);
                                    info!(
                                        "Round {}/{}: coverage {:.1}% ({}/{} lines), cutoff: {:?}",
                                        jid,
                                        rid,
                                        coverage.coverage_percent,
                                        coverage.executed_lines,
                                        coverage.total_executable,
                                        coverage.cutoff_line,
                                    );
                                    if let Err(e) =
                                        storage.update_round_coverage(&jid, &rid, &coverage).await
                                    {
                                        error!("Failed to update round coverage: {}", e);
                                    }
                                } else {
                                    warn!("Round {}/{}: trace content found but no line numbers parsed", jid, rid);
                                }
                            }
                            None => {
                                warn!("Round {}/{}: no trace content found after retry, skipping coverage", jid, rid);
                            }
                        }
                    } else {
                        debug!("Round {}/{}: no assembled source, skipping coverage", jid, rid);
                    }
                });
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
                        warn!("[Orchestrator] Job {} stopped: {}", job_id, reason);
                    }
                    JobOutcome::Failed { error } => {
                        error!("[Orchestrator] Job {} failed: {}", job_id, error);
                    }
                }
                self.job_workers.remove(&job_id);

                // Update job status in ES
                let status = outcome.to_status().to_string();
                let storage = self.storage.clone();
                let jid = job_id.0.clone();
                tokio::spawn(async move {
                    if let Err(e) = storage
                        .update_job_status(&jid, &status, Some(&outcome))
                        .await
                    {
                        error!("Failed to update job status: {}", e);
                    }
                });
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
                    let storage = self.storage.clone();
                    let events = batch.events;
                    let context = TelemetryContext {
                        run_id: if batch.run_id.is_empty() {
                            None
                        } else {
                            Some(batch.run_id.clone())
                        },
                        round_id: None, // Not available on TelemetryBatch
                        vm_id: target_id.to_string(),
                    };
                    tokio::spawn(async move {
                        if let Err(e) = storage.index_telemetry_batch(&events, &context).await {
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
                debug!(
                    "[Orchestrator] Ack: {} - req: {}",
                    target_id, ack.request_id
                );
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

    #[allow(dead_code)]
    pub fn shutdown_all_jobs(&self) {
        warn!("[Orchestrator] Shutting down all jobs");
        for token in self.job_workers.values() {
            token.cancel();
        }
        self.run_pool.shutdown();
        let targets = Arc::clone(&self.targets);
        tokio::spawn(async move {
            targets.disconnect_all("Orchestrator shutdown", true).await;
        });
    }

    #[allow(dead_code)]
    pub fn active_job_count(&self) -> usize {
        self.job_workers.len()
    }

    #[allow(dead_code)]
    pub fn vm_count(&self) -> usize {
        self.vms.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elasticsearch::Elasticsearch;
    use elasticsearch::http::transport::Transport;

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let (_events_tx, events_rx) = mpsc::channel(10);
        let (_job_tx, job_rx) = mpsc::channel(10);
        let (_job_control_tx, job_control_rx) = mpsc::channel(10);
        let run_pool = Arc::new(RunPool::new());

        // Create minimal TargetManager for test
        let (target_events_tx, _) = mpsc::channel(10);
        let targets = Arc::new(TargetManager::new(
            30,
            target_events_tx,
            Arc::clone(&run_pool),
        ));

        // Create EsStorage (won't actually connect in test)
        let transport = Transport::single_node("http://localhost:9200").unwrap();
        let es_client = Elasticsearch::new(transport);
        let storage = Arc::new(EsStorage::new(es_client));

        let orchestrator = Orchestrator::new(
            events_rx,
            job_rx,
            job_control_rx,
            run_pool,
            targets,
            storage,
        );

        assert_eq!(orchestrator.active_job_count(), 0);
        assert_eq!(orchestrator.vm_count(), 0);
    }
}
