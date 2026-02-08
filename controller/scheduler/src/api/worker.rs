//! Worker handlers - worker listing, telemetry streaming, and monitoring

use crate::automutate::common::{TelemetryAck, TelemetryData, ToolVersions};
use crate::automutate::controller::{
    ActiveJobEntry, GetAvailableWorkersRequest, GetAvailableWorkersResponse,
    GetOrchestratorStatusRequest, GetOrchestratorStatusResponse, GetPoolMetricsRequest,
    GetPoolMetricsResponse, GetWorkerMetadataRequest, GetWorkerMetadataResponse, GetWorkerRequest,
    GetWorkerResponse, ListWorkersRequest, ListWorkersResponse, PoolMetricsEntry, WorkerInfo,
    WorkerMetadataEntry,
};
use crate::api::SchedulerService;
use crate::vm::{RegistrationType, TargetStatus};
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
                worker_id: w.id.to_string(),
                address: w.address.clone(),
                status: w.status.to_string(),
                current_job_id: w.current_job.as_ref().map(|j| j.0.clone()).unwrap_or_default(),
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
                worker_id: w.id.to_string(),
                address: w.address.clone(),
                status: w.status.to_string(),
                current_job_id: w.current_job.as_ref().map(|j| j.0.clone()).unwrap_or_default(),
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
                worker_id: w.id.to_string(),
                address: w.address.clone(),
                status: w.status.to_string(),
                current_job_id: w.current_job.as_ref().map(|j| j.0.clone()).unwrap_or_default(),
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
                worker_id: w.id.to_string(),
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
                current_job_id: w.current_job.as_ref().map(|j| j.0.clone()).unwrap_or_default(),
                connected_at,
            }
        })
        .collect();

    Ok(Response::new(GetWorkerMetadataResponse { workers: entries }))
}

/// Get pool metrics
///
/// In the new architecture, there's a single shared RunPool.
/// Jobs are tracked via Orchestrator, not per-pool.
pub async fn get_pool_metrics(
    service: &SchedulerService,
    request: Request<GetPoolMetricsRequest>,
) -> Result<Response<GetPoolMetricsResponse>, Status> {
    let req = request.get_ref();
    debug!("[RPC] GetPoolMetrics: pool_id={}", req.pool_id);

    // In new architecture, there's a single shared RunPool
    let metrics = service.run_pool.get_metrics().await;
    let queue_size = service.run_pool.pool_size().await;
    let job_count = service.run_pool.job_count().await;

    // Get VM count from targets
    let all_workers = service.targets.list_all();
    let vm_count = all_workers
        .iter()
        .filter(|w| w.status == TargetStatus::Available || w.status == TargetStatus::Busy)
        .count();

    let entry = PoolMetricsEntry {
        pool_id: "shared-run-pool".to_string(),
        total_runs_dispatched: metrics.total_runs_added,  // Renamed in new architecture
        total_runs_completed: metrics.total_runs_taken,
        total_rounds_completed: 0,  // TODO: Track in RunPool or Orchestrator
        total_jobs_completed: 0,    // TODO: Track in Orchestrator
        current_queue_size: queue_size as u32,
        worker_count: vm_count as u32,
        current_job_id: format!("{} active jobs", job_count),
    };

    Ok(Response::new(GetPoolMetricsResponse { pools: vec![entry] }))
}

/// Get orchestrator status
///
/// In the new architecture:
/// - There's a single shared RunPool (not per-capability pools)
/// - Jobs are tracked via Orchestrator's job_workers HashMap
/// - VMs are tracked via TargetManager
pub async fn get_orchestrator_status(
    service: &SchedulerService,
    _request: Request<GetOrchestratorStatusRequest>,
) -> Result<Response<GetOrchestratorStatusResponse>, Status> {
    debug!("[RPC] GetOrchestratorStatus");

    let all_workers = service.targets.list_all();

    // Count VM states
    let total_workers = all_workers.len() as u32;
    let available_workers = all_workers
        .iter()
        .filter(|w| w.status == TargetStatus::Available)
        .count() as u32;
    let busy_workers = all_workers
        .iter()
        .filter(|w| w.status == TargetStatus::Busy)
        .count() as u32;

    // In new architecture, there's a single shared pool
    let pool_ids = vec!["shared-run-pool".to_string()];
    let active_pools = 1;

    // Get active jobs from RunPool registry
    let running_jobs = service.run_pool.list_running_jobs();
    let active_jobs: Vec<ActiveJobEntry> = running_jobs
        .iter()
        .map(|job| ActiveJobEntry {
            job_id: job.id.0.clone(),
            pool_id: "shared-run-pool".to_string(),
            current_round: job.current_round,
            max_rounds: job.max_rounds,
            status: job.status.to_string(),
        })
        .collect();

    Ok(Response::new(GetOrchestratorStatusResponse {
        pending_jobs: service.run_pool.pool_size().await as u32,  // Pending runs in pool
        active_pools,
        total_workers,
        available_workers,
        busy_workers,
        pool_ids,
        active_jobs,
    }))
}