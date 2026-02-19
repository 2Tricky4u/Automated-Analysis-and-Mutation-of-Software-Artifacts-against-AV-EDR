//! Scheduler main - JobWorker architecture
//!
//! Flow:
//! 1. Load config, init logging, create ES client
//! 2. Create EsStorage + TargetManager + Orchestrator
//! 3. Establish streams with targets (spawns VMExecutors)
//! 4. gRPC server accepts job submissions -> Orchestrator
//! 5. Orchestrator spawns JobWorkers and handles all events

use edr_config::ControllerConfig;
use elasticsearch::Elasticsearch;
use elasticsearch::http::transport::Transport;
use std::fs::OpenOptions;
use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::transport::Server;
use tracing::{debug, info, warn};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod dispatch;
mod storage;
mod triage;
mod vm;

use api::SchedulerService;
use dispatch::{JobControlCommand, JobSession, Orchestrator, RunPool};
use storage::EsStorage;
use vm::{TargetEvent, TargetManager};

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

use crate::automutate::controller::controller_server::ControllerServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load config
    let config = ControllerConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load controller.toml: {}", e);
        eprintln!("Run 'automation/scripts/generate-configs.ps1' to create config files");
        std::process::exit(1);
    });

    // Initialize logging
    let log_level = match config.logging.level.to_uppercase().as_str() {
        "TRACE" => tracing::Level::TRACE,
        "DEBUG" => tracing::Level::DEBUG,
        "INFO" => tracing::Level::INFO,
        "WARN" => tracing::Level::WARN,
        "ERROR" => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };

    let level_filter = LevelFilter::from_level(log_level);

    // Console layer
    let console_layer = tracing_subscriber::fmt::layer()
        .with_ansi(true)
        .with_filter(level_filter);

    // File layer
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("controller.log")?;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file)
        .with_ansi(true)
        .with_filter(level_filter);

    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .init();

    info!("Controller starting");
    info!("Config: {}", ControllerConfig::find_config_path());

    // Create Elasticsearch client and storage
    let es_transport = Transport::single_node(&config.elasticsearch.url)?;
    let es_client = Elasticsearch::new(es_transport);
    let storage = Arc::new(EsStorage::new(es_client));
    info!("Elasticsearch: {}", config.elasticsearch.url);

    // Bootstrap index templates (log warnings on failure, don't crash)
    if let Err(e) = storage.ensure_templates().await {
        warn!("Failed to ensure ES index templates: {}", e);
    }

    // Create channels
    let (events_tx, events_rx) = mpsc::channel::<TargetEvent>(4096);
    let (job_tx, job_rx) = mpsc::channel::<JobSession>(128);
    let (job_control_tx, job_control_rx) = mpsc::channel::<JobControlCommand>(64);

    // Create shared run pool
    let run_pool = Arc::new(RunPool::new());

    // Create TargetManager
    let targets = Arc::new(TargetManager::new(
        config.scheduler.run_timeout_secs,
        events_tx,
        Arc::clone(&run_pool),
    ));

    // Spawn Orchestrator (handles jobs, VM lifecycle, and telemetry)
    debug!("Orchestrator starting...");
    let orchestrator = Orchestrator::new(
        events_rx,
        job_rx,
        job_control_rx,
        Arc::clone(&run_pool),
        Arc::clone(&targets),
        Arc::clone(&storage),
    );
    tokio::spawn(orchestrator.run());

    // Discover and register targets
    targets.discover_and_register_targets().await;
    info!("Registered {} targets", targets.count());

    // Query target info
    info!("Querying target metadata...");
    let target_infos = targets.query_all_info().await;
    for (target_id, info) in target_infos {
        debug!(
            "Target {} - OS: {}, Caps: {:?}",
            target_id, info.os_version, info.capabilities
        );
    }

    // Establish streams (spawns VMExecutors)
    info!("Establishing streams with targets...");
    let stream_results = targets.establish_all_streams().await;

    let mut ok_count = 0;
    let mut fail_count = 0;
    for (target_id, result) in stream_results {
        match result {
            Ok(()) => {
                debug!("Stream established: {}", target_id);
                ok_count += 1;
            }
            Err(e) => {
                warn!("Stream failed for {}: {}", target_id, e);
                fail_count += 1;
            }
        }
    }
    info!("Streams: {} ok, {} failed", ok_count, fail_count);

    // Create gRPC service
    let service = SchedulerService::new(
        storage,
        job_tx,
        job_control_tx,
        Arc::clone(&targets),
        Arc::clone(&run_pool),
    );

    // gRPC reflection
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(tonic::include_file_descriptor_set!(
            "automutate_descriptor"
        ))
        .build_v1()?;

    // Start server
    let addr = config.server.bind_address.parse()?;
    info!("gRPC server listening on {}", addr);

    Server::builder()
        .add_service(ControllerServer::new(service))
        .add_service(reflection)
        .serve(addr)
        .await?;

    Ok(())
}
