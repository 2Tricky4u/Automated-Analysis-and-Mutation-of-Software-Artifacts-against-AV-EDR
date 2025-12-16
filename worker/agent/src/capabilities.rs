/// Capability detection for worker self-registration
use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct WorkerCapabilities {
    pub capabilities: Vec<String>,
    pub tools: HashMap<String, String>,
    pub metadata: HashMap<String, String>,
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
        match Command::new("sc")
            .args(&["query", "WinDefend"])
            .output()
        {
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
    // Use std::thread::available_parallelism (available in Rust 1.59+)
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
