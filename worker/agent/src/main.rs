use edr_config::WorkerConfig;
use std::time::{Duration, Instant, SystemTime};
use tonic::{Request, Response, Status, transport::Server};
use tracing::{error, info, warn};

mod execution;
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

use edr::common::ArtifactId;
use edr::worker::{
    BuildRequest, BuildResponse, HealthRequest, HealthResponse, PingRequest, PingResponse,
    SampleRequest, SampleResponse,
    worker_agent_server::{WorkerAgent, WorkerAgentServer},
};

#[derive(Debug, Clone)]
pub struct WorkerAgentService {
    worker_id: String,
    config: WorkerConfig,
}

impl WorkerAgentService {
    pub fn new(worker_id: String, config: WorkerConfig) -> Self {
        Self { worker_id, config }
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
        let run_id = uuid::Uuid::new_v4().to_string();
        let job_id = req.job_id.clone();

        info!("Starting sample execution: job_id={}, artifact={}", job_id, req.artifact_path);

        // Check if RedEDR is enabled
        if !self.config.telemetry.rededr.enabled {
            return Err(Status::failed_precondition("RedEDR telemetry is disabled in config"));
        }

        // 1. Create RedEDR collector
        let rededr_collector = telemetry::collectors::rededr::RedEdrCollector::new(
            telemetry::collectors::rededr::RedEdrCollectorConfig {
                base_url: self.config.telemetry.rededr.base_url.clone(),
                flush_interval_ms: 1000,
                job_id: job_id.clone(),
                run_id: run_id.clone(),
            }
        );

        // 2. Extract artifact filename for tracing
        let artifact_name = extract_filename(&req.artifact_path);

        // 3. Start RedEDR tracing
        rededr_collector.start_trace(vec![artifact_name.clone()]).await
            .map_err(|e| Status::internal(format!("Failed to start RedEDR tracing: {}", e)))?;

        info!("RedEDR tracing started for artifact: {}", artifact_name);

        // 4. Start process
        let mut child = tokio::process::Command::new(&req.artifact_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Status::internal(format!("Failed to spawn process: {}", e)))?;

        let pid = child.id().ok_or_else(|| Status::internal("Failed to get PID"))?;

        info!("Artifact process spawned: pid={}", pid);

        // 5. Start monitoring task
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

        let monitor = execution::monitor::ExecutionMonitor::new(
            run_id.clone(),
            job_id.clone(),
            self.worker_id.clone(),
            self.config.worker.ip_address.clone(),
            artifact_name.clone(),
            pid,
            self.config.telemetry.rededr.base_url.clone(),
            self.config.controller.controller_address.clone(),
        );

        let monitor_handle = tokio::spawn(async move {
            monitor.start(stop_rx, event_tx).await;
        });

        // Spawn task to consume monitor events (log them)
        let event_consumer = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event.event_type.as_str() {
                    "started" => info!("Monitor: {}", event.details),
                    "heartbeat" => info!("Monitor: {}", event.details),
                    "stuck" => warn!("Monitor: {}", event.details),
                    "terminated" => info!("Monitor: {}", event.details),
                    "completed" => info!("Monitor: {}", event.details),
                    _ => info!("Monitor: {} - {}", event.event_type, event.details),
                }
            }
        });

        // 6. Wait for process completion or timeout
        let timeout_duration = Duration::from_secs(req.timeout_seconds as u64);
        let start_time = Instant::now();

        let exit_result = tokio::time::timeout(timeout_duration, child.wait()).await;

        let (exit_code, timed_out) = match exit_result {
            Ok(Ok(status)) => {
                let code = status.code().unwrap_or(-1);
                info!("Process exited with code: {}", code);
                (code, false)
            }
            Ok(Err(e)) => {
                error!("Failed to wait for process: {}", e);
                (-1, false)
            }
            Err(_) => {
                warn!("Process timed out after {}s, attempting to kill", req.timeout_seconds);
                let _ = child.kill().await;
                (-1, true)
            }
        };

        let elapsed = start_time.elapsed();

        // 7. Stop monitoring
        stop_tx.send(true).ok();
        monitor_handle.await.ok();
        event_consumer.abort(); // Stop event consumer

        info!("Execution completed in {:.2}s", elapsed.as_secs_f64());

        // 8. Collect full telemetry batch
        info!("Collecting telemetry events from RedEDR...");
        let telemetry_events = rededr_collector.collect_all(&job_id).await
            .map_err(|e| Status::internal(format!("Failed to collect telemetry: {}", e)))?;

        info!("Collected {} telemetry events", telemetry_events.len());

        // 9. Reset RedEDR for next run
        rededr_collector.reset().await.ok();

        // 10. Prepare output
        let output = if timed_out {
            format!("Execution timed out after {}s", req.timeout_seconds)
        } else {
            format!("Execution completed in {:.2}s", elapsed.as_secs_f64())
        };

        // 11. Return response
        Ok(Response::new(SampleResponse {
            job_id,
            success: !timed_out && exit_code == 0,
            exit_code,
            output,
            telemetry_ids: vec![run_id],
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

// NOTE: Streaming telemetry removed - now using batch collection at execution completion
// Telemetry is collected once after artifact execution and returned with RunResult

/// Extract filename from path (cross-platform)
fn extract_filename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Load generated TOML config (auto-finds in standard locations)
    // Search order:
    //   1. AUTOMUTATE_WORKER_CONFIG env var (highest priority)
    //   2. C:\AutoMutate\worker.toml (VM deployment standard location)
    //   3. Auto-detect by hostname (e.g., automation/generated/win10-worker-01.toml)
    //   4. config/worker.toml (local development)
    //   5. automation/generated/win10-worker-01.toml (fallback)
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
        eprintln!("  5. automation/generated/win10-worker-01.toml");
        eprintln!("");
        eprintln!("Solutions:");
        eprintln!("  - Run: .\\automation\\scripts\\generate-configs.ps1");
        eprintln!("  - Deploy: Copy <hostname>.toml to C:\\AutoMutate\\worker.toml");
        eprintln!("  - Or set: $env:AUTOMUTATE_WORKER_CONFIG=\"automation\\generated\\<hostname>.toml\"");
        std::process::exit(1);
    });

    let worker_id = config.worker.worker_id.clone();

    // Ensure controller address has http:// scheme for tonic
    let controller_addr = {
        let addr = config.controller.controller_address.clone();
        if addr.starts_with("http://") || addr.starts_with("https://") {
            addr
        } else {
            format!("http://{}", addr)
        }
    };

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

    let agent = WorkerAgentService::new(worker_id.clone(), config.clone());

    info!("Worker Agent {} starting on {}", worker_id, addr);
    info!("Telemetry mode: Batch collection (no streaming)");

    if config.telemetry.rededr.enabled {
        info!("RedEDR collector available: {}", config.telemetry.rededr.base_url);
        info!("RedEDR telemetry will be collected after each execution completes");
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
