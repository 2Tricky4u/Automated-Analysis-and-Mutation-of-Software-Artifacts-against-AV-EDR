use edr_config::ControllerConfig;
use elasticsearch::{Elasticsearch, http::transport::Transport};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::transport::Server;
use tracing::{debug, error, info, warn};

mod job;
mod queue;
mod round;
mod round_processor;
mod run_queue;
mod run_result;
mod scheduler_core;
mod target_manager;
mod service;
mod storage;

use scheduler_core::{SchedulerConfig as CoreSchedulerConfig, create_scheduler_core};
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
    targets: Option<Arc<TargetManager>>,
}

impl SchedulerService {
    pub fn new(es_client: Elasticsearch, controller_ip: String) -> Self {
        Self {
            state: Arc::new(Mutex::new(SchedulerState::default())),
            es_client,
            controller_ip,
            scheduler_core: None,
            targets: None,
        }
    }

    pub fn set_scheduler_core(&mut self, core: Arc<scheduler_core::SchedulerCore>) {
        self.scheduler_core = Some(core);
    }

    pub fn set_targets(&mut self, targets: Arc<TargetManager>) {
        self.targets = Some(targets);
    }
}

/// Detect the controller's IP address for network communication
fn detect_controller_ip() -> Option<String> {
    use std::net::IpAddr;

    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
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

    warn!("Could not auto-detect controller IP, falling back to localhost");
    None
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load generated TOML
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
        _ => {
            eprintln!("Invalid log level '{}', defaulting to INFO", config.logging.level);
            tracing::Level::INFO
        }
    };

    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .init();

    info!(
        "Loaded controller config successfully from {}",
        ControllerConfig::find_config_path()
    );
    info!("Elasticsearch: {}", config.elasticsearch.url);

    // Create Elasticsearch client
    let es_transport = Transport::single_node(&config.elasticsearch.url)?;
    let es_client = Elasticsearch::new(es_transport);

    let addr = config.server.bind_address.parse()?;

    // Extract controller IP
    let controller_ip = if config.server.bind_address.starts_with("0.0.0.0") {
        detect_controller_ip().unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        config.server.bind_address
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string()
    };

    debug!("Controller IP for Elasticsearch access: {}", controller_ip);

    let mut scheduler = SchedulerService::new(es_client, controller_ip);

    info!("Creating Elasticsearch index templates...");
    info!("Controller/Scheduler starting...");

    // Create target event bus
    use tokio::sync::mpsc;
    let (events_tx, mut events_rx) = mpsc::channel::<TargetEvent>(1000);
    debug!("Created target event bus");

    // Step 1: Initialize TargetManager
    info!("Initializing TargetManager...");
    let targets = Arc::new(TargetManager::new(
        config.scheduler.run_timeout_secs,
        config.scheduler.health_timeout_seconds,
        events_tx.clone(),
    ));

    // Step 2: Create scheduler core configuration
    let scheduler_core_config = CoreSchedulerConfig {
        poll_interval_ms: config.scheduler.poll_interval_ms,
        max_concurrent_jobs: config.scheduler.max_concurrent_runs_per_worker as usize,
        default_timeout_seconds: config.scheduler.run_timeout_secs,
        health_timeout_seconds: config.scheduler.health_timeout_seconds,
    };

    // Step 3: Create and spawn scheduler core (passing TargetManager)
    match create_scheduler_core(scheduler_core_config, Arc::clone(&targets)).await {
        Ok(scheduler_core) => {
            info!("Scheduler core initialized successfully");
            scheduler.set_scheduler_core(Arc::clone(&scheduler_core));

            // Spawn scheduler core in background task
            debug!("Spawning scheduler core run loop...");
            tokio::spawn(async move {
                debug!("Scheduler core task started");
                scheduler_core.run().await;
                warn!("Scheduler core run loop exited unexpectedly");
            });
        }
        Err(e) => {
            warn!("Failed to initialize scheduler core: {}", e);
            warn!("gRPC server will still start, but job scheduling will not be available");
        }
    }

    // Step 4: Query target info on startup
    info!("Querying target metadata...");
    let target_infos = targets.query_all_info().await;
    for (target_id, info) in target_infos {
        info!(
            "Target {} - OS: {}, Capabilities: {:?}",
            target_id, info.os_version, info.capabilities
        );
    }

    // Step 5: Establish bidirectional streams with all targets
    info!("Establishing bidirectional streams with targets...");
    let stream_results = targets.establish_all_streams().await;

    let mut streams_established = 0;
    let mut streams_failed = 0;

    for (target_id, result) in stream_results {
        match result {
            Ok(()) => {
                info!("Stream established with target: {}", target_id);
                streams_established += 1;
            }
            Err(e) => {
                warn!("Failed to establish stream with target {}: {}", target_id, e);
                streams_failed += 1;
            }
        }
    }

    debug!(
        "Stream establishment complete: {} successful, {} failed",
        streams_established, streams_failed
    );

    // Set targets in scheduler service
    scheduler.set_targets(Arc::clone(&targets));
    info!("TargetManager initialized with {} targets", targets.count());

    // Clone scheduler for orchestration loop
    let scheduler_for_events = scheduler.clone();
    let targets_for_events = Arc::clone(&targets);

    // Cache for tracking telemetry counts per run_id

    let telemetry_counts: Arc<tokio::sync::RwLock<HashMap<String, i32>>> =
        Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let telemetry_counts_for_loop = Arc::clone(&telemetry_counts);

    // Spawn orchestration loop to consume target events
    tokio::spawn(async move {
        debug!("Target event orchestration loop started");

        while let Some(event) = events_rx.recv().await {
            match event {
                TargetEvent::Connected { target_id, os_version, capabilities } => {
                    debug!(
                        "[TARGET-EVENT] Target {} connected - OS: {}, Caps: {:?}",
                        target_id, os_version, capabilities
                    );

                    // Mark target as connected
                    if let Err(e) = targets_for_events.mark_connected(&target_id) {
                        warn!("Failed to mark target {} as connected: {}", target_id, e);
                    }

                    // Update health
                    if let Err(e) = targets_for_events.update_health(&target_id) {
                        warn!("Failed to update health for target {}: {}", target_id, e);
                    }
                }

                TargetEvent::Message { target_id, msg } => {
                    use crate::automutate::common::worker_message;

                    match msg.payload {
                        Some(worker_message::Payload::Registration(reg)) => {
                            info!(
                                "[TARGET-EVENT] Target {} registration - IP: '{}', OS: {}, Caps: {:?}",
                                target_id, reg.ip_address, reg.os_version, reg.capabilities
                            );

                            // Convert ToolVersions proto to HashMap
                            let tools = if let Some(tool_versions) = reg.tools {
                                let mut tools_map = HashMap::new();
                                if !tool_versions.rededr_version.is_empty() {
                                    tools_map.insert("rededr".to_string(), tool_versions.rededr_version);
                                }
                                if !tool_versions.defender_version.is_empty() {
                                    tools_map.insert("defender".to_string(), tool_versions.defender_version);
                                }
                                if !tool_versions.etw_version.is_empty() {
                                    tools_map.insert("etw".to_string(), tool_versions.etw_version);
                                }
                                if !tool_versions.llvm_version.is_empty() {
                                    tools_map.insert("llvm".to_string(), tool_versions.llvm_version);
                                }
                                tools_map
                            } else {
                                HashMap::new()
                            };

                            // Register/update target with full metadata
                            if let Err(e) = targets_for_events.register_with_metadata(
                                target_id.clone(),
                                reg.ip_address.clone(),
                                reg.os_version.clone(),
                                reg.capabilities.clone(),
                                reg.metadata.clone(),
                                tools,
                            ) {
                                warn!("Failed to update target {} metadata: {}", target_id, e);
                            } else {
                                debug!(
                                    "[OK] Target {} metadata updated [OS: {}, Caps: {}]",
                                    target_id, reg.os_version, reg.capabilities.len()
                                );
                            }
                        }

                        Some(worker_message::Payload::Status(status)) => {
                            debug!(
                                "[TARGET-EVENT] Target {} status - CPU: {}%, Jobs: {}",
                                target_id, status.cpu_percent, status.active_jobs
                            );

                            // Update target health
                            if let Err(e) = targets_for_events.update_health(&target_id) {
                                warn!("Failed to update health for target {}: {}", target_id, e);
                            }
                        }

                        Some(worker_message::Payload::Telemetry(batch)) => {
                            let events_count = batch.events.len();
                            let job_id = batch.job_id.clone();
                            let run_id = batch.run_id.clone();
                            let is_final = batch.is_final;

                            debug!(
                                "[TARGET-EVENT] Target {} telemetry - {} events (job: {}, run: {}, final: {})",
                                target_id, events_count, job_id, run_id, is_final
                            );

                            // Cache telemetry count
                            {
                                let mut counts = telemetry_counts_for_loop.write().await;
                                *counts.entry(run_id.clone()).or_insert(0) += events_count as i32;
                            }

                            // Forward to Elasticsearch
                            if !batch.events.is_empty() {
                                let scheduler_clone = scheduler_for_events.clone();
                                let target_id_clone = target_id.clone();

                                tokio::spawn(async move {
                                    use tokio::time::{timeout, Duration};

                                    match timeout(
                                        Duration::from_secs(10),
                                        scheduler_clone.index_telemetry_batch(&batch.events),
                                    )
                                    .await
                                    {
                                        Ok(Ok(())) => {
                                            debug!(
                                                "[OK] Indexed {} telemetry events [job: {}, target: {}]",
                                                events_count, job_id, target_id_clone
                                            );
                                        }
                                        Ok(Err(e)) => {
                                            error!(
                                                "[ERROR] Failed to index telemetry: {} [job: {}]",
                                                e, job_id
                                            );
                                        }
                                        Err(_) => {
                                            error!(
                                                "[TIMEOUT] ES timeout indexing telemetry [job: {}]",
                                                job_id
                                            );
                                        }
                                    }
                                });
                            }
                        }

                        Some(worker_message::Payload::SampleResponse(response)) => {
                            let job_id = response.job_id.clone();
                            let run_id = response.run_id.clone();
                            let success = response.success;
                            let exit_code = response.exit_code;

                            debug!(
                                "[TARGET-EVENT] Target {} completed job {} - Success: {}, Exit: {}",
                                target_id, job_id, success, exit_code
                            );

                            // Route response to waiting round_processor
                            if !run_id.is_empty() {
                                if let Err(e) = targets_for_events.complete_pending_execution(&run_id, response.clone()) {
                                    debug!("No pending execution for run_id: {} (might be legacy): {}", run_id, e);
                                }
                            }

                            // Release target
                            if let Err(e) = targets_for_events.release(&target_id) {
                                warn!("Failed to release target {}: {}", target_id, e);
                            } else {
                                debug!("[OK] Target {} released (job: {})", target_id, job_id);
                            }

                            // Store job completion to Elasticsearch
                            let scheduler_clone = scheduler_for_events.clone();
                            let target_id_clone = target_id.clone();
                            let targets_clone = Arc::clone(&targets_for_events);
                            let telemetry_counts_clone = Arc::clone(&telemetry_counts_for_loop);

                            tokio::spawn(async move {
                                use elasticsearch::IndexParts;
                                use serde_json::json;
                                use tokio::time::{timeout, Duration};

                                let index_name = format!("runs-{}", chrono::Utc::now().format("%Y.%m"));

                                let artifact_name = job_id
                                    .split('/')
                                    .last()
                                    .unwrap_or("unknown")
                                    .to_string();

                                let status = if success {
                                    "success"
                                } else if exit_code == -1 {
                                    "timeout"
                                } else {
                                    "error"
                                };

                                let elapsed_seconds = response.output
                                    .split("elapsed: ")
                                    .nth(1)
                                    .and_then(|s| s.split('s').next())
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .map(|f| f.ceil() as i32)
                                    .unwrap_or(0);

                                let target_ip = targets_clone
                                    .get(&target_id_clone)
                                    .map(|t| t.address.split(':').next().unwrap_or("unknown").to_string())
                                    .unwrap_or_else(|| "unknown".to_string());

                                let telemetry_count = {
                                    let counts = telemetry_counts_clone.read().await;
                                    counts.get(&run_id).copied().unwrap_or(0)
                                };

                                // Clean up cache
                                {
                                    let mut counts = telemetry_counts_clone.write().await;
                                    counts.remove(&run_id);
                                }

                                let doc = json!({
                                    "run_id": run_id,
                                    "job_id": job_id,
                                    "status": status,
                                    "elapsed_seconds": elapsed_seconds,
                                    "artifact_name": artifact_name,
                                    "worker_id": target_id_clone,
                                    "worker_ip": target_ip,
                                    "exit_code": exit_code,
                                    "telemetry_events_count": telemetry_count,
                                    "output": response.output,
                                    "telemetry_ids": response.telemetry_ids,
                                    "timestamp": chrono::Utc::now().to_rfc3339(),
                                });

                                match timeout(Duration::from_secs(5), async {
                                    let response = scheduler_clone.es_client
                                        .index(IndexParts::Index(&index_name))
                                        .body(doc)
                                        .send()
                                        .await?;

                                    if response.status_code().is_success() {
                                        Ok(())
                                    } else {
                                        Err(anyhow::anyhow!("ES status: {}", response.status_code()))
                                    }
                                }).await {
                                    Ok(Ok(())) => {
                                        debug!("[OK] Stored job completion [job: {}]", job_id);
                                    }
                                    Ok(Err(e)) => {
                                        error!("[ERROR] Failed to store job completion: {}", e);
                                    }
                                    Err(_) => {
                                        error!("[TIMEOUT] ES timeout storing job completion");
                                    }
                                }
                            });
                        }

                        Some(worker_message::Payload::Ack(ack)) => {
                            debug!(
                                "[TARGET-EVENT] Target {} acked request {} - Success: {}",
                                target_id, ack.request_id, ack.success
                            );
                        }

                        Some(worker_message::Payload::ExecutionStatus(status)) => {
                            match status.event_type.as_str() {
                                "started" => {
                                    info!(
                                        "[EXEC-STATUS] Target {} started [job: {}, PID: {}]",
                                        target_id, status.job_id, status.pid
                                    );
                                }
                                "heartbeat" => {
                                    debug!(
                                        "[EXEC-STATUS] Target {} heartbeat [job: {}, elapsed: {}s]",
                                        target_id, status.job_id, status.elapsed_seconds
                                    );
                                }
                                "stuck" => {
                                    warn!(
                                        "[EXEC-STATUS] Target {} stuck [job: {}]",
                                        target_id, status.job_id
                                    );
                                }
                                "terminated" => {
                                    info!(
                                        "[EXEC-STATUS] Target {} terminated [job: {}]",
                                        target_id, status.job_id
                                    );
                                }
                                _ => {
                                    debug!(
                                        "[EXEC-STATUS] Target {} status: {} [job: {}]",
                                        target_id, status.event_type, status.job_id
                                    );
                                }
                            }
                        }

                        None => {
                            warn!("[TARGET-EVENT] Target {} sent empty message", target_id);
                        }
                        _ => {}
                    }
                }

                TargetEvent::Disconnected { target_id, reason } => {
                    warn!("[TARGET-EVENT] Target {} disconnected: {}", target_id, reason);

                    // Handle job recovery for disconnected target
                    if let Some(core) = &scheduler_for_events.scheduler_core {
                        if let Some(target) = targets_for_events.get(&target_id) {
                            if let Some(ref job_id) = target.current_job {
                                debug!("[JOB-RECOVERY] Target {} had job: {}", target_id, job_id);

                                if let Some(mut job) = core.queue().get_job(job_id) {
                                    if !job.is_terminal() {
                                        job.mark_failed(format!(
                                            "Target {} disconnected: {}",
                                            target_id, reason
                                        ));
                                        if let Err(e) = core.queue().update_job(&job) {
                                            error!("[JOB-RECOVERY] Failed to update job {}: {}", job_id, e);
                                        } else {
                                            warn!("[JOB-RECOVERY] Job {} marked FAILED", job_id);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Mark target offline and disconnected
                    if let Err(e) = targets_for_events.mark_offline(&target_id) {
                        warn!("Failed to mark target {} offline: {}", target_id, e);
                    }
                    if let Err(e) = targets_for_events.mark_disconnected(&target_id) {
                        warn!("Failed to mark target {} disconnected: {}", target_id, e);
                    }
                }
            }
        }

        warn!("Target event orchestration loop terminated");
    });

    debug!("Orchestration loop spawned successfully");

    // gRPC reflection
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