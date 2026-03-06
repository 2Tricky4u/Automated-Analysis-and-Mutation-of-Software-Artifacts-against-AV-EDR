//! Worker runtime state and health metrics.
//!
//! Tracks mutable session state (current job, health, controller connectivity)
//! separately from the immutable capability detection in [`crate::capabilities`].
use std::collections::HashMap;

use crate::automutate::common::ToolVersions;
use crate::capabilities::WorkerCapabilities;

/// Mutable runtime state for the
/// [`StreamHandler`](crate::session::stream_handler::StreamHandler).
///
/// Updated as the worker processes jobs and receives heartbeats from the controller.
#[derive(Debug, Clone)]
pub struct WorkerState {
    /// Unique worker identifier.
    pub worker_id: String,
    /// Capability tags reported to the controller (e.g. `"rededr"`, `"mde"`).
    pub capabilities: Vec<String>,
    /// Host metadata (hostname, cpu_cores, ram_gb, os_build, etc.).
    pub metadata: HashMap<String, String>,
    /// Detected tool versions (RedEDR, Defender, ETW, LLVM).
    pub tools: Option<ToolVersions>,
    /// Latest health metrics snapshot.
    pub health: HealthMetrics,
    /// Currently executing job, if any.
    pub current_job_id: Option<String>,
    /// Currently executing run, if any.
    pub current_run_id: Option<String>,
    /// Timestamp (epoch millis) of the last heartbeat received from the controller.
    pub last_controller_heartbeat: Option<i64>,
    /// `true` if the controller sent a `DisconnectNotice`.
    pub controller_disconnected: bool,
    /// Reason string from the controller's disconnect notice.
    pub disconnect_reason: Option<String>,
    /// Whether the controller allows reconnection.
    pub reconnect_allowed: bool,
}

/// Health metrics for the worker.
#[derive(Debug, Clone, Default)]
pub struct HealthMetrics {
    /// CPU usage as a percentage (0–100).
    pub cpu_percent: i32,
    /// Memory usage as a percentage (0–100).
    pub memory_percent: i32,
    /// Disk usage as a percentage (0–100).
    pub disk_percent: i32,
    /// Number of currently active jobs (0 or 1).
    pub active_jobs: i32,
    /// Uptime in seconds since the worker process started.
    pub uptime_seconds: i64,
}

impl WorkerState {
    /// Create new worker state from config and detected capabilities
    pub fn new(worker_id: String, capabilities: WorkerCapabilities) -> Self {
        let tools = Some(capabilities.to_tool_versions());

        WorkerState {
            worker_id,
            capabilities: capabilities.capabilities,
            metadata: capabilities.metadata,
            tools,
            health: HealthMetrics::default(),
            current_job_id: None,
            current_run_id: None,
            last_controller_heartbeat: None,
            controller_disconnected: false,
            disconnect_reason: None,
            reconnect_allowed: true,
        }
    }

    /// Update health metrics
    pub fn update_health(&mut self) {
        use sysinfo::{CpuRefreshKind, System};
        let mut sys = System::new_all();
        sys.refresh_all();
        sys.refresh_cpu_specifics(CpuRefreshKind::everything());

        let (cpu_percent, memory_percent) = crate::infra::system::collect_system_metrics(&sys);
        self.health.cpu_percent = cpu_percent;
        self.health.memory_percent = memory_percent;
        self.health.active_jobs = if self.current_job_id.is_some() { 1 } else { 0 };
    }
}
