//! Round indexing — index round summaries to ES.
//!
//! Index pattern: rounds-YYYY.MM
//! Document ID: {job_id}/{round_id}

use super::helpers;
use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
use elasticsearch::{Elasticsearch, IndexParts};
use serde_json::json;
use tracing::info;

/// Index a completed round summary with mutation recipe and run IDs.
pub async fn index_round(
    es: &Elasticsearch,
    job_id: &str,
    summary: &RoundSummary,
    mutation_specs: &[MutationSpec],
    baseline_run_id: &str,
    instrumented_run_id: &str,
    started_at: Option<&str>,
    modules: Option<&ModuleSelectionSpec>,
) -> anyhow::Result<()> {
    let index_name = helpers::es_index_name("rounds");

    // Serialize mutation recipe (full specs with params)
    let mutation_recipe: Vec<serde_json::Value> = mutation_specs
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
        "job_id": job_id,
        "round_number": summary.round_number,
        "mutations": summary.mutations,
        "mutation_recipe": mutation_recipe,
        "baseline_run_id": baseline_run_id,
        "instrumented_run_id": instrumented_run_id,
        "detected": summary.detected,
        "behavior_match": summary.behavior_match,
        "evasion_score": summary.evasion_score,
        "differential_category": summary.differential_category.as_str(),
        "modules": modules.map(|m| json!({
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
        "started_at": started_at,
    });

    let doc_id = format!("{}/{}", job_id, summary.round_id.0);
    let response = es
        .index(IndexParts::IndexId(&index_name, &doc_id))
        .body(doc)
        .send()
        .await?;

    helpers::check_index_response(response, "round", &doc_id).await?;

    info!("Indexed round {} to {}", doc_id, index_name);
    Ok(())
}
