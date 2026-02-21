//! Worker management REST endpoints
//!
//! Wraps Controller gRPC worker endpoints.

use super::{ApiError, ApiResponse};
use crate::grpc_client::ControllerGrpcClient;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Serialize)]
pub struct WorkerMetadataResponse {
    pub workers: Vec<WorkerMetadataInfo>,
    pub total: usize,
    pub available: usize,
    pub busy: usize,
    pub offline: usize,
}

#[derive(Debug, Serialize)]
pub struct WorkerMetadataInfo {
    pub id: String,
    pub address: String,
    pub status: String,
    pub os_version: String,
    pub capabilities: Vec<String>,
    pub tools: Option<ToolVersionsInfo>,
    pub last_seen_seconds_ago: i64,
    pub healthy: bool,
    pub current_job: Option<String>,
    pub connected_at: i64,
}

#[derive(Debug, Serialize)]
pub struct ToolVersionsInfo {
    pub rededr_version: String,
    pub defender_version: String,
    pub etw_version: String,
    pub llvm_version: String,
}

#[derive(Debug, Serialize)]
pub struct AvailableWorkersResponse {
    pub workers: Vec<WorkerInfo>,
    pub total_available: i32,
}

#[derive(Debug, Deserialize)]
pub struct AvailableWorkersQuery {
    pub os: Option<String>,
    pub capabilities: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AdminCommandResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct DisconnectAllResponse {
    pub disconnected_count: u32,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct DisconnectBody {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DisconnectAllBody {
    pub reason: Option<String>,
    pub reconnect_allowed: Option<bool>,
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

/// GET /api/workers/metadata - Get enhanced worker metadata (health, tools, last_seen)
pub async fn get_worker_metadata(
    State(client): State<Arc<ControllerGrpcClient>>,
) -> Result<Json<ApiResponse<WorkerMetadataResponse>>, ApiError> {
    debug!("REST: Get worker metadata");

    match client.get_worker_metadata("").await {
        Ok(resp) => {
            let workers: Vec<WorkerMetadataInfo> = resp
                .workers
                .into_iter()
                .map(|w| {
                    let tools = w.tools.map(|t| ToolVersionsInfo {
                        rededr_version: t.rededr_version,
                        defender_version: t.defender_version,
                        etw_version: t.etw_version,
                        llvm_version: t.llvm_version,
                    });

                    WorkerMetadataInfo {
                        id: w.worker_id,
                        address: w.address,
                        status: w.status.clone(),
                        os_version: w.os_version,
                        capabilities: w.capabilities,
                        tools,
                        last_seen_seconds_ago: w.last_seen_seconds_ago,
                        healthy: w.healthy,
                        current_job: if w.current_job_id.is_empty() {
                            None
                        } else {
                            Some(w.current_job_id)
                        },
                        connected_at: w.connected_at,
                    }
                })
                .collect();

            let total = workers.len();
            let available = workers.iter().filter(|w| w.status == "available").count();
            let busy = workers.iter().filter(|w| w.status == "busy").count();
            let offline = workers.iter().filter(|w| w.status == "offline").count();

            Ok(Json(ApiResponse::new(WorkerMetadataResponse {
                workers,
                total,
                available,
                busy,
                offline,
            })))
        }
        Err(e) => {
            error!("Failed to get worker metadata: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

/// GET /api/workers/available?os=X&capabilities=a,b - Get available workers
pub async fn get_available_workers(
    State(client): State<Arc<ControllerGrpcClient>>,
    Query(query): Query<AvailableWorkersQuery>,
) -> Result<Json<ApiResponse<AvailableWorkersResponse>>, ApiError> {
    debug!(
        "REST: Get available workers os={:?} caps={:?}",
        query.os, query.capabilities
    );

    let os = query.os.unwrap_or_default();
    let caps: Vec<String> = query
        .capabilities
        .map(|c| {
            c.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    match client.get_available_workers(&os, caps).await {
        Ok(resp) => {
            let workers: Vec<WorkerInfo> = resp
                .workers
                .into_iter()
                .map(|w| WorkerInfo {
                    id: w.worker_id,
                    address: w.address,
                    os_version: w.os_version,
                    capabilities: w.capabilities,
                    status: w.status,
                    current_job: if w.current_job_id.is_empty() {
                        None
                    } else {
                        Some(w.current_job_id)
                    },
                })
                .collect();

            Ok(Json(ApiResponse::new(AvailableWorkersResponse {
                total_available: resp.total_available,
                workers,
            })))
        }
        Err(e) => {
            error!("Failed to get available workers: {}", e);
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

// ============================================================================
// Admin Command Handlers
// ============================================================================

/// POST /api/workers/:id/ping - Ping a specific worker
pub async fn ping_worker(
    State(client): State<Arc<ControllerGrpcClient>>,
    Path(worker_id): Path<String>,
) -> Result<Json<ApiResponse<AdminCommandResponse>>, ApiError> {
    debug!("REST: Ping worker {}", worker_id);

    match client.ping_worker(&worker_id).await {
        Ok(resp) => Ok(Json(ApiResponse::new(AdminCommandResponse {
            success: resp.success,
            message: resp.message,
        }))),
        Err(e) => {
            error!("Failed to ping worker {}: {}", worker_id, e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

/// POST /api/workers/:id/disconnect - Disconnect a specific worker
pub async fn disconnect_worker(
    State(client): State<Arc<ControllerGrpcClient>>,
    Path(worker_id): Path<String>,
    body: Option<Json<DisconnectBody>>,
) -> Result<Json<ApiResponse<AdminCommandResponse>>, ApiError> {
    let reason = body
        .and_then(|b| b.0.reason)
        .unwrap_or_else(|| "admin_disconnect".to_string());
    debug!("REST: Disconnect worker {} reason={}", worker_id, reason);

    match client.disconnect_worker(&worker_id, &reason).await {
        Ok(resp) => Ok(Json(ApiResponse::new(AdminCommandResponse {
            success: resp.success,
            message: resp.message,
        }))),
        Err(e) => {
            error!("Failed to disconnect worker {}: {}", worker_id, e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

/// POST /api/workers/disconnect-all - Disconnect all workers
pub async fn disconnect_all_workers(
    State(client): State<Arc<ControllerGrpcClient>>,
    body: Option<Json<DisconnectAllBody>>,
) -> Result<Json<ApiResponse<DisconnectAllResponse>>, ApiError> {
    let (reason, reconnect) = match body {
        Some(Json(b)) => (
            b.reason
                .unwrap_or_else(|| "admin_disconnect_all".to_string()),
            b.reconnect_allowed.unwrap_or(true),
        ),
        None => ("admin_disconnect_all".to_string(), true),
    };
    debug!("REST: Disconnect all workers reason={}", reason);

    match client.disconnect_all_workers(&reason, reconnect).await {
        Ok(resp) => Ok(Json(ApiResponse::new(DisconnectAllResponse {
            disconnected_count: resp.disconnected_count,
            message: resp.message,
        }))),
        Err(e) => {
            error!("Failed to disconnect all workers: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}
