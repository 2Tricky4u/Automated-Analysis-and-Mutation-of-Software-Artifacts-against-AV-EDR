//! Round indexing — index round summaries to ES.
//!
//! Index pattern: rounds-YYYY.MM
//! Document ID: {job_id}/{round_id}

use super::helpers;
use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
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
}

/// Index a completed round summary with mutation recipe and run IDs.
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
    });

    let doc_id = format!("{}/{}", params.job_id, summary.round_id.0);
    let response = es
        .index(IndexParts::IndexId(&index_name, &doc_id))
        .body(doc)
        .send()
        .await?;

    helpers::check_index_response(response, "round", &doc_id).await?;

    info!("Indexed round {} to {}", doc_id, index_name);
    Ok(())
}
