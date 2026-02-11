/// Line-level trace collector via named pipe (Lepori 2023-inspired)
///
/// Listens on \\.\pipe\rededr_trace for trace events from instrumented artifacts.
/// Supports both Base64 text format and binary protocol (auto-detection).
///
/// **Async Implementation**: Uses tokio::net::windows::named_pipe for fully async I/O
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Binary protocol header (matches C runtime InstRecordHeader with #pragma pack(1))
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
struct InstRecordHeader {
    magic: u32, // 0x49535452 ('ISTR')
    version: u16,
    event_type: u16, // 1=line_trace (status events 2-4 now use checkpoint pipe)
    thread_id: u32,
    seq_no: u64,
    ts_us: u64,
    payload_len: u32,
}

const MAGIC_BINARY: u32 = 0x49535452; // 'ISTR'
const HEADER_SIZE: usize = std::mem::size_of::<InstRecordHeader>();

/// Parsed line trace event from artifact
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TraceEvent {
    pub seq: u32,
    pub thread_id: u32,
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
    /// Auto-detects Base64 text vs binary protocol by peeking at first 4 bytes
    #[cfg(windows)]
    pub async fn start_server(&self) -> Result<()> {
        use tokio::io::AsyncReadExt;
        use tokio::net::windows::named_pipe::ServerOptions;

        info!(
            "Starting async trace collector on named pipe: {}",
            self.pipe_name
        );

        // Create the first pipe instance with retry logic (pipe may already exist from previous run)
        let mut server = None;
        let max_retries = 5;

        for attempt in 1..=max_retries {
            match ServerOptions::new()
                // Don't require first_pipe_instance - allows reconnection even if pipe still exists
                // .first_pipe_instance(true)  // REMOVED
                // Increase buffer sizes to handle high-frequency line tracing (loops)
                .in_buffer_size(1024 * 1024) // 1MB input buffer (default is 4KB)
                .out_buffer_size(1024 * 1024) // 1MB output buffer
                .create(&self.pipe_name)
            {
                Ok(s) => {
                    server = Some(s);
                    info!(
                        "Named pipe created: {} (supports Base64 + binary) on attempt {}",
                        self.pipe_name, attempt
                    );
                    break;
                }
                Err(e) if attempt < max_retries => {
                    warn!(
                        "Failed to create named pipe (attempt {}/{}): {} - retrying in 200ms...",
                        attempt, max_retries, e
                    );
                    // Pipe may still be in use from previous run, wait for it to be released
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                }
                Err(e) => {
                    error!(
                        "Failed to create named pipe after {} attempts: {}",
                        max_retries, e
                    );
                    return Err(e).context("Failed to create named pipe after retries");
                }
            }
        }

        let mut server = server.context("Failed to create named pipe")?;

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

            // Peek at first 4 bytes to detect protocol (with timeout to avoid hanging)
            let mut peek_buf = [0u8; 4];
            let read_result = tokio::time::timeout(
                tokio::time::Duration::from_secs(2),
                server.read_exact(&mut peek_buf),
            )
            .await;

            match read_result {
                Ok(Ok(_)) => {
                    let magic = u32::from_le_bytes(peek_buf);

                    if magic == MAGIC_BINARY {
                        info!("Detected binary protocol (magic: 0x{:08x})", magic);
                        // Process binary stream
                        if let Err(e) = self.read_binary_stream(&mut server, peek_buf).await {
                            warn!("Binary stream read error: {}", e);
                        }
                    } else {
                        info!(
                            "Detected Base64 text protocol (first bytes: {:?})",
                            peek_buf
                        );
                        // Process text stream
                        if let Err(e) = self.read_text_stream(&mut server, peek_buf).await {
                            warn!("Text stream read error: {}", e);
                        }
                    }
                }
                Ok(Err(e)) => {
                    debug!("Client disconnected before sending data: {}", e);
                }
                Err(_) => {
                    warn!(
                        "Timeout waiting for trace data (2s) - artifact connected but sent nothing"
                    );
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

    /// Read binary protocol stream
    #[cfg(windows)]
    async fn read_binary_stream<S>(&self, stream: &mut S, first_bytes: [u8; 4]) -> Result<()>
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        use tokio::io::AsyncReadExt;

        debug!(
            "Binary stream: reading first header (expecting {} more bytes)",
            HEADER_SIZE - 4
        );

        // Read rest of header (we already have first 4 bytes = magic)
        let mut header_rest = vec![0u8; HEADER_SIZE - 4];
        match stream.read_exact(&mut header_rest).await {
            Ok(_) => {
                debug!("Binary stream: successfully read header rest");
            }
            Err(e) => {
                warn!("Binary stream: failed to read header rest: {}", e);
                return Ok(()); // Not fatal - artifact disconnected
            }
        }

        // Process first record using peek_buf + header_rest
        let mut header_bytes = Vec::with_capacity(HEADER_SIZE);
        header_bytes.extend_from_slice(&first_bytes);
        header_bytes.extend_from_slice(&header_rest);

        let hdr: InstRecordHeader =
            unsafe { std::ptr::read_unaligned(header_bytes.as_ptr() as *const _) };

        // Read fields using ptr::read_unaligned to avoid alignment issues with packed struct
        let magic = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.magic)) };
        let event_type = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.event_type)) };
        let payload_len = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.payload_len)) };
        let seq_no = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.seq_no)) };

        if magic != MAGIC_BINARY {
            warn!("Invalid magic in first record: 0x{:08x}", magic);
            return Ok(());
        }

        debug!(
            "Binary stream: received record (type={}, payload_len={}, seq={})",
            event_type, payload_len, seq_no
        );

        // Read first payload
        let mut payload = vec![0u8; payload_len as usize];
        match stream.read_exact(&mut payload).await {
            Ok(_) => {
                debug!(
                    "Binary stream: successfully read payload ({} bytes)",
                    payload.len()
                );
            }
            Err(e) => {
                warn!("Binary stream: failed to read first payload: {}", e);
                return Ok(());
            }
        }

        // Handle first record
        self.parse_on_event_type(&hdr, event_type, &mut payload);

        // Now read subsequent records (full header each time, no peeking)
        loop {
            // Read full header (32 bytes with packed struct) in one shot
            let mut full_header = vec![0u8; HEADER_SIZE];
            match stream.read_exact(&mut full_header).await {
                Ok(_) => {}
                Err(_) => break, // Client disconnected
            }

            // Parse header with unaligned reads (packed struct)
            let hdr: InstRecordHeader =
                unsafe { std::ptr::read_unaligned(full_header.as_ptr() as *const _) };

            let magic = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.magic)) };
            let event_type =
                unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.event_type)) };
            let payload_len =
                unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.payload_len)) };
            let seq_no = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.seq_no)) };

            // Validate magic
            if magic != MAGIC_BINARY {
                warn!("Invalid magic in stream: 0x{:08x}", magic);
                break;
            }

            debug!(
                "Binary stream: received record (type={}, payload_len={}, seq={})",
                event_type, payload_len, seq_no
            );

            // Read payload
            let mut payload = vec![0u8; payload_len as usize];
            match stream.read_exact(&mut payload).await {
                Ok(_) => {
                    debug!(
                        "Binary stream: successfully read payload ({} bytes)",
                        payload.len()
                    );
                }
                Err(e) => {
                    warn!("Binary stream: failed to read payload: {}", e);
                    break; // Client disconnected
                }
            }

            // Parse based on event_type
            self.parse_on_event_type(&hdr, event_type, &mut payload);
        }

        Ok(())
    }

    fn parse_on_event_type(&self, hdr: &InstRecordHeader, event_type: u16, payload: &mut [u8]) {
        match event_type {
            1 => {
                // LINE_TRACE: payload is "file:line:func" UTF-8 string
                if let Err(e) = self.handle_binary_line_trace(hdr, payload) {
                    warn!("Failed to parse binary line trace: {}", e);
                } else {
                    debug!("Binary stream: successfully parsed line trace event");
                }
            }
            2..=4 => {
                // Artifact status events now use the checkpoint pipe/file.
                // Warn if an old-format artifact sends them here.
                warn!(
                    "Received artifact status event (type={}) on trace pipe; expected on checkpoint pipe. Ignoring.",
                    event_type
                );
            }
            _ => {
                debug!("Unknown event_type: {}", event_type);
            }
        }
    }

    /// Read text-based (Base64) protocol stream
    #[cfg(windows)]
    async fn read_text_stream<S>(&self, stream: &mut S, first_bytes: [u8; 4]) -> Result<()>
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

        // We need to prepend the first_bytes to the stream
        // Strategy: read rest of first line, then continue reading lines normally

        // Convert first_bytes to string (they're likely ASCII text)
        let mut line_start = String::from_utf8_lossy(&first_bytes).to_string();

        // Read until newline to complete the first line
        let mut rest_of_line = Vec::new();
        let mut buf = [0u8; 1];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if buf[0] == b'\n' {
                        break;
                    }
                    rest_of_line.push(buf[0]);
                }
                Err(_) => break,
            }
        }

        line_start.push_str(&String::from_utf8_lossy(&rest_of_line));

        // Process first line
        if !line_start.is_empty()
            && let Err(e) = self.handle_trace_line(&line_start)
        {
            warn!("Failed to parse trace line: {} - {}", line_start, e);
        }

        // Now read remaining lines normally
        let reader = BufReader::new(stream);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if let Err(e) = self.handle_trace_line(&line) {
                warn!("Failed to parse trace line: {} - {}", line, e);
            }
        }

        Ok(())
    }

    /// Handle binary line trace (event_type=1)
    fn handle_binary_line_trace(&self, hdr: &InstRecordHeader, payload: &[u8]) -> Result<()> {
        // Parse payload: "file:line:func"
        let payload_str =
            String::from_utf8(payload.to_vec()).context("Payload is not valid UTF-8")?;

        let parts: Vec<&str> = payload_str.splitn(3, ':').collect();
        if parts.len() < 2 {
            anyhow::bail!("Invalid binary trace payload: {}", payload_str);
        }

        // Read packed struct fields with unaligned access
        let seq_no = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.seq_no)) };
        let thread_id = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.thread_id)) };
        let ts_us = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(hdr.ts_us)) };

        let event = TraceEvent {
            seq: seq_no as u32,
            thread_id,
            file: parts[0].to_string(),
            line: parts[1].parse().context("Invalid line number")?,
            func: parts.get(2).unwrap_or(&"").to_string(),
            ts_us,
        };

        debug!(
            "Binary trace: {}:{}:{} (thread={})",
            event.file, event.line, event.func, event.thread_id
        );

        // Send to gRPC stream
        if let Err(e) = self.event_tx.try_send(event) {
            warn!("Failed to send trace event to gRPC stream: {}", e);
        }

        Ok(())
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

        let decoded_str =
            String::from_utf8(decoded_bytes).context("Decoded data is not valid UTF-8")?;

        // Parse "line:file.c:42:main" or "line:source:42:" (AST format without func)
        let parts: Vec<&str> = decoded_str.splitn(4, ':').collect();
        if parts.len() < 3 || parts[0] != "line" {
            anyhow::bail!("Invalid trace format: {}", decoded_str);
        }

        let event = TraceEvent {
            seq: self
                .sequence_counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            thread_id: 0, // Base64 format doesn't include thread_id
            file: parts[1].to_string(),
            line: parts[2].parse().context("Invalid line number")?,
            func: parts.get(3).unwrap_or(&"").to_string(), // Optional function name
            ts_us: SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64,
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
