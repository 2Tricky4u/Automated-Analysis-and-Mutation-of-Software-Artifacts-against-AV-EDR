//! Worker task - owns job execution for a single VM.
//!
//! Dual-lane model:
//! - Producer lane: creates rounds, builds artifacts, submits RunEnvelopes
//! - Dispatch lane: sends runs to remote VM, receives results

use super::channels::{RemoteRunResult, WorkerCommand, WorkerEvent};
use super::types::{
    ArtifactRef, JobId, JobOutcome, JobSession, MutationSpec, RoundAgg, RoundId, RoundSpec,
    RoundSummary, RunEnvelope, RunId, RunOutcome, RunType, WorkerId, WorkerInfo,
};
use crate::automutate::common::{controller_message, ControllerMessage, RunSampleCommand, SampleRequest};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

// ============================================================================
// Configuration
// ============================================================================

const MAX_POOL_SIZE: usize = 10;
const MAX_IN_FLIGHT_ROUNDS: usize = 3;
const DEFAULT_TIMEOUT_SECONDS: u32 = 120;

// ============================================================================
// Worker
// ============================================================================

pub struct Worker {
    // Identity
    id: WorkerId,
    info: WorkerInfo,

    // Channels
    cmd_rx: mpsc::Receiver<WorkerCommand>,
    event_tx: mpsc::Sender<WorkerEvent>,
    remote_tx: mpsc::Sender<ControllerMessage>,
    remote_rx: mpsc::Receiver<RemoteRunResult>,

    // Artifact sender (for uploading before dispatch)
    artifact_sender: Arc<dyn ArtifactSender + Send + Sync>,

    // Local state (single-writer)
    active_job: Option<JobSession>,
    run_pool: VecDeque<RunEnvelope>,
    round_aggs: HashMap<RoundId, RoundAgg>,
    pending_runs: HashMap<RunId, RunEnvelope>,

    // Dispatch state
    max_concurrent: usize,
    in_flight: usize,
}

/// Trait for sending artifacts to remote VM
pub trait ArtifactSender: std::fmt::Debug {
    fn send_artifact(
        &self,
        worker_id: &str,
        artifact_id: &str,
        path: &Path,
    ) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

impl Worker {
    pub fn new(
        id: WorkerId,
        info: WorkerInfo,
        cmd_rx: mpsc::Receiver<WorkerCommand>,
        event_tx: mpsc::Sender<WorkerEvent>,
        remote_tx: mpsc::Sender<ControllerMessage>,
        remote_rx: mpsc::Receiver<RemoteRunResult>,
        artifact_sender: Arc<dyn ArtifactSender + Send + Sync>,
        max_concurrent: usize,
    ) -> Self {
        Self {
            id,
            info,
            cmd_rx,
            event_tx,
            remote_tx,
            remote_rx,
            artifact_sender,
            active_job: None,
            run_pool: VecDeque::new(),
            round_aggs: HashMap::new(),
            pending_runs: HashMap::new(),
            max_concurrent,
            in_flight: 0,
        }
    }

    /// Main worker loop
    pub async fn run(mut self) {
        info!("[Worker:{}] Started (os={}, caps={:?})",
            self.id, self.info.os, self.info.capabilities);

        let mut production_interval = interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                biased;

                // Priority 1: Commands from orchestrator
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        WorkerCommand::AssignJob(job) => {
                            self.start_job(job);
                        }
                        WorkerCommand::Shutdown => {
                            info!("[Worker:{}] Shutdown requested", self.id);
                            break;
                        }
                    }
                }

                // Priority 2: Results from remote VM
                Some(result) = self.remote_rx.recv() => {
                    self.on_run_completed(result).await;
                }

                // Priority 3: Production tick
                _ = production_interval.tick(), if self.can_produce_rounds() => {
                    self.produce_round().await;
                }
            }

            // After any event, attempt dispatch
            self.try_dispatch().await;
        }

        // Cleanup
        if let Some(job) = self.active_job.take() {
            self.finalize_job(job, JobOutcome::Stopped {
                reason: "Worker shutdown".to_string(),
            }).await;
        }

        info!("[Worker:{}] Stopped", self.id);
    }

    // ========================================================================
    // Job Management
    // ========================================================================

    fn start_job(&mut self, mut job: JobSession) {
        info!("[Worker:{}] Starting job {} (max_rounds={})",
            self.id, job.id, job.max_rounds);

        job.mark_started();
        self.active_job = Some(job);

        // Clear any leftover state
        self.run_pool.clear();
        self.round_aggs.clear();
        self.pending_runs.clear();
    }

    async fn finalize_job(&mut self, job: JobSession, outcome: JobOutcome) {
        info!("[Worker:{}] Finalizing job {}: {:?}", self.id, job.id, outcome);

        // Emit completion event
        let _ = self.event_tx.send(WorkerEvent::JobCompleted {
            worker_id: self.id.clone(),
            job_id: job.id.clone(),
            outcome,
        }).await;

        // Clear state
        self.run_pool.clear();
        self.round_aggs.clear();
        self.pending_runs.clear();

        // Emit available
        let _ = self.event_tx.send(WorkerEvent::Available {
            worker_id: self.id.clone(),
        }).await;
    }

    // ========================================================================
    // Round Production
    // ========================================================================

    fn can_produce_rounds(&self) -> bool {
        // Must have active job
        let job = match &self.active_job {
            Some(j) => j,
            None => return false,
        };

        // Job must want more rounds
        if !job.should_continue() {
            return false;
        }

        // Pool not full
        if self.run_pool.len() >= MAX_POOL_SIZE {
            return false;
        }

        // Not too many in-flight rounds
        if self.round_aggs.len() >= MAX_IN_FLIGHT_ROUNDS {
            return false;
        }

        true
    }

    async fn produce_round(&mut self) {
        let job = match &mut self.active_job {
            Some(j) => j,
            None => return,
        };

        // Start round
        let (round_num, round_id) = job.start_round();
        info!("[Worker:{}][{}] Producing round {}", self.id, job.id, round_num);

        // Create round spec (simplified - no selector for now)
        let spec = RoundSpec {
            id: round_id.clone(),
            job_id: job.id.clone(),
            round_number: round_num,
            mutations: vec![], // TODO: integrate with mutation selector
        };

        // Get artifact path
        let artifact_path = match &job.payload_path {
            Some(p) => p.clone(),
            None => {
                warn!("[Worker:{}] No payload path for job {}", self.id, job.id);
                return;
            }
        };

        // Create run envelopes for baseline and instrumented
        let baseline_run = RunEnvelope {
            run_id: RunId(format!("{}-baseline", round_id.0)),
            job_id: job.id.clone(),
            round_id: round_id.clone(),
            round_number: round_num,
            run_type: RunType::Baseline,
            artifact: ArtifactRef {
                path: artifact_path.clone(),
                sha256: None,
            },
            mutations: spec.mutations.iter().map(|m| m.id.clone()).collect(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        };

        let instrumented_run = RunEnvelope {
            run_id: RunId(format!("{}-instrumented", round_id.0)),
            run_type: RunType::Instrumented,
            ..baseline_run.clone()
        };

        // Create round aggregator
        let agg = RoundAgg {
            spec,
            baseline_run_id: baseline_run.run_id.clone(),
            instrumented_run_id: instrumented_run.run_id.clone(),
            baseline: None,
            instrumented: None,
        };
        self.round_aggs.insert(round_id.clone(), agg);

        // Add to pool
        debug!("[Worker:{}] Enqueuing runs for round {}", self.id, round_id);
        self.run_pool.push_back(baseline_run);
        self.run_pool.push_back(instrumented_run);
    }

    // ========================================================================
    // Dispatch
    // ========================================================================

    async fn try_dispatch(&mut self) {
        // Dispatch up to available slots
        while self.in_flight < self.max_concurrent {
            match self.run_pool.pop_front() {
                Some(envelope) => {
                    if let Err(e) = self.dispatch_run(envelope).await {
                        error!("[Worker:{}] Dispatch failed: {}", self.id, e);
                        self.in_flight = self.in_flight.saturating_sub(1);
                    }
                }
                None => break,
            }
        }

        // Check if job is complete
        if let Some(job) = &self.active_job {
            if !job.should_continue()
                && self.run_pool.is_empty()
                && self.round_aggs.is_empty()
                && self.in_flight == 0
            {
                let job = self.active_job.take().unwrap();
                let rounds_completed = job.current_round;
                self.finalize_job(job, JobOutcome::Completed { rounds_completed }).await;
            }
        }

        // Emit available if idle
        if self.active_job.is_none()
            && self.in_flight == 0
            && self.run_pool.is_empty()
        {
            let _ = self.event_tx.try_send(WorkerEvent::Available {
                worker_id: self.id.clone(),
            });
        }
    }

    async fn dispatch_run(&mut self, envelope: RunEnvelope) -> anyhow::Result<()> {
        debug!("[Worker:{}] Dispatching run {} ({})",
            self.id, envelope.run_id, envelope.run_type);

        // Upload artifact first
        let artifact_id = format!("{}-{}", envelope.run_id.0, envelope.run_type);
        self.artifact_sender
            .send_artifact(&self.id.0, &artifact_id, &envelope.artifact.path)
            .await?;

        // Build command
        let command = ControllerMessage {
            payload: Some(controller_message::Payload::RunSample(RunSampleCommand {
                request_id: envelope.run_id.0.clone(),
                request: Some(SampleRequest {
                    job_id: envelope.job_id.0.clone(),
                    artifact_id: artifact_id.clone(),
                    trace_mode: envelope.run_type.trace_mode().to_string(),
                    timeout_seconds: envelope.timeout_seconds as i32,
                    ..Default::default()
                }),
            })),
        };

        // Track pending
        self.pending_runs.insert(envelope.run_id.clone(), envelope);
        self.in_flight += 1;

        // Send
        self.remote_tx.send(command).await
            .map_err(|_| anyhow::anyhow!("Remote channel closed"))?;

        Ok(())
    }

    // ========================================================================
    // Run Completion
    // ========================================================================

    async fn on_run_completed(&mut self, result: RemoteRunResult) {
        debug!("[Worker:{}] Run completed: {} (detected={}, exit={})",
            self.id, result.run_id, result.detected, result.exit_code);

        self.in_flight = self.in_flight.saturating_sub(1);

        // Get envelope
        let envelope = match self.pending_runs.remove(&result.run_id) {
            Some(e) => e,
            None => {
                warn!("[Worker:{}] Unknown run_id: {}", self.id, result.run_id);
                return;
            }
        };

        // Build outcome
        let outcome = RunOutcome {
            detected: result.detected,
            exit_code: result.exit_code,
            error: result.error.clone(),
        };

        // Emit run completed event
        let _ = self.event_tx.send(WorkerEvent::RunCompleted {
            worker_id: self.id.clone(),
            run_id: result.run_id.clone(),
            outcome: outcome.clone(),
        }).await;

        // Update round aggregator
        if let Some(agg) = self.round_aggs.get_mut(&envelope.round_id) {
            match envelope.run_type {
                RunType::Baseline => agg.baseline = Some(outcome),
                RunType::Instrumented => agg.instrumented = Some(outcome),
            }

            // Check if round complete
            if agg.is_complete() {
                self.finalize_round(&envelope.round_id).await;
            }
        }
    }

    async fn finalize_round(&mut self, round_id: &RoundId) {
        let agg = match self.round_aggs.remove(round_id) {
            Some(a) => a,
            None => return,
        };

        let summary = match agg.to_summary() {
            Some(s) => s,
            None => return,
        };

        info!("[Worker:{}] Round {} complete: detected={}, evasion={:.2}",
            self.id, round_id, summary.detected, summary.evasion_score);

        // Record in job session
        if let Some(job) = &mut self.active_job {
            job.record_round_summary(summary);
        }
    }
}
