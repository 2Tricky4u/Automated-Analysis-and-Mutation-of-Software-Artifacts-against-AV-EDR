//! Channel types and events for dispatch system.
//!
//! Defines the messages that flow between:
//! - Orchestrator <-> Worker (commands and events)
//! - TargetManager -> Orchestrator (worker lifecycle)

use super::types::{JobId, JobOutcome, JobSession, RunId, RunOutcome, WorkerId, WorkerInfo};
use tokio::sync::mpsc;

// ============================================================================
// Worker <-> Orchestrator
// ============================================================================

/// Commands sent from Orchestrator to Worker
/// TODO: Currently unused - jobs go directly to PoolGroup.
/// TODO: Use Shutdown variant for graceful shutdown (call from disconnect_all)
#[derive(Debug)]
#[allow(dead_code)]
pub enum WorkerCommand {
    /// Assign a job to this worker
    AssignJob(JobSession),
    /// Graceful shutdown
    Shutdown,
}

/// Events sent from Worker to Orchestrator
/// TODO: Worker sends these events but Orchestrator poll_worker_events() is disabled
/// TODO: Enable polling in Orchestrator.run() or remove this channel
#[derive(Debug, Clone)]
#[allow(dead_code)]
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
#[allow(dead_code)]
pub enum OrchestratorEvent {
    /// New worker connected
    /// TODO: cmd_tx and event_rx are passed but never used by Orchestrator
    WorkerConnected {
        worker_id: WorkerId,
        info: WorkerInfo,
        cmd_tx: mpsc::Sender<WorkerCommand>,
        event_rx: mpsc::Receiver<WorkerEvent>,
    },
    /// Worker disconnected
    /// TODO: TargetManager never sends this - add to stream_handler on disconnect
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