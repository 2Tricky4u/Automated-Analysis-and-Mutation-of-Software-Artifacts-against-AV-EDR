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

/// Derive detection outcome from exit code.
///
/// ```text
/// exit_code == -2  → detected=true,  outcome="MUTATION_FAILED"
/// exit_code == 0   → detected=false, outcome="FULL_EVASION"
/// exit_code == -1  → detected=false, outcome="TIMEOUT"
/// other            → detected=true,  outcome="MUTATION_SUCCESS"
/// ```
fn derive_detection_outcome(exit_code: i32) -> (&'static str, bool) {
    match exit_code {
        -2 => ("MUTATION_FAILED", true),
        0 => ("FULL_EVASION", false),
        -1 => ("TIMEOUT", false),
        _ => ("MUTATION_SUCCESS", true),
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
    job_id: &str,
    round_id: &str,
    run_id: &str,
    run_type: &str,
    outcome: &RunOutcome,
    mutations: &[String],
    vm_id: &str,
) -> anyhow::Result<()> {
    let index_name = helpers::es_index_name("runs");

    let (detection_outcome, derived_detected) = derive_detection_outcome(outcome.exit_code);
    // Use the RunOutcome.detected if available, otherwise use derived
    let detected = outcome.detected || derived_detected;

    let now = helpers::now_rfc3339();
    let mut doc = json!({
        "run_id": run_id,
        "job_id": job_id,
        "round_id": round_id,
        "run_type": run_type,
        "mutation_chain": mutations,
        "mutations": mutations,
        "vm_id": vm_id,
        "worker_id": vm_id,
        "status": if outcome.error.is_some() { "error" } else { "completed" },
        "detected": detected,
        "exit_code": outcome.exit_code,
        "detection_outcome": detection_outcome,
        "trace_mode": trace_mode_from_run_type(run_type),
        "finished_at": now,
        "timestamp": now,
    });

    // Add error details if present
    if let Some(ref error_msg) = outcome.error {
        doc.as_object_mut().unwrap().insert(
            "error".to_string(),
            json!({
                "class": "execution_error",
                "message": error_msg,
            }),
        );
    }

    let response = es
        .index(IndexParts::IndexId(&index_name, run_id))
        .body(doc)
        .send()
        .await?;

    helpers::check_index_response(response, "run", run_id).await?;

    info!(
        "Indexed run result {} (detected={}, outcome={})",
        run_id, detected, detection_outcome
    );
    Ok(())
}

/// Index a run status from StatusReport (legacy path).
/// This path has worker metadata but no exit_code/detected.
pub async fn index_run_status(
    es: &Elasticsearch,
    report: &StatusReport,
) -> anyhow::Result<()> {
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
