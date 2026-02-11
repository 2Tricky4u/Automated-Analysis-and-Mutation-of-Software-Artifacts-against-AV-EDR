//! Telemetry packaging pipeline
//!
//! Handles the gather → size → compress → batch → send pipeline
//! for trace logs and other telemetry data.

use std::path::Path;
use tracing::{error, info, warn};

const MAX_IMMEDIATE_SIZE: usize = 2_097_152; // 2MB threshold
const MAX_PAYLOAD_SIZE: usize = 4_000_000; // 4MB gRPC limit (slightly under)

/// Package trace events file into telemetry events.
///
/// Two-phase approach:
/// 1. If trace <= 2MB: send entire trace immediately as single event
/// 2. If trace > 2MB: send last 2MB immediately, spawn async compression for full trace
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
    let trace_events_count = contents.lines().count();

    if original_size <= MAX_IMMEDIATE_SIZE {
        package_small_trace(
            trace_events_file,
            &contents,
            original_size,
            trace_events_count,
            job_id,
            telemetry_events,
        );
    } else {
        package_large_trace(
            trace_events_file,
            &contents,
            original_size,
            trace_events_count,
            job_id,
            telemetry_events,
        );
    }
}

/// Package a small trace (<= 2MB) as a single telemetry event
fn package_small_trace(
    trace_events_file: &Path,
    contents: &str,
    original_size: usize,
    trace_events_count: usize,
    job_id: &str,
    telemetry_events: &mut Vec<crate::automutate::common::TelemetryData>,
) {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "trace_file".to_string(),
        trace_events_file.to_string_lossy().to_string(),
    );
    metadata.insert("event_count".to_string(), trace_events_count.to_string());
    metadata.insert("original_size_bytes".to_string(), original_size.to_string());

    let payload = if original_size <= MAX_PAYLOAD_SIZE {
        metadata.insert("compression".to_string(), "none".to_string());
        serde_json::json!({"content": contents})
            .to_string()
            .into_bytes()
    } else {
        metadata.insert("compression".to_string(), "truncated_tail".to_string());

        let max_tail_bytes = MAX_PAYLOAD_SIZE.saturating_sub(1);
        let tail = if contents.len() <= max_tail_bytes {
            contents.to_string()
        } else {
            let start = contents.len() - max_tail_bytes;
            let mut s = start;
            while s < contents.len() && !contents.is_char_boundary(s) {
                s += 1;
            }
            contents[s..].to_string()
        };

        serde_json::json!({ "content": tail })
            .to_string()
            .into_bytes()
    };

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
        "[OK] Collected trace log as single event ({} line traces, {} -> {} bytes)",
        trace_events_count, original_size, final_size
    );
}

/// Package a large trace (> 2MB): send last 2MB immediately, spawn async compression
fn package_large_trace(
    trace_events_file: &Path,
    contents: &str,
    original_size: usize,
    trace_events_count: usize,
    job_id: &str,
    telemetry_events: &mut Vec<crate::automutate::common::TelemetryData>,
) {
    // Extract last 2MB of complete JSON lines
    let byte_offset = original_size.saturating_sub(MAX_IMMEDIATE_SIZE);
    let mut last_2mb = contents[byte_offset..].to_string();

    // Remove incomplete first line
    if let Some(first_newline) = last_2mb.find('\n') {
        last_2mb = last_2mb[first_newline + 1..].to_string();
    }

    let immediate_line_count = last_2mb.lines().count();

    let mut immediate_metadata = std::collections::HashMap::new();
    immediate_metadata.insert(
        "trace_file".to_string(),
        trace_events_file.to_string_lossy().to_string(),
    );
    immediate_metadata.insert("event_count".to_string(), trace_events_count.to_string());
    immediate_metadata.insert("original_size_bytes".to_string(), original_size.to_string());
    immediate_metadata.insert("compression".to_string(), "none".to_string());
    immediate_metadata.insert(
        "payload_type".to_string(),
        "last_complete_jsonl".to_string(),
    );
    immediate_metadata.insert(
        "immediate_line_count".to_string(),
        immediate_line_count.to_string(),
    );
    immediate_metadata.insert(
        "note".to_string(),
        format!(
            "Last {} complete JSON lines (~2MB) sent immediately; full compressed trace will follow",
            immediate_line_count
        ),
    );

    let immediate_payload = serde_json::json!({"content": last_2mb})
        .to_string()
        .into_bytes();
    immediate_metadata.insert(
        "final_size_bytes".to_string(),
        immediate_payload.len().to_string(),
    );

    telemetry_events.push(crate::automutate::common::TelemetryData {
        job_id: job_id.to_string(),
        event_type: "trace_log".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        payload: immediate_payload,
        metadata: immediate_metadata,
        typed_event: None,
    });

    info!(
        "[OK] Collected last {} complete JSON lines (~2MB) of trace log ({} total line traces, {} bytes total)",
        immediate_line_count, trace_events_count, original_size
    );

    // Spawn async compression of full trace
    let job_id_owned = job_id.to_string();
    let trace_file_owned = trace_events_file.to_path_buf();
    let contents_owned = contents.to_string();

    info!(
        "Spawning async compression task for full trace ({} bytes)...",
        original_size
    );

    tokio::spawn(async move {
        compress_and_prepare_trace(
            &trace_file_owned,
            &contents_owned,
            original_size,
            trace_events_count,
            &job_id_owned,
        );
    });
}

/// Compress full trace and prepare reduced telemetry event (runs in background)
fn compress_and_prepare_trace(
    trace_file: &Path,
    contents: &str,
    original_size: usize,
    trace_events_count: usize,
    job_id: &str,
) {
    use crate::telemetry::trace_compressor;

    info!("Async compression task started for trace: {:?}", trace_file);

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "trace_file".to_string(),
        trace_file.to_string_lossy().to_string(),
    );
    metadata.insert("event_count".to_string(), trace_events_count.to_string());
    metadata.insert("original_size_bytes".to_string(), original_size.to_string());

    let payload = if original_size <= MAX_PAYLOAD_SIZE {
        metadata.insert("compression".to_string(), "none".to_string());
        serde_json::json!({"content": contents})
            .to_string()
            .into_bytes()
    } else {
        info!(
            "Trace log too large ({} bytes), applying CLP+MatrixProfile+Sequitur compression...",
            original_size
        );
        let compressed = trace_compressor::compress_trace_log(contents, 3);

        metadata.insert(
            "compression_ratio".to_string(),
            format!("{:.2}x", compressed.compression_ratio),
        );
        metadata.insert(
            "compression_stats".to_string(),
            serde_json::to_string(&compressed.statistics).unwrap_or_default(),
        );

        if compressed.compressed_size <= MAX_PAYLOAD_SIZE {
            info!(
                "Loop compression successful: {} -> {} bytes ({:.1}x)",
                original_size, compressed.compressed_size, compressed.compression_ratio
            );
            metadata.insert("compression".to_string(), "loop_detection".to_string());
            serde_json::json!({"content": compressed.content})
                .to_string()
                .into_bytes()
        } else {
            info!("Still too large after loop compression, applying gzip...");
            match trace_compressor::gzip_compress(compressed.content.as_bytes()) {
                Ok(gzipped) if gzipped.len() <= MAX_PAYLOAD_SIZE => {
                    info!(
                        "Gzip compression successful: {} -> {} bytes",
                        compressed.compressed_size,
                        gzipped.len()
                    );
                    metadata.insert("compression".to_string(), "loop+gzip".to_string());
                    use base64::{Engine as _, engine::general_purpose};
                    let gzipped_b64 = general_purpose::STANDARD.encode(&gzipped);
                    serde_json::json!({"content_b64": gzipped_b64, "encoding": "gzip+base64"})
                        .to_string()
                        .into_bytes()
                }
                Ok(gzipped) => {
                    warn!(
                        "Trace log too large even after compression ({} bytes), sending metadata only",
                        gzipped.len()
                    );
                    metadata.insert("compression".to_string(), "truncated".to_string());
                    metadata.insert(
                        "note".to_string(),
                        "Log too large for gRPC transmission, file stored on worker".to_string(),
                    );

                    let lines: Vec<&str> = contents.lines().collect();
                    let sample = if lines.len() > 200 {
                        format!(
                            "=== First 100 lines ===\n{}\n=== Last 100 lines ===\n{}",
                            lines[..100].join("\n"),
                            lines[lines.len() - 100..].join("\n")
                        )
                    } else {
                        contents.to_string()
                    };
                    serde_json::json!({"content": sample})
                        .to_string()
                        .into_bytes()
                }
                Err(e) => {
                    error!("Gzip compression failed: {}", e);
                    metadata.insert("compression".to_string(), "error".to_string());
                    serde_json::json!({"error": "compression_failed"})
                        .to_string()
                        .into_bytes()
                }
            }
        }
    };

    let final_size = payload.len();
    metadata.insert("final_size_bytes".to_string(), final_size.to_string());

    let _reduced_telemetry = crate::automutate::common::TelemetryData {
        job_id: job_id.to_string(),
        event_type: "trace_log_reduced".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        payload,
        metadata,
        typed_event: None,
    };

    info!(
        "Reduced trace prepared ({} -> {} bytes) - available for controller pull",
        original_size, final_size
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
