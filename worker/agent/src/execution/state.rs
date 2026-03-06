//! Execution lock state machine.
//!
//! Provides [`ExecutionState`], an enum that tracks whether the worker is idle or
//! running an artifact. The single-execution invariant (one run at a time) is
//! enforced by [`ExecutionState::acquire`] / [`ExecutionState::release`].
//! [`ExecutionLockGuard`] wraps the lock in an RAII guard so it is released
//! even on panic or early return.

use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// Execution state for single-job worker.
///
/// Enum variant ensures busy/idle state is always consistent with
/// job metadata (no desync between `busy: bool` and `current_job_id: Option`).
///
/// See [`ExecutionLockGuard`] for RAII-based lock management.
#[derive(Debug, Clone)]
pub enum ExecutionState {
    Idle,
    Running {
        job_id: String,
        artifact: String,
        run_id: String,
    },
}

impl ExecutionState {
    /// Check if the worker is currently executing
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// Get current job_id if running
    pub fn current_job_id(&self) -> Option<&str> {
        match self {
            Self::Running { job_id, .. } => Some(job_id),
            Self::Idle => None,
        }
    }

    /// Get current artifact name if running
    pub fn current_artifact(&self) -> Option<&str> {
        match self {
            Self::Running { artifact, .. } => Some(artifact),
            Self::Idle => None,
        }
    }

    /// Attempt to acquire the execution lock
    pub fn acquire(
        &mut self,
        job_id: String,
        artifact: String,
        run_id: String,
    ) -> Result<(), ExecutionBusyError> {
        match self {
            Self::Running {
                job_id: current_job,
                artifact: current_artifact,
                ..
            } => Err(ExecutionBusyError {
                current_job_id: current_job.clone(),
                current_artifact: current_artifact.clone(),
            }),
            Self::Idle => {
                *self = Self::Running {
                    job_id,
                    artifact,
                    run_id,
                };
                Ok(())
            }
        }
    }

    /// Release the execution lock
    pub fn release(&mut self) -> (String, String) {
        match std::mem::replace(self, Self::Idle) {
            Self::Running {
                job_id, artifact, ..
            } => (job_id, artifact),
            Self::Idle => ("unknown".to_string(), "unknown".to_string()),
        }
    }
}

/// Error returned when [`ExecutionState::acquire`] is called while already running.
#[derive(Debug)]
pub struct ExecutionBusyError {
    /// The `job_id` of the currently running execution.
    pub current_job_id: String,
    /// The artifact name of the currently running execution.
    pub current_artifact: String,
}

impl std::fmt::Display for ExecutionBusyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Worker is busy executing job_id={} artifact={}. This worker supports only ONE concurrent execution.",
            self.current_job_id, self.current_artifact
        )
    }
}

impl std::error::Error for ExecutionBusyError {}

/// RAII guard for the single execution lock.
///
/// Automatically releases the lock on drop. Because `Drop` cannot be async,
/// the release is spawned onto the tokio runtime via `tokio::spawn`.
pub struct ExecutionLockGuard {
    lock: Arc<Mutex<ExecutionState>>,
}

impl ExecutionLockGuard {
    pub fn new(lock: Arc<Mutex<ExecutionState>>) -> Self {
        Self { lock }
    }
}

impl Drop for ExecutionLockGuard {
    fn drop(&mut self) {
        let lock = self.lock.clone();
        tokio::spawn(async move {
            let mut state = lock.lock().await;
            let (job_id, artifact) = state.release();
            info!(
                "Execution lock RELEASED: job_id={}, artifact={}",
                job_id, artifact
            );
        });
    }
}
