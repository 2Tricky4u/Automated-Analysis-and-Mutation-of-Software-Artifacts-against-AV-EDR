/// Capability detection for worker self-registration
use anyhow::Result;
use std::collections::HashMap;
use tracing::debug;

// Import protobuf types
use crate::automutate::common::ToolVersions;

#[derive(Debug, Clone)]
pub struct WorkerCapabilities {
    pub capabilities: Vec<String>,
    pub tools: HashMap<String, String>,
    pub metadata: HashMap<String, String>,
}

/// Worker state for stream handler (Phase 2)
#[derive(Debug, Clone)]
pub struct WorkerState {
    pub worker_id: String,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub tools: Option<ToolVersions>,
    pub health: HealthMetrics,
    pub current_job_id: Option<String>,
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

        // Calculate average CPU usage across all cores
        let cpu_percent = if !sys.cpus().is_empty() {
            sys.cpus().iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / sys.cpus().len() as f32
        } else {
            0.0
        };

        self.health.cpu_percent = cpu_percent as i32;
        self.health.memory_percent =
            ((sys.used_memory() as f64 / sys.total_memory() as f64) * 100.0) as i32;
        // Note: disk_percent would require filesystem-specific checks, leaving as 0 for now
        self.health.active_jobs = if self.current_job_id.is_some() { 1 } else { 0 };
        // Uptime tracking would require storing start time, leaving as 0 for now
    }
}

/// Detect worker capabilities by checking for installed tools
pub async fn detect_capabilities() -> Result<WorkerCapabilities> {
    let mut capabilities = Vec::new();
    let mut tools = HashMap::new();
    let mut metadata = HashMap::new();

    // Check for RedEDR
    if check_rededr_available().await {
        capabilities.push("rededr".to_string());
        if let Some(version) = get_rededr_version().await {
            tools.insert("rededr_version".to_string(), version);
        }
    }

    // Check for Windows Defender
    if check_defender_available() {
        capabilities.push("defender".to_string());
        if let Some(version) = get_defender_version() {
            tools.insert("defender_version".to_string(), version);
        }
    }

    // ETW is always available on Windows
    #[cfg(windows)]
    {
        capabilities.push("etw".to_string());
        tools.insert("etw_version".to_string(), "native".to_string());
    }

    // System metadata
    metadata.insert("hostname".to_string(), get_hostname());
    metadata.insert("cpu_cores".to_string(), get_cpu_cores().to_string());
    metadata.insert("ram_gb".to_string(), get_total_ram_gb().to_string());

    debug!("Detected capabilities: {:?}", capabilities);
    debug!("Detected tools: {:?}", tools);

    Ok(WorkerCapabilities {
        capabilities,
        tools,
        metadata,
    })
}

async fn check_rededr_available() -> bool {
    // Try to connect to RedEDR API
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();

    match client.get("http://localhost:8081/api/health").send().await {
        Ok(response) if response.status().is_success() => {
            debug!("RedEDR detected at localhost:8081");
            true
        }
        _ => {
            debug!("RedEDR not detected");
            false
        }
    }
}

async fn get_rededr_version() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();

    if let Ok(response) = client.get("http://localhost:8081/api/version").send().await {
        if let Ok(text) = response.text().await {
            return Some(text.trim().to_string());
        }
    }
    None
}

fn check_defender_available() -> bool {
    // Check if Windows Defender service is running
    #[cfg(windows)]
    {
        use std::process::Command;
        match Command::new("sc").args(&["query", "WinDefend"]).output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.contains("RUNNING")
            }
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    false
}

fn get_defender_version() -> Option<String> {
    #[cfg(windows)]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("powershell")
            .args(&[
                "-Command",
                "(Get-MpComputerStatus).AMProductVersion 2>$null",
            ])
            .output()
        {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !version.is_empty() && !version.contains("Exception") {
                return Some(version);
            }
        }
    }
    None
}

fn get_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn get_cpu_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn get_total_ram_gb() -> u64 {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.total_memory() / 1024 / 1024 / 1024 // Convert bytes to GB
}
