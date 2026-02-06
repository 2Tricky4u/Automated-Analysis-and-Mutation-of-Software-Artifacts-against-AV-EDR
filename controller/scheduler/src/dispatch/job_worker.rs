//! JobWorker - owns job lifecycle, builds artifacts, aggregates rounds.
//!
//! Architecture:
//! - Spawned per job submission by Orchestrator
//! - Produces runs into shared RunPool
//! - Receives results back and aggregates rounds
//! - Reports round/job completion to Orchestrator
//!
//! Responsibilities:
//! - Build artifacts for each round (baseline + instrumented)
//! - Create RunEnvelopes and add to RunPool
//! - Track in-flight rounds via RoundAgg
//! - Aggregate results and compute RoundSummary
//! - Emit events for ES indexing

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use build::{ArtifactBuilder, BuilderConfig, BuildInput, BuiltArtifact, EncodingType, ModuleSelection};

use super::channels::{JobRunResult, JobWorkerEvent};
use super::run_pool::RunPool;
use super::types::{
    ArtifactRef, JobId, JobOutcome, JobSession, ModularBuildSpec, RoundAgg, RoundId,
    RoundSpec, RunEnvelope, RunId, RunType,
};

// ============================================================================
// Configuration
// ============================================================================

/// Maximum rounds being processed simultaneously
const MAX_IN_FLIGHT_ROUNDS: usize = 5;

/// Default timeout for run execution
const DEFAULT_TIMEOUT_SECONDS: u32 = 120;

/// Interval for checking if more rounds can be produced
const PRODUCTION_CHECK_INTERVAL_MS: u64 = 100;

// ============================================================================
// JobWorker
// ============================================================================

/// JobWorker owns the lifecycle of a single job.
///
/// It produces rounds, builds artifacts, and aggregates results.
/// Each job gets its own JobWorker instance.
pub struct JobWorker {
    /// The job being executed
    job: JobSession,

    /// Shared run pool (puts runs here)
    run_pool: Arc<RunPool>,

    /// Receive results for this job's runs
    result_rx: mpsc::Receiver<JobRunResult>,

    /// Sender for result channel (kept to register with pool)
    result_tx: mpsc::Sender<JobRunResult>,

    /// In-flight round aggregators
    round_aggs: HashMap<RoundId, RoundAgg>,

    /// Event output (to Orchestrator for ES indexing)
    event_tx: mpsc::Sender<JobWorkerEvent>,

    /// Shutdown token
    shutdown_token: CancellationToken,
}

impl JobWorker {
    /// Create a new JobWorker for the given job.
    pub fn new(
        job: JobSession,
        run_pool: Arc<RunPool>,
        event_tx: mpsc::Sender<JobWorkerEvent>,
    ) -> Self {
        let (result_tx, result_rx) = mpsc::channel(64);
        Self {
            job,
            run_pool,
            result_rx,
            result_tx,
            round_aggs: HashMap::new(),
            event_tx,
            shutdown_token: CancellationToken::new(),
        }
    }

    /// Get the job ID.
    pub fn job_id(&self) -> &JobId {
        &self.job.id
    }

    /// Get the shutdown token for external cancellation.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Main loop - produces rounds, receives results, aggregates.
    pub async fn run(mut self) {
        info!(
            "[JobWorker:{}] Started (max_rounds={}, stop_on_evasion={})",
            self.job.id, self.job.max_rounds, self.job.stop_on_evasion
        );

        // Register with pool for result routing
        self.run_pool
            .register_job(self.job.id.clone(), self.result_tx.clone())
            .await;

        // Mark job as started
        self.job.mark_started();

        // Production check interval
        let mut check_interval = tokio::time::interval(Duration::from_millis(PRODUCTION_CHECK_INTERVAL_MS));

        // Get pool cancellation token (needs to be bound to a variable for the select!)
        let pool_shutdown = self.run_pool.cancellation_token();

        loop {
            tokio::select! {
                biased;

                // Shutdown signal
                _ = self.shutdown_token.cancelled() => {
                    info!("[JobWorker:{}] Shutdown requested", self.job.id);
                    break;
                }

                // Global pool shutdown
                _ = pool_shutdown.cancelled() => {
                    info!("[JobWorker:{}] Pool shutdown, stopping", self.job.id);
                    break;
                }

                // Receive results from VMs (via RunPool routing)
                Some(result) = self.result_rx.recv() => {
                    self.on_result(result).await;
                }

                // Periodic check to produce more rounds
                _ = check_interval.tick() => {
                    // Produce more rounds if possible
                    if self.can_produce_round() {
                        if let Err(e) = self.produce_round().await {
                            error!("[JobWorker:{}] Failed to produce round: {}", self.job.id, e);
                        }
                    }

                    // Check if job is done
                    if self.is_job_complete() {
                        info!("[JobWorker:{}] Job complete", self.job.id);
                        break;
                    }
                }
            }
        }

        // Cleanup
        self.run_pool.unregister_job(&self.job.id).await;

        // Emit completion event
        let outcome = JobOutcome::Completed {
            rounds_completed: self.job.current_round,
        };
        let _ = self
            .event_tx
            .send(JobWorkerEvent::JobCompleted {
                job_id: self.job.id.clone(),
                outcome,
            })
            .await;

        info!(
            "[JobWorker:{}] Completed ({} rounds)",
            self.job.id, self.job.current_round
        );
    }

    // ========================================================================
    // Round Production
    // ========================================================================

    /// Check if we can produce more rounds.
    fn can_produce_round(&self) -> bool {
        // Job must want more rounds
        if !self.job.should_continue() {
            return false;
        }

        // Not too many in-flight rounds
        if self.round_aggs.len() >= MAX_IN_FLIGHT_ROUNDS {
            return false;
        }

        true
    }

    /// Check if the job is complete.
    fn is_job_complete(&self) -> bool {
        // No more rounds to produce and all in-flight rounds done
        !self.job.should_continue() && self.round_aggs.is_empty()
    }

    /// Produce a new round (build artifacts, create runs).
    async fn produce_round(&mut self) -> anyhow::Result<()> {
        let (round_num, round_id) = self.job.start_round();
        info!(
            "[JobWorker:{}] Producing round {} (id={})",
            self.job.id, round_num, round_id
        );

        // Create round spec
        let spec = RoundSpec {
            id: round_id.clone(),
            job_id: self.job.id.clone(),
            round_number: round_num,
            mutations: vec![], // TODO: integrate with mutation selector
        };

        // Build baseline artifact (trace_mode = off)
        let baseline_built = self
            .build_artifact(&self.job.build_spec, "off", &spec)
            .await
            .map_err(|e| {
                error!(
                    "[JobWorker:{}] Failed to build baseline artifact: {}",
                    self.job.id, e
                );
                e
            })?;

        // Build instrumented artifact (trace_mode = lines)
        let instrumented_built = self
            .build_artifact(&self.job.build_spec, "lines", &spec)
            .await
            .map_err(|e| {
                error!(
                    "[JobWorker:{}] Failed to build instrumented artifact: {}",
                    self.job.id, e
                );
                e
            })?;

        // Create run envelopes
        let baseline_run = RunEnvelope {
            run_id: RunId(format!("{}-baseline", round_id.0)),
            job_id: self.job.id.clone(),
            round_id: round_id.clone(),
            round_number: round_num,
            run_type: RunType::Baseline,
            artifact: ArtifactRef {
                path: baseline_built.output_path,
                sha256: Some(baseline_built.sha256),
            },
            mutations: spec.mutations.iter().map(|m| m.id.clone()).collect(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        };

        let instrumented_run = RunEnvelope {
            run_id: RunId(format!("{}-instrumented", round_id.0)),
            job_id: self.job.id.clone(),
            round_id: round_id.clone(),
            round_number: round_num,
            run_type: RunType::Instrumented,
            artifact: ArtifactRef {
                path: instrumented_built.output_path,
                sha256: Some(instrumented_built.sha256),
            },
            mutations: spec.mutations.iter().map(|m| m.id.clone()).collect(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
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

        info!(
            "[JobWorker:{}] Built runs for round {} (baseline={}, instrumented={})",
            self.job.id,
            round_id,
            baseline_built.artifact_id,
            instrumented_built.artifact_id
        );

        // Add runs to shared pool
        self.run_pool
            .add_runs(vec![baseline_run, instrumented_run])
            .await;

        Ok(())
    }

    /// Build an artifact using the modular template system.
    async fn build_artifact(
        &self,
        build_spec: &ModularBuildSpec,
        trace_mode: &str,
        round_spec: &RoundSpec,
    ) -> anyhow::Result<BuiltArtifact> {
        // Create builder with default system paths
        let builder = ArtifactBuilder::new(BuilderConfig::default())?;

        // Read payload from file
        let payload = tokio::fs::read(&build_spec.payload_path)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to read payload {}: {}",
                    build_spec.payload_path.display(),
                    e
                )
            })?;

        // Convert module selection
        let modules = ModuleSelection {
            carrier: build_spec.modules.carrier.clone(),
            decoder: build_spec.modules.decoder.clone(),
            antiemulation: build_spec.modules.antiemulation.clone(),
            guardrail: build_spec.modules.guardrail.clone(),
            virtualprotect: build_spec.modules.virtualprotect.clone(),
            decoy: build_spec.modules.decoy.clone(),
        };

        // Parse encoding type
        let encoding = EncodingType::from_str(&build_spec.encoding).unwrap_or(EncodingType::Xor);

        // Convert mutations to builder format
        let mutations: Vec<build::mutator::MutationSpec> = round_spec
            .mutations
            .iter()
            .map(|m| build::mutator::MutationSpec {
                id: m.id.clone(),
                params: m
                    .params
                    .as_ref()
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        obj.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect();

        debug!(
            "[JobWorker:{}] Building artifact (carrier={}, decoder={}, encoding={}, trace_mode={})",
            self.job.id, modules.carrier, modules.decoder, build_spec.encoding, trace_mode
        );

        // Build using modular template
        let built = builder
            .build(BuildInput::ModularTemplate {
                modules,
                payload,
                encoding,
                mutations,
                trace_mode: trace_mode.to_string(),
            })
            .await?;

        debug!(
            "[JobWorker:{}] Build complete: artifact_id={}, size={} bytes",
            self.job.id, built.artifact_id, built.size_bytes
        );

        Ok(built)
    }

    // ========================================================================
    // Result Handling
    // ========================================================================

    /// Handle a result received from VMExecutor (via RunPool routing).
    async fn on_result(&mut self, result: JobRunResult) {
        debug!(
            "[JobWorker:{}] Run {} completed: detected={}, exit={}",
            self.job.id, result.run_id, result.outcome.detected, result.outcome.exit_code
        );

        // Find which round this belongs to and update
        let mut round_to_finalize = None;

        for (round_id, agg) in &mut self.round_aggs {
            if agg.baseline_run_id == result.run_id {
                agg.baseline = Some(result.outcome.clone());
                if agg.is_complete() {
                    round_to_finalize = Some(round_id.clone());
                }
                break;
            } else if agg.instrumented_run_id == result.run_id {
                agg.instrumented = Some(result.outcome.clone());
                if agg.is_complete() {
                    round_to_finalize = Some(round_id.clone());
                }
                break;
            }
        }

        // Finalize round if complete
        if let Some(round_id) = round_to_finalize {
            self.finalize_round(&round_id).await;
        }
    }

    /// Finalize a completed round.
    async fn finalize_round(&mut self, round_id: &RoundId) {
        let agg = match self.round_aggs.remove(round_id) {
            Some(a) => a,
            None => return,
        };

        let summary = match agg.to_summary() {
            Some(s) => s,
            None => {
                warn!(
                    "[JobWorker:{}] Round {} incomplete, cannot finalize",
                    self.job.id, round_id
                );
                return;
            }
        };

        info!(
            "[JobWorker:{}] Round {} complete: detected={}, evasion={:.2}",
            self.job.id, round_id, summary.detected, summary.evasion_score
        );

        // Record in job session
        self.job.record_round_summary(summary.clone());

        // TODO: Report to mutation selector for feedback

        // Emit event for ES indexing
        let _ = self
            .event_tx
            .send(JobWorkerEvent::RoundCompleted {
                job_id: self.job.id.clone(),
                round_id: round_id.clone(),
                summary,
            })
            .await;
    }

    /// Request shutdown of this job worker.
    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
    }
}

impl std::fmt::Debug for JobWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobWorker")
            .field("job_id", &self.job.id)
            .field("current_round", &self.job.current_round)
            .field("in_flight_rounds", &self.round_aggs.len())
            .finish_non_exhaustive()
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

    #[tokio::test]
    async fn test_job_worker_creation() {
        let pool = Arc::new(RunPool::new());
        let (event_tx, _) = mpsc::channel(10);

        let job = JobSession::new("test-job", 3, test_build_spec());
        let worker = JobWorker::new(job, pool, event_tx);

        assert_eq!(worker.job_id().0, "test-job");
    }

    #[tokio::test]
    async fn test_can_produce_round() {
        let pool = Arc::new(RunPool::new());
        let (event_tx, _) = mpsc::channel(10);

        let job = JobSession::new("test-job", 3, test_build_spec());
        let worker = JobWorker::new(job, pool, event_tx);

        // Should be able to produce initially
        assert!(worker.can_produce_round());
    }
}
