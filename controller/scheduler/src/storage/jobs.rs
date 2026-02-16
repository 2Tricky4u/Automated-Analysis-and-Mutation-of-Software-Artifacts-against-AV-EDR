//! Job indexing — create and update job documents in ES.
//!
//! Index pattern: jobs-YYYY.MM
//! Document ID: job_id

use super::queries;
use crate::dispatch::types::{JobOutcome, JobSession};
use elasticsearch::{Elasticsearch, IndexParts, UpdateParts};
use serde_json::json;
use tracing::{info, warn};

/// Index a new job document when a job is submitted.
pub async fn index_job(es: &Elasticsearch, job: &JobSession) -> anyhow::Result<()> {
    let index_name = format!("jobs-{}", chrono::Utc::now().format("%Y.%m"));

    let created_at_secs = job
        .created_at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let doc = json!({
        "job_id": job.id.0,
        "status": "queued",
        "template_name": "modular_template",
        "source_file": job.build_spec.payload_path.display().to_string(),
        "trace_mode": "dual",
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
        "created_at": created_at_secs,
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });

    let response = es
        .index(IndexParts::IndexId(&index_name, &job.id.0))
        .body(doc)
        .send()
        .await?;

    if !response.status_code().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to index job: {}",
            response.status_code()
        ));
    }

    info!("Indexed job {} to {}", job.id.0, index_name);
    Ok(())
}

/// Update job status to "running" and set started_at.
pub async fn update_job_started(es: &Elasticsearch, job_id: &str) -> anyhow::Result<()> {
    let doc = json!({
        "doc": {
            "status": "running",
            "started_at": chrono::Utc::now().to_rfc3339(),
            "updated_at": chrono::Utc::now().to_rfc3339(),
        }
    });

    // Find the job document first to get the actual index
    let index_name = queries::find_index(es, "jobs-*", "job_id", job_id).await;

    if let Some(ref idx) = index_name {
        let response = es
            .update(UpdateParts::IndexId(idx, job_id))
            .body(doc)
            .send()
            .await?;

        if !response.status_code().is_success() {
            warn!("Failed to update job started status: {}", response.status_code());
        } else {
            info!("Updated job {} status to running", job_id);
        }
    } else {
        warn!("Job {} not found in ES for started update", job_id);
    }

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
        "updated_at": chrono::Utc::now().to_rfc3339(),
        "completed_at": chrono::Utc::now().to_rfc3339(),
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

    let index_name = queries::find_index(es, "jobs-*", "job_id", job_id).await;

    if let Some(ref idx) = index_name {
        let response = es
            .update(UpdateParts::IndexId(idx, job_id))
            .body(body)
            .send()
            .await?;

        if !response.status_code().is_success() {
            let err_body = response.text().await.unwrap_or_default();
            warn!(
                "Failed to update job {} status to {}: {}",
                job_id, status, err_body
            );
        } else {
            info!("Updated job {} status to {}", job_id, status);
        }
    } else {
        warn!("Job {} not found in ES for status update", job_id);
    }

    Ok(())
}
