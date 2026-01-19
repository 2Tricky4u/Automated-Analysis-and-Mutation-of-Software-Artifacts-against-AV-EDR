use tracing::{info, warn};

/// Extract filename from full path
pub fn extract_filename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

/// Collect basic-block coverage from metadata file
pub async fn collect_bb_coverage(
    _bitmap_path: &std::path::Path,
    metadata_path: &std::path::Path,
    job_id: &str,
) -> Result<crate::automutate::common::TelemetryData, Box<dyn std::error::Error + Send + Sync>> {
    use tokio::fs;

    // Read BB metadata (text file with BB IDs and hit counts)
    // This is all we need - the bitmap is just for AFL-style fuzzing
    let metadata_text = fs::read_to_string(metadata_path).await?;

    let mut bb_ids = Vec::new();
    let mut hit_counts = Vec::new();

    for line in metadata_text.lines() {
        let line = line.trim();

        // Skip comments and headers
        if line.starts_with('#') || line.is_empty() {
            continue;
        }

        // Parse "BB_ID HIT_COUNT"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            if let (Ok(bb_id), Ok(hit_count)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                bb_ids.push(bb_id);
                hit_counts.push(hit_count);
            }
        }
    }

    let total_bbs = bb_ids.len() as u32;

    info!(
        "Parsed BB metadata from coverage_bbs.txt: {} basic blocks",
        total_bbs
    );

    Ok(crate::automutate::common::TelemetryData {
        job_id: job_id.to_string(),
        event_type: "coverage".to_string(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        payload: vec![], // Empty payload (data in typed_event)
        metadata: std::collections::HashMap::new(),
        typed_event: Some(crate::automutate::common::telemetry_data::TypedEvent::Coverage(
            crate::automutate::common::CoverageEvent {
                bitmap: vec![], // Empty - we only send bb_ids and hit_counts from .txt
                bb_ids,
                hit_counts,
                total_bbs,
            },
        )),
    })
}

/// Collect API checkpoint events from disk file (checkpoints.log)
pub async fn collect_api_checkpoints(
    checkpoints_path: &std::path::Path,
    job_id: &str,
) -> Result<Vec<crate::automutate::common::TelemetryData>, Box<dyn std::error::Error + Send + Sync>> {
    use tokio::fs;

    let checkpoints_text = fs::read_to_string(checkpoints_path).await?;
    let mut events = Vec::new();

    for (line_num, line) in checkpoints_text.lines().enumerate() {
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        // Parse JSON line: {"ts_us":1234567,"checkpoint":"api:VirtualAlloc"}
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(checkpoint_json) => {
                let ts_us = checkpoint_json["ts_us"].as_u64().unwrap_or(0);
                let name = checkpoint_json["checkpoint"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string();

                events.push(crate::automutate::common::TelemetryData {
                    job_id: job_id.to_string(),
                    event_type: "checkpoint".to_string(),
                    timestamp: (ts_us / 1000) as i64, // Convert to milliseconds for consistency
                    payload: vec![],
                    metadata: std::collections::HashMap::new(),
                    typed_event: Some(crate::automutate::common::telemetry_data::TypedEvent::Checkpoint(
                        crate::automutate::common::CheckpointEvent { name, ts_us },
                    )),
                });
            }
            Err(e) => {
                warn!(
                    "Failed to parse checkpoint line {} in {}: {} - Line: {}",
                    line_num + 1,
                    checkpoints_path.display(),
                    e,
                    line
                );
            }
        }
    }

    Ok(events)
}
