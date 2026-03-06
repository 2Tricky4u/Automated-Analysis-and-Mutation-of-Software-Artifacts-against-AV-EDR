//! Channel message types for the dispatch system.
//!
//! Defines the typed messages exchanged between the gRPC service layer,
//! the orchestrator, job workers, and VM executors.

use std::time::SystemTime;

use super::types::{
    JobId, JobOutcome, ModuleSelectionSpec, MutationSpec, RoundId, RoundSummary, RunId, RunOutcome,
};

// ============================================================================
// Service -> Orchestrator (Job Control Commands)
// ============================================================================

/// Commands sent from Service to Orchestrator for job control.
#[derive(Debug, Clone)]
pub enum JobControlCommand {
    /// Stop a running job
    Stop { job_id: JobId },
}

// ============================================================================
// Remote execution results (stream -> VMExecutor)
// ============================================================================

/// Result from remote VM execution
#[derive(Debug, Clone)]
pub struct RemoteRunResult {
    pub run_id: RunId,
    pub detected: bool,
    pub exit_code: i32,
    pub success: bool,
    pub error: Option<String>,
    pub elapsed_ms: f64,
    /// Fine-grained classifier verdict (e.g. "killed_pre_payload"), empty for legacy workers
    pub detection_verdict: String,
    /// Last checkpoint reached before exit (e.g. "Launching")
    pub last_checkpoint: String,
}

impl From<RemoteRunResult> for RunOutcome {
    fn from(r: RemoteRunResult) -> Self {
        RunOutcome {
            detected: r.detected,
            exit_code: r.exit_code,
            error: r.error,
            success: r.success,
            elapsed_ms: r.elapsed_ms,
            detection_verdict: r.detection_verdict,
            last_checkpoint: r.last_checkpoint,
        }
    }
}

// ============================================================================
// JobWorker -> Orchestrator (Job Lifecycle Events)
// ============================================================================

/// All data from a completed round, boxed to keep enum size small.
#[derive(Debug, Clone)]
pub struct RoundCompletedData {
    pub job_id: JobId,
    pub round_id: RoundId,
    pub summary: RoundSummary,
    pub baseline_run_id: RunId,
    pub instrumented_run_id: RunId,
    pub baseline_outcome: RunOutcome,
    pub instrumented_outcome: RunOutcome,
    pub mutation_specs: Vec<MutationSpec>,
    pub mutations: Vec<String>,
    pub modules: ModuleSelectionSpec,
    pub baseline_vm_id: String,
    pub instrumented_vm_id: String,
    pub round_started_at: SystemTime,
    /// Pre-instrumentation assembled C source for line trace resolution.
    pub assembled_source: Option<String>,
    // --- Dryrun fields ---
    pub dryrun_run_id: Option<RunId>,
    pub dryrun_outcome: Option<RunOutcome>,
    pub dryrun_vm_id: String,
}

/// Events emitted by JobWorker to Orchestrator for tracking and ES indexing.
#[derive(Debug, Clone)]
pub enum JobWorkerEvent {
    /// A round completed (both baseline and instrumented runs done).
    /// Carries all data needed for round AND run indexing.
    RoundCompleted(Box<RoundCompletedData>),
    /// Job completed (all rounds done or stopped early)
    JobCompleted { job_id: JobId, outcome: JobOutcome },
}

// ============================================================================
// Orchestrator -> JobWorker (Async Coverage Correction)
// ============================================================================

/// Sent from Orchestrator background task to JobWorker after coverage
/// computation completes, so the selector sees blended evasion scores.
#[derive(Debug, Clone)]
pub struct CoverageCorrection {
    pub round_number: u32,
    pub coverage_percent: f64,
    pub blended_evasion_score: f64,
}

// ============================================================================
// VMExecutor -> RunPool -> JobWorker (Run Results)
// ============================================================================

/// Result routed from VMExecutor back to JobWorker via RunPool.
/// Contains all info needed to correlate result with originating job/round.
#[derive(Debug, Clone)]
pub struct JobRunResult {
    pub run_id: RunId,
    pub job_id: JobId,
    pub round_id: RoundId,
    pub outcome: RunOutcome,
    pub vm_id: String,
}
