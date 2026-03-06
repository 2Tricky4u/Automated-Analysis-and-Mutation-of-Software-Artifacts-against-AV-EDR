//! Token visualization and comparison REST endpoints.
//!
//! Serves per-round triage token data for the frontend and supports
//! token-set comparison between two runs.
//!
//! ## Token Categories
//!
//! Tokens are prefixed by category:
//!
//! | Prefix      | Description                                       |
//! |-------------|---------------------------------------------------|
//! | `etw:`      | ETW provider / event ID                           |
//! | `api:`      | Win32 API call                                    |
//! | `api_arg:`  | API call with argument bucket                     |
//! | `seq2:`     | 2-gram API call sequence                          |
//! | `seq3:`     | 3-gram API call sequence                          |
//! | `dt:`       | Temporal delta between operations                 |
//! | `trunc:`    | Execution truncation point (file:line)            |
//! | `coverage:` | Coverage-derived signal                           |
//!
//! ## Direct ES Access
//!
//! [`get_round_tokens`] queries the `tokens-*` ElasticSearch index directly
//! instead of routing through gRPC. This avoids the overhead of proto
//! serialization for large token lists and allows the UI to render tokens even
//! when the Controller is temporarily unreachable.
//!
//! [`compare_tokens`] uses the gRPC `CompareTokens` RPC because the comparison
//! logic lives in the Controller.

use super::{ApiError, ApiResponse};
use crate::grpc_client::ControllerGrpcClient;
use axum::{Json, extract::Extension, extract::Path, extract::State};
use elasticsearch::Elasticsearch;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error};

/// Per-round tokens grouped by category, with detection context.
///
/// Returned by `GET /api/jobs/:job_id/rounds/:round_id/tokens`.
#[derive(Debug, Serialize)]
pub struct RoundTokensResponse {
    /// Flat list of all token strings for this round.
    pub tokens: Vec<String>,
    /// Tokens grouped by their category prefix (text before the first `:`).
    pub token_categories: HashMap<String, Vec<String>>,
    /// Whether the round's artifact was detected.
    pub detected: bool,
    /// Numeric evasion score (0.0 = fully detected, 1.0 = full evasion).
    pub evasion_score: f64,
    /// Total number of tokens.
    pub token_count: usize,
}

/// `GET /api/jobs/:job_id/rounds/:round_id/tokens` — Fetch per-round tokens
/// from ElasticSearch.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if ElasticSearch is not configured.
/// - `NOT_FOUND` if no token document exists for the given round.
/// - `INTERNAL_ERROR` if the ES query or response parsing fails.
pub async fn get_round_tokens(
    Path((job_id, round_id)): Path<(String, String)>,
    Extension(es): Extension<Option<Arc<Elasticsearch>>>,
) -> Result<Json<ApiResponse<RoundTokensResponse>>, ApiError> {
    let es = match es {
        Some(client) => client,
        None => {
            return Err(ApiError::unavailable("Elasticsearch not configured"));
        }
    };

    debug!("REST: Get tokens for job={}, round={}", job_id, round_id);

    let response = es
        .search(elasticsearch::SearchParts::Index(&["tokens-*"]))
        .body(json!({
            "query": {
                "bool": {
                    "must": [
                        { "term": { "job_id": job_id } },
                        { "term": { "round_id": round_id } }
                    ]
                }
            },
            "size": 1
        }))
        .send()
        .await
        .map_err(|e| ApiError::internal(format!("ES query failed: {}", e)))?;

    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|e| ApiError::internal(format!("ES response parse error: {}", e)))?;

    let hits = &body["hits"]["hits"];
    let hit = match hits.as_array().and_then(|a| a.first()) {
        Some(h) => h,
        None => return Err(ApiError::not_found("No tokens found for this round")),
    };

    let source = &hit["_source"];

    let tokens: Vec<String> = source["tokens"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let detected = source["detected"].as_bool().unwrap_or(false);
    let evasion_score = source["evasion_score"].as_f64().unwrap_or(0.0);

    // Group tokens by category prefix (text before first ':')
    let mut categories: HashMap<String, Vec<String>> = HashMap::new();
    for token in &tokens {
        let category = token.split(':').next().unwrap_or("other").to_string();
        categories.entry(category).or_default().push(token.clone());
    }

    let token_count = tokens.len();

    Ok(Json(ApiResponse::new(RoundTokensResponse {
        tokens,
        token_categories: categories,
        detected,
        evasion_score,
        token_count,
    })))
}

// ============================================================================
// Token Comparison (via gRPC CompareTokens)
// ============================================================================

/// Query parameters for `GET /api/tokens/compare`.
#[derive(Debug, Deserialize)]
pub struct CompareTokensQuery {
    /// First run ID (the "A" side of the diff).
    pub run_a: String,
    /// Second run ID (the "B" side of the diff).
    pub run_b: String,
}

/// Distance metric for a single mutation parameter between two runs.
#[derive(Debug, Serialize)]
pub struct ParamComparisonResponse {
    /// Parameter name (e.g. `"modulo"`, `"key_length"`).
    pub name: String,
    /// Parameter value in run A.
    pub value_a: String,
    /// Parameter value in run B.
    pub value_b: String,
    /// Parameter type hint (e.g. `"int"`, `"string"`, `"enum"`).
    pub param_type: String,
    /// Normalized distance in `[0.0, 1.0]` (0 = identical, 1 = maximally
    /// different).
    pub normalized_distance: f64,
    /// Human-readable description of the value range used for normalization.
    pub range_info: String,
}

/// Per-mutation token diff between two runs, including param-level distances.
#[derive(Debug, Serialize)]
pub struct MutationComparisonResponse {
    /// Mutation identifier (e.g. `"antiemulation.loop_randomize"`).
    pub mutation_id: String,
    /// Presence flag: `"both"`, `"only_a"`, or `"only_b"`.
    pub presence: String,
    /// Mutation token string from run A (empty if absent).
    pub token_a: String,
    /// Mutation token string from run B (empty if absent).
    pub token_b: String,
    /// Per-parameter distance breakdown.
    pub params: Vec<ParamComparisonResponse>,
    /// Aggregate distance across all parameters in `[0.0, 1.0]`.
    pub overall_distance: f64,
}

/// Full token-set diff between two runs with Jaccard distance.
///
/// Returned by `GET /api/tokens/compare`.
#[derive(Debug, Serialize)]
pub struct TokenCompareResponse {
    /// Tokens present only in run A.
    pub only_in_a: Vec<String>,
    /// Tokens present only in run B.
    pub only_in_b: Vec<String>,
    /// Tokens present in both runs.
    pub common: Vec<String>,
    /// Per-mutation comparison with parameter-level distances.
    pub mutation_comparisons: Vec<MutationComparisonResponse>,
    /// Jaccard distance: `1 - |A ∩ B| / |A ∪ B|`. Range `[0.0, 1.0]`.
    pub jaccard_distance: f64,
    /// Total token count in run A.
    pub count_a: u32,
    /// Total token count in run B.
    pub count_b: u32,
}

/// `GET /api/tokens/compare?run_a=X&run_b=Y` — Compare token sets between two
/// runs.
///
/// # Errors
///
/// - `BAD_REQUEST` if `run_a` or `run_b` is empty.
/// - `NOT_FOUND` if token data is missing for either run.
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
pub async fn compare_tokens(
    State(client): State<Arc<ControllerGrpcClient>>,
    axum::extract::Query(query): axum::extract::Query<CompareTokensQuery>,
) -> Result<Json<ApiResponse<TokenCompareResponse>>, ApiError> {
    if query.run_a.is_empty() {
        return Err(ApiError::bad_request("run_a is required"));
    }
    if query.run_b.is_empty() {
        return Err(ApiError::bad_request("run_b is required"));
    }

    debug!(
        "REST: Compare tokens (run_a={}, run_b={})",
        query.run_a, query.run_b
    );

    match client.compare_tokens(&query.run_a, &query.run_b).await {
        Ok(resp) => {
            if !resp.error.is_empty() {
                return Err(ApiError::not_found(resp.error));
            }

            if let Some(cmp) = resp.comparison {
                let mutation_comparisons: Vec<MutationComparisonResponse> = cmp
                    .mutation_comparisons
                    .into_iter()
                    .map(|mc| MutationComparisonResponse {
                        mutation_id: mc.mutation_id,
                        presence: mc.presence,
                        token_a: mc.token_a,
                        token_b: mc.token_b,
                        params: mc
                            .params
                            .into_iter()
                            .map(|p| ParamComparisonResponse {
                                name: p.name,
                                value_a: p.value_a,
                                value_b: p.value_b,
                                param_type: p.param_type,
                                normalized_distance: p.normalized_distance,
                                range_info: p.range_info,
                            })
                            .collect(),
                        overall_distance: mc.overall_distance,
                    })
                    .collect();

                Ok(Json(ApiResponse::new(TokenCompareResponse {
                    only_in_a: cmp.only_in_a,
                    only_in_b: cmp.only_in_b,
                    common: cmp.common,
                    mutation_comparisons,
                    jaccard_distance: cmp.jaccard_distance,
                    count_a: cmp.count_a,
                    count_b: cmp.count_b,
                })))
            } else {
                Err(ApiError::not_found("No token comparison data available"))
            }
        }
        Err(e) => {
            error!("Failed to compare tokens: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}
