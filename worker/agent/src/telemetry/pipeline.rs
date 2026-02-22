//! Telemetry packaging pipeline
//!
//! Packages trace log as the tail of raw JSONL (no compression).
//! The controller reads `payload_content` to compute coverage.

use std::path::Path;
use tracing::{error, info, warn};

/// Max payload size that fits in a single gRPC message (slightly under 4MB).
const MAX_PAYLOAD_SIZE: usize = 4_000_000;

/// Package trace events file into a single `trace_log` telemetry event.
///
/// Always sends raw JSONL. If the file exceeds the gRPC payload limit,
/// the tail (last complete JSONL lines that fit) is sent.
pub fn package_trace_log(
    trace_events_file: &Path,
    job_id: &str,
    telemetry_events: &mut Vec<crate::automutate::common::TelemetryData>,
) {
    if !trace_events_file.exists() {
        info!("No trace events file found (artifact may not have line tracing enabled)");
        return;
    }

    info!("Reading trace events from file: {:?}", trace_events_file);

    let contents = match std::fs::read_to_string(trace_events_file) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to read trace events file: {}", e);
            return;
        }
    };

    let original_size = contents.len();
    let total_lines = contents.lines().count();

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "trace_file".to_string(),
        trace_events_file.to_string_lossy().to_string(),
    );
    metadata.insert("event_count".to_string(), total_lines.to_string());
    metadata.insert("original_size_bytes".to_string(), original_size.to_string());

    // Take the tail that fits within the payload limit.
    // Reserve some bytes for the JSON wrapper {"content":"..."}
    let max_content_bytes = MAX_PAYLOAD_SIZE.saturating_sub(64);

    let (content_to_send, truncated) = if contents.len() <= max_content_bytes {
        (contents.as_str(), false)
    } else {
        // Walk forward from the cut point to the next newline to keep complete lines
        let start = contents.len() - max_content_bytes;
        let adjusted = match contents[start..].find('\n') {
            Some(pos) => start + pos + 1,
            None => start,
        };
        (&contents[adjusted..], true)
    };

    if truncated {
        let sent_lines = content_to_send.lines().count();
        metadata.insert("compression".to_string(), "truncated_tail".to_string());
        metadata.insert("sent_lines".to_string(), sent_lines.to_string());
        info!(
            "Trace too large ({} bytes, {} lines), sending tail ({} lines)",
            original_size, total_lines, sent_lines
        );
    } else {
        metadata.insert("compression".to_string(), "none".to_string());
    }

    let payload = serde_json::json!({"content": content_to_send})
        .to_string()
        .into_bytes();
    let final_size = payload.len();
    metadata.insert("final_size_bytes".to_string(), final_size.to_string());

    telemetry_events.push(crate::automutate::common::TelemetryData {
        job_id: job_id.to_string(),
        event_type: "trace_log".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        payload,
        metadata,
        typed_event: None,
    });

    info!(
        "[OK] Collected trace log ({} line traces, {} -> {} bytes{})",
        total_lines,
        original_size,
        final_size,
        if truncated { ", tail only" } else { "" }
    );
}


/// Parse binary protocol trace.log and extract telemetry events
pub fn collect_trace_log_binary(
    trace_log_path: &Path,
    job_id: &str,
    telemetry_events: &mut Vec<crate::automutate::common::TelemetryData>,
) {
    if !trace_log_path.exists() {
        return;
    }

    info!(
        "Found trace.log file, collecting binary protocol events: {:?}",
        trace_log_path
    );

    let trace_bytes = match std::fs::read(trace_log_path) {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to read trace.log: {}", e);
            return;
        }
    };

    let mut file_trace_count = 0;

    let mut offset = 0;
    while offset + 32 <= trace_bytes.len() {
        let header_bytes = &trace_bytes[offset..offset + 32];

        let magic = u32::from_le_bytes([
            header_bytes[0],
            header_bytes[1],
            header_bytes[2],
            header_bytes[3],
        ]);

        if magic != 0x49535452 {
            warn!(
                "Invalid magic in trace.log at offset {}: 0x{:08x}, stopping parse",
                offset, magic
            );
            break;
        }

        let event_type = u16::from_le_bytes([header_bytes[6], header_bytes[7]]);
        let payload_len = u32::from_le_bytes([
            header_bytes[28],
            header_bytes[29],
            header_bytes[30],
            header_bytes[31],
        ]);

        offset += 32;

        if offset + payload_len as usize > trace_bytes.len() {
            warn!(
                "Incomplete payload in trace.log at offset {}, expected {} bytes",
                offset, payload_len
            );
            break;
        }

        let payload = &trace_bytes[offset..offset + payload_len as usize];
        offset += payload_len as usize;

        match event_type {
            1 => {
                if let Ok(payload_str) = std::str::from_utf8(payload) {
                    telemetry_events.push(crate::automutate::common::TelemetryData {
                        job_id: job_id.to_string(),
                        event_type: "trace_line".to_string(),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        payload: payload_str.as_bytes().to_vec(),
                        metadata: std::collections::HashMap::new(),
                        typed_event: None,
                    });
                    file_trace_count += 1;
                }
            }
            2..=4 => {
                warn!(
                    "Found artifact status event (type={}) in trace.log; expected in checkpoints.log. Ignoring.",
                    event_type
                );
            }
            _ => {
                tracing::debug!("Unknown event_type {} in trace.log", event_type);
            }
        }
    }

    info!(
        "[OK] Collected from trace.log: {} line traces",
        file_trace_count
    );
}
