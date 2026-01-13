use tonic::transport::Server;
use tracing::info;

// Use modules from lib
use worker_agent::{WorkerAgentService, automutate, capabilities};

use automutate::worker::worker_agent_server::WorkerAgentServer;
use edr_config::WorkerConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Initialize logging with level from config
    let log_level = match config.logging.level.to_uppercase().as_str() {
        "TRACE" => tracing::Level::TRACE,
        "DEBUG" => tracing::Level::DEBUG,
        "INFO" => tracing::Level::INFO,
        "WARN" => tracing::Level::WARN,
        "ERROR" => tracing::Level::ERROR,
        _ => {
            eprintln!("Invalid log level '{}', defaulting to INFO", config.logging.level);
            tracing::Level::INFO
        }
    };

    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .init();

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
