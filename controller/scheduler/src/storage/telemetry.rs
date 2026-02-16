//! Telemetry indexing with payload flattening and typed_event handling.
//!
//! Salvaged from the dead storage/elasticsearch.rs code with improvements:
//! - TelemetryContext for correlation keys (run_id, round_id, vm_id)
//! - Smart numeric conversion (pointers→hex, large u64→hex)
//! - Daily index pattern: telemetry-YYYY.MM.DD

use super::TelemetryContext;
use crate::automutate::common::TelemetryData;
use elasticsearch::{Elasticsearch, IndexParts};
use serde_json::json;
use tracing::{info, warn};

pub async fn index_telemetry_batch(
    es: &Elasticsearch,
    batch: &[TelemetryData],
    context: &TelemetryContext,
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    let index_name = format!("telemetry-{}", chrono::Utc::now().format("%Y.%m.%d"));
    let mut indexed = 0;

    for event in batch {
        // Parse payload to extract searchable fields
        let mut payload_fields = if let Ok(payload_json) =
            serde_json::from_slice::<serde_json::Value>(&event.payload)
        {
            payload_json.as_object().cloned().unwrap_or_default()
        } else {
            Default::default()
        };

        // Handle typed_event variants (structured proto instead of JSON payload)
        if let Some(ref typed_event) = event.typed_event {
            use crate::automutate::common::telemetry_data::TypedEvent;
            match typed_event {
                TypedEvent::Trace(trace) => {
                    payload_fields.insert("seq".to_string(), json!(trace.seq));
                    payload_fields.insert("file".to_string(), json!(&trace.file));
                    payload_fields.insert("line".to_string(), json!(trace.line));
                    payload_fields.insert("func".to_string(), json!(&trace.func));
                    payload_fields.insert("ts_us".to_string(), json!(trace.ts_us));
                }
                TypedEvent::Coverage(cov) => {
                    payload_fields.insert("total_bbs".to_string(), json!(cov.total_bbs));
                    payload_fields.insert("bb_ids".to_string(), json!(&cov.bb_ids));
                    payload_fields.insert("hit_counts".to_string(), json!(&cov.hit_counts));
                    payload_fields.insert("bitmap_size".to_string(), json!(cov.bitmap.len()));

                    use base64::{engine::general_purpose, Engine as _};
                    let bitmap_b64 = general_purpose::STANDARD.encode(&cov.bitmap);
                    payload_fields.insert("bitmap_b64".to_string(), json!(bitmap_b64));
                }
                TypedEvent::Checkpoint(cp) => {
                    payload_fields.insert("checkpoint_name".to_string(), json!(&cp.name));
                    payload_fields.insert("ts_us".to_string(), json!(cp.ts_us));
                }
            }
        }

        // Build document with correlation keys
        let mut doc = json!({
            "job_id": event.job_id,
            "event_type": event.event_type,
            "source": "worker",
            "timestamp": event.timestamp,
            "metadata": event.metadata,
            "indexed_at": chrono::Utc::now().to_rfc3339(),
            "vm_id": context.vm_id,
        });

        // Add optional correlation keys
        if let Some(ref run_id) = context.run_id {
            doc.as_object_mut().unwrap().insert("run_id".to_string(), json!(run_id));
        }
        if let Some(ref round_id) = context.round_id {
            doc.as_object_mut().unwrap().insert("round_id".to_string(), json!(round_id));
        }

        // Merge payload fields into top level with payload_ prefix
        if let Some(obj) = doc.as_object_mut() {
            for (key, value) in payload_fields {
                let key_lower = key.to_lowercase();
                let is_pointer_field = key_lower.contains("address")
                    || key_lower.contains("pointer")
                    || key_lower.contains("stack")
                    || key_lower.contains("base")
                    || key_lower.contains("limit")
                    || key_lower.contains("rva")
                    || key_lower.contains("offset") && value.is_number();

                let should_be_numeric = key_lower.contains("addr")
                    || key_lower.contains("port")
                    || key_lower.contains("pid")
                    || key_lower.contains("tid")
                    || key_lower.contains("size");

                let converted_value = match value {
                    serde_json::Value::Number(n) => {
                        if is_pointer_field {
                            if let Some(u) = n.as_u64() {
                                json!(format!("0x{:X}", u))
                            } else if let Some(i) = n.as_i64() {
                                json!(format!("0x{:X}", i))
                            } else {
                                json!(n.to_string())
                            }
                        } else if let Some(u) = n.as_u64() {
                            if u > i64::MAX as u64 {
                                json!(format!("0x{:X}", u))
                            } else {
                                json!(u)
                            }
                        } else if let Some(i) = n.as_i64() {
                            json!(i)
                        } else if let Some(f) = n.as_f64() {
                            json!(f)
                        } else {
                            json!(n.to_string())
                        }
                    }
                    serde_json::Value::String(s) => {
                        if should_be_numeric
                            && (s == "unsupported" || s.parse::<i64>().is_err())
                        {
                            json!(null)
                        } else {
                            json!(s)
                        }
                    }
                    other => other,
                };

                obj.insert(format!("payload_{}", key), converted_value);
            }
        }

        let doc_str = serde_json::to_string(&doc).unwrap_or_default();

        let response = es
            .index(IndexParts::Index(&index_name))
            .body(doc)
            .send()
            .await;

        match response {
            Ok(resp) if resp.status_code().is_success() => {
                indexed += 1;
            }
            Ok(resp) => {
                let status = resp.status_code();
                let body = resp.text().await.unwrap_or_default();
                warn!("Failed to index telemetry event: status {} - {}", status, body);
                if indexed == 0 {
                    warn!("Problematic document: {}", doc_str);
                }
            }
            Err(e) => {
                warn!("Failed to index telemetry event: {}", e);
            }
        }
    }

    info!(
        "Indexed {}/{} telemetry events to {}",
        indexed,
        batch.len(),
        index_name
    );

    Ok(())
}
