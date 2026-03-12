//! ES Export: query ElasticSearch for round + token data and assemble an [`EvalDataset`].
//!
//! Follows the same pattern as [`crate::es_query`]: pure query generators,
//! pure parsers, and a thin async HTTP layer using `reqwest`.

use crate::{
    DifferentialCategory, EvalDataset, ModuleSelectionSpec, MutationSpec, RoundSummary,
    SelectionRecord, TokenMatrixEntry,
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::SystemTime;

use controller::dispatch::types::RoundId;

// ============================================================================
// Query Generators (pure)
// ============================================================================

/// ES query for `rounds-*` index: all rounds for a job, sorted by round_number ascending.
pub fn rounds_query(job_id: &str) -> Value {
    json!({
        "query": {
            "term": { "job_id": job_id }
        },
        "sort": [{ "round_number": "asc" }],
        "size": 500
    })
}

/// ES query for `tokens-*` index: all token sets for a job, sorted by timestamp ascending.
pub fn token_sets_query(job_id: &str) -> Value {
    json!({
        "query": {
            "term": { "job_id": job_id }
        },
        "sort": [{ "timestamp": "asc" }],
        "size": 500
    })
}

// ============================================================================
// Parsers (pure)
// ============================================================================

/// Parse a differential category string from either format:
/// - snake_case from `as_str()` (used in `rounds-*`): `"real_detection"`
/// - PascalCase from `format!("{:?}", ...)` (used in `tokens-*`): `"RealDetection"`
pub fn parse_differential_category(s: &str) -> DifferentialCategory {
    match s {
        // snake_case (rounds-* index, from as_str())
        "real_detection" | "RealDetection" => DifferentialCategory::RealDetection,
        "instrumentation_artifact" | "InstrumentationArtifact" => {
            DifferentialCategory::InstrumentationArtifact
        }
        "flaky" | "Flaky" => DifferentialCategory::Flaky,
        "evasion" | "Evasion" => DifferentialCategory::Evasion,
        "mutation_failed" | "MutationFailed" => DifferentialCategory::MutationFailed,
        "payload_failed" | "PayloadFailed" => DifferentialCategory::PayloadFailed,
        "static_detection" | "StaticDetection" => DifferentialCategory::StaticDetection,
        _ => {
            tracing::warn!("Unknown differential category '{}', defaulting to Flaky", s);
            DifferentialCategory::Flaky
        }
    }
}

/// Parse ES `rounds-*` response into `Vec<RoundSummary>`.
pub fn parse_rounds(es_response: &Value) -> Vec<RoundSummary> {
    let hits = es_response
        .get("hits")
        .and_then(|h| h.get("hits"))
        .and_then(|h| h.as_array());

    let Some(hits) = hits else {
        return Vec::new();
    };

    hits.iter()
        .filter_map(|hit| {
            let src = hit.get("_source")?;

            let round_id = src.get("round_id")?.as_str()?.to_string();
            let round_number = src.get("round_number")?.as_u64()? as u32;

            // mutations: array of string IDs
            let mutations: Vec<String> = src
                .get("mutations")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // mutation_specs: array of {id, params}
            let mutation_specs: Vec<MutationSpec> = src
                .get("mutation_recipe")
                .or_else(|| src.get("mutation_specs"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|entry| {
                            let id = entry.get("id")?.as_str()?.to_string();
                            let params = entry.get("params").cloned();
                            Some(MutationSpec { id, params })
                        })
                        .collect()
                })
                .unwrap_or_default();

            // modules: 7-field object
            let modules = parse_module_spec(src.get("modules")?);

            let detected = src
                .get("detected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let behavior_match = src
                .get("behavior_match")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let evasion_score = src
                .get("evasion_score")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            let cat_str = src
                .get("differential_category")
                .and_then(|v| v.as_str())
                .unwrap_or("flaky");
            let differential_category = parse_differential_category(cat_str);

            let completed_at = parse_completed_at(src.get("completed_at"));

            let dry_run_exit_code = src
                .get("dry_run_exit_code")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32);

            let has_dryrun = src
                .get("has_dryrun")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let detection_verdict = src
                .get("detection_verdict")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let coverage_percent = src.get("coverage_percent").and_then(|v| v.as_f64());

            let time_factor = src
                .get("time_factor")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            Some(RoundSummary {
                round_id: RoundId(round_id),
                round_number,
                mutations,
                mutation_specs,
                modules,
                detected,
                behavior_match,
                evasion_score,
                differential_category,
                completed_at,
                dry_run_exit_code,
                has_dryrun,
                detection_verdict,
                coverage_percent,
                time_factor,
            })
        })
        .collect()
}

/// Parse a `ModuleSelectionSpec` from an ES JSON object.
fn parse_module_spec(v: &Value) -> ModuleSelectionSpec {
    let s = |field: &str, default: &str| -> String {
        v.get(field)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    };
    ModuleSelectionSpec {
        carrier: s("carrier", "alloc_rw_rx"),
        decoder: s("decoder", "xor"),
        antiemulation: s("antiemulation", "none"),
        deconditioner: s("deconditioner", "none"),
        guardrail: s("guardrail", "none"),
        virtualprotect: s("virtualprotect", "standard"),
        decoy: s("decoy", "none"),
    }
}

/// Parse `completed_at` from ES — supports RFC 3339 string or epoch-based object.
fn parse_completed_at(v: Option<&Value>) -> SystemTime {
    let Some(v) = v else {
        return SystemTime::UNIX_EPOCH;
    };

    // RFC 3339 string (ES default timestamp format)
    if let Some(s) = v.as_str()
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s)
    {
        return SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(dt.timestamp() as u64);
    }

    // Epoch millis (numeric)
    if let Some(ms) = v.as_u64() {
        return SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms);
    }

    // Serde-serialized SystemTime: { secs_since_epoch, nanos_since_epoch }
    if let Some(secs) = v.get("secs_since_epoch").and_then(|s| s.as_u64()) {
        let nanos = v
            .get("nanos_since_epoch")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        return SystemTime::UNIX_EPOCH + std::time::Duration::new(secs, nanos as u32);
    }

    SystemTime::UNIX_EPOCH
}

/// Parse ES `tokens-*` response into `Vec<TokenMatrixEntry>`.
///
/// Requires `round_id_to_number` cross-reference because `tokens-*` stores
/// `round_id` but not `round_number`.
pub fn parse_token_matrices(
    es_response: &Value,
    round_id_to_number: &HashMap<String, u32>,
) -> Vec<TokenMatrixEntry> {
    let hits = es_response
        .get("hits")
        .and_then(|h| h.get("hits"))
        .and_then(|h| h.as_array());

    let Some(hits) = hits else {
        return Vec::new();
    };

    hits.iter()
        .filter_map(|hit| {
            let src = hit.get("_source")?;

            let round_id = src.get("round_id")?.as_str()?;

            // Cross-reference round_id → round_number
            let round_number = round_id_to_number
                .get(round_id)
                .copied()
                .unwrap_or_else(|| {
                    tracing::warn!(
                        "Token entry has unknown round_id '{}', using round_number=0",
                        round_id
                    );
                    0
                });

            let tokens: Vec<String> = src
                .get("tokens")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let detected = src
                .get("detected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let cat_str = src
                .get("differential_category")
                .and_then(|v| v.as_str())
                .unwrap_or("Flaky");
            let category = parse_differential_category(cat_str);
            let trustworthy = category.is_trustworthy();

            Some(TokenMatrixEntry {
                round_number,
                tokens,
                detected,
                trustworthy,
            })
        })
        .collect()
}

/// Build stub `SelectionRecord`s from round data.
///
/// `SelectionRecord` data (rationale, avoid/seek tokens) is **not persisted to ES**.
/// This creates placeholders so the dataset is structurally complete.
pub fn build_stub_selections(rounds: &[RoundSummary]) -> Vec<SelectionRecord> {
    rounds
        .iter()
        .map(|r| SelectionRecord {
            round_number: r.round_number,
            rationale: "unknown (not persisted to ES)".to_string(),
            modules: r.modules.clone(),
            mutations: r.mutations.clone(),
            avoid_tokens: vec![],
            seek_tokens: vec![],
        })
        .collect()
}

// ============================================================================
// Async HTTP Layer
// ============================================================================

/// Execute an ES `_search` request.
async fn es_search(
    client: &reqwest::Client,
    es_url: &str,
    index: &str,
    query: &Value,
) -> Result<Value> {
    let url = format!("{}/{}/_search", es_url, index);
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(query)
        .send()
        .await
        .with_context(|| format!("ES request to {} failed", url))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .with_context(|| format!("Failed to parse ES response from {}", url))?;

    if !status.is_success() {
        let error_msg = body
            .get("error")
            .map(|e| e.to_string())
            .unwrap_or_else(|| format!("HTTP {}", status));
        anyhow::bail!("ES query to {} failed: {}", index, error_msg);
    }

    Ok(body)
}

/// Fetch a complete `EvalDataset` from ElasticSearch for the given job.
///
/// Queries `rounds-*` and `tokens-*` indices, cross-references round IDs,
/// and builds stub selections (since selector decisions aren't persisted to ES).
pub async fn fetch_eval_dataset(es_url: &str, job_id: &str) -> Result<EvalDataset> {
    let client = reqwest::Client::new();

    // 1. Fetch rounds
    let rounds_resp = es_search(&client, es_url, "rounds-*", &rounds_query(job_id)).await?;
    let rounds = parse_rounds(&rounds_resp);

    if rounds.is_empty() {
        anyhow::bail!(
            "No rounds found in rounds-* for job_id '{}'. Check that the job exists in ES.",
            job_id
        );
    }

    // 2. Build round_id → round_number mapping for cross-reference
    let round_id_to_number: HashMap<String, u32> = rounds
        .iter()
        .map(|r| (r.round_id.0.clone(), r.round_number))
        .collect();

    // 3. Fetch token matrices
    let tokens_resp = es_search(&client, es_url, "tokens-*", &token_sets_query(job_id)).await?;
    let token_matrices = parse_token_matrices(&tokens_resp, &round_id_to_number);

    // 4. Build stub selections
    let selections = build_stub_selections(&rounds);

    Ok(EvalDataset {
        job_id: job_id.to_string(),
        rounds,
        selections,
        token_matrices,
        telemetry_tokens: None,
    })
}

// ============================================================================
// Curl Helper
// ============================================================================

/// Generate curl commands for manual ES export (documentation aid).
pub fn curl_commands(es_host: &str, job_id: &str) -> String {
    let rounds_q = serde_json::to_string(&rounds_query(job_id)).unwrap_or_default();
    let tokens_q = serde_json::to_string(&token_sets_query(job_id)).unwrap_or_default();

    format!(
        "# Rounds query\n\
         curl -s -X POST '{es_host}/rounds-*/_search' \\\n  \
           -H 'Content-Type: application/json' \\\n  \
           -d '{rounds_q}' \\\n  \
           | jq '[.hits.hits[]._source]'\n\n\
         # Token sets query\n\
         curl -s -X POST '{es_host}/tokens-*/_search' \\\n  \
           -H 'Content-Type: application/json' \\\n  \
           -d '{tokens_q}' \\\n  \
           | jq '[.hits.hits[]._source]'"
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rounds_response() -> Value {
        json!({
            "hits": {
                "total": { "value": 2 },
                "hits": [
                    {
                        "_source": {
                            "round_id": "job-test-round-1",
                            "round_number": 1,
                            "mutations": ["ast.string_xor", "ast.fill_pattern"],
                            "mutation_recipe": [
                                { "id": "ast.string_xor", "params": { "xor_key": "0xBB" } },
                                { "id": "ast.fill_pattern", "params": null }
                            ],
                            "modules": {
                                "carrier": "alloc_rw_rx",
                                "decoder": "xor",
                                "antiemulation": "none",
                                "deconditioner": "alloc_loop",
                                "guardrail": "none",
                                "virtualprotect": "standard",
                                "decoy": "none"
                            },
                            "detected": true,
                            "behavior_match": true,
                            "evasion_score": 0.15,
                            "differential_category": "real_detection",
                            "completed_at": "2025-06-01T12:00:00Z",
                            "dry_run_exit_code": null,
                            "has_dryrun": true,
                            "detection_verdict": "detected",
                            "coverage_percent": 0.62,
                            "time_factor": 0.0
                        }
                    },
                    {
                        "_source": {
                            "round_id": "job-test-round-2",
                            "round_number": 2,
                            "mutations": ["ast.benign_preamble"],
                            "mutation_recipe": [
                                { "id": "ast.benign_preamble", "params": null }
                            ],
                            "modules": {
                                "carrier": "peb_walk",
                                "decoder": "english",
                                "antiemulation": "none",
                                "deconditioner": "none",
                                "guardrail": "env",
                                "virtualprotect": "standard",
                                "decoy": "winexec"
                            },
                            "detected": false,
                            "behavior_match": true,
                            "evasion_score": 0.85,
                            "differential_category": "evasion",
                            "completed_at": "2025-06-01T12:05:00Z",
                            "dry_run_exit_code": 0,
                            "has_dryrun": true,
                            "detection_verdict": "evasion",
                            "coverage_percent": null,
                            "time_factor": 0.5
                        }
                    }
                ]
            }
        })
    }

    fn sample_tokens_response() -> Value {
        json!({
            "hits": {
                "total": { "value": 2 },
                "hits": [
                    {
                        "_source": {
                            "round_id": "job-test-round-1",
                            "tokens": [
                                "module:carrier=alloc_rw_rx",
                                "module:decoder=xor",
                                "mutation:ast.string_xor",
                                "api:VirtualProtect",
                                "seq2:VirtualAlloc->memcpy"
                            ],
                            "detected": true,
                            "differential_category": "RealDetection"
                        }
                    },
                    {
                        "_source": {
                            "round_id": "job-test-round-2",
                            "tokens": [
                                "module:carrier=peb_walk",
                                "module:decoder=english",
                                "mutation:ast.benign_preamble",
                                "api:NtAllocateVirtualMemory",
                                "etw:Microsoft-Windows-Kernel-Process/1"
                            ],
                            "detected": false,
                            "differential_category": "Evasion"
                        }
                    }
                ]
            }
        })
    }

    #[test]
    fn test_parse_rounds_from_es() {
        let resp = sample_rounds_response();
        let rounds = parse_rounds(&resp);

        assert_eq!(rounds.len(), 2);

        // Round 1
        assert_eq!(rounds[0].round_id.0, "job-test-round-1");
        assert_eq!(rounds[0].round_number, 1);
        assert_eq!(
            rounds[0].mutations,
            vec!["ast.string_xor", "ast.fill_pattern"]
        );
        assert_eq!(rounds[0].mutation_specs.len(), 2);
        assert_eq!(rounds[0].mutation_specs[0].id, "ast.string_xor");
        assert!(rounds[0].detected);
        assert!(rounds[0].behavior_match);
        assert!((rounds[0].evasion_score - 0.15).abs() < f64::EPSILON);
        assert_eq!(
            rounds[0].differential_category,
            DifferentialCategory::RealDetection
        );
        assert!(rounds[0].has_dryrun);
        assert_eq!(rounds[0].detection_verdict, "detected");
        assert!((rounds[0].coverage_percent.unwrap() - 0.62).abs() < f64::EPSILON);
        assert_eq!(rounds[0].modules.carrier, "alloc_rw_rx");
        assert_eq!(rounds[0].modules.deconditioner, "alloc_loop");

        // Round 2
        assert_eq!(rounds[1].round_number, 2);
        assert!(!rounds[1].detected);
        assert_eq!(
            rounds[1].differential_category,
            DifferentialCategory::Evasion
        );
        assert_eq!(rounds[1].dry_run_exit_code, Some(0));
        assert_eq!(rounds[1].modules.carrier, "peb_walk");
        assert_eq!(rounds[1].modules.guardrail, "env");
        assert!(rounds[1].coverage_percent.is_none());
    }

    #[test]
    fn test_parse_token_matrices_from_es() {
        let resp = sample_tokens_response();
        let round_id_to_number: HashMap<String, u32> = [
            ("job-test-round-1".to_string(), 1),
            ("job-test-round-2".to_string(), 2),
        ]
        .into();

        let matrices = parse_token_matrices(&resp, &round_id_to_number);

        assert_eq!(matrices.len(), 2);

        // Entry 1 — detected, RealDetection → trustworthy
        assert_eq!(matrices[0].round_number, 1);
        assert!(matrices[0].detected);
        assert!(matrices[0].trustworthy);
        assert!(
            matrices[0]
                .tokens
                .contains(&"api:VirtualProtect".to_string())
        );
        assert!(
            matrices[0]
                .tokens
                .contains(&"seq2:VirtualAlloc->memcpy".to_string())
        );

        // Entry 2 — not detected, Evasion → trustworthy
        assert_eq!(matrices[1].round_number, 2);
        assert!(!matrices[1].detected);
        assert!(matrices[1].trustworthy); // Evasion is trustworthy
        assert!(
            matrices[1]
                .tokens
                .contains(&"etw:Microsoft-Windows-Kernel-Process/1".to_string())
        );
    }

    #[test]
    fn test_parse_differential_category_both_formats() {
        // snake_case (rounds-* index)
        assert_eq!(
            parse_differential_category("real_detection"),
            DifferentialCategory::RealDetection
        );
        assert_eq!(
            parse_differential_category("evasion"),
            DifferentialCategory::Evasion
        );
        assert_eq!(
            parse_differential_category("instrumentation_artifact"),
            DifferentialCategory::InstrumentationArtifact
        );
        assert_eq!(
            parse_differential_category("static_detection"),
            DifferentialCategory::StaticDetection
        );
        assert_eq!(
            parse_differential_category("mutation_failed"),
            DifferentialCategory::MutationFailed
        );
        assert_eq!(
            parse_differential_category("payload_failed"),
            DifferentialCategory::PayloadFailed
        );
        assert_eq!(
            parse_differential_category("flaky"),
            DifferentialCategory::Flaky
        );

        // PascalCase (tokens-* index)
        assert_eq!(
            parse_differential_category("RealDetection"),
            DifferentialCategory::RealDetection
        );
        assert_eq!(
            parse_differential_category("Evasion"),
            DifferentialCategory::Evasion
        );
        assert_eq!(
            parse_differential_category("InstrumentationArtifact"),
            DifferentialCategory::InstrumentationArtifact
        );
        assert_eq!(
            parse_differential_category("StaticDetection"),
            DifferentialCategory::StaticDetection
        );
        assert_eq!(
            parse_differential_category("MutationFailed"),
            DifferentialCategory::MutationFailed
        );
        assert_eq!(
            parse_differential_category("PayloadFailed"),
            DifferentialCategory::PayloadFailed
        );
        assert_eq!(
            parse_differential_category("Flaky"),
            DifferentialCategory::Flaky
        );

        // Unknown → Flaky (fallback)
        assert_eq!(
            parse_differential_category("garbage"),
            DifferentialCategory::Flaky
        );
    }

    #[test]
    fn test_build_stub_selections() {
        let resp = sample_rounds_response();
        let rounds = parse_rounds(&resp);
        let selections = build_stub_selections(&rounds);

        assert_eq!(selections.len(), 2);

        assert_eq!(selections[0].round_number, 1);
        assert_eq!(selections[0].rationale, "unknown (not persisted to ES)");
        assert_eq!(
            selections[0].mutations,
            vec!["ast.string_xor", "ast.fill_pattern"]
        );
        assert!(selections[0].avoid_tokens.is_empty());
        assert!(selections[0].seek_tokens.is_empty());
        assert_eq!(selections[0].modules.carrier, "alloc_rw_rx");

        assert_eq!(selections[1].round_number, 2);
        assert_eq!(selections[1].modules.carrier, "peb_walk");
    }

    #[test]
    fn test_empty_es_response() {
        // No hits key
        let empty1 = json!({});
        assert!(parse_rounds(&empty1).is_empty());
        assert!(parse_token_matrices(&empty1, &HashMap::new()).is_empty());

        // Empty hits array
        let empty2 = json!({ "hits": { "hits": [] } });
        assert!(parse_rounds(&empty2).is_empty());
        assert!(parse_token_matrices(&empty2, &HashMap::new()).is_empty());

        // Stub selections from empty rounds
        assert!(build_stub_selections(&[]).is_empty());
    }

    #[test]
    fn test_parse_completed_at_formats() {
        // RFC 3339
        let rfc = json!("2025-06-01T12:00:00Z");
        let t = parse_completed_at(Some(&rfc));
        assert!(t > SystemTime::UNIX_EPOCH);

        // Epoch millis
        let epoch_ms = json!(1717243200000u64);
        let t = parse_completed_at(Some(&epoch_ms));
        assert!(t > SystemTime::UNIX_EPOCH);

        // Serde SystemTime format
        let serde_fmt = json!({ "secs_since_epoch": 1717243200, "nanos_since_epoch": 0 });
        let t = parse_completed_at(Some(&serde_fmt));
        assert!(t > SystemTime::UNIX_EPOCH);

        // None
        assert_eq!(parse_completed_at(None), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn test_token_matrix_unknown_round_id() {
        // Token references a round_id not in the mapping → round_number=0
        let resp = json!({
            "hits": {
                "hits": [{
                    "_source": {
                        "round_id": "unknown-round",
                        "tokens": ["api:Test"],
                        "detected": false,
                        "differential_category": "Evasion"
                    }
                }]
            }
        });
        let matrices = parse_token_matrices(&resp, &HashMap::new());
        assert_eq!(matrices.len(), 1);
        assert_eq!(matrices[0].round_number, 0);
    }

    #[test]
    fn test_query_generators() {
        let rq = rounds_query("job-123");
        assert_eq!(rq["query"]["term"]["job_id"], "job-123");
        assert_eq!(rq["size"], 500);

        let tq = token_sets_query("job-456");
        assert_eq!(tq["query"]["term"]["job_id"], "job-456");
        assert_eq!(tq["size"], 500);
    }
}
