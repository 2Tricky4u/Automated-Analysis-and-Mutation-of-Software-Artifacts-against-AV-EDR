//! Token visualization REST endpoint
//!
//! Queries `tokens-*` ES index directly (bypasses gRPC) to serve
//! per-round token data for the frontend.

use super::{ApiError, ApiResponse};
use axum::{Json, extract::Extension, extract::Path};
use elasticsearch::Elasticsearch;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

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
