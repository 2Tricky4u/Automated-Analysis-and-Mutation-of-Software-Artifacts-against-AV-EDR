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

    // Clone scheduler for orchestration loop
    let scheduler_for_events = scheduler.clone();

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

                    // Update WorkerPool state: mark worker as connected
                    if let Some(core) = &scheduler_for_events.scheduler_core {
                        if let Err(e) = core.pool().mark_connected(&worker_id) {
                            warn!("Failed to mark worker {} as connected: {}", worker_id, e);
                        }
                        // Also update health to mark as Available if it was Offline
                        if let Err(e) = core.pool().update_health(&worker_id) {
                            warn!("Failed to update health for worker {}: {}", worker_id, e);
                        }
                    }
                }

                // Message event handler
                worker_manager::WorkerEvent::Message { worker_id, msg } => {
                    use crate::automutate::common::worker_message;

                    match msg.payload {
                        Some(worker_message::Payload::Registration(reg)) => {
                            info!(
                                "[WORKER-EVENT] Worker {} registration - OS: {}, Capabilities: {:?}",
                                worker_id, reg.os_version, reg.capabilities
                            );

                            // Convert ToolVersions proto to HashMap
                            let tools = if let Some(tool_versions) = reg.tools {
                                let mut tools_map = std::collections::HashMap::new();
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
                                std::collections::HashMap::new()
                            };

                            // Log tool versions if present
                            if !tools.is_empty() {
                                debug!("  Tools: {:?}", tools);
                            }

                            // Log metadata if present
                            if !reg.metadata.is_empty() {
                                debug!("  Metadata: {:?}", reg.metadata);
                            }

                            // Register/update worker in WorkerPool with full metadata
                            if let Some(core) = &scheduler_for_events.scheduler_core {
                                match core.pool().register_worker_with_metadata(
                                    worker_id.clone(),
                                    reg.ip_address.clone(),
                                    true, // enabled
                                    reg.os_version.clone(),
                                    reg.capabilities.clone(),
                                    reg.metadata.clone(),
                                    tools,
                                ) {
                                    Ok(()) => {
                                        info!(
                                            "[OK] Worker {} metadata updated in WorkerPool [OS: {}, Caps: {}]",
                                            worker_id,
                                            reg.os_version,
                                            reg.capabilities.len()
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to update worker {} metadata in WorkerPool: {}",
                                            worker_id, e
                                        );
                                    }
                                }
                            }
                        }

                        Some(worker_message::Payload::Status(status)) => {
                            debug!(
                                "[WORKER-EVENT] Worker {} status - CPU: {}%, Jobs: {}",
                                worker_id, status.cpu_percent, status.active_jobs
                            );

                            // Update worker health: this is the heartbeat from the stream
                            if let Some(core) = &scheduler_for_events.scheduler_core {
                                if let Err(e) = core.pool().update_health(&worker_id) {
                                    warn!("Failed to update health for worker {}: {}", worker_id, e);
                                }
                            }
                        }

                        Some(worker_message::Payload::Telemetry(batch)) => {
                            let events_count = batch.events.len();
                            let job_id = batch.job_id.clone();
                            let is_final = batch.is_final;

                            info!(
                                "[WORKER-EVENT] Worker {} telemetry - {} events (job: {}, final: {})",
                                worker_id, events_count, job_id, is_final
                            );

                            // Forward to Elasticsearch asynchronously (non-blocking)
                            if !batch.events.is_empty() {
                                let scheduler_clone = scheduler_for_events.clone();
                                let worker_id_clone = worker_id.clone();

                                tokio::spawn(async move {
                                    use tokio::time::{Duration, timeout};

                                    info!(
                                        "[UPLOAD] Indexing {} events to Elasticsearch [job: {}, worker: {}]",
                                        events_count, job_id, worker_id_clone
                                    );

                                    // Index with 10s timeout (matches legacy behavior)
                                    match timeout(Duration::from_secs(10), scheduler_clone.index_telemetry_batch(&batch.events)).await {
                                        Ok(Ok(())) => {
                                            info!(
                                                "[OK] Successfully indexed {} telemetry events to Elasticsearch [job: {}, worker: {}]",
                                                events_count, job_id, worker_id_clone
                                            );
                                        }
                                        Ok(Err(e)) => {
                                            error!("[ERROR] ELASTICSEARCH ERROR: Failed to index telemetry batch");
                                            error!(
                                                "   Job: {}, Events: {}, Worker: {}",
                                                job_id, events_count, worker_id_clone
                                            );
                                            error!("   Error details: {}", e);
                                            warn!(
                                                "   Telemetry received but NOT INDEXED (Elasticsearch may be down/unreachable)"
                                            );
                                            warn!(
                                                "   Possible causes: Elasticsearch down, network issue, mapping conflict, disk full"
                                            );
                                            // Don't crash - telemetry was received, just not indexed
                                        }
                                        Err(_) => {
                                            error!(
                                                "[TIMEOUT] ELASTICSEARCH TIMEOUT: Indexing exceeded 10s limit [job: {}, worker: {}]",
                                                job_id, worker_id_clone
                                            );
                                            error!(
                                                "   {} events NOT INDEXED due to timeout",
                                                events_count
                                            );
                                            warn!("   Possible causes: Elasticsearch overloaded, large batch, slow network");
                                            // Don't crash - continue processing other events
                                        }
                                    }
                                });
                            } else {
                                debug!("[WORKER-EVENT] Empty telemetry batch from worker {}, skipping indexing", worker_id);
                            }
                        }

                        Some(worker_message::Payload::SampleResponse(response)) => {
                            let job_id = response.job_id.clone();
                            let success = response.success;
                            let exit_code = response.exit_code;
                            let output_preview = if response.output.len() > 200 {
                                format!("{}... ({} bytes)", &response.output[..200], response.output.len())
                            } else {
                                response.output.clone()
                            };

                            info!(
                                "[WORKER-EVENT] Worker {} completed job {} - Success: {}, Exit code: {}",
                                worker_id, job_id, success, exit_code
                            );
                            debug!("Output preview: {}", output_preview);

                            // Release worker (mark as available for new jobs)
                            if let Some(core) = &scheduler_for_events.scheduler_core {
                                match core.pool().release_worker(&worker_id) {
                                    Ok(()) => {
                                        info!(
                                            "[OK] Worker {} released and marked available (job: {})",
                                            worker_id, job_id
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to release worker {} after job completion: {}",
                                            worker_id, e
                                        );
                                        // Continue processing - non-critical error
                                    }
                                }
                            }

                            // Store job completion result to Elasticsearch asynchronously
                            let scheduler_clone = scheduler_for_events.clone();
                            let worker_id_clone = worker_id.clone();

                            tokio::spawn(async move {
                                use serde_json::json;
                                use tokio::time::{Duration, timeout};

                                // Build a simple completion document for ES
                                let index_name = format!("job-completions-{}", chrono::Utc::now().format("%Y.%m"));

                                let doc = json!({
                                    "job_id": job_id,
                                    "worker_id": worker_id_clone,
                                    "success": success,
                                    "exit_code": exit_code,
                                    "output": response.output,
                                    "telemetry_ids": response.telemetry_ids,
                                    "completed_at": chrono::Utc::now().to_rfc3339(),
                                });

                                info!(
                                    "[UPLOAD] Storing job completion to Elasticsearch [job: {}, worker: {}]",
                                    job_id, worker_id_clone
                                );

                                // Store with 5s timeout
                                match timeout(Duration::from_secs(5), async {
                                    use elasticsearch::{IndexParts, http::StatusCode};

                                    let response = scheduler_clone.es_client
                                        .index(IndexParts::Index(&index_name))
                                        .body(doc)
                                        .send()
                                        .await?;

                                    if response.status_code().is_success() {
                                        Ok(())
                                    } else {
                                        Err(anyhow::anyhow!("ES returned status: {}", response.status_code()))
                                    }
                                }).await {
                                    Ok(Ok(())) => {
                                        info!(
                                            "[OK] Stored job completion to Elasticsearch [job: {}, worker: {}]",
                                            job_id, worker_id_clone
                                        );
                                    }
                                    Ok(Err(e)) => {
                                        error!(
                                            "[ERROR] Failed to store job completion to ES [job: {}, worker: {}]: {}",
                                            job_id, worker_id_clone, e
                                        );
                                        // Don't crash - job completion was handled, just not indexed
                                    }
                                    Err(_) => {
                                        error!(
                                            "[TIMEOUT] ES timeout storing job completion [job: {}, worker: {}]",
                                            job_id, worker_id_clone
                                        );
                                        // Don't crash - job completion was handled
                                    }
                                }
                            });

                            // TODO: Update job status in queue if this was the final round
                            // This would require coordination with scheduler_core/round_processor
                        }

                        Some(worker_message::Payload::Ack(ack)) => {
                            debug!(
                                "[WORKER-EVENT] Worker {} acked request {} - Success: {}",
                                worker_id, ack.request_id, ack.success
                            );
                            // Optional: Update pending command tracking
                        }

                        Some(worker_message::Payload::ExecutionStatus(status)) => {
                            // Log detailed execution progress based on event type
                            match status.event_type.as_str() {
                                "started" => {
                                    info!(
                                        "[EXEC-STATUS] Worker {} started execution [job: {}, run: {}, artifact: {}, PID: {}]",
                                        worker_id, status.job_id, status.run_id, status.artifact_name, status.pid
                                    );
                                }
                                "heartbeat" => {
                                    debug!(
                                        "[EXEC-STATUS] Worker {} heartbeat [job: {}, PID: {}, elapsed: {}s, alive: {}, events: {}, CPU: {}%, MEM: {}MB]",
                                        worker_id,
                                        status.job_id,
                                        status.pid,
                                        status.elapsed_seconds,
                                        status.process_alive,
                                        status.telemetry_events_count,
                                        status.cpu_percent,
                                        status.memory_mb
                                    );
                                }
                                "stuck" => {
                                    warn!(
                                        "[EXEC-STATUS] Worker {} execution stuck [job: {}, PID: {}, elapsed: {}s, events: {}]",
                                        worker_id,
                                        status.job_id,
                                        status.pid,
                                        status.elapsed_seconds,
                                        status.telemetry_events_count
                                    );
                                    if !status.details.is_empty() {
                                        warn!("  Details: {}", status.details);
                                    }
                                }
                                "approaching_timeout" => {
                                    warn!(
                                        "[EXEC-STATUS] Worker {} approaching timeout [job: {}, PID: {}, elapsed: {}s, alive: {}]",
                                        worker_id,
                                        status.job_id,
                                        status.pid,
                                        status.elapsed_seconds,
                                        status.process_alive
                                    );
                                }
                                "terminated" => {
                                    info!(
                                        "[EXEC-STATUS] Worker {} execution terminated [job: {}, PID: {}, elapsed: {}s, events: {}]",
                                        worker_id,
                                        status.job_id,
                                        status.pid,
                                        status.elapsed_seconds,
                                        status.telemetry_events_count
                                    );
                                    if !status.details.is_empty() {
                                        debug!("  Details: {}", status.details);
                                    }
                                }
                                _ => {
                                    debug!(
                                        "[EXEC-STATUS] Worker {} execution status [type: {}, job: {}, PID: {}]",
                                        worker_id, status.event_type, status.job_id, status.pid
                                    );
                                }
                            }

                            // Optional: Store intermediate execution status in Elasticsearch for monitoring dashboards
                            // Uncomment this block to enable ES storage of execution status
                            /*
                            let scheduler_clone = scheduler_for_events.clone();
                            let worker_id_clone = worker_id.clone();

                            tokio::spawn(async move {
                                use serde_json::json;
                                use tokio::time::{Duration, timeout};

                                let index_name = format!("execution-status-{}", chrono::Utc::now().format("%Y.%m"));

                                let doc = json!({
                                    "worker_id": worker_id_clone,
                                    "worker_ip": status.worker_ip,
                                    "job_id": status.job_id,
                                    "run_id": status.run_id,
                                    "artifact_name": status.artifact_name,
                                    "pid": status.pid,
                                    "elapsed_seconds": status.elapsed_seconds,
                                    "process_alive": status.process_alive,
                                    "telemetry_events_count": status.telemetry_events_count,
                                    "event_type": status.event_type,
                                    "cpu_percent": status.cpu_percent,
                                    "memory_mb": status.memory_mb,
                                    "details": status.details,
                                    "timestamp": chrono::Utc::now().to_rfc3339(),
                                });

                                // Store with 3s timeout (lower than telemetry since this is more frequent)
                                match timeout(Duration::from_secs(3), async {
                                    use elasticsearch::{IndexParts, http::StatusCode};

                                    let response = scheduler_clone.es_client
                                        .index(IndexParts::Index(&index_name))
                                        .body(doc)
                                        .send()
                                        .await?;

                                    if response.status_code().is_success() {
                                        Ok(())
                                    } else {
                                        Err(anyhow::anyhow!("ES returned status: {}", response.status_code()))
                                    }
                                }).await {
                                    Ok(Ok(())) => {
                                        debug!("[OK] Stored execution status to ES [job: {}]", status.job_id);
                                    }
                                    Ok(Err(e)) => {
                                        debug!("[ERROR] Failed to store execution status to ES: {}", e);
                                    }
                                    Err(_) => {
                                        debug!("[TIMEOUT] ES timeout storing execution status");
                                    }
                                }
                            });
                            */
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

                    // Reschedule jobs assigned to this worker (BEFORE marking offline)
                    if let Some(core) = &scheduler_for_events.scheduler_core {
                        // Get worker's current job (if any) before marking offline
                        if let Some(worker) = core.pool().get_worker(&worker_id) {
                            if let Some(job_id) = worker.current_job {
                                info!(
                                    "[JOB-RECOVERY] Worker {} had assigned job: {} - marking as failed",
                                    worker_id, job_id
                                );

                                // Get job from queue
                                if let Some(mut job) = core.queue().get_job(&job_id) {
                                    // Check if job is still running (not already completed)
                                    if !job.is_terminal() {
                                        // Mark job as failed
                                        job.mark_failed(format!(
                                            "Worker {} disconnected during execution: {}",
                                            worker_id, reason
                                        ));

                                        // Update job in queue
                                        if let Err(e) = core.queue().update_job(&job) {
                                            error!(
                                                "[JOB-RECOVERY] Failed to update job {} status: {}",
                                                job_id, e
                                            );
                                        } else {
                                            warn!(
                                                "[JOB-RECOVERY] Job {} marked as FAILED due to worker disconnect",
                                                job_id
                                            );
                                            warn!(
                                                "  Reason: Worker {} - {}",
                                                worker_id, reason
                                            );
                                            warn!(
                                                "  Job was in round {}/{} when worker disconnected",
                                                job.current_round, job.max_rounds
                                            );
                                        }
                                    } else {
                                        debug!(
                                            "[JOB-RECOVERY] Job {} already terminal (status: {}), no recovery needed",
                                            job_id, job.status
                                        );
                                    }
                                } else {
                                    warn!(
                                        "[JOB-RECOVERY] Job {} not found in queue (worker: {})",
                                        job_id, worker_id
                                    );
                                }
                            } else {
                                debug!(
                                    "[JOB-RECOVERY] Worker {} had no assigned job",
                                    worker_id
                                );
                            }
                        }
                    }

                    // Mark worker as offline in WorkerPool (clears current_job)
                    if let Some(core) = &scheduler_for_events.scheduler_core {
                        if let Err(e) = core.pool().mark_worker_offline(&worker_id) {
                            warn!("Failed to mark worker {} as offline: {}", worker_id, e);
                        }
                        if let Err(e) = core.pool().mark_disconnected(&worker_id) {
                            warn!("Failed to mark worker {} as disconnected: {}", worker_id, e);
                        }
                    }
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
