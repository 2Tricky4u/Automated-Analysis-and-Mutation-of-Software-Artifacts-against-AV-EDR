/// Line-level trace collector via named pipe (Lepori 2023-inspired)
///
/// Listens on \\.\pipe\rededr_trace for Base64-encoded trace events
/// from instrumented artifacts. Decodes and streams to controller via gRPC.
use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use std::io::{BufRead, BufReader};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[cfg(windows)]
use windows::Win32::System::Pipes::{CreateNamedPipeA, NAMED_PIPE_MODE};
#[cfg(windows)]
use windows::core::PCSTR;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;

// Named pipe constants (from winbase.h)
#[cfg(windows)]
const PIPE_ACCESS_INBOUND: FILE_FLAGS_AND_ATTRIBUTES = FILE_FLAGS_AND_ATTRIBUTES(0x00000001);
#[cfg(windows)]
const PIPE_TYPE_BYTE: NAMED_PIPE_MODE = NAMED_PIPE_MODE(0x00000000);
#[cfg(windows)]
const PIPE_READMODE_BYTE: NAMED_PIPE_MODE = NAMED_PIPE_MODE(0x00000000);
#[cfg(windows)]
const PIPE_WAIT: NAMED_PIPE_MODE = NAMED_PIPE_MODE(0x00000000);
#[cfg(windows)]
const PIPE_UNLIMITED_INSTANCES: u32 = 255;

/// Parsed line trace event from artifact
#[derive(Debug, Clone)]
pub struct TraceEvent {
    pub seq: u32,
    pub file: String,
    pub line: u32,
    pub func: String,
    pub ts_us: u64,
}

/// Named pipe trace collector
pub struct TraceCollector {
    pipe_name: String,
    event_tx: mpsc::Sender<TraceEvent>,
    sequence_counter: std::sync::atomic::AtomicU32,
}

impl TraceCollector {
    pub fn new(event_tx: mpsc::Sender<TraceEvent>) -> Self {
        Self {
            pipe_name: r"\\.\pipe\rededr_trace".to_string(),
            event_tx,
            sequence_counter: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Start named pipe server (blocking, run in tokio::task::spawn_blocking)
    #[cfg(windows)]
    pub fn start_server(&self) -> Result<()> {
        use std::os::windows::io::FromRawHandle;

        info!("Starting trace collector on named pipe: {}", self.pipe_name);

        loop {
            // Create named pipe instance
            let pipe_handle = unsafe {
                CreateNamedPipeA(
                    PCSTR(format!("{}\0", self.pipe_name).as_ptr()),
                    PIPE_ACCESS_INBOUND,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    4096,  // Output buffer size
                    4096,  // Input buffer size
                    0,     // Default timeout
                    None,  // Default security
                )
            };

            let pipe_handle = match pipe_handle {
                Ok(handle) => handle,
                Err(e) => {
                    error!("Failed to create named pipe: {} - {:?}", self.pipe_name, e);
                    return Err(anyhow::anyhow!("CreateNamedPipeA failed: {:?}", e));
                }
            };

            debug!("Named pipe created, waiting for client connection...");

            // Wait for client to connect
            unsafe {
                use windows::Win32::System::Pipes::ConnectNamedPipe;
                if !ConnectNamedPipe(pipe_handle, None).is_ok() {
                    use windows::Win32::Foundation::CloseHandle;
                    let _ = CloseHandle(pipe_handle);
                    continue;
                }
            }

            info!("Artifact connected to trace pipe");

            // Read trace lines from pipe
            let file = unsafe { std::fs::File::from_raw_handle(pipe_handle.0 as _) };
            let reader = BufReader::new(file);

            for line in reader.lines() {
                match line {
                    Ok(line_str) => {
                        if let Err(e) = self.handle_trace_line(&line_str) {
                            warn!("Failed to parse trace line: {} - {}", line_str, e);
                        }
                    }
                    Err(e) => {
                        debug!("Pipe read error (client disconnected?): {}", e);
                        break;
                    }
                }
            }

            info!("Artifact disconnected from trace pipe");
        }
    }

    #[cfg(not(windows))]
    pub fn start_server(&self) -> Result<()> {
        anyhow::bail!("Named pipe trace collector only supported on Windows");
    }

    /// Handle a single trace line: "b64line:<base64>"
    fn handle_trace_line(&self, line: &str) -> Result<()> {
        // Parse "b64line:<base64_data>"
        let b64_data = line
            .strip_prefix("b64line:")
            .context("Line does not start with b64line:")?;

        // Decode Base64
        let decoded_bytes = general_purpose::STANDARD
            .decode(b64_data.trim())
            .context("Failed to decode Base64")?;

        let decoded_str = String::from_utf8(decoded_bytes)
            .context("Decoded data is not valid UTF-8")?;

        // Parse "line:file.c:42:main"
        let parts: Vec<&str> = decoded_str.splitn(4, ':').collect();
        if parts.len() != 4 || parts[0] != "line" {
            anyhow::bail!("Invalid trace format: {}", decoded_str);
        }

        let event = TraceEvent {
            seq: self.sequence_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            file: parts[1].to_string(),
            line: parts[2].parse().context("Invalid line number")?,
            func: parts[3].to_string(),
            ts_us: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_micros() as u64,
        };

        debug!("Parsed trace: {}:{}:{}", event.file, event.line, event.func);

        // Send to gRPC stream
        if let Err(e) = self.event_tx.try_send(event) {
            warn!("Failed to send trace event to gRPC stream: {}", e);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_trace_line() {
        let (tx, mut rx) = mpsc::channel(10);
        let collector = TraceCollector::new(tx);

        // Base64 encode "line:test.c:42:main"
        let input = "line:test.c:42:main";
        let encoded = general_purpose::STANDARD.encode(input);
        let line = format!("b64line:{}", encoded);

        collector.handle_trace_line(&line).unwrap();

        let event = rx.try_recv().unwrap();
        assert_eq!(event.file, "test.c");
        assert_eq!(event.line, 42);
        assert_eq!(event.func, "main");
    }
}
