//! Worker runtime state and health metrics.
//!
//! Tracks mutable session state (current job, health, controller connectivity)
//! separately from the immutable capability detection in [`crate::capabilities`].
use std::collections::HashMap;

use crate::automutate::common::ToolVersions;
use crate::capabilities::WorkerCapabilities;

/// Worker state for stream handler
#[derive(Debug, Clone)]
pub struct WorkerState {
    pub worker_id: String,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub tools: Option<ToolVersions>,
    pub health: HealthMetrics,
    pub current_job_id: Option<String>,
    pub current_run_id: Option<String>,
    pub last_controller_heartbeat: Option<i64>,
    pub controller_disconnected: bool,
    pub disconnect_reason: Option<String>,
    pub reconnect_allowed: bool,
}

/// Health metrics for worker
#[derive(Debug, Clone, Default)]
pub struct HealthMetrics {
    pub cpu_percent: i32,
    pub memory_percent: i32,
    pub disk_percent: i32,
    pub active_jobs: i32,
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
