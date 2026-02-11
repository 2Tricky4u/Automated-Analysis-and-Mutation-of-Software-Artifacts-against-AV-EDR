//! System-level operations — telemetry directory management.

use std::path::Path;
use tracing::info;

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
