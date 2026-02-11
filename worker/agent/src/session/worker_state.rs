/// Worker runtime state and health metrics
///
/// Extracted from capabilities.rs to separate runtime state
/// from capability detection logic.
use std::collections::HashMap;
use sysinfo::CpuRefreshKind;

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
        let tools = Some(ToolVersions {
            rededr_version: capabilities
                .tools
                .get("rededr_version")
                .cloned()
                .unwrap_or_default(),
            defender_version: capabilities
                .tools
                .get("defender_version")
                .cloned()
                .unwrap_or_default(),
            etw_version: capabilities
                .tools
                .get("etw_version")
                .cloned()
                .unwrap_or_default(),
            llvm_version: capabilities
                .tools
                .get("llvm_version")
                .cloned()
                .unwrap_or_default(),
        });

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
        use sysinfo::System;
        let mut sys = System::new_all();
        sys.refresh_all();
        sys.refresh_cpu_specifics(CpuRefreshKind::everything());

        let cpu_percent = if !sys.cpus().is_empty() {
            sys.cpus().iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / sys.cpus().len() as f32
        } else {
            0.0
        };

        self.health.cpu_percent = cpu_percent as i32;
        self.health.memory_percent =
            ((sys.used_memory() as f64 / sys.total_memory() as f64) * 100.0) as i32;
        self.health.active_jobs = if self.current_job_id.is_some() { 1 } else { 0 };
    }
}
