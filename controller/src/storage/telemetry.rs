//! Telemetry indexing with payload flattening and typed_event handling.
//!
//! Salvaged from the dead storage/elasticsearch.rs code with improvements:
//! - TelemetryContext for correlation keys (run_id, round_id, vm_id)
//! - Smart numeric conversion (pointers→hex, large u64→hex)
//! - Daily index pattern: telemetry-YYYY.MM.DD

use super::TelemetryContext;
use super::helpers;
use crate::automutate::common::TelemetryData;
use elasticsearch::{BulkOperation, BulkParts, Elasticsearch};
use elasticsearch::params::Refresh;
use serde_json::{json, Value};
use tracing::{info, warn};

pub async fn index_telemetry_batch(
    es: &Elasticsearch,
    batch: &[TelemetryData],
    context: &TelemetryContext,
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    let index_name = helpers::es_index_name_daily("telemetry");
    let mut ops: Vec<BulkOperation<Value>> = Vec::with_capacity(batch.len());

    for event in batch {
        // Parse payload to extract searchable fields
        let mut payload_fields =
            if let Ok(payload_json) = serde_json::from_slice::<serde_json::Value>(&event.payload) {
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

                    use base64::{Engine as _, engine::general_purpose};
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
            "indexed_at": helpers::now_rfc3339(),
            "vm_id": context.vm_id,
        });

        // Add optional correlation keys
        helpers::insert_optional_field(&mut doc, "run_id", context.run_id.as_deref());
        helpers::insert_optional_field(&mut doc, "round_id", context.round_id.as_deref());

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
                        if should_be_numeric && (s == "unsupported" || s.parse::<i64>().is_err()) {
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

        ops.push(BulkOperation::index(doc).into());
    }

    let total = ops.len();
    let response = es
        .bulk(BulkParts::Index(&index_name))
        .body(ops)
        .refresh(Refresh::WaitFor)
        .send()
        .await;

    match response {
        Ok(resp) if resp.status_code().is_success() => {
            let body: Value = resp.json().await.unwrap_or_default();
            let mut indexed = 0usize;
            let mut failed = 0usize;
            if let Some(items) = body["items"].as_array() {
                for item in items {
                    let status = item["index"]["status"].as_u64().unwrap_or(0);
                    if (200..300).contains(&status) {
                        indexed += 1;
                    } else {
                        failed += 1;
                        if failed <= 3 {
                            warn!(
                                "Bulk index item error: {}",
                                item["index"]["error"]
                            );
                        }
                    }
                }
            }
            if failed > 0 {
                warn!(
                    "Indexed {}/{} telemetry events to {} ({} failed)",
                    indexed, total, index_name, failed
                );
            } else {
                info!(
                    "Indexed {}/{} telemetry events to {}",
                    indexed, total, index_name
                );
            }
        }
        Ok(resp) => {
            let status = resp.status_code();
            let body = resp.text().await.unwrap_or_default();
            warn!(
                "Bulk telemetry index failed: status {} - {}",
                status, body
            );
        }
        Err(e) => {
            warn!("Bulk telemetry index failed: {}", e);
        }
    }

    Ok(())
}
