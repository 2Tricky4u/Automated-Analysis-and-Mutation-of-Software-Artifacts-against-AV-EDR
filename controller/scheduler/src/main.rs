use edr_config::ControllerConfig;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status, transport::Server};
use tracing::info;

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
    QueryRequest, QueryResponse, TriageRequest, TriageResponse,
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

#[derive(Debug)]
pub struct SchedulerService {
    state: Arc<Mutex<SchedulerState>>,
}

impl Default for SchedulerService {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerService {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SchedulerState::default())),
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

        info!("Telemetry stream opened");

        // Process incoming telemetry data
        while let Some(telemetry) = stream.message().await? {
            events_count += 1;
            info!(
                "Telemetry event #{}: job={}, type={}, ts={}",
                events_count, telemetry.job_id, telemetry.event_type, telemetry.timestamp
            );

            // TODO: Forward to Elasticsearch, store in buffer, etc.
            // For now, just log and count
        }

        info!("Telemetry stream closed, received {} events", events_count);

        Ok(Response::new(TelemetryAck {
            received: true,
            events_count,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Load generated TOML config (auto-finds in standard locations)
    // Search order:
    //   1. AUTOMUTATE_CONTROLLER_CONFIG env var
    //   2. ~/automutate/config/controller.toml (WSL2 deployment default)
    //   3. config/controller.toml (local development)
    //   4. automation/templates/controller.toml (template fallback)
    let config = ControllerConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load controller.toml: {}", e);
        eprintln!("Run 'automation/scripts/generate-configs.ps1' to create config files");
        eprintln!("Or set AUTOMUTATE_CONTROLLER_CONFIG environment variable");
        std::process::exit(1);
    });

    info!("Loaded controller config successfully");
    info!("Bind address: {}", config.server.bind_address);
    info!("Elasticsearch: {}", config.elasticsearch.url);
    info!(
        "Triage model: {} (threshold: {})",
        config.triage.model_type, config.triage.confidence_threshold
    );

    let addr = config.server.bind_address.parse()?;
    let scheduler = SchedulerService::new();

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
