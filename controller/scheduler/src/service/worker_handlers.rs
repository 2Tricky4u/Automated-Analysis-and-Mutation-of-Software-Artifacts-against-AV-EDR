//! Worker handlers - worker listing and telemetry streaming

use crate::automutate::common::{TelemetryAck, TelemetryData, ToolVersions};
use crate::automutate::controller::{ListWorkersRequest, ListWorkersResponse, WorkerInfo};
use crate::service::SchedulerService;
use crate::target_manager::RegistrationType;
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