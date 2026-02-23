//! Telemetry packaging pipeline
//!
//! Packages trace log as deduplicated JSONL. Duplicate `(file, line, func)`
//! entries (from loops) are collapsed, keeping the highest `seq` per key.
//! The controller reads `payload_content` to compute coverage.

use std::collections::HashMap;
use std::path::Path;
use tracing::{error, info, warn};

/// Hard limit for the serialized payload bytes (gRPC default max is 4_194_304).
/// We stay well under to leave room for the outer TelemetryData proto envelope.
const MAX_SERIALIZED_PAYLOAD: usize = 3_500_000;

/// Deduplicate JSONL trace lines by `(file, line, func)`.
///
/// For each unique key, keeps the entry with the highest `seq` value.
/// Entries seen more than once get an added `"count": N` field.
/// Returns `(deduplicated_jsonl, raw_count, unique_count)`.
fn deduplicate_trace_jsonl(raw: &str) -> (String, usize, usize) {
    // Key: (file, line, func) → (best_seq, json_value, count)
    let mut seen: HashMap<(String, i64, String), (i64, serde_json::Value, usize)> = HashMap::new();
    let mut raw_count: usize = 0;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        raw_count += 1;

        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // skip malformed lines
        };

        let file = val["file"].as_str().unwrap_or("").to_string();
        let line_no = val["line"].as_i64().unwrap_or(0);
        let func = val["func"].as_str().unwrap_or("").to_string();
        let seq = val["seq"].as_i64().unwrap_or(0);

        let key = (file, line_no, func);
        match seen.get_mut(&key) {
            Some((best_seq, best_val, count)) => {
                *count += 1;
                if seq > *best_seq {
                    *best_seq = seq;
                    *best_val = val;
                }
            }
            None => {
                seen.insert(key, (seq, val, 1));
            }
        }
    }

    // Sort by seq ascending
    let mut entries: Vec<_> = seen.into_values().collect();
    entries.sort_by_key(|(seq, _, _)| *seq);

    // Build output, adding "count" field where > 1
    let mut output = String::new();
    for (_, mut val, count) in entries {
        if count > 1 {
            if let Some(obj) = val.as_object_mut() {
                obj.insert("count".to_string(), serde_json::json!(count));
            }
        }
        output.push_str(&val.to_string());
        output.push('\n');
    }

    let unique_count = output.lines().count();
    (output, raw_count, unique_count)
}

/// Package trace events file into a single `trace_log` telemetry event.
///
/// Always sends raw JSONL. If the serialized JSON payload would exceed the
/// gRPC limit, the content is trimmed from the front (keeping the tail —
/// most recent lines) until it fits.
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

    // Deduplicate trace lines by (file, line, func), keeping highest seq per key
    let (contents, raw_count, unique_count) = deduplicate_trace_jsonl(&contents);
    if raw_count > 0 {
        let reduction = 100.0 * (1.0 - unique_count as f64 / raw_count as f64);
        info!(
            "Trace dedup: {} raw -> {} unique ({:.0}% reduction)",
            raw_count, unique_count, reduction
        );
    }

    let mut metadata = HashMap::new();
    metadata.insert(
        "trace_file".to_string(),
        trace_events_file.to_string_lossy().to_string(),
    );
    metadata.insert("event_count".to_string(), total_lines.to_string());
    metadata.insert("original_size_bytes".to_string(), original_size.to_string());
    metadata.insert("raw_event_count".to_string(), raw_count.to_string());
    metadata.insert("unique_lines".to_string(), unique_count.to_string());

    // Try the full content first; if the serialized payload is too large,
    // progressively take a smaller tail until it fits.
    // JSON-encoding escapes every \n in the JSONL to \\n, roughly doubling
    // newline bytes, so raw content size is not a reliable predictor.
    let mut slice: &str = &contents;
    let mut truncated = false;

    loop {
        let payload_bytes = serde_json::json!({"content": slice})
            .to_string()
            .into_bytes();

        if payload_bytes.len() <= MAX_SERIALIZED_PAYLOAD {
            // Fits — ship it
            let final_size = payload_bytes.len();
            let sent_lines = slice.lines().count();

            if truncated {
                metadata.insert("compression".to_string(), "truncated_tail".to_string());
                metadata.insert("sent_lines".to_string(), sent_lines.to_string());
            } else {
                metadata.insert("compression".to_string(), "none".to_string());
            }
            metadata.insert("final_size_bytes".to_string(), final_size.to_string());

            telemetry_events.push(crate::automutate::common::TelemetryData {
                job_id: job_id.to_string(),
                event_type: "trace_log".to_string(),
                timestamp: chrono::Utc::now().timestamp(),
                payload: payload_bytes,
                metadata,
                typed_event: None,
            });

            info!(
                "[OK] Collected trace log ({}/{} lines, {} -> {} bytes{})",
                sent_lines,
                total_lines,
                original_size,
                final_size,
                if truncated { ", tail only" } else { "" }
            );
            return;
        }

        // Too large — cut roughly in half and take the tail
        truncated = true;
        let mid = slice.len() / 2;
        // Advance to the next newline so we keep complete JSONL lines
        let adjusted = match slice[mid..].find('\n') {
            Some(pos) => mid + pos + 1,
            None => mid,
        };
        if adjusted >= slice.len() {
            // Single line that's too big — give up
            warn!("Trace log has a single line exceeding payload limit, skipping trace_log event");
            return;
        }
        slice = &slice[adjusted..];
    }
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
