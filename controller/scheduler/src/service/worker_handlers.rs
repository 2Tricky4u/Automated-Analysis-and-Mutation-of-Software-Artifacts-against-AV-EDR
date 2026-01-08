use crate::automutate::common::{TelemetryAck, TelemetryData, ToolVersions};
use crate::automutate::controller::{ListWorkersRequest, ListWorkersResponse, WorkerInfo};
use crate::worker_pool;
use crate::SchedulerService;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

pub async fn list_workers(
    service: &SchedulerService,
    _request: Request<ListWorkersRequest>,
) -> Result<Response<ListWorkersResponse>, Status> {
    let scheduler_core = match &service.scheduler_core {
        Some(core) => core,
        None => {
            return Err(Status::unavailable("Scheduler core not initialized"));
        }
    };

    let workers = scheduler_core.pool().list_workers();
    let worker_infos: Vec<WorkerInfo> = workers
        .iter()
        .map(|w| {
            let last_ping_secs = w
                .last_ping
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
                // NEW FIELDS for dynamic registration
                os_version: w.os_version.clone(),
                capabilities: w.capabilities.clone(),
                metadata: w.metadata.clone(),
                tools: Some(ToolVersions {
                    rededr_version: w.tools.get("rededr_version").cloned().unwrap_or_default(),
                    defender_version: w
                        .tools
                        .get("defender_version")
                        .cloned()
                        .unwrap_or_default(),
                    etw_version: w.tools.get("etw_version").cloned().unwrap_or_default(),
                    llvm_version: w.tools.get("llvm_version").cloned().unwrap_or_default(),
                }),
                registration_type: match w.registration_type {
                    worker_pool::RegistrationType::Dynamic => "dynamic".to_string(),
                    worker_pool::RegistrationType::Static => "static".to_string(),
                },
            }
        })
        .collect();

    Ok(Response::new(ListWorkersResponse {
        workers: worker_infos,
    }))
}

pub async fn stream_telemetry(
    service: &SchedulerService,
    request: Request<tonic::Streaming<TelemetryData>>,
) -> Result<Response<TelemetryAck>, Status> {
    use tokio::time::{Duration, timeout};

    // Get remote_addr BEFORE into_inner() consumes the request
    let remote_addr = request
        .remote_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let mut stream = request.into_inner();
    let mut events_count = 0;
    let mut first_job_id = String::new();
    let mut batch = Vec::new();
    const MAX_BATCH_SIZE: usize = 10000; // Prevent memory exhaustion

    info!(
        "[RECV]  Telemetry stream opened from worker: {}",
        remote_addr
    );

    // Collect all events from stream with timeout
    let collection_result = timeout(Duration::from_secs(30), async {
        while let Some(telemetry) = stream.message().await? {
            if events_count == 0 {
                first_job_id = telemetry.job_id.clone();
            }

            events_count += 1;
            batch.push(telemetry);

            // Prevent unbounded memory growth
            if batch.len() >= MAX_BATCH_SIZE {
                warn!(
                    "[WARN]  Telemetry batch size limit reached ({} events), stopping collection [worker: {}]",
                    MAX_BATCH_SIZE, remote_addr
                );
                break;
            }
        }
        Ok::<(), tonic::Status>(())
    })
    .await;

    match collection_result {
        Ok(Ok(())) => {
            info!(
                "[OK] Telemetry batch collected: job={}, events_count={}, worker={}",
                first_job_id, events_count, remote_addr
            );
        }
        Ok(Err(e)) => {
            error!(
                "[ERROR] STREAM ERROR: Failed to collect telemetry from worker: {}",
                remote_addr
            );
            error!("   Error details: {:?}", e);
            error!("   Status code: {}, Message: {}", e.code(), e.message());
            return Err(e);
        }
        Err(_) => {
            error!(
                "[TIMEOUT]  TIMEOUT: Telemetry stream collection exceeded 30s limit [worker: {}]",
                remote_addr
            );
            error!(
                "   Collected {} events before timeout (partial batch)",
                events_count
            );
            warn!("   Possible causes: slow network, large payload, worker stalled");
            // Continue with partial batch rather than failing
        }
    }

    // Index batch to Elasticsearch with timeout
    if !batch.is_empty() {
        info!(
            "[UPLOAD]Indexing {} events to Elasticsearch [job: {}]",
            events_count, first_job_id
        );
        match timeout(Duration::from_secs(10), service.index_telemetry_batch(&batch)).await {
            Ok(Ok(())) => {
                info!(
                    "[OK] Successfully indexed {} telemetry events to Elasticsearch [job: {}]",
                    events_count, first_job_id
                );
            }
            Ok(Err(e)) => {
                error!("[ERROR] ELASTICSEARCH ERROR: Failed to index telemetry batch");
                error!(
                    "   Job: {}, Events: {}, Worker: {}",
                    first_job_id, events_count, remote_addr
                );
                error!("   Error details: {}", e);
                error!(
                    "   [WARN]  Telemetry received but NOT INDEXED (Elasticsearch may be down/unreachable)"
                );
                warn!(
                    "   Possible causes: Elasticsearch down, network issue, mapping conflict, disk full"
                );
                // Don't fail the RPC - telemetry was received, just not indexed
            }
            Err(_) => {
                error!(
                    "[TIMEOUT]  ELASTICSEARCH TIMEOUT: Indexing exceeded 10s limit [job: {}]",
                    first_job_id
                );
                error!("   Events: {}, Worker: {}", events_count, remote_addr);
                error!(
                    "   [WARN]  Telemetry received but NOT INDEXED (Elasticsearch is slow/unavailable)"
                );
                warn!(
                    "   Possible causes: Elasticsearch overloaded, slow disk I/O, large batch size"
                );
                // Don't fail the RPC - telemetry was received, just not indexed
            }
        }
    } else {
        warn!(
            "[WARN]  Telemetry stream closed with ZERO events [worker: {}]",
            remote_addr
        );
        warn!("   This may indicate: worker had no telemetry to send, or stream failed early");
    }

    Ok(Response::new(TelemetryAck {
        received: true,
        events_count,
    }))
}
