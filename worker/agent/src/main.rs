//! Worker agent entry point.
//!
//! Loads configuration, detects host capabilities (RedEDR, Defender, MDE, Cortex),
//! and starts the gRPC server that accepts controller commands.

use std::fs::OpenOptions;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use worker_agent::{WorkerAgentService, automutate, capabilities};

use automutate::worker::worker_agent_server::WorkerAgentServer;
use automutate_config::WorkerConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Disable QuickEdit Mode on Windows console to prevent process freeze
    // when the user accidentally clicks inside the cmd window.
    disable_quick_edit();

    // Load generated TOML config
    let config = WorkerConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load worker config: {}", e);
        eprintln!();
        eprintln!(
            "Hostname: {}",
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN".to_string())
        );
        eprintln!();
        eprintln!("Solutions:");
        eprintln!("  - Run: .\\automation\\scripts\\generate-configs.ps1");
        eprintln!("  - Deploy: Copy <hostname>.toml to C:\\AutoMutate\\worker.toml");
        std::process::exit(1);
    });

    // Initialize logging with per-crate suppression support
    let env_filter =
        EnvFilter::try_new(config.logging.to_env_filter_string()).unwrap_or_else(|e| {
            eprintln!(
                "Invalid log filter '{}': {}, defaulting to INFO",
                config.logging.level, e
            );
            EnvFilter::new("info")
        });

    // File layer — write to worker.log (truncated on each start)
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("worker.log")?;

    let console_layer = tracing_subscriber::fmt::layer().with_ansi(true);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file)
        .with_ansi(true);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
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
    let mut capabilities = capabilities::detect_capabilities().await?;

    // Merge extra_capabilities from config (e.g. "dryrun" for clean-VM workers)
    for cap in &config.worker.extra_capabilities {
        if !capabilities
            .capabilities
            .iter()
            .any(|c| c.eq_ignore_ascii_case(cap))
        {
            capabilities.capabilities.push(cap.clone());
        }
    }

    info!("Capabilities: {:?}", capabilities.capabilities);
    info!("Tools: {:?}", capabilities.tools);

    // stream closes automatically on exit
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        info!("Received Ctrl+C, shutting down...");
        std::process::exit(0);
    });

    // Create worker agent service
    let worker_service = WorkerAgentService::new(worker_id.clone(), config.clone(), capabilities);

    info!("Starting worker agent gRPC server...");

    // Start gRPC server
    Server::builder()
        .add_service(WorkerAgentServer::new(worker_service))
        .serve(addr)
        .await?;

    Ok(())
}

/// Disable Windows Console QuickEdit Mode.
///
/// When QuickEdit is enabled (default), clicking inside the console window
/// selects text and **blocks** all stdout/stderr writes — freezing the process
/// until the user presses Enter/Escape. This is the classic "worker stuck until
/// I press Enter" symptom.
#[cfg(windows)]
fn disable_quick_edit() {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_EXTENDED_FLAGS, ENABLE_QUICK_EDIT_MODE, GetConsoleMode, GetStdHandle,
        STD_INPUT_HANDLE, SetConsoleMode,
    };

    unsafe {
        let handle = match GetStdHandle(STD_INPUT_HANDLE) {
            Ok(h) => h,
            Err(_) => return,
        };
        let mut mode = CONSOLE_MODE::default();
        if GetConsoleMode(handle, &mut mode).is_err() {
            return;
        }
        // Clear QUICK_EDIT, set EXTENDED_FLAGS (required for the change to take effect)
        mode &= !ENABLE_QUICK_EDIT_MODE;
        mode |= ENABLE_EXTENDED_FLAGS;
        let _ = SetConsoleMode(handle, mode);
    }
}

#[cfg(not(windows))]
fn disable_quick_edit() {}
