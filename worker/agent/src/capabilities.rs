//! Capability detection for worker self-registration.
//!
//! Probes the host environment at startup to identify available security
//! tools (RedEDR, Defender, MDE, Cortex XDR) and system metadata (OS build,
//! CPU cores, RAM). Results are cached in [`WorkerCapabilities`] and reported
//! to the controller during stream registration.

use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use tracing::debug;

/// Detected capabilities of this worker host.
///
/// Populated once at startup by [`detect_capabilities`] and cached for the
/// lifetime of the process. Reported to the controller during stream registration.
#[derive(Debug, Clone)]
pub struct WorkerCapabilities {
    /// Capability tags (e.g. `"rededr"`, `"mde"`, `"cortex"`, `"dryrun"`).
    pub capabilities: Vec<String>,
    /// Tool name-to-version map (e.g. `"rededr_version" => "0.7.2"`).
    pub tools: HashMap<String, String>,
    /// Freeform host metadata (hostname, cpu_cores, ram_gb, os_build, os_key).
    pub metadata: HashMap<String, String>,
}

/// Parsed Windows version metadata from the registry.
///
/// Read from `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
/// Used to populate the `os_key` metadata field (e.g. `"win11-build-22631"`).
#[derive(Debug, Clone)]
pub struct WindowsVersionInfo {
    /// e.g. `"Windows 10 Pro"` or `"Windows 11 Enterprise"`.
    pub product_name: Option<String>,
    /// e.g. `"Professional"`, `"Enterprise"`.
    pub edition_id: Option<String>,
    /// e.g. `"23H2"`.
    pub display_version: Option<String>,
    /// Legacy release identifier (e.g. `"2009"`).
    pub release_id: Option<String>,
    /// `CurrentBuildNumber` (e.g. `22631`).
    pub build: Option<u32>,
    /// Update Build Revision (minor patch level).
    pub ubr: Option<u32>,
    /// `true` when `build >= 22000`.
    pub is_windows_11: Option<bool>,
}

impl WorkerCapabilities {
    /// Convert the tools HashMap into the proto ToolVersions struct.
    pub fn to_tool_versions(&self) -> crate::automutate::common::ToolVersions {
        crate::automutate::common::ToolVersions {
            rededr_version: self
                .tools
                .get("rededr_version")
                .cloned()
                .unwrap_or_default(),
            defender_version: self
                .tools
                .get("defender_version")
                .cloned()
                .unwrap_or_default(),
            etw_version: self.tools.get("etw_version").cloned().unwrap_or_default(),
            llvm_version: self.tools.get("llvm_version").cloned().unwrap_or_default(),
        }
    }
}

/// Detect worker capabilities by checking for installed tools.
///
/// # Errors
///
/// Returns an error if the HTTP client for RedEDR probing cannot be built.
pub async fn detect_capabilities() -> Result<WorkerCapabilities> {
    let mut capabilities = Vec::new();
    let mut tools = HashMap::new();
    let mut metadata = HashMap::new();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;

    // Check for RedEDR
    if check_rededr_available(&client).await {
        capabilities.push("rededr".to_string());
        if let Some(version) = get_rededr_version(&client).await {
            tools.insert("rededr_version".to_string(), version);
        }
    }

    // Check for Windows Defender
    if check_defender_available() {
        //capabilities.push("defender".to_string());
        if let Some(version) = get_defender_version() {
            tools.insert("defender_version".to_string(), version);
        }
    }
    // Check for MDE
    if is_mde_onboarded() {
        capabilities.push("mde".to_string());
    }
    if is_cortex_xdr_present() {
        capabilities.push("cortex".to_string());
    }
    // System metadata
    metadata.insert("hostname".to_string(), get_hostname());
    metadata.insert("cpu_cores".to_string(), get_cpu_cores().to_string());
    metadata.insert("ram_gb".to_string(), get_total_ram_gb().to_string());

    #[cfg(windows)]
    {
        let w = get_windows_version_info();
        let is_win11 = w.is_windows_11.unwrap_or(false);
        let build = w.build.unwrap_or(0);

        metadata.insert(
            "os_key".to_string(),
            format!("win{}-build-{}", if is_win11 { 11 } else { 10 }, build),
        );
        metadata.insert("os_build".to_string(), build.to_string());
    }

    debug!("Detected capabilities: {:?}", capabilities);
    debug!("Detected tools: {:?}", tools);

    Ok(WorkerCapabilities {
        capabilities,
        tools,
        metadata,
    })
}

async fn check_rededr_available(client: &reqwest::Client) -> bool {
    match client.get("http://localhost:8081/api/stats").send().await {
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

async fn get_rededr_version(client: &reqwest::Client) -> Option<String> {
    let re = Regex::new(r"RedEdr\s+([0-9]+(?:\.[0-9]+)*)").ok()?;

    let resp = client
        .get("http://localhost:8081/api/logs/agent")
        .send()
        .await
        .ok()?;

    let logs: Vec<String> = resp.json().await.ok()?;

    logs.iter()
        .find_map(|l| re.captures(l).and_then(|c| c.get(1)))
        .map(|m| m.as_str().to_string())
}

fn check_defender_available() -> bool {
    // Check if Windows Defender service is running
    #[cfg(windows)]
    {
        use std::process::Command;
        match Command::new("sc").args(["query", "WinDefend"]).output() {
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

#[cfg(windows)]
fn is_mde_onboarded() -> bool {
    use winreg::RegKey;
    use winreg::RegValue;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    let key = match hklm.open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows Advanced Threat Protection",
        KEY_READ,
    ) {
        Ok(k) => k,
        Err(_) => return false,
    };

    // OnboardingInfo is commonly REG_BINARY; read bytes and ensure non-empty.
    let value: RegValue = match key.get_raw_value("OnboardedInfo") {
        Ok(v) => v,
        Err(_) => return false,
    };

    !value.bytes.is_empty()
}

#[cfg(not(windows))]
fn is_mde_onboarded() -> bool {
    false
}

#[cfg(windows)]
fn is_cortex_xdr_installed() -> bool {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    // Service registration is a strong indicator of installation.
    hklm.open_subkey_with_flags(r"SYSTEM\CurrentControlSet\Services\CyveraService", KEY_READ)
        .is_ok()
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn is_cortex_xdr_installed() -> bool {
    false
}

#[cfg(windows)]
fn is_cortex_xdr_footprint_present() -> bool {
    use std::path::Path;

    // High-signal data directory used by the agent
    let cyvera_data = Path::new(r"C:\ProgramData\Cyvera");

    // Legacy install location often seen in the field (Traps/Cortex lineage)
    let traps_install = Path::new(r"C:\Program Files\Palo Alto Networks\Traps");

    cyvera_data.exists() || traps_install.exists()
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn is_cortex_xdr_footprint_present() -> bool {
    false
}

#[cfg(windows)]
fn is_cortex_xdr_present() -> bool {
    is_cortex_xdr_installed() || is_cortex_xdr_footprint_present()
}

#[cfg(not(windows))]
fn is_cortex_xdr_present() -> bool {
    false
}

#[cfg(windows)]
/// Read Windows version details from the registry.
pub fn get_windows_version_info() -> WindowsVersionInfo {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = match hklm
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", KEY_READ)
    {
        Ok(k) => k,
        Err(_) => {
            return WindowsVersionInfo {
                product_name: None,
                edition_id: None,
                display_version: None,
                release_id: None,
                build: None,
                ubr: None,
                is_windows_11: None,
            };
        }
    };

    let product_name: Option<String> = key.get_value("ProductName").ok();
    let edition_id: Option<String> = key.get_value("EditionID").ok();
    let display_version: Option<String> = key.get_value("DisplayVersion").ok();
    let release_id: Option<String> = key.get_value("ReleaseId").ok();

    // CurrentBuildNumber is stored as a string
    let build: Option<u32> = key
        .get_value::<String, _>("CurrentBuildNumber")
        .ok()
        .and_then(|s| s.parse::<u32>().ok());

    // UBR is a DWORD
    let ubr: Option<u32> = key.get_value("UBR").ok();

    // Practical rule: Windows 11 builds start at 22000+
    let is_windows_11 = build.map(|b| b >= 22000);

    WindowsVersionInfo {
        product_name,
        edition_id,
        display_version,
        release_id,
        build,
        ubr,
        is_windows_11,
    }
}

#[cfg(not(windows))]
/// Stub — returns empty version info on non-Windows platforms.
pub fn get_windows_version_info() -> WindowsVersionInfo {
    WindowsVersionInfo {
        product_name: None,
        edition_id: None,
        display_version: None,
        release_id: None,
        build: None,
        ubr: None,
        is_windows_11: None,
    }
}

fn get_defender_version() -> Option<String> {
    #[cfg(windows)]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("powershell")
            .args([
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
