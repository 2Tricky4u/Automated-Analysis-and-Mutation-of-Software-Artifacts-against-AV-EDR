//! Worker management REST endpoints
//!
//! Wraps Controller gRPC worker endpoints.

use super::{ApiError, ApiResponse};
use crate::grpc_client::ControllerGrpcClient;
use axum::{extract::State, Json};
use serde::Serialize;
use std::sync::Arc;
use tracing::{debug, error};

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct WorkersListResponse {
    pub workers: Vec<WorkerInfo>,
    pub total: usize,
    pub available: usize,
    pub busy: usize,
    pub offline: usize,
}

#[derive(Debug, Serialize)]
pub struct WorkerInfo {
    pub id: String,
    pub address: String,
    pub os_version: String,
    pub capabilities: Vec<String>,
    pub status: String,
    pub current_job: Option<String>,
}

// ============================================================================
// Handlers
// ============================================================================

/// GET /api/workers - List all workers
pub async fn list_workers(
    State(client): State<Arc<ControllerGrpcClient>>,
) -> Result<Json<ApiResponse<WorkersListResponse>>, ApiError> {
    debug!("REST: List workers");

    match client.list_workers().await {
        Ok(resp) => {
            let workers: Vec<WorkerInfo> = resp
                .workers
                .into_iter()
                .map(|w| WorkerInfo {
                    id: w.worker_id,
                    address: w.address,
                    os_version: w.os_version,
                    capabilities: w.capabilities,
                    status: w.status.clone(),
                    current_job: if w.current_job_id.is_empty() {
                        None
                    } else {
                        Some(w.current_job_id)
                    },
                })
                .collect();

            let total = workers.len();
            let available = workers.iter().filter(|w| w.status == "available").count();
            let busy = workers.iter().filter(|w| w.status == "busy").count();
            let offline = workers.iter().filter(|w| w.status == "offline").count();

            Ok(Json(ApiResponse::new(WorkersListResponse {
                workers,
                total,
                available,
                busy,
                offline,
            })))
        }
        Err(e) => {
            error!("Failed to list workers: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}
