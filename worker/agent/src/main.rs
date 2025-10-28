use std::time::SystemTime;
use tonic::{transport::Server, Request, Response, Status};
use tracing::{error, info};

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

use edr::worker::{
    worker_agent_server::{WorkerAgent, WorkerAgentServer},
    BuildRequest, BuildResponse, HealthRequest, HealthResponse, SampleRequest, SampleResponse,
};
use edr::controller::controller_client::ControllerClient;
use edr::common::{TelemetryData, ArtifactId};

#[derive(Debug, Default)]
pub struct WorkerAgentService {
    worker_id: String,
}

impl WorkerAgentService {
    pub fn new(worker_id: String) -> Self {
        Self { worker_id }
    }
}

#[tonic::async_trait]
impl WorkerAgent for WorkerAgentService {
    async fn execute_build(
        &self,
        request: Request<BuildRequest>,
    ) -> Result<Response<BuildResponse>, Status> {
        let req = request.into_inner();
        info!("Building job: {} (language: {})", req.job_id, req.language);

        let start_time = SystemTime::now();

        // Simulate build process
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let build_time = SystemTime::now()
            .duration_since(start_time)
            .unwrap()
            .as_millis() as i64;

        let artifact_path = format!("/artifacts/{}.exe", req.job_id);
        let artifact_hash = format!("sha256:{:064x}", 0u128); // Placeholder hash

        Ok(Response::new(BuildResponse {
            job_id: req.job_id,
            success: true,
            artifact_path,
            artifact_id: Some(ArtifactId {
                sha256: artifact_hash,
            }),
            error_message: String::new(),
            build_time_ms: build_time,
        }))
    }

    async fn run_sample(
        &self,
        request: Request<SampleRequest>,
    ) -> Result<Response<SampleResponse>, Status> {
        let req = request.into_inner();
        info!("Running sample: {} (ETW: {})", req.job_id, req.enable_etw);

        // Simulate sample execution
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        Ok(Response::new(SampleResponse {
            job_id: req.job_id,
            success: true,
            exit_code: 0,
            output: "Sample executed successfully".to_string(),
            telemetry_ids: vec!["telemetry-001".to_string()],
        }))
    }

    async fn health_check(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let _req = request.into_inner();

        Ok(Response::new(HealthResponse {
            worker_id: self.worker_id.clone(),
            healthy: true,
            cpu_percent: 25,
            memory_percent: 40,
            active_jobs: 0,
        }))
    }
}

/// Background task: push telemetry to controller (client-streaming RPC)
async fn push_telemetry_to_controller(
    controller_addr: String,
    rx: tokio::sync::mpsc::Receiver<TelemetryData>,
) {
    use tokio_stream::wrappers::ReceiverStream;

    // Retry connection to controller
    loop {
        match ControllerClient::connect(format!("http://{}", controller_addr)).await {
            Ok(mut client) => {
                info!("Connected to controller at {}", controller_addr);

                // Create stream from receiver and send to controller
                let outbound = ReceiverStream::new(rx);

                match client.stream_telemetry(outbound).await {
                    Ok(resp) => {
                        let ack = resp.into_inner();
                        info!(
                            "Telemetry stream completed: received={}, events={}",
                            ack.received, ack.events_count
                        );
                        // Stream successfully completed, exit
                        break;
                    }
                    Err(err) => {
                        error!("Telemetry stream error: {}", err);
                        // Stream consumed, can't retry with same receiver, exit
                        break;
                    }
                }
            }
            Err(err) => {
                error!("Failed to connect to controller {}: {}", controller_addr, err);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                // Retry connection
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Env:
    //   WORKER_ID        (default: "worker-01")
    //   CONTROLLER_ADDR  (default: "10.200.200.1:50051")
    //   LISTEN_ADDR      (default: "0.0.0.0:50052")
    let worker_id = std::env::var("WORKER_ID").unwrap_or_else(|_| "worker-01".to_string());
    let controller_addr =
        std::env::var("CONTROLLER_ADDR").unwrap_or_else(|_| "10.200.200.1:50051".to_string());
    let addr = "0.0.0.0:50052".parse()?;

    let agent = WorkerAgentService::new(worker_id.clone());

    info!("Worker Agent {} starting on {}", worker_id, addr);
    info!("Controller address: {}", controller_addr);

    // Create channel for telemetry events
    let (_tx, rx) = tokio::sync::mpsc::channel::<TelemetryData>(1000);

    // Spawn background task to push telemetry to controller
    tokio::spawn(push_telemetry_to_controller(controller_addr, rx));

    // TODO: Pass _tx to agent service so it can send telemetry events
    // For now, agent just handles RPC calls

    Server::builder()
        .add_service(WorkerAgentServer::new(agent))
        .serve(addr)
        .await?;

    Ok(())
}
