//! Channel types and events for dispatch system.
//!
//! Defines the messages that flow between:
//! - Worker -> Orchestrator (events via aggregated bus)
//! - TargetManager -> Orchestrator (worker lifecycle)
//!
//! Graceful shutdown is handled via CancellationToken in PoolGroup,
//! not per-worker command channels.

use super::types::{JobId, JobOutcome, RunId, RunOutcome, WorkerId, WorkerInfo};

// ============================================================================
// Worker -> Orchestrator (Aggregated Event Bus)
// ============================================================================

/// Events sent from Worker to Orchestrator via shared event bus.
/// All workers send to a single aggregated channel for O(1) receive.
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// Worker is idle and can accept a job
    Available { worker_id: WorkerId },
    /// Job completed (success or failure) - unused in pool architecture
    JobCompleted {
        worker_id: WorkerId,
        job_id: JobId,
        outcome: JobOutcome,
    },
    /// Individual run completed (for observability)
    RunCompleted {
        worker_id: WorkerId,
        run_id: RunId,
        outcome: RunOutcome,
    },
}

// ============================================================================
// TargetManager -> Orchestrator
// ============================================================================

/// Events sent from TargetManager to Orchestrator
#[derive(Debug)]
pub enum OrchestratorEvent {
    /// New worker connected
    WorkerConnected {
        worker_id: WorkerId,
        info: WorkerInfo,
    },
    /// Worker disconnected
    WorkerDisconnected {
        worker_id: WorkerId,
        reason: String,
    },
}

// ============================================================================
// Remote execution results (stream -> Worker)
// ============================================================================

/// Result from remote VM execution
#[derive(Debug, Clone)]
#[allow(dead_code)] // success field kept for protocol compatibility
pub struct RemoteRunResult {
    pub run_id: RunId,
    pub detected: bool,
    pub exit_code: i32,
    pub success: bool,
    pub error: Option<String>,
}

impl From<RemoteRunResult> for RunOutcome {
    fn from(r: RemoteRunResult) -> Self {
        RunOutcome {
            detected: r.detected,
            exit_code: r.exit_code,
            error: r.error,
        }
    }
}