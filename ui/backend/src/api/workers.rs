//! Worker pool management REST endpoints.
//!
//! Provides listing, metadata inspection, availability filtering, and
//! administrative commands (ping, disconnect) for the worker VM pool.
//! Also exposes the orchestrator status endpoint that aggregates active jobs,
//! queue depth, and pool utilization metrics.

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

/// Worker list with aggregated status counts.
///
/// Returned by `GET /api/workers`.
#[derive(Debug, Serialize)]
pub struct WorkersListResponse {
    /// All registered workers.
    pub workers: Vec<WorkerInfo>,
    /// Total number of workers.
    pub total: usize,
    /// Workers in `"available"` status.
    pub available: usize,
    /// Workers in `"busy"` status.
    pub busy: usize,
    /// Workers in `"offline"` status.
    pub offline: usize,
}

/// Basic worker state snapshot.
#[derive(Debug, Serialize)]
pub struct WorkerInfo {
    /// Unique worker identifier.
    pub id: String,
    /// Network address (host:port) of the worker agent.
    pub address: String,
    /// Windows version string (e.g. `"Windows 10 21H2"`).
    pub os_version: String,
    /// Advertised capabilities (e.g. `["defender", "rededr", "etw"]`).
    pub capabilities: Vec<String>,
    /// Current status: `"available"`, `"busy"`, or `"offline"`.
    pub status: String,
    /// Job ID currently assigned to this worker, if any.
    pub current_job: Option<String>,
}

/// Aggregated orchestrator metrics.
///
/// Returned by `GET /api/orchestrator/status`.
#[derive(Debug, Serialize)]
pub struct OrchestratorStatusResponse {
    /// Number of jobs waiting in the queue.
    pub pending_jobs: u32,
    /// Number of active worker pools.
    pub active_pools: u32,
    /// Total registered workers (all statuses).
    pub total_workers: u32,
    /// Workers currently in `"available"` status.
    pub available_workers: u32,
    /// Workers currently in `"busy"` status.
    pub busy_workers: u32,
    /// IDs of active worker pools.
    pub pool_ids: Vec<String>,
    /// Per-job entries for currently active jobs.
    pub active_jobs: Vec<ActiveJobEntry>,
}

/// Per-job entry in the orchestrator's active-jobs list.
#[derive(Debug, Serialize)]
pub struct ActiveJobEntry {
    /// Job identifier.
    pub job_id: String,
    /// Worker pool assigned to this job.
    pub pool_id: String,
    /// Current round number.
    pub current_round: u32,
    /// Total budgeted rounds.
    pub max_rounds: u32,
    /// Job status: `"running"`, `"building"`, etc.
    pub status: String,
}

/// Enhanced worker metadata with health and tool information.
///
/// Returned by `GET /api/workers/metadata`.
#[derive(Debug, Serialize)]
pub struct WorkerMetadataResponse {
    /// All workers with extended metadata.
    pub workers: Vec<WorkerMetadataInfo>,
    /// Total number of workers.
    pub total: usize,
    /// Workers in `"available"` status.
    pub available: usize,
    /// Workers in `"busy"` status.
    pub busy: usize,
    /// Workers in `"offline"` status.
    pub offline: usize,
}

/// Extended per-worker information including health checks and tool versions.
#[derive(Debug, Serialize)]
pub struct WorkerMetadataInfo {
    /// Unique worker identifier.
    pub id: String,
    /// Network address (host:port).
    pub address: String,
    /// Current status: `"available"`, `"busy"`, or `"offline"`.
    pub status: String,
    /// Windows version string.
    pub os_version: String,
    /// Advertised capabilities.
    pub capabilities: Vec<String>,
    /// Installed tool versions on the worker VM, if reported.
    pub tools: Option<ToolVersionsInfo>,
    /// Seconds since the Controller last received a heartbeat.
    pub last_seen_seconds_ago: i64,
    /// `true` if the worker passed its most recent health check.
    pub healthy: bool,
    /// Job ID currently assigned to this worker, if any.
    pub current_job: Option<String>,
    /// Unix timestamp (seconds) when the worker first connected.
    pub connected_at: i64,
}

/// Installed tool versions on a worker VM.
#[derive(Debug, Serialize)]
pub struct ToolVersionsInfo {
    /// RedEDR telemetry collector version.
    pub rededr_version: String,
    /// Windows Defender engine version.
    pub defender_version: String,
    /// ETW provider version.
    pub etw_version: String,
    /// LLVM toolchain version (used for IR instrumentation).
    pub llvm_version: String,
}

/// Filtered worker list.
///
/// Returned by `GET /api/workers/available`.
#[derive(Debug, Serialize)]
pub struct AvailableWorkersResponse {
    /// Workers matching the filter criteria.
    pub workers: Vec<WorkerInfo>,
    /// Count of matching workers.
    pub total_available: i32,
}

/// Query parameters for `GET /api/workers/available`.
#[derive(Debug, Deserialize)]
pub struct AvailableWorkersQuery {
    /// Target OS filter (e.g. `"win10"`). Empty or absent means any OS.
    pub os: Option<String>,
    /// Comma-separated required capabilities (e.g. `"defender,rededr"`).
    pub capabilities: Option<String>,
}

/// Generic success/failure response for admin operations.
#[derive(Debug, Serialize)]
pub struct AdminCommandResponse {
    /// `true` if the operation succeeded.
    pub success: bool,
    /// Human-readable result message.
    pub message: String,
}

/// Result of a bulk disconnect operation.
#[derive(Debug, Serialize)]
pub struct DisconnectAllResponse {
    /// Number of workers that were disconnected.
    pub disconnected_count: u32,
    /// Human-readable summary.
    pub message: String,
}

/// Optional request body for `POST /api/workers/:id/disconnect`.
#[derive(Debug, Deserialize)]
pub struct DisconnectBody {
    /// Reason for disconnecting. Defaults to `"admin_disconnect"`.
    pub reason: Option<String>,
}

/// Optional request body for `POST /api/workers/disconnect-all`.
#[derive(Debug, Deserialize)]
pub struct DisconnectAllBody {
    /// Reason for disconnecting all workers. Defaults to
    /// `"admin_disconnect_all"`.
    pub reason: Option<String>,
    /// Whether workers may re-register after disconnect. Defaults to `true`.
    pub reconnect_allowed: Option<bool>,
}

// ============================================================================
// Handlers
// ============================================================================

/// `GET /api/workers` — List all registered workers with status counts.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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

/// `GET /api/workers/metadata` — Get enhanced worker metadata for all workers.
///
/// Returns health flags, tool versions, last-seen timestamps, and connection
/// times in addition to the basic worker state.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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

/// `GET /api/workers/available?os=X&capabilities=a,b` — Get available workers
/// matching OS and capability filters.
///
/// The `capabilities` query parameter is a comma-separated list; each entry is
/// trimmed and empty segments are discarded.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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

/// `GET /api/orchestrator/status` — Get orchestrator status with active jobs,
/// queue depth, and pool metrics.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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

/// `POST /api/workers/:id/ping` — Ping a specific worker through the
/// Controller relay.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable or the worker does
///   not respond.
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

/// `POST /api/workers/:id/disconnect` — Disconnect a specific worker.
///
/// Accepts an optional JSON body with a `reason` field; defaults to
/// `"admin_disconnect"` when absent.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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

/// `POST /api/workers/disconnect-all` — Disconnect all registered workers.
///
/// Accepts an optional JSON body with `reason` (defaults to
/// `"admin_disconnect_all"`) and `reconnect_allowed` (defaults to `true`).
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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
