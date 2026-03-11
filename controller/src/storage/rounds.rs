//! Round indexing — index round summaries to ES.
//!
//! Index pattern: rounds-YYYY.MM
//! Document ID: {job_id}/{round_id}

use super::helpers;
use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
use crate::triage::source_resolver::CoverageResult;
use elasticsearch::{Elasticsearch, IndexParts};
use serde_json::json;
use tracing::info;

/// Context for indexing a round (groups identity and metadata fields).
pub struct RoundIndexParams<'a> {
    pub job_id: &'a str,
    pub summary: &'a RoundSummary,
    pub mutation_specs: &'a [MutationSpec],
    pub baseline_run_id: &'a str,
    pub instrumented_run_id: &'a str,
    pub started_at: Option<&'a str>,
    pub modules: Option<&'a ModuleSelectionSpec>,
    /// Pre-instrumentation assembled C source for line trace resolution.
    pub assembled_source: Option<&'a str>,
    /// Dryrun exit code (None if dryrun was not available)
    pub dry_run_exit_code: Option<i32>,
    /// Whether a dryrun result was used for this round
    pub has_dryrun: bool,
    /// Dryrun run ID (for cross-referencing)
    pub dryrun_run_id: Option<&'a str>,
}

/// Index a completed round summary with mutation recipe and run IDs.
///
/// # Errors
///
/// Returns an error if the Elasticsearch index request fails or the response
/// indicates a non-success status code.
pub async fn index_round(es: &Elasticsearch, params: &RoundIndexParams<'_>) -> anyhow::Result<()> {
    let index_name = helpers::es_index_name("rounds");
    let summary = params.summary;

    // Serialize mutation recipe (full specs with params)
    let mutation_recipe: Vec<serde_json::Value> = params
        .mutation_specs
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "params": m.params,
            })
        })
        .collect();

    let doc = json!({
        "round_id": summary.round_id.0,
        "job_id": params.job_id,
        "round_number": summary.round_number,
        "mutations": summary.mutations,
        "mutation_recipe": mutation_recipe,
        "baseline_run_id": params.baseline_run_id,
        "instrumented_run_id": params.instrumented_run_id,
        "detected": summary.detected,
        "behavior_match": summary.behavior_match,
        "evasion_score": summary.evasion_score,
        "differential_category": summary.differential_category.as_str(),
        "modules": params.modules.map(|m| json!({
            "carrier": m.carrier,
            "decoder": m.decoder,
            "antiemulation": m.antiemulation,
            "deconditioner": m.deconditioner,
            "guardrail": m.guardrail,
            "virtualprotect": m.virtualprotect,
            "decoy": m.decoy,
        })),
        "status": "completed",
        "completed_at": helpers::system_time_to_rfc3339(summary.completed_at),
        "started_at": params.started_at,
        "assembled_source": params.assembled_source,
        "dry_run_exit_code": params.dry_run_exit_code,
        "has_dryrun": params.has_dryrun,
        "dryrun_run_id": params.dryrun_run_id,
        "detection_verdict": params.summary.detection_verdict,
    });

    let doc_id = format!("{}/{}", params.job_id, summary.round_id.0);
    let response = es
        .index(IndexParts::IndexId(&index_name, &doc_id))
        .body(doc)
        .refresh(elasticsearch::params::Refresh::WaitFor)
        .send()
        .await?;

    helpers::check_index_response(response, "round", &doc_id).await?;

    info!("Indexed round {} to {}", doc_id, index_name);
    Ok(())
}

/// Update an existing round document with computed coverage fields.
///
/// # Errors
///
/// Returns an error if the round document cannot be located by `round_id`
/// or the update request fails.
pub async fn update_round_coverage(
    es: &Elasticsearch,
    job_id: &str,
    round_id: &str,
    coverage: &CoverageResult,
) -> anyhow::Result<()> {
    let doc_id = format!("{}/{}", job_id, round_id);

    let function_coverage: Vec<serde_json::Value> = coverage
        .functions
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "total": f.total_lines,
                "executed": f.executed_lines,
                "percent": (f.percent * 10.0).round() / 10.0,
            })
        })
        .collect();

    let body = json!({
        "doc": {
            "coverage_total_lines": coverage.total_lines,
            "coverage_executable_lines": coverage.total_executable,
            "coverage_executed_lines": coverage.executed_lines,
            "coverage_percent": (coverage.coverage_percent * 10.0).round() / 10.0,
            "cutoff_line": coverage.cutoff_line,
            "cutoff_func": coverage.cutoff_func,
            "function_coverage": function_coverage,
        }
    });

    helpers::update_doc_by_es_id(es, "rounds-*", &doc_id, body, "round-coverage").await?;

    info!(
        "Updated round {} coverage: {:.1}% ({}/{})",
        doc_id, coverage.coverage_percent, coverage.executed_lines, coverage.total_executable
    );
    Ok(())
}

/// Update an existing round document with a blended evasion score.
///
/// # Errors
///
/// Returns an error if the round document cannot be located by `round_id`
/// or the update request fails.
pub async fn update_round_evasion_score(
    es: &Elasticsearch,
    job_id: &str,
    round_id: &str,
    blended_score: f64,
) -> anyhow::Result<()> {
    let body = json!({
        "doc": {
            "evasion_score": (blended_score * 1000.0).round() / 1000.0,
            "evasion_score_blended": true,
        }
    });

    let doc_id = format!("{}/{}", job_id, round_id);
    helpers::update_doc_by_es_id(
        es,
        "rounds-*",
        &doc_id,
        body,
        "round-evasion-blend",
    )
    .await?;

    info!(
        "Updated round {}/{} blended evasion score: {:.3}",
        job_id, round_id, blended_score
    );
    Ok(())
}

/// Patch an existing round document with late-arriving dryrun data.
///
/// Updates: has_dryrun, dry_run_exit_code, dryrun_run_id, differential_category,
/// detection_verdict, detected, evasion_score.
///
/// # Errors
///
/// Returns an error if the round document cannot be located or the update fails.
pub async fn update_round_dryrun(
    es: &Elasticsearch,
    job_id: &str,
    round_id: &str,
    dryrun_run_id: &str,
    dry_run_exit_code: i32,
    differential_category: &str,
    detection_verdict: &str,
    detected: bool,
    evasion_score: f64,
) -> anyhow::Result<()> {
    let body = json!({
        "doc": {
            "has_dryrun": true,
            "dry_run_exit_code": dry_run_exit_code,
            "dryrun_run_id": dryrun_run_id,
            "differential_category": differential_category,
            "detection_verdict": detection_verdict,
            "detected": detected,
            "evasion_score": (evasion_score * 1000.0).round() / 1000.0,
            "late_dryrun": true,
            "dryrun_arrived_at": helpers::now_rfc3339(),
        }
    });

    let doc_id = format!("{}/{}", job_id, round_id);
    helpers::update_doc_by_es_id(es, "rounds-*", &doc_id, body, "round-late-dryrun").await?;

    info!(
        "Updated round {} with late dryrun (cat={}, verdict={})",
        doc_id, differential_category, detection_verdict
    );
    Ok(())
}
