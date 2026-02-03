//! Query and triage REST endpoints
//!
//! Wraps Controller gRPC query endpoints.

use super::{ApiError, ApiResponse};
use crate::grpc_client::ControllerGrpcClient;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error};

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    /// Job IDs to query
    #[serde(default)]
    pub job_ids: Vec<String>,

    /// Date range start (ISO 8601)
    pub date_from: Option<String>,

    /// Date range end (ISO 8601)
    pub date_to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub total_count: i32,
    pub results: Vec<AnalysisResultInfo>,
}

#[derive(Debug, Serialize)]
pub struct AnalysisResultInfo {
    pub job_id: String,
    pub artifact_hash: String,
    pub detected: bool,
    pub detection_rate: String,
    pub evasion_techniques: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct TriageRequest {
    pub job_id: String,
    #[serde(default)]
    pub detected: bool,
    #[serde(default)]
    pub av_product: String,
}

#[derive(Debug, Serialize)]
pub struct TriageResponse {
    pub job_id: String,
    pub stored: bool,
    pub triage_id: String,
}

// ============================================================================
// Handlers
// ============================================================================

/// POST /api/query - Query analysis results
pub async fn query_results(
    State(client): State<Arc<ControllerGrpcClient>>,
    Json(payload): Json<QueryRequest>,
) -> Result<Json<ApiResponse<QueryResponse>>, ApiError> {
    // Validate: at least one filter should be provided
    if payload.job_ids.is_empty() && payload.date_from.is_none() && payload.date_to.is_none() {
        return Err(ApiError::bad_request(
            "At least one of job_ids, date_from, or date_to is required",
        ));
    }

    debug!(
        "REST: Query results (job_ids={:?}, date_from={:?})",
        payload.job_ids, payload.date_from
    );

    match client
        .query_results(payload.job_ids, payload.date_from, payload.date_to)
        .await
    {
        Ok(resp) => {
            let results: Vec<AnalysisResultInfo> = resp
                .results
                .into_iter()
                .map(|r| AnalysisResultInfo {
                    job_id: r.job_id,
                    artifact_hash: r.artifact_hash,
                    detected: r.detected,
                    detection_rate: r.detection_rate,
                    evasion_techniques: r.evasion_techniques,
                })
                .collect();

            Ok(Json(ApiResponse::new(QueryResponse {
                total_count: resp.total_count,
                results,
            })))
        }
        Err(e) => {
            error!("Failed to query results: {}", e);
            // Check if it's a connection error vs internal error
            let err_str = e.to_string();
            if err_str.contains("connect") || err_str.contains("timeout") {
                Err(ApiError::unavailable(format!("Controller unavailable: {}", e)))
            } else {
                Err(ApiError::internal(format!("Query failed: {}", e)))
            }
        }
    }
}

/// POST /api/triage - Submit triage data
pub async fn submit_triage(
    State(client): State<Arc<ControllerGrpcClient>>,
    Json(payload): Json<TriageRequest>,
) -> Result<Json<ApiResponse<TriageResponse>>, ApiError> {
    // Validate required fields
    if payload.job_id.is_empty() {
        return Err(ApiError::bad_request("job_id is required"));
    }

    debug!("REST: Submit triage (job_id={})", payload.job_id);

    match client
        .submit_triage(&payload.job_id, payload.detected, &payload.av_product)
        .await
    {
        Ok(resp) => Ok(Json(ApiResponse::new(TriageResponse {
            job_id: resp.job_id,
            stored: resp.stored,
            triage_id: resp.triage_id,
        }))),
        Err(e) => {
            error!("Failed to submit triage: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}
