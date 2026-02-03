//! PoolGroup - shared run pool for workers with matching capabilities.
//!
//! Multiple workers can share a PoolGroup, allowing parallel execution
//! of runs from the same job across multiple VMs.
//!
//! Key features:
//! - Signal-based dispatch (efficient, no polling)
//! - Round production with artifact building
//! - Result aggregation across workers
//! - Job lifecycle management

use super::group_id::GroupId;
use super::types::{
    ArtifactRef, JobId, JobOutcome, JobSession, ModularBuildSpec, RoundAgg, RoundId, RoundSpec,
    RoundSummary, RunEnvelope, RunId, RunOutcome, RunType,
};
use build::{ArtifactBuilder, BuilderConfig, BuildInput, BuiltArtifact, EncodingType, ModuleSelection};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, error, info};

// ============================================================================
// Configuration
// ============================================================================

/// Maximum runs to buffer in pool before backpressure
const MAX_POOL_SIZE: usize = 20;

/// Maximum rounds being processed simultaneously
const MAX_IN_FLIGHT_ROUNDS: usize = 5;

/// Default timeout for run execution
const DEFAULT_TIMEOUT_SECONDS: u32 = 120;

/// Interval for round production polling (acceptable for heavy operation)
const PRODUCTION_INTERVAL_MS: u64 = 100;

// ============================================================================
// Pool Metrics
// ============================================================================

#[derive(Debug, Default)]
pub struct PoolMetrics {
    pub total_runs_dispatched: u64,
    pub total_runs_completed: u64,
    pub total_rounds_completed: u64,
    pub total_jobs_completed: u64,
}

// ============================================================================
// Pool Events (for Orchestrator observation)
// ============================================================================

/// Events emitted by PoolGroup
#[derive(Debug, Clone)]
pub enum PoolEvent {
    /// Run was dispatched to a worker
    RunDispatched {
        pool_id: GroupId,
        run_id: RunId,
        worker_id: String,
    },
    /// Run completed
    RunCompleted {
        pool_id: GroupId,
        run_id: RunId,
        outcome: RunOutcome,
    },
    /// Round completed (both runs done)
    RoundCompleted {
        pool_id: GroupId,
        round_id: RoundId,
        summary: RoundSummary,
    },
    /// Job completed
    JobCompleted {
        pool_id: GroupId,
        job_id: JobId,
        outcome: JobOutcome,
    },
}

// ============================================================================
// PoolGroup
// ============================================================================

/// Shared pool for workers with matching capabilities.
///
/// Workers in the same group share the run pool, allowing parallel
/// execution across multiple VMs.
pub struct PoolGroup {
    /// Group identifier (OS + capabilities)
    pub id: GroupId,

    // === Run Pool (signal-driven, efficient) ===
    run_pool: Mutex<VecDeque<RunEnvelope>>,
    runs_available: Notify,

    // === Round Tracking ===
    round_aggs: Mutex<HashMap<RoundId, RoundAgg>>,

    // === Job Management ===
    /// Currently active job (one at a time for now)
    active_job: RwLock<Option<JobSession>>,

    /// Jobs waiting to be processed
    pending_jobs: Mutex<VecDeque<JobSession>>,

    // === Event Channel ===
    event_tx: tokio::sync::mpsc::Sender<PoolEvent>,

    // === Metrics ===
    metrics: Mutex<PoolMetrics>,

    // === Shutdown ===
    shutdown: Notify,
}

impl PoolGroup {
    /// Create a new pool group.
    pub fn new(id: GroupId, event_tx: tokio::sync::mpsc::Sender<PoolEvent>) -> Self {
        Self {
            id,
            run_pool: Mutex::new(VecDeque::new()),
            runs_available: Notify::new(),
            round_aggs: Mutex::new(HashMap::new()),
            active_job: RwLock::new(None),
            pending_jobs: Mutex::new(VecDeque::new()),
            event_tx,
            metrics: Mutex::new(PoolMetrics::default()),
            shutdown: Notify::new(),
        }
    }

    /// Get the group ID.
    pub fn group_id(&self) -> &GroupId {
        &self.id
    }

    // ========================================================================
    // Job Management
    // ========================================================================

    /// Assign a job to this pool.
    ///
    /// If no job is active, starts immediately. Otherwise queued.
    pub async fn assign_job(&self, job: JobSession) {
        info!(
            "[Pool:{}] Job {} assigned (max_rounds={})",
            self.id, job.id, job.max_rounds
        );

        let mut active = self.active_job.write().await;
        if active.is_none() {
            *active = Some(job);
            drop(active);
            // Signal producer to start
            self.runs_available.notify_waiters();
        } else {
            drop(active);
            self.pending_jobs.lock().await.push_back(job);
        }
    }

    /// Check if pool has capacity for more jobs.
    #[allow(dead_code)]
    pub async fn has_capacity(&self) -> bool {
        self.active_job.read().await.is_none()
    }

    /// Get current job ID if any.
    #[allow(dead_code)]
    pub async fn current_job_id(&self) -> Option<JobId> {
        self.active_job.read().await.as_ref().map(|j| j.id.clone())
    }

    // ========================================================================
    // Run Pool (Signal-Driven)
    // ========================================================================

    /// Add runs to pool and signal waiting workers.
    pub async fn add_runs(&self, runs: Vec<RunEnvelope>) {
        if runs.is_empty() {
            return;
        }

        let count = runs.len();
        {
            let mut pool = self.run_pool.lock().await;
            pool.extend(runs);
        }

        debug!("[Pool:{}] Added {} runs to pool", self.id, count);

        // Wake ALL idle workers - they compete for runs
        self.runs_available.notify_waiters();
    }

    /// Try to take a run from the pool (non-blocking).
    ///
    /// Returns None if pool is empty.
    pub async fn try_take_run(&self) -> Option<RunEnvelope> {
        let run = self.run_pool.lock().await.pop_front();
        if let Some(ref r) = run {
            debug!("[Pool:{}] Run {} taken from pool", self.id, r.run_id);
            self.metrics.lock().await.total_runs_dispatched += 1;
        }
        run
    }

    /// Wait for runs to become available.
    ///
    /// Use in tokio::select! to efficiently wait for work.
    pub async fn wait_for_runs(&self) {
        self.runs_available.notified().await
    }

    /// Get current pool size.
    #[allow(dead_code)]
    pub async fn pool_size(&self) -> usize {
        self.run_pool.lock().await.len()
    }

    // ========================================================================
    // Result Reporting
    // ========================================================================

    /// Report run completion and aggregate results.
    pub async fn report_result(&self, run_id: RunId, outcome: RunOutcome) {
        debug!(
            "[Pool:{}] Run {} completed: detected={}",
            self.id, run_id, outcome.detected
        );

        self.metrics.lock().await.total_runs_completed += 1;

        // Emit event
        let _ = self
            .event_tx
            .send(PoolEvent::RunCompleted {
                pool_id: self.id.clone(),
                run_id: run_id.clone(),
                outcome: outcome.clone(),
            })
            .await;

        // Find and update round aggregator
        let mut round_id_to_finalize = None;
        {
            let mut aggs = self.round_aggs.lock().await;

            // Find which round this run belongs to
            for (rid, agg) in aggs.iter_mut() {
                if agg.baseline_run_id == run_id {
                    agg.baseline = Some(outcome.clone());
                    if agg.is_complete() {
                        round_id_to_finalize = Some(rid.clone());
                    }
                    break;
                } else if agg.instrumented_run_id == run_id {
                    agg.instrumented = Some(outcome.clone());
                    if agg.is_complete() {
                        round_id_to_finalize = Some(rid.clone());
                    }
                    break;
                }
            }
        }

        // Finalize round if complete
        if let Some(round_id) = round_id_to_finalize {
            self.finalize_round(&round_id).await;
        }
    }

    /// Finalize a completed round.
    async fn finalize_round(&self, round_id: &RoundId) {
        let agg = {
            let mut aggs = self.round_aggs.lock().await;
            aggs.remove(round_id)
        };

        let agg = match agg {
            Some(a) => a,
            None => return,
        };

        let summary = match agg.to_summary() {
            Some(s) => s,
            None => return,
        };

        info!(
            "[Pool:{}] Round {} complete: detected={}, evasion={:.2}",
            self.id, round_id, summary.detected, summary.evasion_score
        );

        self.metrics.lock().await.total_rounds_completed += 1;

        // Emit event
        let _ = self
            .event_tx
            .send(PoolEvent::RoundCompleted {
                pool_id: self.id.clone(),
                round_id: round_id.clone(),
                summary: summary.clone(),
            })
            .await;

        // Record in job session
        {
            let mut job_guard = self.active_job.write().await;
            if let Some(ref mut job) = *job_guard {
                job.record_round_summary(summary);
            }
        }

        // Check if job is complete
        self.check_job_completion().await;
    }

    /// Check if current job is complete and handle transition.
    async fn check_job_completion(&self) {
        let should_complete = {
            let job_guard = self.active_job.read().await;
            let pool_size = self.run_pool.lock().await.len();
            let agg_count = self.round_aggs.lock().await.len();

            if let Some(ref job) = *job_guard {
                !job.should_continue() && pool_size == 0 && agg_count == 0
            } else {
                false
            }
        };

        if should_complete {
            let job = self.active_job.write().await.take();
            if let Some(job) = job {
                let outcome = JobOutcome::Completed {
                    rounds_completed: job.current_round,
                };

                info!(
                    "[Pool:{}] Job {} completed: {:?}",
                    self.id, job.id, outcome
                );

                self.metrics.lock().await.total_jobs_completed += 1;

                // Emit event
                let _ = self
                    .event_tx
                    .send(PoolEvent::JobCompleted {
                        pool_id: self.id.clone(),
                        job_id: job.id.clone(),
                        outcome,
                    })
                    .await;

                // Start next pending job if any
                self.start_next_job().await;
            }
        }
    }

    /// Start next pending job.
    async fn start_next_job(&self) {
        let next_job = self.pending_jobs.lock().await.pop_front();
        if let Some(job) = next_job {
            info!("[Pool:{}] Starting next job: {}", self.id, job.id);
            *self.active_job.write().await = Some(job);
            // Signal producer
            self.runs_available.notify_waiters();
        }
    }

    // ========================================================================
    // Producer Task
    // ========================================================================

    /// Run the producer task (builds artifacts, creates runs).
    ///
    /// This should be spawned as a separate tokio task.
    pub async fn run_producer(self: Arc<Self>) {
        info!("[Pool:{}] Producer task started", self.id);

        let mut production_interval = interval(Duration::from_millis(PRODUCTION_INTERVAL_MS));

        loop {
            tokio::select! {
                biased;

                // Shutdown signal
                _ = self.shutdown.notified() => {
                    info!("[Pool:{}] Producer task shutting down", self.id);
                    break;
                }

                // Production tick (polling OK for heavy operation)
                _ = production_interval.tick() => {
                    if self.can_produce_rounds().await {
                        if let Err(e) = self.produce_round().await {
                            error!("[Pool:{}] Failed to produce round: {}", self.id, e);
                        }
                    }
                }
            }
        }

        info!("[Pool:{}] Producer task stopped", self.id);
    }

    /// Check if we should produce more rounds.
    async fn can_produce_rounds(&self) -> bool {
        // Must have active job
        let job_guard = self.active_job.read().await;
        let job = match &*job_guard {
            Some(j) => j,
            None => return false,
        };

        // Job must want more rounds
        if !job.should_continue() {
            return false;
        }

        drop(job_guard);

        // Pool not full
        if self.run_pool.lock().await.len() >= MAX_POOL_SIZE {
            return false;
        }

        // Not too many in-flight rounds
        if self.round_aggs.lock().await.len() >= MAX_IN_FLIGHT_ROUNDS {
            return false;
        }

        true
    }

    /// Produce a new round (build artifacts, create runs).
    async fn produce_round(&self) -> anyhow::Result<()> {
        // Extract data from job first (to avoid long lock hold)
        let (round_num, round_id, job_id, build_spec) = {
            let mut job_guard = self.active_job.write().await;
            let job = match &mut *job_guard {
                Some(j) => j,
                None => return Ok(()),
            };

            let (round_num, round_id) = job.start_round();
            info!("[Pool:{}][{}] Producing round {}", self.id, job.id, round_num);

            (round_num, round_id, job.id.clone(), job.build_spec.clone())
        };

        // Create round spec
        let spec = RoundSpec {
            id: round_id.clone(),
            job_id: job_id.clone(),
            round_number: round_num,
            mutations: vec![], // TODO: integrate with mutation selector
        };

        // Build baseline artifact (trace_mode = off)
        let baseline_built = self
            .build_artifact(&build_spec, "off", &spec)
            .await
            .map_err(|e| {
                error!("[Pool:{}] Failed to build baseline artifact: {}", self.id, e);
                e
            })?;

        // Build instrumented artifact (trace_mode = lines)
        let instrumented_built = self
            .build_artifact(&build_spec, "lines", &spec)
            .await
            .map_err(|e| {
                error!(
                    "[Pool:{}] Failed to build instrumented artifact: {}",
                    self.id, e
                );
                e
            })?;

        // Create run envelopes
        let baseline_run = RunEnvelope {
            run_id: RunId(format!("{}-baseline", round_id.0)),
            job_id: job_id.clone(),
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
            job_id: job_id.clone(),
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

        {
            self.round_aggs.lock().await.insert(round_id.clone(), agg);
        }

        info!(
            "[Pool:{}] Built runs for round {} (baseline={}, instrumented={})",
            self.id,
            round_id,
            baseline_built.artifact_id,
            instrumented_built.artifact_id
        );

        // Add to pool
        self.add_runs(vec![baseline_run, instrumented_run]).await;

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
        let payload = tokio::fs::read(&build_spec.payload_path).await.map_err(|e| {
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
        let encoding =
            EncodingType::from_str(&build_spec.encoding).unwrap_or(EncodingType::Xor);

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
            "[Pool:{}] Building artifact (carrier={}, decoder={}, encoding={}, trace_mode={})",
            self.id, modules.carrier, modules.decoder, build_spec.encoding, trace_mode
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

        info!(
            "[Pool:{}] Build complete: artifact_id={}, size={} bytes",
            self.id, built.artifact_id, built.size_bytes
        );

        Ok(built)
    }

    // ========================================================================
    // Lifecycle (public API for graceful shutdown and monitoring)
    // ========================================================================

    /// Signal shutdown to producer task.
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// Get current metrics.
    #[allow(dead_code)]
    pub async fn get_metrics(&self) -> PoolMetrics {
        let m = self.metrics.lock().await;
        PoolMetrics {
            total_runs_dispatched: m.total_runs_dispatched,
            total_runs_completed: m.total_runs_completed,
            total_rounds_completed: m.total_rounds_completed,
            total_jobs_completed: m.total_jobs_completed,
        }
    }
}

impl std::fmt::Debug for PoolGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolGroup")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{ModuleSelectionSpec, WorkerId, WorkerInfo};
    use std::path::PathBuf;

    fn test_build_spec() -> ModularBuildSpec {
        ModularBuildSpec {
            modules: ModuleSelectionSpec::default(),
            payload_path: PathBuf::from("test.bin"),
            encoding: "xor".to_string(),
        }
    }

    #[tokio::test]
    async fn test_pool_group_creation() {
        let (tx, _rx) = tokio::sync::mpsc::channel(100);
        let group_id = GroupId::new("windows-defender");
        let pool = PoolGroup::new(group_id.clone(), tx);

        assert_eq!(pool.group_id().as_str(), "windows-defender");
        assert!(pool.has_capacity().await);
        assert_eq!(pool.pool_size().await, 0);
    }

    #[tokio::test]
    async fn test_run_pool_signal() {
        let (tx, _rx) = tokio::sync::mpsc::channel(100);
        let group_id = GroupId::new("windows-defender");
        let pool = Arc::new(PoolGroup::new(group_id, tx));

        // Create a test run
        let run = RunEnvelope {
            run_id: RunId("test-run".into()),
            job_id: JobId("test-job".into()),
            round_id: RoundId("test-round".into()),
            round_number: 1,
            run_type: RunType::Baseline,
            artifact: ArtifactRef {
                path: PathBuf::from("test.exe"),
                sha256: Some("abc123".into()),
            },
            mutations: vec![],
            timeout_seconds: 120,
        };

        // Add run
        pool.add_runs(vec![run]).await;
        assert_eq!(pool.pool_size().await, 1);

        // Take run
        let taken = pool.try_take_run().await;
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().run_id.0, "test-run");
        assert_eq!(pool.pool_size().await, 0);

        // Pool is empty
        assert!(pool.try_take_run().await.is_none());
    }

    #[tokio::test]
    async fn test_job_assignment() {
        let (tx, _rx) = tokio::sync::mpsc::channel(100);
        let group_id = GroupId::new("windows-defender");
        let pool = PoolGroup::new(group_id, tx);

        let job = JobSession::new("job-1", 5, test_build_spec());

        assert!(pool.has_capacity().await);
        pool.assign_job(job).await;
        assert!(!pool.has_capacity().await);
        assert_eq!(
            pool.current_job_id().await.map(|j| j.0),
            Some("job-1".to_string())
        );
    }
}

// ============================================================================
// PoolGroupRegistry - Shared registry for pool groups
// ============================================================================

use super::types::WorkerInfo;
use tokio::sync::mpsc as tokio_mpsc;

/// Shared registry for pool groups.
///
/// Both TargetManager and Orchestrator reference this to manage pool groups.
/// Thread-safe via RwLock.
pub struct PoolGroupRegistry {
    groups: RwLock<HashMap<GroupId, Arc<PoolGroup>>>,
    event_tx: tokio_mpsc::Sender<PoolEvent>,
}

impl PoolGroupRegistry {
    /// Create a new registry.
    pub fn new(event_tx: tokio_mpsc::Sender<PoolEvent>) -> Self {
        Self {
            groups: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    /// Get or create a pool group for the given worker info.
    pub async fn get_or_create(&self, info: &WorkerInfo) -> Arc<PoolGroup> {
        let group_id = GroupId::from_worker_info(info);
        self.get_or_create_by_id(group_id, &info.os, &info.capabilities).await
    }

    /// Get or create a pool group by ID.
    pub async fn get_or_create_by_id(
        &self,
        group_id: GroupId,
        os: &str,
        capabilities: &[String],
    ) -> Arc<PoolGroup> {
        // Check if exists (read lock)
        {
            let groups = self.groups.read().await;
            if let Some(pool) = groups.get(&group_id) {
                return Arc::clone(pool);
            }
        }

        // Create new (write lock)
        let mut groups = self.groups.write().await;

        // Double-check after acquiring write lock
        if let Some(pool) = groups.get(&group_id) {
            return Arc::clone(pool);
        }

        info!(
            "Creating pool group: {} (os={}, caps={:?})",
            group_id, os, capabilities
        );

        let pool = Arc::new(PoolGroup::new(group_id.clone(), self.event_tx.clone()));

        // Spawn producer task
        let pool_clone = Arc::clone(&pool);
        tokio::spawn(async move {
            pool_clone.run_producer().await;
        });

        groups.insert(group_id, Arc::clone(&pool));
        pool
    }

    /// Get a pool group by ID.
    pub async fn get(&self, group_id: &GroupId) -> Option<Arc<PoolGroup>> {
        self.groups.read().await.get(group_id).cloned()
    }

    /// Find a pool group that satisfies the given requirements.
    pub async fn find_compatible(
        &self,
        target_os: Option<&str>,
        required_capabilities: &[String],
    ) -> Option<Arc<PoolGroup>> {
        let groups = self.groups.read().await;
        for (group_id, pool) in groups.iter() {
            if group_id.satisfies(target_os, required_capabilities) {
                return Some(Arc::clone(pool));
            }
        }
        None
    }

    /// List all group IDs.
    #[allow(dead_code)]
    pub async fn list_ids(&self) -> Vec<GroupId> {
        self.groups.read().await.keys().cloned().collect()
    }

    /// Get count of pool groups.
    #[allow(dead_code)]
    pub async fn count(&self) -> usize {
        self.groups.read().await.len()
    }

    /// Shutdown all pool groups.
    #[allow(dead_code)]
    pub async fn shutdown_all(&self) {
        let groups = self.groups.read().await;
        for pool in groups.values() {
            pool.shutdown();
        }
    }
}

impl std::fmt::Debug for PoolGroupRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolGroupRegistry").finish_non_exhaustive()
    }
}
