//! System-level operations — telemetry directory management.

use std::path::Path;
use tracing::{info, warn};

/// Read CPU and memory percent from an already-refreshed System.
///
/// Uses `global_cpu_usage()` for consistency across all call sites.
/// Guards against div-by-zero on total_memory.
pub fn collect_system_metrics(sys: &sysinfo::System) -> (i32, i32) {
    let cpu_percent = sys.global_cpu_usage() as i32;
    let total = sys.total_memory();
    let memory_percent = if total > 0 {
        ((sys.used_memory() as f64 / total as f64) * 100.0) as i32
    } else {
        0
    };
    (cpu_percent, memory_percent)
}

/// Remove artifact exe and telemetry directory after a completed run.
pub fn cleanup_run_artifacts(artifact_path: &Path, telemetry_dir: &Path) {
    if artifact_path.exists() {
        match std::fs::remove_file(artifact_path) {
            Ok(_) => info!("Cleaned up artifact: {:?}", artifact_path),
            Err(e) => warn!("Failed to clean artifact {:?}: {}", artifact_path, e),
        }
    }
    if telemetry_dir.exists() {
        match std::fs::remove_dir_all(telemetry_dir) {
            Ok(_) => info!("Cleaned up telemetry dir: {:?}", telemetry_dir),
            Err(e) => warn!("Failed to clean telemetry dir {:?}: {}", telemetry_dir, e),
        }
    }
}

/// Create (or recreate) a telemetry directory for a run.
/// Removes stale files from previous runs if the directory exists.
pub fn prepare_telemetry_dir(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
    std::fs::create_dir_all(dir)?;
    info!("Created telemetry directory: {:?}", dir);
    Ok(())
}
