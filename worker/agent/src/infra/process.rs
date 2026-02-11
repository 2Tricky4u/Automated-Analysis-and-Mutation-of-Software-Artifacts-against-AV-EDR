//! Process lifecycle operations — spawn, kill, verify, capture.

use std::path::Path;
use tokio::io::AsyncRead;
use tokio::task::JoinHandle;
use tracing::{error, info};

/// Spawn an artifact process in the given working directory.
/// Returns the Child handle. Stdout/stderr are piped.
pub fn spawn_artifact(
    artifact_path: &Path,
    working_dir: &Path,
) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::new(artifact_path)
        .current_dir(working_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
}

/// Kill a process tree (Windows: taskkill /F /T, then child.kill()).
pub async fn kill_process_tree(child: &mut tokio::process::Child, pid: Option<u32>) {
    #[cfg(target_os = "windows")]
    if let Some(pid) = pid {
        info!("Forcefully killing process tree for PID {}", pid);
        let kill_result = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
        if let Err(e) = kill_result {
            error!("Failed to run taskkill: {}", e);
        }
    }

    let _ = child.kill().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

/// Check if a process is still alive (Windows-specific).
/// Returns true if process still exists.
#[cfg(target_os = "windows")]
pub fn is_process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::OpenProcess;
    unsafe {
        if let Ok(handle) = OpenProcess(
            windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        ) {
            if !handle.is_invalid() {
                let _ = CloseHandle(handle);
                return true;
            }
        }
    }
    false
}

#[cfg(not(target_os = "windows"))]
pub fn is_process_alive(_pid: u32) -> bool {
    false
}

/// Spawn a task to capture an async read stream into a String.
pub fn capture_stream<R>(stream: Option<R>) -> JoinHandle<String>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        if let Some(stream) = stream {
            use tokio::io::AsyncReadExt;
            let mut reader = tokio::io::BufReader::new(stream);
            let mut output = String::new();
            let _ = reader.read_to_string(&mut output).await;
            output
        } else {
            String::new()
        }
    })
}
