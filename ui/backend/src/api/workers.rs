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

#[derive(Debug, Serialize)]
pub struct OrchestratorStatusResponse {
    pub pending_jobs: u32,
    pub active_pools: u32,
    pub total_workers: u32,
    pub available_workers: u32,
    pub busy_workers: u32,
    pub pool_ids: Vec<String>,
    pub active_jobs: Vec<ActiveJobEntry>,
}

#[derive(Debug, Serialize)]
pub struct ActiveJobEntry {
    pub job_id: String,
    pub pool_id: String,
    pub current_round: u32,
    pub max_rounds: u32,
    pub status: String,
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

/// GET /api/orchestrator/status - Get orchestrator status with active jobs
pub async fn get_orchestrator_status(
    State(client): State<Arc<ControllerGrpcClient>>,
) -> Result<Json<ApiResponse<OrchestratorStatusResponse>>, ApiError> {
    debug!("REST: Get orchestrator status");

    match client.get_orchestrator_status().await {
        Ok(resp) => {
            let active_jobs: Vec<ActiveJobEntry> = resp
                .active_jobs
                .into_iter()
                .map(|j| ActiveJobEntry {
                    job_id: j.job_id,
                    pool_id: j.pool_id,
                    current_round: j.current_round,
                    max_rounds: j.max_rounds,
                    status: j.status,
                })
                .collect();

            Ok(Json(ApiResponse::new(OrchestratorStatusResponse {
                pending_jobs: resp.pending_jobs,
                active_pools: resp.active_pools,
                total_workers: resp.total_workers,
                available_workers: resp.available_workers,
                busy_workers: resp.busy_workers,
                pool_ids: resp.pool_ids,
                active_jobs,
            })))
        }
        Err(e) => {
            error!("Failed to get orchestrator status: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}
