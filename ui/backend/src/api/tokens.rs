//! Token visualization REST endpoint
//!
//! Queries `tokens-*` ES index directly (bypasses gRPC) to serve
//! per-round token data for the frontend.

use super::{ApiError, ApiResponse};
use crate::grpc_client::ControllerGrpcClient;
use axum::{Json, extract::Extension, extract::Path, extract::State};
use elasticsearch::Elasticsearch;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error};

#[derive(Debug, Serialize)]
pub struct RoundTokensResponse {
    pub tokens: Vec<String>,
    pub token_categories: HashMap<String, Vec<String>>,
    pub detected: bool,
    pub evasion_score: f64,
    pub token_count: usize,
}

/// GET /api/jobs/:job_id/rounds/:round_id/tokens
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

#[derive(Debug, Deserialize)]
pub struct CompareTokensQuery {
    pub run_a: String,
    pub run_b: String,
}

#[derive(Debug, Serialize)]
pub struct ParamComparisonResponse {
    pub name: String,
    pub value_a: String,
    pub value_b: String,
    pub param_type: String,
    pub normalized_distance: f64,
    pub range_info: String,
}

#[derive(Debug, Serialize)]
pub struct MutationComparisonResponse {
    pub mutation_id: String,
    pub presence: String,
    pub token_a: String,
    pub token_b: String,
    pub params: Vec<ParamComparisonResponse>,
    pub overall_distance: f64,
}

#[derive(Debug, Serialize)]
pub struct TokenCompareResponse {
    pub only_in_a: Vec<String>,
    pub only_in_b: Vec<String>,
    pub common: Vec<String>,
    pub mutation_comparisons: Vec<MutationComparisonResponse>,
    pub jaccard_distance: f64,
    pub count_a: u32,
    pub count_b: u32,
}

/// GET /api/tokens/compare?run_a=X&run_b=Y
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
