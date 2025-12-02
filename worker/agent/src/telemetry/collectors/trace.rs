/// Line-level trace collector via named pipe (Lepori 2023-inspired)
///
/// Listens on \\.\pipe\rededr_trace for Base64-encoded trace events
/// from instrumented artifacts. Decodes and streams to controller via gRPC.
///
/// **Async Implementation**: Uses tokio::net::windows::named_pipe for fully async I/O
use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Parsed line trace event from artifact
#[derive(Debug, Clone)]
pub struct TraceEvent {
    pub seq: u32,
    pub file: String,
    pub line: u32,
    pub func: String,
    pub ts_us: u64,
}

/// Named pipe trace collector (async)
pub struct TraceCollector {
    pipe_name: String,
    event_tx: mpsc::Sender<TraceEvent>,
    sequence_counter: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl TraceCollector {
    pub fn new(event_tx: mpsc::Sender<TraceEvent>) -> Self {
        Self {
            pipe_name: r"\\.\pipe\rededr_trace".to_string(),
            event_tx,
            sequence_counter: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    /// Start async named pipe server (fully async, no spawn_blocking needed)
    #[cfg(windows)]
    pub async fn start_server(&self) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;

        info!("Starting async trace collector on named pipe: {}", self.pipe_name);

        // Create the first pipe instance
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&self.pipe_name)
            .context("Failed to create named pipe")?;

        info!("Named pipe created: {}", self.pipe_name);

        loop {
            // Wait for client to connect (async!)
            match server.connect().await {
                Ok(_) => {
                    info!("Artifact connected to trace pipe");
                }
                Err(e) => {
                    warn!("Failed to accept connection: {}", e);
                    // Try to recreate pipe
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    continue;
                }
            }

            // Read trace lines from connected client (async I/O)
            let reader = BufReader::new(&mut server);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if let Err(e) = self.handle_trace_line(&line) {
                    warn!("Failed to parse trace line: {} - {}", line, e);
                }
            }

            info!("Artifact disconnected from trace pipe");

            // Disconnect and prepare for next client
            if let Err(e) = server.disconnect() {
                warn!("Failed to disconnect pipe: {}", e);
                // Recreate the pipe on error
                match ServerOptions::new().create(&self.pipe_name) {
                    Ok(new_server) => {
                        server = new_server;
                        info!("Recreated named pipe after disconnect error");
                    }
                    Err(e) => {
                        error!("Failed to recreate named pipe: {}", e);
                        return Err(e.into());
                    }
                }
            }
        }
    }

    #[cfg(not(windows))]
    pub async fn start_server(&self) -> Result<()> {
        anyhow::bail!("Named pipe trace collector only supported on Windows");
    }

    /// Handle a single trace line: "b64line:<base64>" or "YjY0<base64>" (new AST format)
    fn handle_trace_line(&self, line: &str) -> Result<()> {
        // Support both formats:
        // 1. Old IR format: "b64line:<base64_data>"
        // 2. New AST format: "YjY0<base64_data>" (YjY0 = Base64("b64"))

        let b64_data = if let Some(data) = line.strip_prefix("b64line:") {
            // Old IR format
            data
        } else if let Some(data) = line.strip_prefix("YjY0") {
            // New AST format (Lepori thesis format)
            data
        } else {
            anyhow::bail!("Line does not start with b64line: or YjY0: {}", line);
        };

        // Decode Base64
        let decoded_bytes = general_purpose::STANDARD
            .decode(b64_data.trim())
            .context("Failed to decode Base64")?;

        let decoded_str = String::from_utf8(decoded_bytes)
            .context("Decoded data is not valid UTF-8")?;

        // Parse "line:file.c:42:main" or "line:source:42:" (AST format without func)
        let parts: Vec<&str> = decoded_str.splitn(4, ':').collect();
        if parts.len() < 3 || parts[0] != "line" {
            anyhow::bail!("Invalid trace format: {}", decoded_str);
        }

        let event = TraceEvent {
            seq: self
                .sequence_counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            file: parts[1].to_string(),
            line: parts[2].parse().context("Invalid line number")?,
            func: parts.get(3).unwrap_or(&"").to_string(), // Optional function name
            ts_us: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_micros() as u64,
        };

        debug!("Parsed trace: {}:{}:{}", event.file, event.line, event.func);

        // Send to gRPC stream (async send)
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
    fn test_parse_ir_trace_line() {
        let (tx, mut rx) = mpsc::channel(10);
        let collector = TraceCollector::new(tx);

        // Old IR format: Base64 encode "line:test.c:42:main"
        let input = "line:test.c:42:main";
        let encoded = general_purpose::STANDARD.encode(input);
        let line = format!("b64line:{}", encoded);

        collector.handle_trace_line(&line).unwrap();

        let event = rx.try_recv().unwrap();
        assert_eq!(event.file, "test.c");
        assert_eq!(event.line, 42);
        assert_eq!(event.func, "main");
    }

    #[test]
    fn test_parse_ast_trace_line() {
        let (tx, mut rx) = mpsc::channel(10);
        let collector = TraceCollector::new(tx);

        // New AST format: "YjY0" + Base64 encode "line:source:42:"
        let input = "line:source:42:";
        let encoded = general_purpose::STANDARD.encode(input);
        let line = format!("YjY0{}", encoded);

        collector.handle_trace_line(&line).unwrap();

        let event = rx.try_recv().unwrap();
        assert_eq!(event.file, "source");
        assert_eq!(event.line, 42);
        assert_eq!(event.func, ""); // No function name in AST format
    }

    #[test]
    fn test_parse_ast_trace_with_func() {
        let (tx, mut rx) = mpsc::channel(10);
        let collector = TraceCollector::new(tx);

        // AST format with optional function: "YjY0" + Base64 encode "line:loader.c:100:main"
        let input = "line:loader.c:100:main";
        let encoded = general_purpose::STANDARD.encode(input);
        let line = format!("YjY0{}", encoded);

        collector.handle_trace_line(&line).unwrap();

        let event = rx.try_recv().unwrap();
        assert_eq!(event.file, "loader.c");
        assert_eq!(event.line, 100);
        assert_eq!(event.func, "main");
    }
}
