//! Per-job worker that owns the full lifecycle of a mutation exploration job.
//!
//! Builds baseline, instrumented, and dryrun artifacts for each round,
//! submits runs to the shared [`RunPool`], aggregates results into
//! [`RoundSummary`](super::types::RoundSummary) records, and emits
//! completion events for Elasticsearch indexing. Consults the configured
//! [`Selector`] to choose mutations for each
//! successive round.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use std::str::FromStr;

use build::{
    ArtifactBuilder, BuildInput, BuilderConfig, BuiltArtifact, EncodingType, ModuleSelection,
    MsvcCompat, PreparedPayload, prepare_payload,
};

use super::channels::{CoverageCorrection, JobRunResult, JobWorkerEvent, RoundCompletedData};
use super::run_pool::RunPool;
use super::types::{
    ArtifactRef, JobId, JobOutcome, JobSession, ModularBuildSpec, RoundAgg, RoundId, RoundSpec,
    RunEnvelope, RunId, RunOutcome, RunType,
};
use crate::storage::EsStorage;
use crate::triage::{Selector, TriageGuidance};

// ============================================================================
// Configuration
// ============================================================================

/// Maximum rounds being processed simultaneously
const MAX_IN_FLIGHT_ROUNDS: usize = 5;

/// Maximum pending runs in pool for this job (backpressure)
/// Each round = 3 runs (baseline + instrumented + dryrun), so 9 = 3 rounds worth
const MAX_PENDING_RUNS: usize = 9;

/// Default timeout for run execution
const DEFAULT_TIMEOUT_SECONDS: u32 = 100;

/// Grace period (seconds) to wait for dryrun result after baseline+instrumented complete.
/// If no dryrun worker picks up the run within this window, the round finalizes without it.
const DRYRUN_GRACE_PERIOD_SECS: u64 = 5;

/// Interval for checking if more rounds can be produced
const PRODUCTION_CHECK_INTERVAL_MS: u64 = 100;

// ============================================================================
// Static Defender Scan
// ============================================================================

/// Run a Defender static file scan on the built artifact from WSL.
///
/// Converts the WSL path to a Windows path via `wslpath -w`, then invokes
/// `MpCmdRun.exe -Scan -ScanType 3 -File <win_path> -DisableRemediation`.
///
/// Returns `true` if the artifact was statically detected (exit code == 2),
/// `false` otherwise (clean, error, or timeout).
async fn static_defender_scan(artifact_path: &Path) -> bool {
    // Convert WSL path → Windows path
    let win_path = match tokio::process::Command::new("wslpath")
        .arg("-w")
        .arg(artifact_path.as_os_str())
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        Ok(output) => {
            warn!(
                "wslpath failed (status={}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return false;
        }
        Err(e) => {
            warn!("Failed to run wslpath: {}", e);
            return false;
        }
    };

    debug!("Static scan: {} → {}", artifact_path.display(), win_path);

    // Run Defender scan
    let scan_result =
        tokio::process::Command::new("/mnt/c/Program Files/Windows Defender/MpCmdRun.exe")
            .args([
                "-Scan",
                "-ScanType",
                "3",
                "-File",
                &win_path,
                "-DisableRemediation",
            ])
            .output()
            .await;

    match scan_result {
        Ok(output) => {
            let code = output.status.code().unwrap_or(-1);
            if code == 2 {
                info!("Static Defender scan DETECTED: {}", artifact_path.display());
                true
            } else {
                debug!(
                    "Static Defender scan clean (exit={}): {}",
                    code,
                    artifact_path.display()
                );
                false
            }
        }
        Err(e) => {
            warn!("Failed to run MpCmdRun.exe: {}", e);
            false
        }
    }
}

// ============================================================================
// JobWorker
// ============================================================================

/// Per-job worker that drives the full mutation-exploration lifecycle.
///
/// Owns the round-production loop, artifact building, result aggregation, and
/// triage extraction for a single job. Communicates with the shared
/// [`RunPool`] via channels and the [`Selector`] to choose mutations.
///
/// Backpressure is enforced by two constants:
/// - `MAX_IN_FLIGHT_ROUNDS` (5) — rounds being aggregated concurrently.
/// - `MAX_PENDING_RUNS` (9) — pending runs in the pool for this job.
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

    /// Mutation selector (coverage-driven or token-guided)
    selector: Arc<dyn Selector>,

    /// Shutdown token
    shutdown_token: CancellationToken,

    /// Artifact paths to clean up when the job finishes.
    /// Deferred from per-round cleanup to avoid deleting files still referenced
    /// by other rounds (content-addressed paths can collide across rounds).
    artifact_cleanup: Vec<PathBuf>,

    /// Cached payload headers: baseline (trace=off), computed on first build.
    baseline_payload: Option<PreparedPayload>,

    /// Cached payload headers: instrumented (trace≠off), computed on first build.
    instrumented_payload: Option<PreparedPayload>,

    /// Cached raw payload bytes (read from disk once).
    cached_payload: Option<Vec<u8>>,

    /// Receives async coverage corrections from Orchestrator background tasks.
    correction_rx: mpsc::Receiver<CoverageCorrection>,

    /// ES storage for async triage token extraction (None = triage disabled).
    storage: Option<Arc<EsStorage>>,

    /// Latest triage guidance from async background task. Updated non-blocking.
    latest_guidance: Option<TriageGuidance>,

    /// Receives triage guidance from background extraction tasks.
    guidance_rx: mpsc::Receiver<TriageGuidance>,

    /// Sender cloned into spawned triage tasks.
    guidance_tx: mpsc::Sender<TriageGuidance>,
}

impl JobWorker {
    /// Create a new JobWorker for the given job.
    pub fn new(
        job: JobSession,
        run_pool: Arc<RunPool>,
        event_tx: mpsc::Sender<JobWorkerEvent>,
        selector: Arc<dyn Selector>,
        correction_rx: mpsc::Receiver<CoverageCorrection>,
        storage: Option<Arc<EsStorage>>,
    ) -> Self {
        let (result_tx, result_rx) = mpsc::channel(64);
        let (guidance_tx, guidance_rx) = mpsc::channel(16);
        Self {
            job,
            run_pool,
            result_rx,
            result_tx,
            round_aggs: HashMap::new(),
            event_tx,
            selector,
            shutdown_token: CancellationToken::new(),
            artifact_cleanup: Vec::new(),
            baseline_payload: None,
            instrumented_payload: None,
            cached_payload: None,
            correction_rx,
            storage,
            latest_guidance: None,
            guidance_rx,
            guidance_tx,
        }
    }

    /// Get the job ID.
    #[allow(dead_code)]
    pub fn job_id(&self) -> &JobId {
        &self.job.id
    }

    /// Get the shutdown token for external cancellation.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Main event loop — multiplexes five `select!` branches (biased):
    ///
    /// 1. **Shutdown** — cancellation token from orchestrator.
    /// 2. **Pool shutdown** — global pool teardown.
    /// 3. **Run results** — outcomes from VMs via [`RunPool`] routing.
    /// 4. **Coverage corrections** — async blended-score patches.
    /// 5. **Production tick** — periodic check to produce more rounds.
    pub async fn run(mut self) {
        info!(
            "[JobWorker:{}] Started (max_rounds={}, stop_on_evasion={})",
            self.job.id, self.job.max_rounds, self.job.stop_on_evasion
        );

        // Register with pool for result routing and job tracking
        self.run_pool
            .register_job(&self.job, self.result_tx.clone())
            .await;

        // Mark job as started
        self.job.mark_started();

        // Production check interval
        let mut check_interval =
            tokio::time::interval(Duration::from_millis(PRODUCTION_CHECK_INTERVAL_MS));

        // Get pool cancellation token (needs to be bound to a variable for the select!)
        let pool_shutdown = self.run_pool.cancellation_token();

        // Track exit reason
        enum ExitReason {
            Completed,
            Cancelled,
            PoolShutdown,
        }
        #[allow(unused_assignments)]
        let mut exit_reason = ExitReason::Completed;

        loop {
            tokio::select! {
                biased;

                // Shutdown signal (job cancelled)
                _ = self.shutdown_token.cancelled() => {
                    info!("[JobWorker:{}] Shutdown requested", self.job.id);
                    exit_reason = ExitReason::Cancelled;
                    break;
                }

                // Global pool shutdown
                _ = pool_shutdown.cancelled() => {
                    info!("[JobWorker:{}] Pool shutdown, stopping", self.job.id);
                    exit_reason = ExitReason::PoolShutdown;
                    break;
                }

                // Receive results from VMs (via RunPool routing)
                Some(result) = self.result_rx.recv() => {
                    self.on_result(result).await;
                }

                // Receive async coverage corrections from Orchestrator
                Some(correction) = self.correction_rx.recv() => {
                    self.apply_coverage_correction(correction);
                }

                // Receive triage guidance (non-blocking update from background tasks)
                Some(guidance) = self.guidance_rx.recv() => {
                    info!(
                        "[JobWorker:{}] Triage guidance: {} avoid, {} seek tokens",
                        self.job.id, guidance.avoid_tokens.len(), guidance.seek_tokens.len()
                    );
                    self.latest_guidance = Some(guidance);
                }

                // Periodic check to produce more rounds
                _ = check_interval.tick() => {
                    // Check for rounds past dryrun grace deadline
                    let expired: Vec<RoundId> = self.round_aggs.iter()
                        .filter(|(_, agg)| {
                            agg.dryrun_deadline.is_some_and(|d| Instant::now() >= d)
                        })
                        .map(|(id, _)| id.clone())
                        .collect();
                    for round_id in expired {
                        // Remove the unclaimed dryrun run from the pool before finalizing
                        if let Some(agg) = self.round_aggs.get(&round_id) {
                            self.run_pool.remove_run(&agg.dryrun_run_id);
                        }
                        info!(
                            "[JobWorker:{}] Dryrun grace expired for round {}, finalizing without",
                            self.job.id, round_id
                        );
                        self.finalize_round(&round_id).await;
                    }

                    // Produce more rounds if possible
                    if self.can_produce_round()
                        && let Err(e) = self.produce_round().await {
                            error!("[JobWorker:{}] Failed to produce round: {}", self.job.id, e);
                        }

                    // Check if job is done
                    if self.is_job_complete() {
                        info!("[JobWorker:{}] Job complete", self.job.id);
                        exit_reason = ExitReason::Completed;
                        break;
                    }
                }
            }
        }

        // Cleanup
        self.run_pool.unregister_job(&self.job.id).await;

        // Clean up build artifacts now that all rounds are done
        self.artifact_cleanup.sort();
        self.artifact_cleanup.dedup();
        for path in &self.artifact_cleanup {
            match std::fs::remove_file(path) {
                Ok(()) => debug!("Cleaned build artifact: {:?}", path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warn!("Failed to clean build artifact {:?}: {}", path, e),
            }
        }

        // Emit appropriate outcome based on exit reason
        let outcome = match exit_reason {
            ExitReason::Completed => JobOutcome::Completed {
                rounds_completed: self.job.current_round,
            },
            ExitReason::Cancelled => JobOutcome::Stopped {
                reason: "Job cancelled".to_string(),
            },
            ExitReason::PoolShutdown => JobOutcome::Stopped {
                reason: "Scheduler shutdown".to_string(),
            },
        };

        // Update job registry with final status
        self.run_pool.complete_job(&self.job.id, &outcome).await;

        let _ = self
            .event_tx
            .send(JobWorkerEvent::JobCompleted {
                job_id: self.job.id.clone(),
                outcome: outcome.clone(),
            })
            .await;

        match exit_reason {
            ExitReason::Completed => {
                info!(
                    "[JobWorker:{}] Completed ({} rounds)",
                    self.job.id, self.job.current_round
                );
            }
            ExitReason::Cancelled => {
                warn!(
                    "[JobWorker:{}] Stopped (cancelled after {} rounds)",
                    self.job.id, self.job.current_round
                );
            }
            ExitReason::PoolShutdown => {
                warn!(
                    "[JobWorker:{}] Stopped (pool shutdown after {} rounds)",
                    self.job.id, self.job.current_round
                );
            }
        }
    }

    // ========================================================================
    // Round Production
    // ========================================================================

    /// Check whether a new round may be produced.
    ///
    /// Returns `false` when any of three backpressure conditions hold:
    /// 1. The job's round budget is exhausted (or stop-on-evasion triggered).
    /// 2. Too many rounds are in-flight (`>= MAX_IN_FLIGHT_ROUNDS`).
    /// 3. Too many pending runs in the pool (`>= MAX_PENDING_RUNS`).
    fn can_produce_round(&self) -> bool {
        // Job must want more rounds
        if !self.job.should_continue() {
            return false;
        }

        // Not too many in-flight rounds (being aggregated)
        if self.round_aggs.len() >= MAX_IN_FLIGHT_ROUNDS {
            return false;
        }

        // Backpressure: don't overload the run pool
        let pending = self.run_pool.pending_runs_for_job(&self.job.id);
        if pending >= MAX_PENDING_RUNS {
            return false;
        }

        true
    }

    /// Returns `true` when the job should exit: no more rounds to produce
    /// **and** all in-flight round aggregators have been finalized.
    fn is_job_complete(&self) -> bool {
        // No more rounds to produce and all in-flight rounds done
        !self.job.should_continue() && self.round_aggs.is_empty()
    }

    /// Produce a new round: select mutations, build artifacts, and enqueue runs.
    ///
    /// # Errors
    ///
    /// Returns an error if artifact building fails (e.g. payload not found,
    /// Clang/LLVM toolchain error).
    async fn produce_round(&mut self) -> anyhow::Result<()> {
        let (round_num, round_id) = self.job.start_round();
        info!(
            "[JobWorker:{}] Producing round {} (id={})",
            self.job.id, round_num, round_id
        );

        // Call selector with in-memory history + async triage guidance
        let selection = self
            .selector
            .select(
                &self.job.id.0,
                round_num,
                &self.job.search_space,
                &self.job.build_spec.modules,
                &self.job.rounds,
                self.latest_guidance.as_ref(),
            )
            .await;

        info!(
            "[JobWorker:{}] Selector: {}",
            self.job.id, selection.rationale
        );

        // Create round spec with SELECTED modules
        let spec = RoundSpec {
            id: round_id.clone(),
            job_id: self.job.id.clone(),
            round_number: round_num,
            mutations: selection.mutations,
            modules: selection.modules.clone(),
        };

        // Build with SELECTED modules (not job defaults)
        let selected_build_spec = ModularBuildSpec {
            modules: selection.modules,
            payload_path: self.job.build_spec.payload_path.clone(),
            encoding: self.job.build_spec.encoding.clone(),
        };

        // Build baseline artifact (trace_mode = off)
        let baseline_built = self
            .build_artifact(&selected_build_spec, "off", &spec)
            .await
            .map_err(|e| {
                error!(
                    "[JobWorker:{}] Failed to build baseline artifact: {:#}",
                    self.job.id, e
                );
                e
            })?;

        // Static Defender scan: check if artifact is detected on-disk before VM dispatch
        if static_defender_scan(&baseline_built.output_path).await {
            info!(
                "[JobWorker:{}] Round {} statically detected by Defender, skipping VM dispatch",
                self.job.id, round_id
            );

            // Defer cleanup of the baseline artifact
            self.artifact_cleanup
                .push(baseline_built.output_path.clone());

            // Build a synthetic RoundAgg with static_scan_detected flag
            let static_outcome = RunOutcome {
                detected: true,
                exit_code: 2,
                error: None,
                success: false,
                elapsed_ms: 0.0,
                detection_verdict: "static_detection".to_string(),
                last_checkpoint: String::new(),
            };
            let mut static_agg = RoundAgg::new(spec, DEFAULT_TIMEOUT_SECONDS as u64 * 1000);
            static_agg.baseline = Some(static_outcome.clone());
            static_agg.instrumented = Some(static_outcome);
            static_agg.baseline_vm_id = "static_scan".to_string();
            static_agg.instrumented_vm_id = "static_scan".to_string();
            static_agg.baseline_artifact_path = baseline_built.output_path.clone();
            static_agg.instrumented_artifact_path = baseline_built.output_path;
            static_agg.static_scan_detected = true;

            self.round_aggs.insert(round_id.clone(), static_agg);
            self.finalize_round(&round_id).await;
            return Ok(());
        }

        // Build instrumented artifact (trace_mode from job config)
        let trace_mode = self.job.trace_mode.clone();
        let instrumented_built = self
            .build_artifact(&selected_build_spec, &trace_mode, &spec)
            .await
            .map_err(|e| {
                error!(
                    "[JobWorker:{}] Failed to build instrumented artifact: {:#}",
                    self.job.id, e
                );
                e
            })?;

        // Use instrumented build's assembled source for coverage/source viewer.
        // The checkpoint stub prepended to the instrumented payload shifts payload.h
        // line counts; trace events embed line numbers from this source.
        let assembled_source = instrumented_built.assembled_source.clone();

        // Determine target OS and capabilities from job constraints
        let target_os = self
            .job
            .target_os
            .clone()
            .unwrap_or_else(|| "windows".to_string());
        let required_caps = self.job.required_capabilities.clone();

        // Save artifact paths for post-round cleanup
        let baseline_artifact_path = baseline_built.output_path.clone();
        let instrumented_artifact_path = instrumented_built.output_path.clone();

        // Create run envelopes
        let (runs, dryrun_run_id) = create_run_envelopes(
            &self.job.id,
            &round_id,
            round_num,
            &spec,
            &baseline_built,
            &instrumented_built,
            &baseline_artifact_path,
            &target_os,
            &required_caps,
        );

        // Create round aggregator
        let mut agg = RoundAgg::new(spec, DEFAULT_TIMEOUT_SECONDS as u64 * 1000);
        agg.assembled_source = assembled_source;
        agg.baseline_artifact_path = baseline_artifact_path;
        agg.instrumented_artifact_path = instrumented_artifact_path;
        self.round_aggs.insert(round_id.clone(), agg);

        info!(
            "[JobWorker:{}] Built runs for round {} (baseline={}, instrumented={}, dryrun={})",
            self.job.id,
            round_id,
            baseline_built.artifact_id,
            instrumented_built.artifact_id,
            dryrun_run_id
        );

        // Add all 3 runs to shared pool
        self.run_pool.add_runs(runs).await;

        Ok(())
    }

    /// Build a single artifact using the modular template system.
    ///
    /// Caches the raw payload bytes and precomputed payload headers across rounds
    /// (unless `cache_payload` is disabled). Optionally enables MSVC-compat mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the payload file cannot be read, the builder config is
    /// invalid, or the Clang/LLVM build pipeline fails.
    async fn build_artifact(
        &mut self,
        build_spec: &ModularBuildSpec,
        trace_mode: &str,
        round_spec: &RoundSpec,
    ) -> anyhow::Result<BuiltArtifact> {
        // Create builder config, optionally enabling MSVC-compat mode
        let msvc_compat = if self.job.msvc_compat {
            Some(MsvcCompat {
                vcvarsall_path: if self.job.msvc_vcvarsall.is_empty() {
                    MsvcCompat::default_vcvarsall()
                } else {
                    std::path::PathBuf::from(&self.job.msvc_vcvarsall)
                },
            })
        } else {
            None
        };
        let config = BuilderConfig {
            msvc_compat,
            ..BuilderConfig::default()
        };
        let builder = ArtifactBuilder::new(config)?;

        // Read payload from file (cached across rounds)
        let payload = match &self.cached_payload {
            Some(p) => p.clone(),
            None => {
                let p = tokio::fs::read(&build_spec.payload_path)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "Failed to read payload {}: {}",
                            build_spec.payload_path.display(),
                            e
                        )
                    })?;
                self.cached_payload = Some(p.clone());
                p
            }
        };

        // Convert module selection
        let modules: ModuleSelection = build_spec.modules.clone().into();

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

        // Select cached precomputed payload header (lazy init), gated on cache_payload flag
        let precomputed = if self.job.cache_payload {
            if trace_mode == "off" || trace_mode.is_empty() {
                if self.baseline_payload.is_none() {
                    let prepared = prepare_payload(
                        &payload,
                        encoding,
                        trace_mode,
                        self.job.sc_checkpoint_count,
                    )?;
                    self.baseline_payload = Some(prepared);
                }
                self.baseline_payload.clone()
            } else {
                if self.instrumented_payload.is_none() {
                    let prepared = prepare_payload(
                        &payload,
                        encoding,
                        trace_mode,
                        self.job.sc_checkpoint_count,
                    )?;
                    self.instrumented_payload = Some(prepared);
                }
                self.instrumented_payload.clone()
            }
        } else {
            // cache_payload=false: force fresh encoding every round
            None
        };

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
                mutation_targets: self.job.search_space.mutation_targets.clone(),
                sc_checkpoint_count: self.job.sc_checkpoint_count,
                precomputed_payload: precomputed,
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

    /// Handle a run result received from a VMExecutor (via [`RunPool`] routing).
    ///
    /// Performs O(1) round lookup, stores the outcome in the [`RoundAgg`], and
    /// triggers finalization when core runs (baseline + instrumented) complete.
    /// A grace period is started for the optional dryrun before finalizing without it.
    async fn on_result(&mut self, result: JobRunResult) {
        debug!(
            "[JobWorker:{}] Run {} completed: detected={}, exit={}, success={}",
            self.job.id,
            result.run_id,
            result.outcome.detected,
            result.outcome.exit_code,
            result.outcome.success
        );

        // Direct O(1) lookup by round_id instead of linear scan
        let round_to_finalize = if let Some(agg) = self.round_aggs.get_mut(&result.round_id) {
            if agg.baseline_run_id == result.run_id {
                agg.baseline = Some(result.outcome.clone());
                agg.baseline_vm_id = result.vm_id.clone();
            } else if agg.instrumented_run_id == result.run_id {
                agg.instrumented = Some(result.outcome.clone());
                agg.instrumented_vm_id = result.vm_id.clone();
            } else if agg.dryrun_run_id == result.run_id {
                agg.dryrun = Some(result.outcome.clone());
                agg.dryrun_vm_id = result.vm_id.clone();
            } else {
                warn!(
                    "[JobWorker:{}] Run {} doesn't match round {} runs",
                    self.job.id, result.run_id, result.round_id
                );
                return;
            }

            // Determine if round is ready to finalize:
            // Core runs (baseline+instrumented) must be done.
            // Dryrun is optional — we use a grace period.
            if agg.is_complete() {
                if agg.dryrun.is_some() {
                    // All 3 done — finalize immediately
                    Some(result.round_id.clone())
                } else if agg.dryrun_deadline.is_none() {
                    // Core runs done, dryrun pending — start grace period
                    agg.dryrun_deadline =
                        Some(Instant::now() + Duration::from_secs(DRYRUN_GRACE_PERIOD_SECS));
                    debug!(
                        "[JobWorker:{}] Round {} core runs done, waiting {}s for dryrun",
                        self.job.id, result.round_id, DRYRUN_GRACE_PERIOD_SECS
                    );
                    None
                } else {
                    // Already waiting for dryrun or deadline
                    None
                }
            } else {
                None
            }
        } else {
            warn!(
                "[JobWorker:{}] Result for unknown round: {}",
                self.job.id, result.round_id
            );
            return;
        };

        if let Some(round_id) = round_to_finalize {
            self.finalize_round(&round_id).await;
        }
    }

    /// Finalize a completed round: compute summary, record in session, spawn
    /// triage extraction, and emit the [`RoundCompleted`](super::channels::JobWorkerEvent::RoundCompleted)
    /// event for Elasticsearch indexing.
    async fn finalize_round(&mut self, round_id: &RoundId) {
        let agg = match self.round_aggs.remove(round_id) {
            Some(a) => a,
            None => return,
        };

        // Defer artifact cleanup until job finishes — content-addressed paths
        // (sha256.exe) can collide across rounds, so deleting here would break
        // later rounds that reference the same file.
        self.artifact_cleanup
            .push(agg.baseline_artifact_path.clone());
        self.artifact_cleanup
            .push(agg.instrumented_artifact_path.clone());

        if let Some(dr) = &agg.dryrun {
            info!(
                "[JobWorker:{}] Round {} has dryrun result (exit={})",
                self.job.id, round_id, dr.exit_code
            );
        } else {
            debug!(
                "[JobWorker:{}] Round {} finalizing without dryrun result",
                self.job.id, round_id
            );
        }

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
            "[JobWorker:{}] Round {} complete: detected={}, evasion={:.2}, has_dryrun={}",
            self.job.id, round_id, summary.detected, summary.evasion_score, summary.has_dryrun
        );

        // Record in job session (selector reads history from here)
        self.job.record_round_summary(summary.clone());

        // Update job registry for API visibility
        self.run_pool.update_job_progress(&self.job);
        self.run_pool.record_round_completed().await;

        // Spawn async triage extraction (non-blocking, non-fatal)
        if let Some(ref storage) = self.storage {
            let guidance_tx = self.guidance_tx.clone();
            let storage = storage.clone();
            let job_id = self.job.id.0.clone();
            let round_id_str = round_id.0.clone();
            let baseline_run_id = format!("{}-baseline", round_id_str);
            let instrumented_run_id = format!("{}-instrumented", round_id_str);
            let summary_clone = summary.clone();
            let all_summaries: Vec<(super::types::RoundSummary, bool)> = self
                .job
                .rounds
                .values()
                .map(|s| (s.clone(), s.detected))
                .collect();
            tokio::spawn(async move {
                match crate::triage::extractor::extract_and_score(
                    &storage,
                    &job_id,
                    &round_id_str,
                    &baseline_run_id,
                    &instrumented_run_id,
                    &summary_clone,
                    &all_summaries,
                )
                .await
                {
                    Ok(guidance) => {
                        let _ = guidance_tx.send(guidance).await;
                    }
                    Err(e) => {
                        warn!("[Triage:{}] Extraction failed (non-fatal): {}", job_id, e);
                    }
                }
            });
        }

        // Emit enriched event for ES indexing (round + both runs + optional dryrun)
        let data = build_round_completed_data(self.job.id.clone(), round_id.clone(), agg, summary);
        let _ = self
            .event_tx
            .send(JobWorkerEvent::RoundCompleted(Box::new(data)))
            .await;
    }

    /// Apply an async coverage correction to a completed round's evasion score.
    /// This patches the in-memory history so subsequent selector calls see the blended score.
    fn apply_coverage_correction(&mut self, correction: CoverageCorrection) {
        if let Some(summary) = self.job.rounds.get_mut(&correction.round_number) {
            let old_score = summary.evasion_score;
            summary.evasion_score = correction.blended_evasion_score;
            summary.coverage_percent = Some(correction.coverage_percent);
            debug!(
                "[JobWorker:{}] Coverage correction for round {}: score {:.3} → {:.3} (cov={:.1}%)",
                self.job.id,
                correction.round_number,
                old_score,
                correction.blended_evasion_score,
                correction.coverage_percent,
            );
        } else {
            warn!(
                "[JobWorker:{}] Coverage correction for unknown round {}",
                self.job.id, correction.round_number
            );
        }
    }
}

/// Build the 3 RunEnvelopes (baseline + instrumented + dryrun) for a round.
/// Returns the envelopes and the dryrun RunId (for logging).
#[allow(clippy::too_many_arguments)]
fn create_run_envelopes(
    job_id: &JobId,
    round_id: &RoundId,
    round_number: u32,
    spec: &RoundSpec,
    baseline_built: &BuiltArtifact,
    instrumented_built: &BuiltArtifact,
    baseline_artifact_path: &Path,
    target_os: &str,
    required_caps: &[String],
) -> (Vec<RunEnvelope>, RunId) {
    let mutations: Vec<String> = spec.mutations.iter().map(|m| m.id.clone()).collect();

    let baseline_run = RunEnvelope {
        run_id: RunId(format!("{}-baseline", round_id.0)),
        job_id: job_id.clone(),
        round_id: round_id.clone(),
        round_number,
        run_type: RunType::Baseline,
        artifact: ArtifactRef {
            path: baseline_built.output_path.clone(),
            sha256: Some(baseline_built.sha256.clone()),
        },
        mutations: mutations.clone(),
        timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        required_os: target_os.to_string(),
        required_capabilities: required_caps.to_vec(),
    };

    let instrumented_run = RunEnvelope {
        run_id: RunId(format!("{}-instrumented", round_id.0)),
        job_id: job_id.clone(),
        round_id: round_id.clone(),
        round_number,
        run_type: RunType::Instrumented,
        artifact: ArtifactRef {
            path: instrumented_built.output_path.clone(),
            sha256: Some(instrumented_built.sha256.clone()),
        },
        mutations: mutations.clone(),
        timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        required_os: target_os.to_string(),
        required_capabilities: required_caps.to_vec(),
    };

    let dryrun_run_id = RunId(format!("{}-dryrun", round_id.0));
    let dryrun_run = RunEnvelope {
        run_id: dryrun_run_id.clone(),
        job_id: job_id.clone(),
        round_id: round_id.clone(),
        round_number,
        run_type: RunType::DryRun,
        artifact: ArtifactRef {
            path: baseline_artifact_path.to_path_buf(),
            sha256: Some(baseline_built.sha256.clone()),
        },
        mutations,
        timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        required_os: target_os.to_string(),
        required_capabilities: vec!["dryrun".to_string()],
    };

    (
        vec![baseline_run, instrumented_run, dryrun_run],
        dryrun_run_id,
    )
}

/// Extract all fields from a RoundAgg into a RoundCompletedData for ES indexing.
fn build_round_completed_data(
    job_id: JobId,
    round_id: RoundId,
    agg: RoundAgg,
    summary: super::types::RoundSummary,
) -> RoundCompletedData {
    RoundCompletedData {
        job_id,
        round_id,
        summary,
        baseline_run_id: agg.baseline_run_id,
        instrumented_run_id: agg.instrumented_run_id,
        baseline_outcome: agg.baseline.unwrap(),
        instrumented_outcome: agg.instrumented.unwrap(),
        mutation_specs: agg.spec.mutations.clone(),
        mutations: agg.spec.mutations.into_iter().map(|m| m.id).collect(),
        modules: agg.spec.modules,
        baseline_vm_id: agg.baseline_vm_id,
        instrumented_vm_id: agg.instrumented_vm_id,
        round_started_at: agg.started_at,
        assembled_source: agg.assembled_source,
        dryrun_run_id: Some(agg.dryrun_run_id),
        dryrun_outcome: agg.dryrun,
        dryrun_vm_id: agg.dryrun_vm_id,
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
    use crate::dispatch::types::{ModularBuildSpec, ModuleSelectionSpec, RoundSummary};
    use crate::triage::{SearchSpace, Selection, Selector, TriageGuidance};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn test_build_spec() -> ModularBuildSpec {
        ModularBuildSpec {
            modules: ModuleSelectionSpec::default(),
            payload_path: PathBuf::from("test.bin"),
            encoding: "xor".to_string(),
        }
    }

    struct NoopSelector;

    #[async_trait::async_trait]
    impl Selector for NoopSelector {
        async fn select(
            &self,
            _: &str,
            _: u32,
            _: &SearchSpace,
            defaults: &ModuleSelectionSpec,
            _: &BTreeMap<u32, RoundSummary>,
            _: Option<&TriageGuidance>,
        ) -> Selection {
            Selection {
                modules: defaults.clone(),
                mutations: vec![],
                rationale: "noop".into(),
            }
        }
    }

    #[tokio::test]
    async fn test_job_worker_creation() {
        let pool = Arc::new(RunPool::new());
        let (event_tx, _) = mpsc::channel(10);
        let (_corr_tx, corr_rx) = mpsc::channel(10);

        let job = JobSession::new("test-job", 3, test_build_spec());
        let worker = JobWorker::new(job, pool, event_tx, Arc::new(NoopSelector), corr_rx, None);

        assert_eq!(worker.job_id().0, "test-job");
    }

    #[tokio::test]
    async fn test_can_produce_round() {
        let pool = Arc::new(RunPool::new());
        let (event_tx, _) = mpsc::channel(10);
        let (_corr_tx, corr_rx) = mpsc::channel(10);

        let job = JobSession::new("test-job", 3, test_build_spec());
        let worker = JobWorker::new(job, pool, event_tx, Arc::new(NoopSelector), corr_rx, None);

        // Should be able to produce initially
        assert!(worker.can_produce_round());
    }
}
