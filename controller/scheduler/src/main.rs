use edr_config::ControllerConfig;
use elasticsearch::{Elasticsearch, http::transport::Transport};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{transport::Server};
use tracing::{debug, error, info, warn};

mod job;
mod queue;
mod round;
mod round_processor;
mod run_queue;
mod run_result;
mod scheduler_core;
mod worker_manager;
mod worker_pool;

// NEW: Service and storage modules
mod service;
mod storage;

use scheduler_core::{SchedulerConfig as CoreSchedulerConfig, create_scheduler_core};

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

#[derive(Clone)]
pub struct SchedulerService {
    state: Arc<Mutex<SchedulerState>>,
    es_client: Elasticsearch,
    controller_ip: String,
    scheduler_core: Option<Arc<scheduler_core::SchedulerCore>>,
    worker_manager: Option<Arc<worker_manager::WorkerManager>>,
}

impl SchedulerService {
    pub fn new(es_client: Elasticsearch, controller_ip: String) -> Self {
        Self {
            state: Arc::new(Mutex::new(SchedulerState::default())),
            es_client,
            controller_ip,
            scheduler_core: None,
            worker_manager: None,
        }
    }

    pub fn set_scheduler_core(&mut self, core: Arc<scheduler_core::SchedulerCore>) {
        self.scheduler_core = Some(core);
    }

    pub fn set_worker_manager(&mut self, manager: Arc<worker_manager::WorkerManager>) {
        self.worker_manager = Some(manager);
    }
}

/// Detect the controller's IP address for network communication
/// Returns the first non-loopback IPv4 address found
fn detect_controller_ip() -> Option<String> {
    use std::net::IpAddr;

    // Try to detect by connecting to a known external address
    // This works even without actual internet connectivity
    // The connect() call doesn't send packets, just selects the appropriate local interface
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        // Connect to a public DNS server (doesn't actually send packets)
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                if let IpAddr::V4(ipv4) = local_addr.ip() {
                    if !ipv4.is_loopback() {
                        debug!("Detected controller IP via routing table: {}", ipv4);
                        return Some(ipv4.to_string());
                    }
                }
            }
        }
    }

    // If all else fails, fallback to localhost
    warn!("Could not auto-detect controller IP, falling back to localhost");
    None
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing with INFO level (visible in both debug and release builds)
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

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
    debug!("Bind address: {}", config.server.bind_address);
    info!("Elasticsearch: {}", config.elasticsearch.url);
    debug!(
        "Triage model: {} (threshold: {})",
        config.triage.model_type, config.triage.confidence_threshold
    );

    // Create Elasticsearch client
    let es_transport = Transport::single_node(&config.elasticsearch.url)?;
    let es_client = Elasticsearch::new(es_transport);

    debug!(
        "Elasticsearch client initialized: {}",
        config.elasticsearch.url
    );

    let addr = config.server.bind_address.parse()?;

    // Extract controller IP from bind address (e.g., "0.0.0.0:50051" or "10.200.200.1:50051")
    // If bind address is 0.0.0.0, try to detect actual IP
    let controller_ip = if config.server.bind_address.starts_with("0.0.0.0") {
        // Try to detect the actual network IP
        detect_controller_ip().unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        // Extract IP from bind address
        config
            .server
            .bind_address
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string()
    };

    debug!("Controller IP for Elasticsearch access: {}", controller_ip);

    let mut scheduler = SchedulerService::new(es_client, controller_ip);

    // Create Elasticsearch index templates
    info!("Creating Elasticsearch index templates...");
    if let Err(e) = scheduler.create_jobs_index_template().await {
        warn!("Failed to create jobs index template: {}", e);
    }
    if let Err(e) = scheduler.create_rounds_index_template().await {
        warn!("Failed to create rounds index template: {}", e);
    }

    info!("Controller/Scheduler starting...");

    // Create worker event bus
    use tokio::sync::mpsc;
    let (events_tx, mut events_rx) = mpsc::channel::<worker_manager::WorkerEvent>(1000); //todo make into config
    debug!("Created worker event bus");

    // Step 2: Initialize WorkerManager
    info!("Initializing WorkerManager...");
    let worker_manager = Arc::new(worker_manager::WorkerManager::new(30, events_tx.clone())); // 30s RPC timeout //todo put in config

    // Step 3: Create scheduler core configuration
    let scheduler_core_config = CoreSchedulerConfig {
        poll_interval_seconds: 5, //todo in config
        max_concurrent_jobs: config.scheduler.max_concurrent_runs_per_worker as usize,
        default_timeout_seconds: config.scheduler.run_timeout_secs,
        health_timeout_seconds: 30, //todo in config
    };

    // Step 4: Create and spawn scheduler core (passing WorkerManager)
    match create_scheduler_core(scheduler_core_config, Arc::clone(&worker_manager)) {
        Ok(scheduler_core) => {
            info!("Scheduler core initialized successfully");

            // Set scheduler core in service (for gRPC methods)
            scheduler.set_scheduler_core(Arc::clone(&scheduler_core));

            // Spawn scheduler core in background task
            tokio::spawn(async move {
                scheduler_core.run().await;
            });
        }
        Err(e) => {
            warn!("Failed to initialize scheduler core: {}", e);
            warn!("gRPC server will still start, but job scheduling will not be available");
        }
    }

    // Query worker info on startup (optional, non-blocking)
    info!("Querying worker metadata...");
    let worker_infos = worker_manager.query_all_workers().await;
    for (worker_id, info) in worker_infos {
        info!(
            "Worker {} - OS: {}, Capabilities: {:?}",
            worker_id, info.os_version, info.capabilities
        );
    }

    // Establish bidirectional streams with all workers for real-time communication
    info!("Establishing bidirectional streams with workers...");
    let stream_results = worker_manager.establish_all_streams().await;

    let mut streams_established = 0;
    let mut streams_failed = 0;

    for (worker_id, result) in stream_results {
        match result {
            Ok(()) => {
                info!("Stream established with worker: {}", worker_id);
                streams_established += 1;
            }
            Err(e) => {
                warn!(
                    "Failed to establish stream with worker {}: {}",
                    worker_id, e
                );
                streams_failed += 1;
            }
        }
    }

    info!(
        "Stream establishment complete: {} successful, {} failed",
        streams_established, streams_failed
    );

    if streams_established > 0 {
        debug!("Workers connected - real-time communication active");
    }

    // Set worker manager in scheduler service
    scheduler.set_worker_manager(Arc::clone(&worker_manager));
    info!(
        "WorkerManager initialized with {} workers",
        &worker_manager.list_workers().len()
    );

    // Spawn orchestration loop to consume worker events
    tokio::spawn(async move {
        info!("Worker event orchestration loop started");

        while let Some(event) = events_rx.recv().await {
            match event {
                // Connected event handler
                worker_manager::WorkerEvent::Connected {
                    worker_id,
                    os_version,
                    capabilities,
                } => {
                    debug!(
                        "[WORKER-EVENT] Worker {} connected - OS: {}, Caps: {:?}",
                        worker_id, os_version, capabilities
                    );
                    // TODO: Send initial configuration or sync state
                    // TODO: Update WorkerPool state
                }

                // Message event handler
                worker_manager::WorkerEvent::Message { worker_id, msg } => {
                    use crate::automutate::common::worker_message;

                    match msg.payload {
                        Some(worker_message::Payload::Registration(reg)) => {
                            debug!("[WORKER-EVENT] Worker {} registered", worker_id);
                        }

                        Some(worker_message::Payload::Status(status)) => {
                            debug!(
                                "[WORKER-EVENT] Worker {} status - CPU: {}%, Jobs: {}",
                                worker_id, status.cpu_percent, status.active_jobs
                            );
                            // TODO: Update worker health metrics
                        }

                        Some(worker_message::Payload::Telemetry(batch)) => {
                            info!(
                                "[WORKER-EVENT] Worker {} telemetry - {} events (job: {}, final: {})",
                                worker_id,
                                batch.events.len(),
                                batch.job_id,
                                batch.is_final
                            );
                            // TODO: Forward to Elasticsearch asynchronously
                            // let es = es_client_orch.clone();
                            // tokio::spawn(async move {
                            //     index_telemetry_batch(&es, &batch).await;
                            // });
                        }

                        Some(worker_message::Payload::SampleResponse(response)) => {
                            info!(
                                "[WORKER-EVENT] Worker {} completed job {} - Success: {}, Exit code: {}",
                                worker_id, response.job_id, response.success, response.exit_code
                            );
                            // TODO: Mark job as complete in scheduler
                            // scheduler_core_orch.complete_job(&response.job_id, response);
                        }

                        Some(worker_message::Payload::Ack(ack)) => {
                            debug!(
                                "[WORKER-EVENT] Worker {} acked request {} - Success: {}",
                                worker_id, ack.request_id, ack.success
                            );
                            // Optional: Update pending command tracking
                        }

                        None => {
                            warn!("[WORKER-EVENT] Worker {} sent empty message", worker_id);
                        }
                        _ => {}
                    }
                }

                // Disconnected event handler
                worker_manager::WorkerEvent::Disconnected { worker_id, reason } => {
                    warn!(
                        "[WORKER-EVENT] Worker {} disconnected: {}",
                        worker_id, reason
                    );
                    // TODO: Reschedule jobs assigned to this worker
                    // if let Some(scheduler) = scheduler_core_orch.as_ref() {
                    //     scheduler.reschedule_worker_jobs(&worker_id);
                    // }

                    // TODO: Mark worker as offline in WorkerPool
                    // if let Some(scheduler) = scheduler_core_orch.as_ref() {
                    //     let _ = scheduler.pool().mark_worker_offline(&worker_id);
                    // }
                }
            }
        }

        warn!("Worker event orchestration loop terminated (event channel closed)");
    });

    info!("Orchestration loop spawned successfully");

    // gRPC reflection for grpcurl
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(tonic::include_file_descriptor_set!(
            "automutate_descriptor"
        ))
        .build_v1()?;

    Server::builder()
        .add_service(ControllerServer::new(scheduler))
        .add_service(reflection_service)
        .serve(addr)
        .await?;

    Ok(())
}
