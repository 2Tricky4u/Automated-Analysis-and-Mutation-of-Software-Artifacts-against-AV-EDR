use crate::automutate::common::TelemetryData;
use crate::automutate::worker::{
    HealthRequest, HealthResponse, PingRequest, PingResponse,
};
use crate::{capabilities, telemetry, WorkerAgentService};
use std::time::SystemTime;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

pub async fn ping(
    service: &WorkerAgentService,
    request: Request<PingRequest>,
) -> Result<Response<PingResponse>, Status> {
    let req = request.into_inner();
    let timestamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    info!("Ping received: {}", req.message);

    Ok(Response::new(PingResponse {
        message: format!("pong: {}", req.message),
        timestamp,
        server: format!("worker-agent/{}", service.worker_id),
    }))
}

pub async fn health_check(
    service: &WorkerAgentService,
    request: Request<HealthRequest>,
) -> Result<Response<HealthResponse>, Status> {
    let _req = request.into_inner();

    // Refresh system information
    let mut sys = service.system_info.lock().await;
    sys.refresh_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );

    // Calculate average CPU usage across all cores
    let cpu_percent = sys.global_cpu_usage() as i32;

    // Calculate memory usage percentage
    let total_memory = sys.total_memory();
    let used_memory = sys.used_memory();
    let memory_percent = if total_memory > 0 {
        ((used_memory as f64 / total_memory as f64) * 100.0) as i32
    } else {
        0
    };

    // Get execution state (busy/idle)
    let exec_state = service.get_execution_state().await;
    let active_jobs = if exec_state.is_busy() { 1 } else { 0 };

    // Determine health status
    // Unhealthy if: CPU > 95% OR memory > 95%
    let healthy = cpu_percent < 95 && memory_percent < 95;

    // Log health check with execution state
    if exec_state.is_busy() {
        info!(
            "Health check: cpu={}%, mem={}%, status=BUSY (job_id={}, artifact={})",
            cpu_percent,
            memory_percent,
            exec_state.current_job_id().unwrap_or("unknown"),
            exec_state.current_artifact().unwrap_or("unknown")
        );
    } else if !healthy {
        warn!(
            "Health check UNHEALTHY: cpu={}%, mem={}%, status=IDLE",
            cpu_percent, memory_percent
        );
    }

    Ok(Response::new(HealthResponse {
        worker_id: service.worker_id.clone(),
        healthy,
        cpu_percent,
        memory_percent,
        active_jobs,
    }))
}

/// Get worker metadata
/// Controller calls this to query worker capabilities
pub async fn get_worker_info(
    service: &WorkerAgentService,
    _request: Request<crate::automutate::worker::WorkerInfoRequest>,
) -> Result<Response<crate::automutate::worker::WorkerInfoResponse>, Status> {
    use crate::automutate::common::ToolVersions;
    use crate::automutate::worker::HealthMetrics;

    info!("GetWorkerInfo RPC called by controller");

    // Detect current capabilities
    let capabilities_data = capabilities::detect_capabilities()
        .await
        .map_err(|e| Status::internal(format!("Failed to detect capabilities: {}", e)))?;

    // Get system health metrics
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );

    // Refresh CPU info and get usage
    sys.refresh_cpu_usage();
    let cpu_percent =
        sys.cpus().iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / sys.cpus().len() as f32;
    let cpu_percent = cpu_percent as i32;
    let memory_percent =
        ((sys.used_memory() as f64 / sys.total_memory() as f64) * 100.0) as i32;
    let disk_percent = 0; // TODO: Implement disk usage

    // Check if currently executing a job
    let execution_state = service.execution_lock.lock().await;
    let current_job_id = execution_state
        .current_job_id()
        .unwrap_or_default()
        .to_string();

    let active_jobs = if current_job_id.is_empty() { 0 } else { 1 };

    // Calculate uptime (time since worker started)
    let uptime_seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let response = crate::automutate::worker::WorkerInfoResponse {
        worker_id: service.worker_id.clone(),
        ip_address: service.config.worker.ip_address.clone(),
        os_version: service.config.worker.os_version.clone(),
        capabilities: capabilities_data.capabilities,
        metadata: capabilities_data.metadata,
        tools: Some(ToolVersions {
            rededr_version: capabilities_data
                .tools
                .get("rededr_version")
                .cloned()
                .unwrap_or_default(),
            defender_version: capabilities_data
                .tools
                .get("defender_version")
                .cloned()
                .unwrap_or_default(),
            etw_version: capabilities_data
                .tools
                .get("etw_version")
                .cloned()
                .unwrap_or_default(),
            llvm_version: capabilities_data
                .tools
                .get("llvm_version")
                .cloned()
                .unwrap_or_default(),
        }),
        health: Some(HealthMetrics {
            cpu_percent,
            memory_percent,
            disk_percent,
            active_jobs,
            uptime_seconds,
        }),
        current_job_id,
    };

    info!(
        "Returning worker metadata: {} capabilities, {} tools",
        response.capabilities.len(),
        response.tools.as_ref().map(|_| 4).unwrap_or(0)
    );

    Ok(Response::new(response))
}

/// Get telemetry from worker
/// Controller calls this to pull telemetry on demand
pub async fn get_telemetry(
    service: &WorkerAgentService,
    request: Request<crate::automutate::worker::TelemetryRequest>,
) -> Result<
    Response<
        std::pin::Pin<
            Box<dyn tokio_stream::Stream<Item = Result<TelemetryData, Status>> + Send>,
        >,
    >,
    Status,
> {
    use tokio_stream::wrappers::ReceiverStream;

    let req = request.into_inner();

    info!(
        "GetTelemetry RPC called: job_id={}, since_timestamp={}, max_events={}",
        req.job_id, req.since_timestamp, req.max_events
    );

    // For now, return telemetry from the most recent run
    // TODO: Implement persistent storage of telemetry per job_id

    // Create a channel for streaming telemetry events
    let (tx, rx) = tokio::sync::mpsc::channel(100);

    // Spawn task to collect and stream telemetry
    let base_url = service.config.telemetry.rededr.base_url.clone();
    let job_id = req.job_id.clone();
    let max_events = req.max_events;

    tokio::spawn(async move {
        // Collect telemetry from RedEDR
        let collector = telemetry::collectors::rededr::RedEdrCollector::new(
            telemetry::collectors::rededr::RedEdrCollectorConfig {
                base_url,
                flush_interval_ms: 1000,
                job_id: job_id.clone(),
                run_id: "telemetry-pull".to_string(),
            },
        );

        match collector.collect_all(&job_id).await {
            Ok(events) => {
                let events_to_send: Vec<_> = if max_events > 0 {
                    events.into_iter().take(max_events as usize).collect()
                } else {
                    events
                };

                info!(
                    "Collected {} telemetry events for job {}",
                    events_to_send.len(),
                    job_id
                );

                for event in events_to_send {
                    if tx.send(Ok(event)).await.is_err() {
                        break; // Client disconnected
                    }
                }
            }
            Err(e) => {
                error!("Failed to collect telemetry: {}", e);
                let _ = tx
                    .send(Err(Status::internal(format!(
                        "Failed to collect telemetry: {}",
                        e
                    ))))
                    .await;
            }
        }
    });

    let stream = ReceiverStream::new(rx);
    Ok(Response::new(Box::pin(stream)))
}
