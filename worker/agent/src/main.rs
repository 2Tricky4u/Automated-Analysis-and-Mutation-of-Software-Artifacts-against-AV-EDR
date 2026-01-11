use edr_config::WorkerConfig;
use std::sync::Arc;
use sysinfo::System;
use tokio::sync::Mutex;
use tonic::transport::Server;
use tracing::info;

mod capabilities;
mod execution;
mod service;  // NEW: Service handlers module
mod stream_handler;
mod telemetry;

pub mod automutate {
    pub mod common {
        tonic::include_proto!("automutate.common");
    }
    pub mod controller {
        tonic::include_proto!("automutate.controller");
    }
    pub mod worker {
        tonic::include_proto!("automutate.worker");
    }
}

use automutate::worker::worker_agent_server::WorkerAgentServer;
use execution::guards::ExecutionState;

// ============================================================================
// Worker Agent Service
// ============================================================================

#[derive(Clone)]
pub struct WorkerAgentService {
    pub(crate) worker_id: String,
    pub(crate) config: WorkerConfig,
    pub(crate) system_info: Arc<Mutex<System>>,
    /// Single execution lock needed for rededr
    /// This ensures clean telemetry collection with no cross-contamination
    pub(crate) execution_lock: Arc<Mutex<ExecutionState>>,
    /// StreamHandler for bidirectional communication (set when establish_stream is called)
    pub(crate) stream_handler: Arc<tokio::sync::RwLock<Option<Arc<stream_handler::StreamHandler>>>>,
}

impl WorkerAgentService {
    pub fn new(worker_id: String, config: WorkerConfig) -> Self {
        Self {
            worker_id,
            config,
            system_info: Arc::new(Mutex::new(System::new_all())),
            execution_lock: Arc::new(Mutex::new(ExecutionState {
                busy: false,
                current_job_id: None,
                current_artifact: None,
            })),
            stream_handler: Arc::new(Default::default()),
        }
    }

    /// Get current execution state (for health check)
    pub async fn get_execution_state(&self) -> ExecutionState {
        self.execution_lock.lock().await.clone()
    }

    pub fn truncate_middle_output(stdout_output: &String) -> String {
        let formatted = if stdout_output.len() > 1000 {
            // Show first 400 chars and last 400 chars, truncate middle
            let first_part = &stdout_output[..400];
            let last_part = &stdout_output[stdout_output.len() - 400..];
            format!(
                "{}\n\n... ({} bytes truncated) ...\n\n{}",
                first_part,
                stdout_output.len() - 800,
                last_part
            )
        } else {
            stdout_output.clone()
        };
        formatted
    }
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
        eprintln!(
            "Hostname: {}",
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN".to_string())
        );
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
        eprintln!(
            "  - Or set: $env:AUTOMUTATE_WORKER_CONFIG=\"automation\\generated\\<hostname>.toml\""
        );
        std::process::exit(1);
    });

    let worker_id = config.worker.worker_id.clone();

    // Worker listen port from config or env var
    let worker_port = std::env::var("WORKER_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(config.worker.listen_port);
    let addr = format!("0.0.0.0:{}", worker_port).parse()?;

    info!(
        "Worker configuration loaded successfully at {}",
        WorkerConfig::find_config_path()
    );
    info!("Worker ID: {}", worker_id);
    info!("Worker IP: {}", config.worker.ip_address);
    info!("OS Version: {}", config.worker.os_version);
    info!("Worker listening on: {}", addr);
    info!("Sandbox enabled: {}", config.harness.sandbox_enabled);
    info!("ETW enabled: {}", config.telemetry.etw.enabled);

    // === Detect capabilities ===
    info!("Detecting worker capabilities...");
    let capabilities = capabilities::detect_capabilities().await?;
    info!("Capabilities: {:?}", capabilities.capabilities);
    info!("Tools: {:?}", capabilities.tools);

    // Setup graceful shutdown handler, stream closes automatically on exit
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        info!("Received Ctrl+C, shutting down gracefully...");
        std::process::exit(0);
    });

    // Create worker agent service
    let worker_service = WorkerAgentService::new(worker_id.clone(), config.clone());

    info!("Starting worker agent gRPC server...");

    // Start gRPC server
    Server::builder()
        .add_service(WorkerAgentServer::new(worker_service))
        .serve(addr)
        .await?;

    Ok(())
}
