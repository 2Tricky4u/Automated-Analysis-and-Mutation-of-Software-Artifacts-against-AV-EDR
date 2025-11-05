use edr_config::WorkerConfig;
use std::time::SystemTime;
use tonic::{Request, Response, Status, transport::Server};
use tracing::{error, info};

mod telemetry;

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

use edr::common::{ArtifactId, TelemetryData};
use edr::controller::controller_client::ControllerClient;
use edr::worker::{
    BuildRequest, BuildResponse, HealthRequest, HealthResponse, PingRequest, PingResponse,
    SampleRequest, SampleResponse,
    worker_agent_server::{WorkerAgent, WorkerAgentServer},
};

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
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        let req = request.into_inner();
        let timestamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        info!("Ping received: {}", req.message);

        Ok(Response::new(PingResponse {
            message: format!("pong: {}", req.message),
            timestamp,
            server: format!("worker-agent/{}", self.worker_id),
        }))
    }

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
        match ControllerClient::connect(controller_addr.clone()).await {
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
                error!(
                    "Failed to connect to controller {}: {}",
                    controller_addr, err
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                // Retry connection
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Load generated TOML config (auto-finds in standard locations)
    // Search order:
    //   1. AUTOMUTATE_WORKER_CONFIG env var (highest priority)
    //   2. C:\AutoMutate\worker.toml (VM deployment standard location)
    //   3. Auto-detect by hostname (e.g., automation/generated/win10-worker-00.toml)
    //   4. config/worker.toml (local development)
    //   5. automation/generated/win10-worker-00.toml (fallback)
    let config = WorkerConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load worker config: {}", e);
        eprintln!("");
        eprintln!("Hostname: {}", std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN".to_string()));
        eprintln!("");
        eprintln!("Config search order:");
        eprintln!("  1. AUTOMUTATE_WORKER_CONFIG env var");
        eprintln!("  2. C:\\AutoMutate\\worker.toml");
        eprintln!("  3. automation/generated/<hostname>.toml (auto-detect)");
        eprintln!("  4. config/worker.toml");
        eprintln!("  5. automation/generated/win10-worker-00.toml");
        eprintln!("");
        eprintln!("Solutions:");
        eprintln!("  - Run: .\\automation\\scripts\\generate-configs.ps1");
        eprintln!("  - Deploy: Copy <hostname>.toml to C:\\AutoMutate\\worker.toml");
        eprintln!("  - Or set: $env:AUTOMUTATE_WORKER_CONFIG=\"automation\\generated\\<hostname>.toml\"");
        std::process::exit(1);
    });

    let worker_id = config.worker.worker_id.clone();
    let controller_addr = config.controller.controller_address.clone();

    // Worker listen port can be overridden via env var
    let worker_port = std::env::var("WORKER_PORT")
        .unwrap_or_else(|_| "50052".to_string())
        .parse::<u16>()
        .unwrap_or(50052);
    let addr = format!("0.0.0.0:{}", worker_port).parse()?;

    info!("Worker configuration loaded successfully at {}", WorkerConfig::find_config_path());
    info!("Worker ID: {}", worker_id);
    info!("Worker IP: {}", config.worker.ip_address);
    info!("OS Version: {}", config.worker.os_version);
    info!("Controller: {}", controller_addr);
    info!("Sandbox enabled: {}", config.harness.sandbox_enabled);
    info!("ETW enabled: {}", config.telemetry.etw.enabled);

    let agent = WorkerAgentService::new(worker_id.clone());

    info!("Worker Agent {} starting on {}", worker_id, addr);

    // Create channel for telemetry events (increased buffer for RedEDR)
    let buffer_size = config.telemetry.stream_buffer_size;
    let (tx, rx) = tokio::sync::mpsc::channel::<TelemetryData>(buffer_size);

    // Spawn background task to push telemetry to controller
    let controller_addr_clone = controller_addr.clone();
    tokio::spawn(push_telemetry_to_controller(controller_addr_clone, rx));

    // Spawn RedEDR collector if enabled
    if config.telemetry.rededr.enabled {
        info!("RedEDR collector enabled: {}", config.telemetry.rededr.base_url);

        let rededr_config = telemetry::collectors::rededr::RedEdrCollectorConfig {
            base_url: config.telemetry.rededr.base_url.clone(),
            flush_interval_ms: config.telemetry.flush_interval_ms,
            job_id: "global".to_string(), // TODO: Get from active job
            run_id: "global".to_string(),  // TODO: Get from active run
        };

        let collector = telemetry::collectors::rededr::RedEdrCollector::new(rededr_config);
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            if let Err(e) = collector.start(tx_clone).await {
                error!("RedEDR collector error: {}", e);
            }
        });
    } else {
        info!("RedEDR collector disabled in config");
    }

    // gRPC reflection for grpcurl
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(tonic::include_file_descriptor_set!("edr_descriptor"))
        .build_v1()?;

    Server::builder()
        .add_service(WorkerAgentServer::new(agent))
        .add_service(reflection_service)
        .serve(addr)
        .await?;

    Ok(())
}
