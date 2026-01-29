//! Scheduler main - Dispatch-based architecture
//!
//! Flow:
//! 1. Load config, init logging, create ES client
//! 2. Create TargetManager + Orchestrator
//! 3. Establish streams with targets (spawns Workers)
//! 4. gRPC server accepts job submissions -> Orchestrator
//! 5. Orchestrator routes jobs to compatible Workers

use edr_config::ControllerConfig;
use elasticsearch::http::transport::Transport;
use elasticsearch::Elasticsearch;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::transport::Server;
use std::fs::OpenOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::Layer;
use tracing::{debug, error, info, warn};

mod dispatch;
mod service;
mod target_manager;

use dispatch::{JobSession, Orchestrator, OrchestratorEvent};
use service::SchedulerService;
use target_manager::{TargetConfig, TargetEvent, TargetManager};

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

use crate::automutate::common::worker_message;
use crate::automutate::controller::controller_server::ControllerServer;

// ============================================================================
// Target Discovery (from scheduler_core_old.rs)
// ============================================================================

/// Target configuration from individual worker TOML files
#[derive(Debug, Clone, serde::Deserialize)]
struct TargetTomlConfig {
    worker: TargetInfo,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct TargetInfo {
    worker_id: String,
    ip_address: String,
}

/// Discover targets from automation/generated/win*-worker-*.toml files
async fn discover_and_register_targets(targets: &TargetManager) {
    use std::path::Path;
    use std::collections::HashMap as StdHashMap;

    let generated_dir = Path::new("automation/generated");

    if !generated_dir.exists() {
        warn!("automation/generated directory not found, no targets registered");
        warn!("Run 'automation/scripts/generate-configs.ps1' to create target configs");
        return;
    }

    let entries = match std::fs::read_dir(generated_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("Failed to read automation/generated: {}", e);
            return;
        }
    };

    let mut target_count = 0;
    let mut duplicate_count = 0;
    let mut registered_ips: StdHashMap<String, (String, String)> = StdHashMap::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Match pattern: win*-worker-*.toml
        if filename.starts_with("win") && filename.contains("-worker-") && filename.ends_with(".toml") {
            match load_target_config(&path) {
                Ok((target_id, address)) => {
                    let ip = address.split(':').next().unwrap_or(&address).to_string();

                    // Check for duplicate IP
                    if let Some((existing_id, existing_file)) = registered_ips.get(&ip) {
                        warn!(
                            "Duplicate IP: {} in '{}' (target: {}) - already from '{}' (target: {}). Skipping.",
                            ip, filename, target_id, existing_file, existing_id
                        );
                        duplicate_count += 1;
                        continue;
                    }

                    // Register target
                    if let Err(e) = targets.register(TargetConfig {
                        id: target_id.clone(),
                        address: address.clone(),
                        enabled: true,
                    }) {
                        warn!("Failed to register target {}: {}", target_id, e);
                        continue;
                    }

                    info!("  Registered target: {} at {}", target_id, address);
                    registered_ips.insert(ip, (target_id.clone(), filename.to_string()));
                    target_count += 1;
                }
                Err(e) => {
                    warn!("Failed to load target config {}: {}", filename, e);
                }
            }
        }
    }

    if duplicate_count > 0 {
        warn!("{} duplicate target config(s) were skipped (same IP)", duplicate_count);
    }

    if target_count == 0 {
        warn!("No targets registered! Scheduler will not be able to execute jobs.");
        warn!("Create target configs in automation/generated/ (e.g., win10-worker-01.toml)");
    } else {
        info!("Target pool initialized with {} unique targets", target_count);
    }
}

/// Load target configuration from individual worker TOML file
fn load_target_config(path: &std::path::Path) -> anyhow::Result<(String, String)> {
    let content = std::fs::read_to_string(path)?;
    let config: TargetTomlConfig = toml::from_str(&content)?;

    // Target gRPC address is IP + port 50052 (standard worker port)
    let address = format!("{}:50052", config.worker.ip_address);

    Ok((config.worker.worker_id, address))
}

// ============================================================================
// Helpers
// ============================================================================

fn detect_controller_ip() -> Option<String> {
    use std::net::IpAddr;

    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                if let IpAddr::V4(ipv4) = local_addr.ip() {
                    if !ipv4.is_loopback() {
                        debug!("Detected controller IP: {}", ipv4);
                        return Some(ipv4.to_string());
                    }
                }
            }
        }
    }
    warn!("Could not auto-detect controller IP, using localhost");
    None
}

// ============================================================================
// Main
// ============================================================================

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
        .open("scheduler.log")?;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file)
        .with_ansi(true)
        .with_filter(level_filter);

    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .init();

    info!("Scheduler starting (dispatch architecture)");
    info!("Config: {}", ControllerConfig::find_config_path());

    // Create Elasticsearch client
    let es_transport = Transport::single_node(&config.elasticsearch.url)?;
    let es_client = Elasticsearch::new(es_transport);
    info!("Elasticsearch: {}", config.elasticsearch.url);

    // Create channels
    let (events_tx, mut events_rx) = mpsc::channel::<TargetEvent>(1000);
    let (orchestrator_tx, orchestrator_rx) = mpsc::channel::<OrchestratorEvent>(100);
    let (job_tx, job_rx) = mpsc::channel::<JobSession>(100);

    // Create TargetManager
    let targets = Arc::new(TargetManager::new(
        config.scheduler.run_timeout_secs,
        events_tx.clone(),
        orchestrator_tx.clone(),
    ));

    // Spawn Orchestrator
    let orchestrator = Orchestrator::new(orchestrator_rx, job_rx);
    tokio::spawn(orchestrator.run());
    info!("Orchestrator started");

    // Discover and register targets from automation/generated/*.toml
    discover_and_register_targets(&targets).await;
    info!("Registered {} targets", targets.count());

    // Query target info
    info!("Querying target metadata...");
    let target_infos = targets.query_all_info().await;
    for (target_id, info) in target_infos {
        info!(
            "Target {} - OS: {}, Caps: {:?}",
            target_id, info.os_version, info.capabilities
        );
    }

    // Establish streams (spawns Workers and notifies Orchestrator)
    info!("Establishing streams with targets...");
    let stream_results = targets.establish_all_streams().await;

    let mut ok_count = 0;
    let mut fail_count = 0;
    for (target_id, result) in stream_results {
        match result {
            Ok(()) => {
                info!("Stream established: {}", target_id);
                ok_count += 1;
            }
            Err(e) => {
                warn!("Stream failed for {}: {}", target_id, e);
                fail_count += 1;
            }
        }
    }
    info!("Streams: {} ok, {} failed", ok_count, fail_count);

    // Clone for event loop
    let targets_for_events = Arc::clone(&targets);
    let es_client_for_events = es_client.clone();

    // Spawn event handler loop
    tokio::spawn(async move {
        info!("Event handler started");
        handle_target_events(events_rx, targets_for_events, es_client_for_events).await;
        warn!("Event handler stopped");
    });

    // Create gRPC service
    let service = SchedulerService::new(es_client, job_tx, Arc::clone(&targets));

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

// ============================================================================
// Event Handler
// ============================================================================

async fn handle_target_events(
    mut events_rx: mpsc::Receiver<TargetEvent>,
    targets: Arc<TargetManager>,
    es_client: Elasticsearch,
) {
    while let Some(event) = events_rx.recv().await {
        match event {
            TargetEvent::Connected {
                target_id,
                os_version,
                capabilities,
            } => {
                debug!(
                    "[EVENT] Connected: {} (os={}, caps={:?})",
                    target_id, os_version, capabilities
                );
                let _ = targets.mark_connected(&target_id);
                let _ = targets.update_health(&target_id);
            }

            TargetEvent::Disconnected { target_id, reason } => {
                warn!("[EVENT] Disconnected: {} ({})", target_id, reason);
                let _ = targets.mark_offline(&target_id);
            }

            TargetEvent::Message { target_id, msg } => {
                handle_worker_message(&target_id, msg, &targets, &es_client).await;
            }
        }
    }
}

async fn handle_worker_message(
    target_id: &str,
    msg: automutate::common::WorkerMessage,
    targets: &Arc<TargetManager>,
    es_client: &Elasticsearch,
) {
    match msg.payload {
        Some(worker_message::Payload::Registration(reg)) => {
            info!(
                "[EVENT] Registration: {} - OS: {}, Caps: {:?}",
                target_id, reg.os_version, reg.capabilities
            );

            let tools = if let Some(tv) = reg.tools {
                let mut m = HashMap::new();
                if !tv.rededr_version.is_empty() {
                    m.insert("rededr".to_string(), tv.rededr_version);
                }
                if !tv.defender_version.is_empty() {
                    m.insert("defender".to_string(), tv.defender_version);
                }
                m
            } else {
                HashMap::new()
            };

            let _ = targets.register_with_metadata(
                target_id.to_string(),
                reg.ip_address,
                reg.os_version,
                reg.capabilities,
                reg.metadata,
                tools,
            );
        }

        Some(worker_message::Payload::Status(status)) => {
            debug!(
                "[EVENT] Status: {} - CPU: {}%, Jobs: {}",
                target_id, status.cpu_percent, status.active_jobs
            );
            let _ = targets.update_health(target_id);
        }

        Some(worker_message::Payload::Telemetry(batch)) => {
            let count = batch.events.len();
            debug!(
                "[EVENT] Telemetry: {} - {} events (run: {})",
                target_id, count, batch.run_id
            );

            // Index to ES
            if !batch.events.is_empty() {
                let es = es_client.clone();
                let events = batch.events;
                tokio::spawn(async move {
                    if let Err(e) = index_telemetry(&es, &events).await {
                        error!("Failed to index telemetry: {}", e);
                    }
                });
            }
        }

        Some(worker_message::Payload::SampleResponse(response)) => {
            debug!(
                "[EVENT] SampleResponse: {} - success={}, exit={}",
                target_id, response.success, response.exit_code
            );
            // Response already routed to Worker via result_tx in stream_handler
            let _ = targets.release(target_id);
        }

        Some(worker_message::Payload::ExecutionStatus(status)) => {
            debug!(
                "[EVENT] ExecStatus: {} - {} (job: {})",
                target_id, status.event_type, status.job_id
            );
        }

        Some(worker_message::Payload::Ack(ack)) => {
            debug!("[EVENT] Ack: {} - req: {}", target_id, ack.request_id);
        }

        _ => {}
    }
}

async fn index_telemetry(
    es: &Elasticsearch,
    events: &[automutate::common::TelemetryData],
) -> anyhow::Result<()> {
    use elasticsearch::IndexParts;
    use serde_json::json;

    if events.is_empty() {
        return Ok(());
    }

    let index_name = format!("telemetry-{}", chrono::Utc::now().format("%Y.%m"));

    for event in events {
        let doc = json!({
            "event_type": event.event_type,
            "timestamp": event.timestamp,
            "job_id": event.job_id,
            "metadata": event.metadata,
        });

        let response = es
            .index(IndexParts::Index(&index_name))
            .body(doc)
            .send()
            .await?;

        if !response.status_code().is_success() {
            return Err(anyhow::anyhow!(
                "Index failed: {}",
                response.status_code()
            ));
        }
    }

    Ok(())
}