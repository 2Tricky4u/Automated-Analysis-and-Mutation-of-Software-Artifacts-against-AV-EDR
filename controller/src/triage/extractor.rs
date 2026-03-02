//! Token extraction from round data and ES telemetry.
//!
//! Two sources:
//! - **In-memory** (`extract_round_tokens`): module + mutation tokens from `RoundSummary`
//! - **ES telemetry** (`extract_telemetry_tokens`): `api:*` and `seq2:*` from `payload_func` events
//!
//! `extract_and_score` is the top-level async function spawned in the background
//! by `finalize_round()`. It combines both sources, indexes to ES, and produces
//! `TriageGuidance` for the selector.

use crate::dispatch::types::RoundSummary;
use crate::storage::EsStorage;
use crate::triage::TriageGuidance;
use crate::triage::scorer;
use serde_json::json;
use tracing::{debug, warn};

// ============================================================================
// In-memory token extraction (pure, no IO)
// ============================================================================

/// Extract `module:*` and `mutation:*` tokens from a RoundSummary.
///
/// Module tokens: one per category (carrier, decoder, antiemulation,
/// deconditioner, guardrail, virtualprotect, decoy).
/// Mutation tokens: one per applied mutation ID.
pub fn extract_round_tokens(summary: &RoundSummary) -> Vec<String> {
    let mut tokens = Vec::with_capacity(7 + summary.mutations.len());

    // Module tokens — 7 categories
    tokens.push(format!("module:carrier={}", summary.modules.carrier));
    tokens.push(format!("module:decoder={}", summary.modules.decoder));
    tokens.push(format!(
        "module:antiemulation={}",
        summary.modules.antiemulation
    ));
    tokens.push(format!(
        "module:deconditioner={}",
        summary.modules.deconditioner
    ));
    tokens.push(format!("module:guardrail={}", summary.modules.guardrail));
    tokens.push(format!(
        "module:virtualprotect={}",
        summary.modules.virtualprotect
    ));
    tokens.push(format!("module:decoy={}", summary.modules.decoy));

    // Mutation tokens
    for mutation_id in &summary.mutations {
        tokens.push(format!("mutation:{}", mutation_id));
    }

    tokens
}

// ============================================================================
// ES telemetry token extraction (async)
// ============================================================================

/// Extract `api:*` and `seq2:*` tokens from ES telemetry for a specific run.
///
/// Queries `telemetry-*` for `payload_func` events, sorted by `payload_seq`.
/// Returns deduplicated tokens.
pub async fn extract_telemetry_tokens(
    storage: &EsStorage,
    _job_id: &str,
    run_id: &str,
) -> anyhow::Result<Vec<String>> {
    let docs = storage.query_api_telemetry(run_id).await;

    if docs.is_empty() {
        return Ok(Vec::new());
    }

    let mut tokens = Vec::new();
    let mut prev_func: Option<String> = None;
    let mut seen = std::collections::HashSet::new();

    for doc in &docs {
        if let Some(func) = doc["payload_func"].as_str() {
            let api_token = format!("api:{}", func);
            if seen.insert(api_token.clone()) {
                tokens.push(api_token);
            }

            // Bigram: seq2 token from consecutive payload_func calls
            if let Some(ref prev) = prev_func {
                let seq_token = format!("seq2:{}→{}", prev, func);
                if seen.insert(seq_token.clone()) {
                    tokens.push(seq_token);
                }
            }
            prev_func = Some(func.to_string());
        }
    }

    Ok(tokens)
}

// ============================================================================
// Combined extract + index + score (async background task)
// ============================================================================

/// Background task: extract tokens, index to ES, compute scores, return guidance.
///
/// Called by `finalize_round()` via `tokio::spawn`. Non-fatal — errors are logged
/// and the guidance channel simply doesn't receive an update.
pub async fn extract_and_score(
    storage: &EsStorage,
    job_id: &str,
    round_id: &str,
    baseline_run_id: &str,
    summary: &RoundSummary,
    all_summaries: &[(RoundSummary, bool)], // (summary, detected) for all prior rounds
) -> anyhow::Result<TriageGuidance> {
    // 1. Extract in-memory tokens from this round
    let mut tokens = extract_round_tokens(summary);

    // 2. Extract telemetry tokens from ES (api + seq2)
    match extract_telemetry_tokens(storage, job_id, baseline_run_id).await {
        Ok(telemetry_tokens) => {
            debug!(
                "[Triage:{}] Extracted {} telemetry tokens for round {}",
                job_id,
                telemetry_tokens.len(),
                round_id
            );
            tokens.extend(telemetry_tokens);
        }
        Err(e) => {
            warn!(
                "[Triage:{}] Failed to extract telemetry tokens: {}",
                job_id, e
            );
            // Continue with in-memory tokens only
        }
    }

    // 3. Index combined token set to `tokens-YYYY.MM`
    let doc = json!({
        "job_id": job_id,
        "round_id": round_id,
        "run_id": baseline_run_id,
        "detected": summary.detected,
        "differential_category": format!("{:?}", summary.differential_category),
        "evasion_score": summary.evasion_score,
        "modules": {
            "carrier": summary.modules.carrier,
            "decoder": summary.modules.decoder,
            "antiemulation": summary.modules.antiemulation,
            "deconditioner": summary.modules.deconditioner,
            "guardrail": summary.modules.guardrail,
            "virtualprotect": summary.modules.virtualprotect,
            "decoy": summary.modules.decoy,
        },
        "mutations": summary.mutations,
        "tokens": tokens,
        "token_count": tokens.len(),
        "timestamp": crate::storage::helpers::now_rfc3339(),
    });
    if let Err(e) = storage.index_token_set(doc).await {
        warn!("[Triage:{}] Failed to index token set: {}", job_id, e);
    }

    // 4. Build token-round matrix from all prior rounds + current
    let mut round_tokens: Vec<(Vec<String>, bool)> = Vec::with_capacity(all_summaries.len() + 1);

    for (s, detected) in all_summaries {
        // Skip InstrumentationArtifact rounds — not trustworthy
        if !s.differential_category.is_trustworthy() {
            continue;
        }
        let s_tokens = extract_round_tokens(s);
        round_tokens.push((s_tokens, *detected));
    }

    // Add current round (if trustworthy)
    if summary.differential_category.is_trustworthy() {
        round_tokens.push((tokens, summary.detected));
    }

    // 5. Compute scores and build guidance
    let scores = scorer::compute_token_scores(&round_tokens);
    let guidance = scorer::build_guidance(&scores, 1.5, 0.3);

    debug!(
        "[Triage:{}] Guidance: {} avoid, {} seek tokens",
        job_id,
        guidance.avoid_tokens.len(),
        guidance.seek_tokens.len()
    );

    Ok(guidance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{
        DifferentialCategory, ModuleSelectionSpec, MutationSpec, RoundId,
    };
    use std::time::SystemTime;

    fn make_summary(modules: ModuleSelectionSpec, mutations: Vec<String>) -> RoundSummary {
        RoundSummary {
            round_id: RoundId("test-round".to_string()),
            round_number: 1,
            mutation_specs: mutations
                .iter()
                .map(|id| MutationSpec {
                    id: id.clone(),
                    params: None,
                })
                .collect(),
            mutations,
            modules,
            detected: false,
            behavior_match: true,
            evasion_score: 0.5,
            differential_category: DifferentialCategory::Evasion,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        }
    }

    #[test]
    fn test_extract_round_tokens() {
        let summary = make_summary(
            ModuleSelectionSpec {
                carrier: "alloc_rw_rx".to_string(),
                decoder: "xor".to_string(),
                antiemulation: "none".to_string(),
                deconditioner: "alloc_loop".to_string(),
                guardrail: "none".to_string(),
                virtualprotect: "standard".to_string(),
                decoy: "none".to_string(),
            },
            vec![
                "ast.string_xor".to_string(),
                "binary.rich_header".to_string(),
            ],
        );

        let tokens = extract_round_tokens(&summary);

        // 7 module tokens + 2 mutation tokens
        assert_eq!(tokens.len(), 9);
        assert!(tokens.contains(&"module:carrier=alloc_rw_rx".to_string()));
        assert!(tokens.contains(&"module:deconditioner=alloc_loop".to_string()));
        assert!(tokens.contains(&"mutation:ast.string_xor".to_string()));
        assert!(tokens.contains(&"mutation:binary.rich_header".to_string()));
    }

    #[test]
    fn test_extract_round_tokens_no_mutations() {
        let summary = make_summary(ModuleSelectionSpec::default(), vec![]);
        let tokens = extract_round_tokens(&summary);

        // Only 7 module tokens, no mutation tokens
        assert_eq!(tokens.len(), 7);
        assert!(tokens.iter().all(|t| t.starts_with("module:")));
    }
}
