//! Job indexing — create and update job documents in ES.
//!
//! Index pattern: jobs-YYYY.MM
//! Document ID: job_id

use super::helpers;
use crate::dispatch::types::{JobOutcome, JobSession};
use elasticsearch::{Elasticsearch, IndexParts};
use serde_json::json;
use tracing::info;

/// Index a new job document when a job is submitted.
pub async fn index_job(es: &Elasticsearch, job: &JobSession) -> anyhow::Result<()> {
    let index_name = helpers::es_index_name("jobs");

    let doc = json!({
        "job_id": job.id.0,
        "status": "queued",
        "template_name": "modular_template",
        "source_file": job.build_spec.payload_path.display().to_string(),
        "trace_mode": job.trace_mode,
        "encoding": job.build_spec.encoding,
        "priority": 0,
        "current_round": job.current_round,
        "max_rounds": job.max_rounds,
        "stop_on_evasion": job.stop_on_evasion,
        "stop_on_detection": false,
        "target_os": job.target_os,
        "required_capabilities": job.required_capabilities,
        "modules": {
            "carrier": job.build_spec.modules.carrier,
            "decoder": job.build_spec.modules.decoder,
            "antiemulation": job.build_spec.modules.antiemulation,
            "deconditioner": job.build_spec.modules.deconditioner,
            "guardrail": job.build_spec.modules.guardrail,
            "virtualprotect": job.build_spec.modules.virtualprotect,
            "decoy": job.build_spec.modules.decoy,
        },
        "created_at": helpers::system_time_to_rfc3339(job.created_at),
        "updated_at": helpers::now_rfc3339(),
    });

    let response = es
        .index(IndexParts::IndexId(&index_name, &job.id.0))
        .body(doc)
        .refresh(elasticsearch::params::Refresh::WaitFor)
        .send()
        .await?;

    helpers::check_index_response(response, "job", &job.id.0).await?;

    info!("Indexed job {} to {}", job.id.0, index_name);
    Ok(())
}

/// Update job status to "running" and set started_at.
pub async fn update_job_started(es: &Elasticsearch, job_id: &str) -> anyhow::Result<()> {
    let doc = json!({
        "doc": {
            "status": "running",
            "started_at": helpers::now_rfc3339(),
            "updated_at": helpers::now_rfc3339(),
        }
    });

    helpers::update_doc_by_id(es, "jobs-*", "job_id", job_id, doc, "job").await?;
    Ok(())
}

/// Update job progress (current_round) after a round completes.
pub async fn update_job_progress(
    es: &Elasticsearch,
    job_id: &str,
    current_round: u32,
) -> anyhow::Result<()> {
    let doc = json!({
        "doc": {
            "current_round": current_round,
            "updated_at": helpers::now_rfc3339(),
        }
    });

    helpers::update_doc_by_id(es, "jobs-*", "job_id", job_id, doc, "job").await?;
    Ok(())
}

/// Update job status (completed, stopped, failed) with optional outcome.
pub async fn update_job_status(
    es: &Elasticsearch,
    job_id: &str,
    status: &str,
    outcome: Option<&JobOutcome>,
) -> anyhow::Result<()> {
    let mut update_doc = json!({
        "status": status,
        "updated_at": helpers::now_rfc3339(),
        "completed_at": helpers::now_rfc3339(),
    });

    // Add outcome details
    if let Some(outcome) = outcome {
        match outcome {
            JobOutcome::Completed { rounds_completed } => {
                update_doc.as_object_mut().unwrap().insert(
                    "outcome".to_string(),
                    json!({
                        "total_rounds": rounds_completed,
                        "detection_outcome": "completed",
                    }),
                );
            }
            JobOutcome::Stopped { reason } => {
                update_doc.as_object_mut().unwrap().insert(
                    "outcome".to_string(),
                    json!({
                        "reason": reason,
                    }),
                );
            }
            JobOutcome::Failed { error } => {
                update_doc.as_object_mut().unwrap().insert(
                    "outcome".to_string(),
                    json!({
                        "error": error,
                    }),
                );
            }
        }
    }

    let body = json!({ "doc": update_doc });

    helpers::update_doc_by_id(es, "jobs-*", "job_id", job_id, body, "job").await?;
    Ok(())
}
