use edr_config::ControllerConfig;
use elasticsearch::{Elasticsearch, IndexParts, http::transport::Transport};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status, transport::Server};
use tracing::{error, info, warn};

pub mod edr {
    pub mod common {
        tonic::include_proto!("edr.common");
    }
    pub mod controller {
        tonic::include_proto!("edr.controller");
    }
    pub mod worker {
        tonic::include_proto!("edr.worker");
    }
}

use edr::common::{JobId, TelemetryAck, TelemetryData};
use edr::controller::{
    JobRequest, JobResponse, JobStatusRequest, JobStatusResponse, PingRequest, PingResponse,
    QueryRequest, QueryResponse, StatusAck, StatusReport, TriageRequest, TriageResponse,
    controller_server::{Controller, ControllerServer},
};

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Job {
    id: String,
    name: String,
    status: String,
    progress: i32,
    phase: String,
    logs: Vec<String>,
}

#[derive(Debug, Default)]
struct SchedulerState {
    jobs: HashMap<String, Job>,
    job_counter: u64,
}

#[derive(Debug, Clone)]
pub struct SchedulerService {
    state: Arc<Mutex<SchedulerState>>,
    es_client: Elasticsearch,
}

impl SchedulerService {
    pub fn new(es_client: Elasticsearch) -> Self {
        Self {
            state: Arc::new(Mutex::new(SchedulerState::default())),
            es_client,
        }
    }
}

#[tonic::async_trait]
impl Controller for SchedulerService {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        let req = request.into_inner();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        info!("Ping received: {}", req.message);

        Ok(Response::new(PingResponse {
            message: format!("pong: {}", req.message),
            timestamp,
            server: "controller/scheduler".to_string(),
        }))
    }

    async fn schedule_job(
        &self,
        request: Request<JobRequest>,
    ) -> Result<Response<JobResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.state.lock().await;

        state.job_counter += 1;
        let job_id = format!("job-{:06}", state.job_counter);

        let job = Job {
            id: job_id.clone(),
            name: req.name.clone(),
            status: "queued".to_string(),
            progress: 0,
            phase: "initializing".to_string(),
            logs: vec![format!("Job {} created", job_id)],
        };

        state.jobs.insert(job_id.clone(), job);

        info!("Scheduled job: {} ({})", job_id, req.name);

        Ok(Response::new(JobResponse {
            job_id: Some(JobId {
                value: job_id.clone(),
            }),
            accepted: true,
            message: format!("Job {} scheduled successfully", job_id),
            estimated_duration_seconds: 300,
        }))
    }

    async fn get_job_status(
        &self,
        request: Request<JobStatusRequest>,
    ) -> Result<Response<JobStatusResponse>, Status> {
        let req = request.into_inner();
        let state = self.state.lock().await;

        match state.jobs.get(&req.job_id) {
            Some(job) => Ok(Response::new(JobStatusResponse {
                job_id: job.id.clone(),
                status: job.status.clone(),
                progress_percent: job.progress,
                current_phase: job.phase.clone(),
                logs: job.logs.clone(),
            })),
            None => Err(Status::not_found(format!("Job {} not found", req.job_id))),
        }
    }

    async fn submit_triage(
        &self,
        request: Request<TriageRequest>,
    ) -> Result<Response<TriageResponse>, Status> {
        let req = request.into_inner();
        let triage_id = format!(
            "triage-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );

        info!(
            "Triage submitted for job {}: detected={}",
            req.job_id, req.detected
        );

        Ok(Response::new(TriageResponse {
            job_id: req.job_id,
            stored: true,
            triage_id,
        }))
    }

    async fn query_results(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        let _req = request.into_inner();

        Ok(Response::new(QueryResponse {
            results: vec![],
            total_count: 0,
        }))
    }

    async fn stream_telemetry(
        &self,
        request: Request<tonic::Streaming<TelemetryData>>,
    ) -> Result<Response<TelemetryAck>, Status> {
        let mut stream = request.into_inner();
        let mut events_count = 0;
        let mut first_job_id = String::new();
        let mut batch = Vec::new();

        info!("Telemetry stream opened");

        // Collect all events from stream
        while let Some(telemetry) = stream.message().await? {
            if events_count == 0 {
                first_job_id = telemetry.job_id.clone();
            }

            events_count += 1;
            batch.push(telemetry);
        }

        info!(
            "Telemetry batch received: job={}, events_count={}",
            first_job_id, events_count
        );

        // Index batch to Elasticsearch
        if !batch.is_empty() {
            if let Err(e) = self.index_telemetry_batch(&batch).await {
                error!("Failed to index telemetry batch: {}", e);
                // Don't fail the RPC - telemetry is logged even if indexing fails
            }
        }

        Ok(Response::new(TelemetryAck {
            received: true,
            events_count,
        }))
    }

    async fn report_status(
        &self,
        request: Request<StatusReport>,
    ) -> Result<Response<StatusAck>, Status> {
        let report = request.into_inner();

        // Format status display: [WORKER: ip] [PID: pid] [JOB: job_id] [STATUS: event_type] details
        let status_line = format!(
            "[WORKER: {} ({})] [PID: {}] [JOB: {}] [RUN: {}] [STATUS: {}] [ARTIFACT: {}] {}",
            report.worker_ip,
            report.worker_id,
            report.pid,
            report.job_id,
            report.run_id,
            report.event_type.to_uppercase(),
            report.artifact_name,
            report.details
        );

        // Use appropriate log level based on status type
        match report.event_type.as_str() {
            "error" | "timeout" | "stuck" | "crashed" => {
                tracing::warn!("{}", status_line);
            }
            _ => {
                info!("{}", status_line);
            }
        }

        // Store RunResult to Elasticsearch when final status received
        if matches!(report.event_type.as_str(), "success" | "error" | "timeout") {
            if let Err(e) = self.store_run_result(&report).await {
                error!("Failed to store run result: {}", e);
            }
        }

        Ok(Response::new(StatusAck { received: true }))
    }
}

impl SchedulerService {
    /// Index telemetry batch to Elasticsearch
    async fn index_telemetry_batch(
        &self,
        batch: &[TelemetryData],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use base64::Engine;
        use serde_json::json;

        let index_name = format!("telemetry-{}", chrono::Utc::now().format("%Y.%m.%d"));
        let mut indexed = 0;

        // Index events individually (simpler API usage)
        for event in batch {
            let doc = json!({
                "job_id": event.job_id,
                "event_type": event.event_type,
                "timestamp": event.timestamp,
                "payload": base64::engine::general_purpose::STANDARD.encode(&event.payload),
                "metadata": event.metadata,
                "indexed_at": chrono::Utc::now().to_rfc3339(),
            });

            let response = self
                .es_client
                .index(IndexParts::Index(&index_name))
                .body(doc)
                .send()
                .await;

            match response {
                Ok(resp) if resp.status_code().is_success() => {
                    indexed += 1;
                }
                Ok(resp) => {
                    warn!("Failed to index event: status {}", resp.status_code());
                }
                Err(e) => {
                    warn!("Failed to index event: {}", e);
                }
            }
        }

        info!(
            "Indexed {}/{} telemetry events to {}",
            indexed,
            batch.len(),
            index_name
        );

        Ok(())
    }

    /// Store run result to Elasticsearch
    async fn store_run_result(
        &self,
        report: &StatusReport,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let index_name = format!("runs-{}", chrono::Utc::now().format("%Y.%m"));

        let doc = json!({
            "run_id": report.run_id,
            "job_id": report.job_id,
            "worker_id": report.worker_id,
            "worker_ip": report.worker_ip,
            "artifact_name": report.artifact_name,
            "pid": report.pid,
            "status": report.event_type,
            "elapsed_seconds": report.elapsed_seconds,
            "telemetry_events_count": report.telemetry_events_count,
            "details": report.details,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        let response = self
            .es_client
            .index(IndexParts::IndexId(&index_name, &report.run_id))
            .body(doc)
            .send()
            .await?;

        if response.status_code().is_success() {
            info!(
                "Stored run result: {} (status: {})",
                report.run_id, report.event_type
            );
        } else {
            warn!(
                "Index run result returned non-success status: {}",
                response.status_code()
            );
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Load generated TOML config (auto-finds in standard locations)
    // Search order:
    //   1. AUTOMUTATE_CONTROLLER_CONFIG env var
    //   2. ~/automutate/config/controller.toml (WSL2 deployment default)
    //   3. automation/generated/controller.toml (generated from config.yaml)
    //   4. config/controller.toml (local development)
    //   5. automation/templates/controller.toml (template fallback)
    let config = ControllerConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load controller.toml: {}", e);
        eprintln!("Run 'automation/scripts/generate-configs.ps1' to create config files");
        eprintln!("Or set AUTOMUTATE_CONTROLLER_CONFIG environment variable");
        std::process::exit(1);
    });

    info!(
        "Loaded controller config successfully from {}",
        ControllerConfig::find_config_path()
    );
    info!("Bind address: {}", config.server.bind_address);
    info!("Elasticsearch: {}", config.elasticsearch.url);
    info!(
        "Triage model: {} (threshold: {})",
        config.triage.model_type, config.triage.confidence_threshold
    );

    // Create Elasticsearch client
    let es_transport = Transport::single_node(&config.elasticsearch.url)?;
    let es_client = Elasticsearch::new(es_transport);

    info!(
        "Elasticsearch client initialized: {}",
        config.elasticsearch.url
    );

    let addr = config.server.bind_address.parse()?;
    let scheduler = SchedulerService::new(es_client);

    info!("Controller/Scheduler starting...");

    // gRPC reflection for grpcurl
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(tonic::include_file_descriptor_set!("edr_descriptor"))
        .build_v1()?;

    Server::builder()
        .add_service(ControllerServer::new(scheduler))
        .add_service(reflection_service)
        .serve(addr)
        .await?;

    Ok(())
}
