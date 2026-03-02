//! Index template bootstrap — create/update ES index templates on startup.
//!
//! Templates define mappings for: jobs, rounds, runs, telemetry.
//! Uses `_meta.version` for idempotent updates.

use elasticsearch::Elasticsearch;
use elasticsearch::indices::IndicesPutIndexTemplateParts;
use serde_json::json;
use tracing::{info, warn};

pub async fn ensure_templates(es: &Elasticsearch) -> anyhow::Result<()> {
    let results = tokio::join!(
        create_jobs_template(es),
        create_rounds_template(es),
        create_runs_template(es),
        create_telemetry_template(es),
        create_tokens_template(es),
    );

    // Log failures but don't crash on template errors
    if let Err(e) = results.0 {
        warn!("Failed to create jobs template: {}", e);
    }
    if let Err(e) = results.1 {
        warn!("Failed to create rounds template: {}", e);
    }
    if let Err(e) = results.2 {
        warn!("Failed to create runs template: {}", e);
    }
    if let Err(e) = results.3 {
        warn!("Failed to create telemetry template: {}", e);
    }
    if let Err(e) = results.4 {
        warn!("Failed to create tokens template: {}", e);
    }

    info!("Index templates ensured");
    Ok(())
}

async fn create_jobs_template(es: &Elasticsearch) -> anyhow::Result<()> {
    let template = json!({
        "index_patterns": ["jobs-*"],
        "template": {
            "settings": {
                "number_of_shards": 1,
                "number_of_replicas": 0
            },
            "mappings": {
                "_meta": { "version": 3 },
                "properties": {
                    "job_id": { "type": "keyword" },
                    "status": { "type": "keyword" },
                    "template_name": { "type": "keyword" },
                    "source_file": { "type": "keyword" },
                    "trace_mode": { "type": "keyword" },
                    "encoding": { "type": "keyword" },
                    "priority": { "type": "integer" },
                    "current_round": { "type": "integer" },
                    "max_rounds": { "type": "integer" },
                    "stop_on_evasion": { "type": "boolean" },
                    "stop_on_detection": { "type": "boolean" },
                    "target_os": { "type": "keyword" },
                    "required_capabilities": { "type": "keyword" },
                    "modules": {
                        "properties": {
                            "carrier": { "type": "keyword" },
                            "decoder": { "type": "keyword" },
                            "antiemulation": { "type": "keyword" },
                            "deconditioner": { "type": "keyword" },
                            "guardrail": { "type": "keyword" },
                            "virtualprotect": { "type": "keyword" },
                            "decoy": { "type": "keyword" }
                        }
                    },
                    "outcome": {
                        "properties": {
                            "total_rounds": { "type": "integer" },
                            "detection_outcome": { "type": "keyword" },
                            "reason": { "type": "text" },
                            "error": { "type": "text" }
                        }
                    },
                    "created_at": { "type": "date" },
                    "started_at": { "type": "date" },
                    "completed_at": { "type": "date" },
                    "updated_at": { "type": "date" }
                }
            }
        }
    });

    es.indices()
        .put_index_template(IndicesPutIndexTemplateParts::Name("jobs-template"))
        .body(template)
        .send()
        .await?;

    info!("Created index template: jobs-template");
    Ok(())
}

async fn create_rounds_template(es: &Elasticsearch) -> anyhow::Result<()> {
    let template = json!({
        "index_patterns": ["rounds-*"],
        "template": {
            "settings": {
                "number_of_shards": 1,
                "number_of_replicas": 0
            },
            "mappings": {
                "_meta": { "version": 6 },
                "properties": {
                    "round_id": { "type": "keyword" },
                    "job_id": { "type": "keyword" },
                    "round_number": { "type": "integer" },
                    "mutations": { "type": "keyword" },
                    "mutation_recipe": {
                        "properties": {
                            "id": { "type": "keyword" },
                            "params": { "type": "object", "enabled": false }
                        }
                    },
                    "baseline_run_id": { "type": "keyword" },
                    "instrumented_run_id": { "type": "keyword" },
                    "detected": { "type": "boolean" },
                    "behavior_match": { "type": "boolean" },
                    "evasion_score": { "type": "float" },
                    "differential_category": { "type": "keyword" },
                    "modules": {
                        "properties": {
                            "carrier": { "type": "keyword" },
                            "decoder": { "type": "keyword" },
                            "antiemulation": { "type": "keyword" },
                            "deconditioner": { "type": "keyword" },
                            "guardrail": { "type": "keyword" },
                            "virtualprotect": { "type": "keyword" },
                            "decoy": { "type": "keyword" }
                        }
                    },
                    "status": { "type": "keyword" },
                    "started_at": { "type": "date" },
                    "completed_at": { "type": "date" },
                    "assembled_source": { "type": "text", "index": false },
                    "detection_verdict": { "type": "keyword" }
                }
            }
        }
    });

    es.indices()
        .put_index_template(IndicesPutIndexTemplateParts::Name("rounds-template"))
        .body(template)
        .send()
        .await?;

    info!("Created index template: rounds-template");
    Ok(())
}

async fn create_runs_template(es: &Elasticsearch) -> anyhow::Result<()> {
    let template = json!({
        "index_patterns": ["runs-*"],
        "template": {
            "settings": {
                "number_of_shards": 1,
                "number_of_replicas": 0
            },
            "mappings": {
                "_meta": { "version": 4 },
                "properties": {
                    "run_id": { "type": "keyword" },
                    "job_id": { "type": "keyword" },
                    "round_id": { "type": "keyword" },
                    "run_type": { "type": "keyword" },
                    "mutation_chain": { "type": "keyword" },
                    "mutations": { "type": "keyword" },
                    "vm_id": { "type": "keyword" },
                    "worker_id": { "type": "keyword" },
                    "worker_ip": { "type": "ip" },
                    "artifact_name": { "type": "keyword" },
                    "pid": { "type": "long" },
                    "status": { "type": "keyword" },
                    "detected": { "type": "boolean" },
                    "exit_code": { "type": "integer" },
                    "detection_outcome": { "type": "keyword" },
                    "detection_verdict": { "type": "keyword" },
                    "last_checkpoint": { "type": "keyword" },
                    "trace_mode": { "type": "keyword" },
                    "elapsed_seconds": { "type": "float" },
                    "telemetry_events_count": { "type": "long" },
                    "details": { "type": "text" },
                    "error": {
                        "properties": {
                            "class": { "type": "keyword" },
                            "message": { "type": "text" }
                        }
                    },
                    "finished_at": { "type": "date" },
                    "timestamp": { "type": "date" }
                }
            }
        }
    });

    es.indices()
        .put_index_template(IndicesPutIndexTemplateParts::Name("runs-template"))
        .body(template)
        .send()
        .await?;

    info!("Created index template: runs-template");
    Ok(())
}

async fn create_telemetry_template(es: &Elasticsearch) -> anyhow::Result<()> {
    let template = json!({
        "index_patterns": ["telemetry-*"],
        "template": {
            "settings": {
                "number_of_shards": 1,
                "number_of_replicas": 0
            },
            "mappings": {
                "_meta": { "version": 3 },
                "dynamic_templates": [
                    {
                        "payload_strings": {
                            "path_match": "payload_*",
                            "match_mapping_type": "string",
                            "mapping": { "type": "keyword" }
                        }
                    },
                    {
                        "payload_longs": {
                            "path_match": "payload_*",
                            "match_mapping_type": "long",
                            "mapping": { "type": "long" }
                        }
                    }
                ],
                "properties": {
                    "job_id": { "type": "keyword" },
                    "run_id": { "type": "keyword" },
                    "round_id": { "type": "keyword" },
                    "vm_id": { "type": "keyword" },
                    "event_type": { "type": "keyword" },
                    "source": { "type": "keyword" },
                    "timestamp": { "type": "date" },
                    "indexed_at": { "type": "date" },
                    "metadata": { "type": "object", "enabled": false },
                    "payload_seq": { "type": "long" },
                    "payload_file": { "type": "keyword" },
                    "payload_line": { "type": "long" },
                    "payload_func": { "type": "keyword" },
                    "payload_ts_us": { "type": "long" },
                    "payload_total_bbs": { "type": "long" },
                    "payload_checkpoint_name": { "type": "keyword" },
                    "payload_bitmap_b64": { "type": "keyword", "index": false }
                }
            }
        }
    });

    es.indices()
        .put_index_template(IndicesPutIndexTemplateParts::Name("telemetry-template"))
        .body(template)
        .send()
        .await?;

    info!("Created index template: telemetry-template");
    Ok(())
}

async fn create_tokens_template(es: &Elasticsearch) -> anyhow::Result<()> {
    let template = json!({
        "index_patterns": ["tokens-*"],
        "template": {
            "settings": {
                "number_of_shards": 1,
                "number_of_replicas": 0
            },
            "mappings": {
                "_meta": { "version": 1 },
                "properties": {
                    "job_id": { "type": "keyword" },
                    "round_id": { "type": "keyword" },
                    "run_id": { "type": "keyword" },
                    "detected": { "type": "boolean" },
                    "differential_category": { "type": "keyword" },
                    "evasion_score": { "type": "float" },
                    "modules": {
                        "properties": {
                            "carrier": { "type": "keyword" },
                            "decoder": { "type": "keyword" },
                            "antiemulation": { "type": "keyword" },
                            "deconditioner": { "type": "keyword" },
                            "guardrail": { "type": "keyword" },
                            "virtualprotect": { "type": "keyword" },
                            "decoy": { "type": "keyword" }
                        }
                    },
                    "mutations": { "type": "keyword" },
                    "tokens": { "type": "keyword" },
                    "token_count": { "type": "integer" },
                    "timestamp": { "type": "date" }
                }
            }
        }
    });

    es.indices()
        .put_index_template(IndicesPutIndexTemplateParts::Name("tokens-template"))
        .body(template)
        .send()
        .await?;

    info!("Created index template: tokens-template");
    Ok(())
}
