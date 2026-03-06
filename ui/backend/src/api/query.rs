//! Query and triage REST endpoints.
//!
//! Provides two endpoints:
//!
//! - **`POST /api/query`** — Flexible ElasticSearch query filtered by job IDs
//!   and/or a date range. Returns paginated analysis results.
//! - **`POST /api/triage`** — Submit a triage verdict (detected / not detected)
//!   for a specific job, which the Controller stores in ElasticSearch.
//!
//! Both endpoints forward requests to the Controller via gRPC; the Controller
//! handles the actual ES interaction.

use super::{ApiError, ApiResponse};
use crate::grpc_client::ControllerGrpcClient;
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error};

// ============================================================================
// Request/Response Types
// ============================================================================

/// Request body for `POST /api/query`.
///
/// At least one filter (`job_ids`, `date_from`, or `date_to`) must be
/// provided; otherwise the handler returns `BAD_REQUEST`.
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

/// Paginated analysis results returned by `POST /api/query`.
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    /// Total number of matching results (may exceed `results.len()` if
    /// paginated server-side).
    pub total_count: i32,
    /// Analysis result records for the current page.
    pub results: Vec<AnalysisResultInfo>,
}

/// A single analysis result record from ElasticSearch.
#[derive(Debug, Serialize)]
pub struct AnalysisResultInfo {
    /// Job that produced this result.
    pub job_id: String,
    /// SHA-256 hash of the artifact binary.
    pub artifact_hash: String,
    /// Whether the artifact was detected by the AV/EDR product.
    pub detected: bool,
    /// Detection rate as a fraction string (e.g. `"3/5"`).
    pub detection_rate: String,
    /// Evasion technique IDs applied to the artifact.
    pub evasion_techniques: Vec<String>,
    /// Summarized telemetry key-value pairs.
    pub telemetry_summary: HashMap<String, String>,
}

/// Request body for `POST /api/triage`.
#[derive(Debug, Deserialize)]
pub struct TriageRequest {
    /// Job ID to attach the verdict to. Required.
    pub job_id: String,
    /// `true` if the artifact was detected by the AV/EDR product.
    #[serde(default)]
    pub detected: bool,
    /// Name of the AV/EDR product (e.g. `"defender"`, `"cortex"`).
    #[serde(default)]
    pub av_product: String,
}

/// Confirmation returned after storing a triage verdict.
#[derive(Debug, Serialize)]
pub struct TriageResponse {
    /// Job ID the verdict was stored against.
    pub job_id: String,
    /// `true` if the verdict was persisted successfully.
    pub stored: bool,
    /// Unique identifier for the stored triage record.
    pub triage_id: String,
}

// ============================================================================
// Handlers
// ============================================================================

/// `POST /api/query` — Query analysis results from ElasticSearch.
///
/// # Errors
///
/// - `BAD_REQUEST` if none of `job_ids`, `date_from`, or `date_to` are
///   provided.
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable (connection /
///   timeout errors).
/// - `INTERNAL_ERROR` for other query failures.
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
                    telemetry_summary: r.telemetry_summary,
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
                Err(ApiError::unavailable(format!(
                    "Controller unavailable: {}",
                    e
                )))
            } else {
                Err(ApiError::internal(format!("Query failed: {}", e)))
            }
        }
    }
}

/// `POST /api/triage` — Submit a triage verdict for a job.
///
/// # Errors
///
/// - `BAD_REQUEST` if `job_id` is empty.
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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
