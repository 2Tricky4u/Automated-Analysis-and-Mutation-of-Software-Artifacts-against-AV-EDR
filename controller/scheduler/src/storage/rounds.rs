//! Round indexing — index round summaries to ES.
//!
//! Index pattern: rounds-YYYY.MM
//! Document ID: {job_id}/{round_id}

use crate::dispatch::types::{MutationSpec, RoundSummary};
use elasticsearch::{Elasticsearch, IndexParts};
use serde_json::json;
use tracing::{info, warn};

/// Index a completed round summary with mutation recipe and run IDs.
pub async fn index_round(
    es: &Elasticsearch,
    job_id: &str,
    summary: &RoundSummary,
    mutation_specs: &[MutationSpec],
    baseline_run_id: &str,
    instrumented_run_id: &str,
) -> anyhow::Result<()> {
    let index_name = format!("rounds-{}", chrono::Utc::now().format("%Y.%m"));

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

    let completed_at_secs = summary
        .completed_at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

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
        "status": "completed",
        "completed_at": completed_at_secs,
    });

    let doc_id = format!("{}/{}", job_id, summary.round_id.0);
    let response = es
        .index(IndexParts::IndexId(&index_name, &doc_id))
        .body(doc)
        .send()
        .await?;

    if !response.status_code().is_success() {
        let body = response.text().await.unwrap_or_default();
        warn!("Failed to index round {}: {}", doc_id, body);
        return Err(anyhow::anyhow!("Failed to index round: {}", doc_id));
    }

    info!("Indexed round {} to {}", doc_id, index_name);
    Ok(())
}
