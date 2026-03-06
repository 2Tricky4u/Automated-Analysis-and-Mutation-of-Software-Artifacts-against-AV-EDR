//! Worker agent library for the AutoMutate++ EDR evaluation framework.
//!
//! Exposes the core [`WorkerAgentService`] and all internal modules for
//! integration testing and reuse. The agent runs on Windows worker VMs and
//! provides gRPC endpoints for artifact reception, monitored execution,
//! telemetry collection, and detection outcome classification.

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

pub mod api;
pub mod capabilities;
pub mod constants;
pub mod execution;
pub mod infra;
pub mod session;
pub mod telemetry;

// Re-export WorkerAgentService for use in session and main
use automutate_config::WorkerConfig;
use capabilities::WorkerCapabilities;
use execution::state::ExecutionState;
use std::sync::Arc;
use sysinfo::System;
use tokio::sync::Mutex;

/// Core gRPC service implementation for the worker agent.
///
/// Holds shared state needed by all RPC handlers: worker identity, configuration,
/// the single-execution lock, a cached capability snapshot, and the optional
/// bidirectional [`StreamHandler`](crate::session::stream_handler::StreamHandler)
/// for real-time controller communication.
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
    pub(crate) heartbeat_handle: Arc<tokio::sync::RwLock<Option<tokio::task::JoinHandle<()>>>>,
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

    /// Truncate a long output string by keeping the first and last 400 characters.
    pub fn truncate_middle_output(stdout_output: &str) -> String {
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
