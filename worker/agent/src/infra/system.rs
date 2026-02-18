//! System-level operations — telemetry directory management.

use std::path::Path;
use tracing::info;

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
