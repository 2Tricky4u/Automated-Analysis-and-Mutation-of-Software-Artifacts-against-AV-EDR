use crate::automutate::common::TelemetryData;
use crate::automutate::controller::StatusReport;
use crate::job;
use crate::round;
use elasticsearch::{Elasticsearch, IndexParts, http::transport::Transport};
use tracing::{info, warn};

/// Elasticsearch storage helpers for SchedulerService
impl crate::SchedulerService {
    /// Index telemetry batch to Elasticsearch
    pub async fn index_telemetry_batch(
        &self,
        batch: &[TelemetryData],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let index_name = format!("telemetry-{}", chrono::Utc::now().format("%Y.%m.%d"));
        let mut indexed = 0;

        // Index events individually (simpler API usage)
        for event in batch {
            // Parse payload to extract searchable fields, but keep original as string
            let mut payload_fields = if let Ok(payload_json) =
                serde_json::from_slice::<serde_json::Value>(&event.payload)
            {
                payload_json.as_object().cloned().unwrap_or_default()
            } else {
                Default::default()
            };

            // Handle typed_event variants (for events using structured proto instead of JSON payload)
            // This is critical for trace events which use typed_event.trace instead of payload
            if let Some(ref typed_event) = event.typed_event {
                use crate::automutate::common::telemetry_data::TypedEvent;
                match typed_event {
                    TypedEvent::Trace(trace) => {
                        // Extract trace event fields into payload_fields for indexing
                        payload_fields.insert("seq".to_string(), json!(trace.seq));
                        payload_fields.insert("file".to_string(), json!(&trace.file));
                        payload_fields.insert("line".to_string(), json!(trace.line));
                        payload_fields.insert("func".to_string(), json!(&trace.func));
                        payload_fields.insert("ts_us".to_string(), json!(trace.ts_us));
                    }
                    TypedEvent::Coverage(cov) => {
                        // Extract BB coverage fields into payload_fields for indexing
                        payload_fields.insert("total_bbs".to_string(), json!(cov.total_bbs));
                        payload_fields.insert("bb_ids".to_string(), json!(&cov.bb_ids));
                        payload_fields.insert("hit_counts".to_string(), json!(&cov.hit_counts));
                        payload_fields.insert("bitmap_size".to_string(), json!(cov.bitmap.len()));

                        // Store bitmap as Base64 for Elasticsearch (more efficient than raw bytes)
                        use base64::{Engine as _, engine::general_purpose};
                        let bitmap_b64 = general_purpose::STANDARD.encode(&cov.bitmap);
                        payload_fields.insert("bitmap_b64".to_string(), json!(bitmap_b64));
                    }
                    TypedEvent::Checkpoint(cp) => {
                        // Extract checkpoint fields into payload_fields for indexing
                        payload_fields.insert("checkpoint_name".to_string(), json!(&cp.name));
                        payload_fields.insert("ts_us".to_string(), json!(cp.ts_us));
                    }
                }
            }

            // Build document with flattened payload fields at top level for searchability
            let mut doc = json!({
                "job_id": event.job_id,
                "event_type": event.event_type,
                "timestamp": event.timestamp,
                "metadata": event.metadata,
                "indexed_at": chrono::Utc::now().to_rfc3339(),
            });

            // Merge payload fields into top level (makes them searchable)
            if let Some(obj) = doc.as_object_mut() {
                for (key, value) in payload_fields {
                    // Detect pointer/address fields by name pattern
                    let key_lower = key.to_lowercase();
                    let is_pointer_field = key_lower.contains("address")
                        || key_lower.contains("pointer")
                        || key_lower.contains("stack")
                        || key_lower.contains("base")
                        || key_lower.contains("limit")
                        || key_lower.contains("rva")
                        || key_lower.contains("offset") && value.is_number();

                    // Detect fields that should be numeric (daddr, saddr, port, etc.)
                    let should_be_numeric = key_lower.contains("addr")
                        || key_lower.contains("port")
                        || key_lower.contains("pid")
                        || key_lower.contains("tid")
                        || key_lower.contains("size");

                    // Smart conversion: keep small numbers as numbers, convert problematic ones
                    let converted_value = match value {
                        serde_json::Value::Number(n) => {
                            if is_pointer_field {
                                // Pointer/address field: always convert to hex string
                                if let Some(u) = n.as_u64() {
                                    json!(format!("0x{:X}", u))
                                } else if let Some(i) = n.as_i64() {
                                    json!(format!("0x{:X}", i))
                                } else {
                                    json!(n.to_string())
                                }
                            } else {
                                // Non-pointer field: check range
                                if let Some(u) = n.as_u64() {
                                    if u > i64::MAX as u64 {
                                        // Exceeds i64 range - convert to string
                                        json!(format!("0x{:X}", u))
                                    } else {
                                        // Safe range - keep as number for Kibana aggregations
                                        json!(u)
                                    }
                                } else if let Some(i) = n.as_i64() {
                                    // Signed integer in safe range
                                    json!(i)
                                } else if let Some(f) = n.as_f64() {
                                    // Float - keep as-is
                                    json!(f)
                                } else {
                                    // Fallback to string
                                    json!(n.to_string())
                                }
                            }
                        }
                        serde_json::Value::String(s) => {
                            // If field should be numeric but contains "unsupported" or other non-numeric string,
                            // convert to null to avoid Elasticsearch type conflicts
                            if should_be_numeric
                                && (s == "unsupported" || s.parse::<i64>().is_err())
                            {
                                json!(null)
                            } else {
                                json!(s)
                            }
                        }
                        serde_json::Value::Bool(b) => json!(b),
                        serde_json::Value::Null => json!(null),
                        serde_json::Value::Array(arr) => json!(arr),
                        serde_json::Value::Object(obj) => json!(obj),
                    };

                    // Prefix payload fields to avoid conflicts with top-level fields
                    obj.insert(format!("payload_{}", key), converted_value);
                }
            }

            let doc_str = serde_json::to_string(&doc).unwrap_or_default();

            let response = self
                .es_client
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
                    warn!("Failed to index event: status {} - {}", status, body);

                    // Log the problematic document for debugging (first occurrence only)
                    if indexed == 0 {
                        warn!("Problematic document: {}", doc_str);
                    }
                }
                Err(e) => {
                    warn!("Failed to index event: {}", e);
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

    /// Store run result to Elasticsearch
    /// Tries worker IP first, falls back to configured localhost if that fails
    pub async fn store_run_result(
        &self,
        report: &StatusReport,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let index_name = format!("runs-{}", chrono::Utc::now().format("%Y.%m"));

        let doc = json!({
            "run_id": report.run_id,
            "job_id": report.job_id,
            "worker_id": report.worker_id,
            "worker_ip": report.worker_ip,
            "artifact_name": report.artifact_name,
            "pid": report.pid,
            "status": report.event_type,
            "elapsed_seconds": report.elapsed_seconds,
            "telemetry_events_count": report.telemetry_events_count,
            "details": report.details,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        // Strategy: Try controller IP first (Elasticsearch runs on controller),
        // then fall back to configured localhost
        let controller_es_url = format!("http://{}:9200", self.controller_ip);

        // Try controller IP first (as seen from worker's network perspective)
        info!(
            "Attempting to store run result to Elasticsearch at controller IP: {}",
            controller_es_url
        );
        match self
            .try_index_to_es(&controller_es_url, &index_name, &report.run_id, &doc)
            .await
        {
            Ok(()) => {
                info!(
                    "[OK] Stored run result to controller ES ({}): {} (status: {})",
                    self.controller_ip, report.run_id, report.event_type
                );
                return Ok(());
            }
            Err(e) => {
                warn!(
                    "Failed to store to controller ES ({}): {} - falling back to localhost",
                    controller_es_url, e
                );
            }
        }

        // Fallback to configured localhost client
        info!("Falling back to configured Elasticsearch client (localhost)");
        let response = self
            .es_client
            .index(IndexParts::IndexId(&index_name, &report.run_id))
            .body(doc)
            .send()
            .await?;

        if response.status_code().is_success() {
            info!(
                "[OK] Stored run result to localhost ES: {} (status: {})",
                report.run_id, report.event_type
            );
        } else {
            warn!(
                "Index run result returned non-success status: {}",
                response.status_code()
            );
        }

        Ok(())
    }

    /// Try to index document to a specific Elasticsearch URL
    pub async fn try_index_to_es(
        &self,
        es_url: &str,
        index_name: &str,
        doc_id: &str,
        doc: &serde_json::Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Create temporary ES client for this specific URL
        let es_transport = Transport::single_node(es_url)?;
        let es_client = Elasticsearch::new(es_transport);

        let response = es_client
            .index(IndexParts::IndexId(index_name, doc_id))
            .body(doc.clone())
            .send()
            .await?;

        if response.status_code().is_success() {
            Ok(())
        } else {
            Err(format!(
                "Elasticsearch returned non-success status: {}",
                response.status_code()
            )
            .into())
        }
    }

    /// Create Elasticsearch index template for jobs
    pub async fn create_jobs_index_template(&self) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let template = json!({
            "index_patterns": ["jobs-*"],
            "template": {
                "settings": {
                    "number_of_shards": 1,
                    "number_of_replicas": 0
                },
                "mappings": {
                    "properties": {
                        "job_id": { "type": "keyword" },
                        "status": { "type": "keyword" },
                        "template_name": { "type": "keyword" },
                        "source_file": { "type": "keyword" },
                        "trace_mode": { "type": "keyword" },
                        "priority": { "type": "integer" },
                        "current_round": { "type": "integer" },
                        "max_rounds": { "type": "integer" },
                        "stop_on_evasion": { "type": "boolean" },
                        "stop_on_detection": { "type": "boolean" },
                        "created_at": { "type": "date" },
                        "updated_at": { "type": "date" }
                    }
                }
            }
        });

        self.es_client
            .indices()
            .put_index_template(elasticsearch::indices::IndicesPutIndexTemplateParts::Name(
                "jobs-template",
            ))
            .body(template)
            .send()
            .await?;

        info!("Created Elasticsearch index template: jobs-template");
        Ok(())
    }

    /// Create Elasticsearch index template for rounds
    pub async fn create_rounds_index_template(&self) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let template = json!({
            "index_patterns": ["rounds-*"],
            "template": {
                "settings": {
                    "number_of_shards": 1,
                    "number_of_replicas": 0
                },
                "mappings": {
                    "properties": {
                        "round_id": { "type": "keyword" },
                        "job_id": { "type": "keyword" },
                        "round_number": { "type": "integer" },
                        "mutations": { "type": "keyword" },
                        "detected": { "type": "boolean" },
                        "behavior_match": { "type": "boolean" },
                        "evasion_score": { "type": "float" },
                        "completed_at": { "type": "date" }
                    }
                }
            }
        });

        self.es_client
            .indices()
            .put_index_template(elasticsearch::indices::IndicesPutIndexTemplateParts::Name(
                "rounds-template",
            ))
            .body(template)
            .send()
            .await?;

        info!("Created Elasticsearch index template: rounds-template");
        Ok(())
    }

    /// Index job document to Elasticsearch
    pub async fn index_job(&self, job: &job::Job) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let index_name = format!("jobs-{}", chrono::Utc::now().format("%Y.%m"));

        let doc = json!({
            "job_id": job.id,
            "status": job.status.to_string(),
            "template_name": job.template_name,
            "source_file": job.source_file,
            "trace_mode": job.trace_mode,
            "priority": job.priority,
            "current_round": job.current_round,
            "max_rounds": job.max_rounds,
            "stop_on_evasion": job.stop_on_evasion,
            "stop_on_detection": job.stop_on_detection,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "updated_at": chrono::Utc::now().to_rfc3339(),
        });

        self.es_client
            .index(IndexParts::IndexId(&index_name, &job.id))
            .body(doc)
            .send()
            .await?;

        Ok(())
    }

    /// Index round document to Elasticsearch
    pub async fn index_round(
        &self,
        job_id: &str,
        round: &round::RoundSummary,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let index_name = format!("rounds-{}", chrono::Utc::now().format("%Y.%m"));

        let doc = json!({
            "round_id": round.round_id,
            "job_id": job_id,
            "round_number": round.round_number,
            "mutations": round.mutations,
            "detected": round.detected,
            "behavior_match": round.behavior_match,
            "evasion_score": round.evasion_score,
            "completed_at": round.completed_at.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });

        let doc_id = format!("{}/{}", job_id, round.round_id);
        self.es_client
            .index(IndexParts::IndexId(&index_name, &doc_id))
            .body(doc)
            .send()
            .await?;

        Ok(())
    }
}
