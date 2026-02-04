//! Worker handlers - worker listing, telemetry streaming, and monitoring

use crate::automutate::common::{TelemetryAck, TelemetryData, ToolVersions};
use crate::automutate::controller::{
    ActiveJobEntry, GetAvailableWorkersRequest, GetAvailableWorkersResponse,
    GetOrchestratorStatusRequest, GetOrchestratorStatusResponse, GetPoolMetricsRequest,
    GetPoolMetricsResponse, GetWorkerMetadataRequest, GetWorkerMetadataResponse, GetWorkerRequest,
    GetWorkerResponse, ListWorkersRequest, ListWorkersResponse, PoolMetricsEntry, WorkerInfo,
    WorkerMetadataEntry,
};
use crate::dispatch::group_id::GroupId;
use crate::service::SchedulerService;
use crate::target_manager::{RegistrationType, TargetStatus};
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

/// List all registered workers
pub async fn list_workers(
    service: &SchedulerService,
    _request: Request<ListWorkersRequest>,
) -> Result<Response<ListWorkersResponse>, Status> {
    let workers = service.targets.list_all();
    let worker_infos: Vec<WorkerInfo> = workers
        .iter()
        .map(|w| {
            let last_ping_secs = w
                .last_seen
                .elapsed()
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            WorkerInfo {
                worker_id: w.id.clone(),
                address: w.address.clone(),
                status: w.status.to_string(),
                current_job_id: w.current_job.clone().unwrap_or_default(),
                last_ping_seconds_ago: last_ping_secs,
                enabled: w.enabled,
                os_version: w.os_version.clone(),
                capabilities: w.capabilities.clone(),
                metadata: w.metadata.clone(),
                tools: Some(ToolVersions {
                    rededr_version: w.tools.get("rededr").cloned().unwrap_or_default(),
                    defender_version: w.tools.get("defender").cloned().unwrap_or_default(),
                    etw_version: w.tools.get("etw").cloned().unwrap_or_default(),
                    llvm_version: w.tools.get("llvm").cloned().unwrap_or_default(),
                }),
                registration_type: match w.registration_type {
                    RegistrationType::Dynamic => "dynamic".to_string(),
                    RegistrationType::Static => "static".to_string(),
                },
            }
        })
        .collect();

    Ok(Response::new(ListWorkersResponse {
        workers: worker_infos,
    }))
}

/// Stream telemetry from workers
pub async fn stream_telemetry(
    service: &SchedulerService,
    request: Request<tonic::Streaming<TelemetryData>>,
) -> Result<Response<TelemetryAck>, Status> {
    use tokio::time::{timeout, Duration};

    let remote_addr = request
        .remote_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let mut stream = request.into_inner();
    let mut events_count: i32 = 0;
    let mut first_job_id = String::new();
    let mut batch = Vec::new();
    const MAX_BATCH_SIZE: usize = 10000;

    info!("[RECV] Telemetry stream opened from: {}", remote_addr);

    // Collect events from stream
    let collection_result = timeout(Duration::from_secs(30), async {
        while let Some(telemetry) = stream.message().await? {
            if events_count == 0 {
                first_job_id = telemetry.job_id.clone();
            }

            events_count = events_count.saturating_add(1);
            batch.push(telemetry);

            if batch.len() >= MAX_BATCH_SIZE {
                warn!(
                    "[WARN] Telemetry batch size limit reached ({} events)",
                    MAX_BATCH_SIZE
                );
                break;
            }
        }
        Ok::<(), tonic::Status>(())
    })
    .await;

    match collection_result {
        Ok(Ok(())) => {
            debug!(
                "[OK] Telemetry collected: job={}, events={}",
                first_job_id, events_count
            );
        }
        Ok(Err(e)) => {
            error!("[ERROR] Stream error: {:?}", e);
            return Err(e);
        }
        Err(_) => {
            error!("[TIMEOUT] Telemetry collection exceeded 30s");
        }
    }

    // Index to Elasticsearch
    if !batch.is_empty() {
        match timeout(Duration::from_secs(10), service.index_telemetry_batch(&batch)).await {
            Ok(Ok(())) => {
                info!("[OK] Indexed {} telemetry events", events_count);
            }
            Ok(Err(e)) => {
                error!("[ERROR] ES indexing failed: {}", e);
            }
            Err(_) => {
                error!("[TIMEOUT] ES indexing exceeded 10s");
            }
        }
    } else {
        warn!("[WARN] Telemetry stream closed with zero events");
    }

    Ok(Response::new(TelemetryAck {
        received: true,
        events_count,
    }))
}

/// Get a specific worker by ID
pub async fn get_worker(
    service: &SchedulerService,
    request: Request<GetWorkerRequest>,
) -> Result<Response<GetWorkerResponse>, Status> {
    let worker_id = &request.get_ref().worker_id;
    debug!("[RPC] GetWorker: worker_id={}", worker_id);

    match service.targets.get(worker_id) {
        Some(w) => {
            let last_ping_secs = w
                .last_seen
                .elapsed()
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let worker_info = WorkerInfo {
                worker_id: w.id.clone(),
                address: w.address.clone(),
                status: w.status.to_string(),
                current_job_id: w.current_job.clone().unwrap_or_default(),
                last_ping_seconds_ago: last_ping_secs,
                enabled: w.enabled,
                os_version: w.os_version.clone(),
                capabilities: w.capabilities.clone(),
                metadata: w.metadata.clone(),
                tools: Some(ToolVersions {
                    rededr_version: w.tools.get("rededr").cloned().unwrap_or_default(),
                    defender_version: w.tools.get("defender").cloned().unwrap_or_default(),
                    etw_version: w.tools.get("etw").cloned().unwrap_or_default(),
                    llvm_version: w.tools.get("llvm").cloned().unwrap_or_default(),
                }),
                registration_type: match w.registration_type {
                    RegistrationType::Dynamic => "dynamic".to_string(),
                    RegistrationType::Static => "static".to_string(),
                },
            };

            Ok(Response::new(GetWorkerResponse {
                worker: Some(worker_info),
                found: true,
            }))
        }
        None => Ok(Response::new(GetWorkerResponse {
            worker: None,
            found: false,
        })),
    }
}

/// Get available workers (not busy)
pub async fn get_available_workers(
    service: &SchedulerService,
    request: Request<GetAvailableWorkersRequest>,
) -> Result<Response<GetAvailableWorkersResponse>, Status> {
    let req = request.get_ref();
    debug!(
        "[RPC] GetAvailableWorkers: target_os={}, caps={:?}",
        req.target_os, req.required_capabilities
    );

    let all_workers = service.targets.list_all();

    // Filter available workers
    let available: Vec<WorkerInfo> = all_workers
        .iter()
        .filter(|w| {
            // Must be available
            if w.status != TargetStatus::Available {
                return false;
            }

            // Filter by OS if specified
            if !req.target_os.is_empty() && !w.os_version.contains(&req.target_os) {
                return false;
            }

            // Filter by capabilities if specified
            if !req.required_capabilities.is_empty() {
                for cap in &req.required_capabilities {
                    if !w.capabilities.contains(cap) {
                        return false;
                    }
                }
            }

            true
        })
        .map(|w| {
            let last_ping_secs = w
                .last_seen
                .elapsed()
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            WorkerInfo {
                worker_id: w.id.clone(),
                address: w.address.clone(),
                status: w.status.to_string(),
                current_job_id: w.current_job.clone().unwrap_or_default(),
                last_ping_seconds_ago: last_ping_secs,
                enabled: w.enabled,
                os_version: w.os_version.clone(),
                capabilities: w.capabilities.clone(),
                metadata: w.metadata.clone(),
                tools: Some(ToolVersions {
                    rededr_version: w.tools.get("rededr").cloned().unwrap_or_default(),
                    defender_version: w.tools.get("defender").cloned().unwrap_or_default(),
                    etw_version: w.tools.get("etw").cloned().unwrap_or_default(),
                    llvm_version: w.tools.get("llvm").cloned().unwrap_or_default(),
                }),
                registration_type: match w.registration_type {
                    RegistrationType::Dynamic => "dynamic".to_string(),
                    RegistrationType::Static => "static".to_string(),
                },
            }
        })
        .collect();

    let total = available.len() as i32;

    Ok(Response::new(GetAvailableWorkersResponse {
        workers: available,
        total_available: total,
    }))
}

/// Get enhanced worker metadata
pub async fn get_worker_metadata(
    service: &SchedulerService,
    request: Request<GetWorkerMetadataRequest>,
) -> Result<Response<GetWorkerMetadataResponse>, Status> {
    let req = request.get_ref();
    debug!("[RPC] GetWorkerMetadata: worker_id={}", req.worker_id);

    const HEALTH_THRESHOLD_SECS: u64 = 120; // 2 minutes

    let workers = if req.worker_id.is_empty() {
        // Return all workers
        service.targets.list_all()
    } else {
        // Return specific worker
        match service.targets.get(&req.worker_id) {
            Some(w) => vec![w],
            None => vec![],
        }
    };

    let entries: Vec<WorkerMetadataEntry> = workers
        .iter()
        .map(|w| {
            let last_seen_secs = w
                .last_seen
                .elapsed()
                .map(|d| d.as_secs())
                .unwrap_or(u64::MAX);

            let healthy = last_seen_secs < HEALTH_THRESHOLD_SECS
                && w.status != TargetStatus::Offline;

            let connected_at = w
                .connected_at
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                })
                .unwrap_or(0);

            WorkerMetadataEntry {
                worker_id: w.id.clone(),
                address: w.address.clone(),
                status: w.status.to_string(),
                os_version: w.os_version.clone(),
                capabilities: w.capabilities.clone(),
                metadata: w.metadata.clone(),
                tools: Some(ToolVersions {
                    rededr_version: w.tools.get("rededr").cloned().unwrap_or_default(),
                    defender_version: w.tools.get("defender").cloned().unwrap_or_default(),
                    etw_version: w.tools.get("etw").cloned().unwrap_or_default(),
                    llvm_version: w.tools.get("llvm").cloned().unwrap_or_default(),
                }),
                last_seen_seconds_ago: last_seen_secs as i64,
                healthy,
                current_job_id: w.current_job.clone().unwrap_or_default(),
                connected_at,
            }
        })
        .collect();

    Ok(Response::new(GetWorkerMetadataResponse { workers: entries }))
}

/// Get pool metrics
pub async fn get_pool_metrics(
    service: &SchedulerService,
    request: Request<GetPoolMetricsRequest>,
) -> Result<Response<GetPoolMetricsResponse>, Status> {
    let req = request.get_ref();
    debug!("[RPC] GetPoolMetrics: pool_id={}", req.pool_id);

    let registry = service.targets.pool_registry();
    let mut entries = Vec::new();

    if req.pool_id.is_empty() {
        // Return all pools
        for group_id in registry.list_ids().await {
            if let Some(pool) = registry.get(&group_id).await {
                let metrics = pool.get_metrics().await;
                let queue_size = pool.pool_size().await;
                let current_job = pool.current_job_id().await;

                entries.push(PoolMetricsEntry {
                    pool_id: group_id.as_str().to_string(),
                    total_runs_dispatched: metrics.total_runs_dispatched,
                    total_runs_completed: metrics.total_runs_completed,
                    total_rounds_completed: metrics.total_rounds_completed,
                    total_jobs_completed: metrics.total_jobs_completed,
                    current_queue_size: queue_size as u32,
                    worker_count: 0, // TODO: Track worker count per pool
                    current_job_id: current_job.map(|j| j.0).unwrap_or_default(),
                });
            }
        }
    } else {
        // Return specific pool
        let group_id = GroupId::new(&req.pool_id);
        if let Some(pool) = registry.get(&group_id).await {
            let metrics = pool.get_metrics().await;
            let queue_size = pool.pool_size().await;
            let current_job = pool.current_job_id().await;

            entries.push(PoolMetricsEntry {
                pool_id: req.pool_id.clone(),
                total_runs_dispatched: metrics.total_runs_dispatched,
                total_runs_completed: metrics.total_runs_completed,
                total_rounds_completed: metrics.total_rounds_completed,
                total_jobs_completed: metrics.total_jobs_completed,
                current_queue_size: queue_size as u32,
                worker_count: 0,
                current_job_id: current_job.map(|j| j.0).unwrap_or_default(),
            });
        }
    }

    Ok(Response::new(GetPoolMetricsResponse { pools: entries }))
}

/// Get orchestrator status
pub async fn get_orchestrator_status(
    service: &SchedulerService,
    _request: Request<GetOrchestratorStatusRequest>,
) -> Result<Response<GetOrchestratorStatusResponse>, Status> {
    debug!("[RPC] GetOrchestratorStatus");

    let registry = service.targets.pool_registry();
    let all_workers = service.targets.list_all();

    // Count worker states
    let total_workers = all_workers.len() as u32;
    let available_workers = all_workers
        .iter()
        .filter(|w| w.status == TargetStatus::Available)
        .count() as u32;
    let busy_workers = all_workers
        .iter()
        .filter(|w| w.status == TargetStatus::Busy)
        .count() as u32;

    // Get pool info
    let pool_id_list = registry.list_ids().await;
    let pool_ids: Vec<String> = pool_id_list.iter().map(|g| g.as_str().to_string()).collect();
    let active_pools = pool_ids.len() as u32;

    // Get active jobs from pools
    let mut active_jobs = Vec::new();
    for group_id in pool_id_list {
        if let Some(pool) = registry.get(&group_id).await {
            if let Some(job_id) = pool.current_job_id().await {
                active_jobs.push(ActiveJobEntry {
                    job_id: job_id.0,
                    pool_id: group_id.as_str().to_string(),
                    current_round: 0, // TODO: Track current round
                    max_rounds: 0,    // TODO: Track max rounds
                    status: "running".to_string(),
                });
            }
        }
    }

    Ok(Response::new(GetOrchestratorStatusResponse {
        pending_jobs: 0, // TODO: Get from orchestrator
        active_pools,
        total_workers,
        available_workers,
        busy_workers,
        pool_ids,
        active_jobs,
    }))
}