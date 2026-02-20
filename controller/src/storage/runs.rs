//! Run indexing — index run results with detection outcome derivation.
//!
//! Two indexing paths:
//! 1. `index_run_result()` — called from RoundCompleted event (has exit_code, detected)
//! 2. `index_run_status()` — called from StatusReport handler (legacy path)
//!
//! Index pattern: runs-YYYY.MM
//! Document ID: run_id (for index_run_result), auto-generated (for index_run_status)

use super::helpers;
use crate::automutate::controller::StatusReport;
use crate::dispatch::types::RunOutcome;
use elasticsearch::{Elasticsearch, IndexParts};
use serde_json::json;
use tracing::info;

/// Context for indexing a run result (groups run identity fields).
pub struct RunIndexParams<'a> {
    pub job_id: &'a str,
    pub round_id: &'a str,
    pub run_id: &'a str,
    pub run_type: &'a str,
    pub outcome: &'a RunOutcome,
    pub mutations: &'a [String],
    pub vm_id: &'a str,
}

/// Derive detection outcome from verdict (preferred) or exit code (legacy fallback).
///
/// Uses `automutate_common::DetectionVerdict` as the single source of truth for
/// verdict → (detection_outcome, detected) mapping.
fn derive_detection_outcome(exit_code: i32, verdict: &str) -> (&'static str, bool) {
    if let Some(v) = automutate_common::DetectionVerdict::from_verdict_str(verdict) {
        return (v.detection_outcome(), v.is_detected());
    }
    // Legacy exit-code-only fallback (old workers without classifier)
    match exit_code {
        0 => ("FULL_EVASION", false),
        -1 => ("KILLED_PRE_PAYLOAD", true), // conservative
        _ => ("KILLED_PRE_PAYLOAD", true),  // no checkpoint info available
    }
}

/// Derive trace_mode from run_type string.
fn trace_mode_from_run_type(run_type: &str) -> &str {
    match run_type {
        "baseline" => "off",
        "instrumented" => "lines",
        _ => "unknown",
    }
}

/// Index a run result from RoundCompleted event.
/// This is the PRIMARY path with exit_code, detected, round context.
pub async fn index_run_result(
    es: &Elasticsearch,
    params: &RunIndexParams<'_>,
) -> anyhow::Result<()> {
    let index_name = helpers::es_index_name("runs");

    let (detection_outcome, derived_detected) =
        derive_detection_outcome(params.outcome.exit_code, &params.outcome.detection_verdict);
    // Use the RunOutcome.detected if available, otherwise use derived
    let detected = params.outcome.detected || derived_detected;

    let now = helpers::now_rfc3339();
    let mut doc = json!({
        "run_id": params.run_id,
        "job_id": params.job_id,
        "round_id": params.round_id,
        "run_type": params.run_type,
        "mutation_chain": params.mutations,
        "mutations": params.mutations,
        "vm_id": params.vm_id,
        "worker_id": params.vm_id,
        "status": if params.outcome.error.is_some() { "error" } else { "completed" },
        "detected": detected,
        "exit_code": params.outcome.exit_code,
        "success": params.outcome.success,
        "elapsed_ms": params.outcome.elapsed_ms,
        "detection_outcome": detection_outcome,
        "detection_verdict": params.outcome.detection_verdict,
        "last_checkpoint": params.outcome.last_checkpoint,
        "trace_mode": trace_mode_from_run_type(params.run_type),
        "finished_at": now,
        "timestamp": now,
    });

    // Add error details if present
    if let Some(ref error_msg) = params.outcome.error {
        doc.as_object_mut().unwrap().insert(
            "error".to_string(),
            json!({
                "class": "execution_error",
                "message": error_msg,
            }),
        );
    }

    let response = es
        .index(IndexParts::IndexId(&index_name, params.run_id))
        .body(doc)
        .send()
        .await?;

    helpers::check_index_response(response, "run", params.run_id).await?;

    info!(
        "Indexed run result {} (detected={}, outcome={})",
        params.run_id, detected, detection_outcome
    );
    Ok(())
}

/// Index a run status from StatusReport (legacy path).
/// This path has worker metadata but no exit_code/detected.
pub async fn index_run_status(es: &Elasticsearch, report: &StatusReport) -> anyhow::Result<()> {
    let index_name = helpers::es_index_name("runs");

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
        "timestamp": helpers::now_rfc3339(),
    });

    let response = es
        .index(IndexParts::Index(&index_name))
        .body(doc)
        .send()
        .await?;

    if !response.status_code().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to index run status: {}",
            response.status_code()
        ));
    }

    Ok(())
}
