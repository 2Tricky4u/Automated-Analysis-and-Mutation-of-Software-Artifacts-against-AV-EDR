/// Worker agent library - exposes modules for testing and reuse
///
/// This allows integration tests to access internal modules like
/// telemetry collectors without duplicating code.
pub mod automutate {
    pub mod common {
        tonic::include_proto!("automutate.common");
    }
    pub mod controller {
        tonic::include_proto!("automutate.controller");
    }
    pub mod worker {
        tonic::include_proto!("automutate.worker");
    }
}

// New module structure
pub mod api;
pub mod capabilities;
pub mod dispatch;
pub mod infra;
pub mod session;
pub mod telemetry;

// Re-export WorkerAgentService for use in session and main
use automutate_config::WorkerConfig;
use capabilities::WorkerCapabilities;
use dispatch::state::ExecutionState;
use std::sync::Arc;
use sysinfo::System;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct WorkerAgentService {
    pub(crate) worker_id: String,
    pub(crate) config: WorkerConfig,
    pub(crate) system_info: Arc<Mutex<System>>,
    /// Single execution lock needed for rededr
    /// This ensures clean telemetry collection with no cross-contamination
    pub(crate) execution_lock: Arc<Mutex<ExecutionState>>,
    /// StreamHandler for bidirectional communication
    pub(crate) stream_handler:
        Arc<tokio::sync::RwLock<Option<Arc<session::stream_handler::StreamHandler>>>>,
    /// Handle to the heartbeat background task (aborted on reconnect)
    pub(crate) heartbeat_handle:
        Arc<tokio::sync::RwLock<Option<tokio::task::JoinHandle<()>>>>,
    /// Cached capabilities detected at startup (expensive I/O, doesn't change at runtime)
    pub(crate) capabilities: Arc<WorkerCapabilities>,
}

impl WorkerAgentService {
    pub fn new(worker_id: String, config: WorkerConfig, capabilities: WorkerCapabilities) -> Self {
        Self {
            worker_id,
            config,
            system_info: Arc::new(Mutex::new(System::new_all())),
            execution_lock: Arc::new(Mutex::new(ExecutionState::Idle)),
            stream_handler: Arc::new(Default::default()),
            heartbeat_handle: Arc::new(Default::default()),
            capabilities: Arc::new(capabilities),
        }
    }

    /// Get current execution state (for health check)
    pub async fn get_execution_state(&self) -> ExecutionState {
        self.execution_lock.lock().await.clone()
    }

    pub fn truncate_middle_output(stdout_output: &String) -> String {
        if stdout_output.len() > 1000 {
            // Show first 400 chars and last 400 chars, truncate middle
            let first_part = &stdout_output[..400];
            let last_part = &stdout_output[stdout_output.len() - 400..];
            format!(
                "{}\n\n... ({} bytes truncated) ...\n\n{}",
                first_part,
                stdout_output.len() - 800,
                last_part
            )
        } else {
            stdout_output.to_owned()
        }
    }
}
