// Scheduler core - main scheduling loop

use crate::job::Job;
use crate::queue::JobQueue;
use crate::run_queue::RunQueue;  // NEW: Async run queue
use crate::worker_pool::WorkerPool;
use crate::round_processor::RoundProcessor;
use crate::round::Round;
use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn, debug};

/// Scheduler configuration
#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    /// Poll interval in seconds
    pub poll_interval_seconds: u64,
    /// Maximum concurrent jobs
    pub max_concurrent_jobs: usize,
    /// Default timeout in seconds
    pub default_timeout_seconds: u64,
    /// Health check timeout for workers in seconds
    pub health_timeout_seconds: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig {
            poll_interval_seconds: 5,
            max_concurrent_jobs: 1,
            default_timeout_seconds: 60,
            health_timeout_seconds: 30,
        }
    }
}

/// Worker configuration from individual worker TOML files
#[derive(Debug, Clone, Deserialize)]
struct WorkerTomlConfig {
    worker: WorkerInfo,
    controller: ControllerInfo,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkerInfo {
    worker_id: String,
    ip_address: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ControllerInfo {
    controller_address: String,
}

/// Scheduler core managing the scheduling loop
pub struct SchedulerCore {
    /// Job queue (high-level: multi-round tasks)
    queue: JobQueue,
    /// Run queue (low-level: individual baseline/instrumented executions)
    run_queue: RunQueue,
    /// Worker pool
    pool: WorkerPool,
    /// Worker manager for communication
    worker_manager: Arc<crate::worker_manager::WorkerManager>,
    /// Round processor for iterative mutation
    round_processor: RoundProcessor,
    /// Scheduler configuration
    config: SchedulerConfig,
}

impl SchedulerCore {
    /// Create a new scheduler core
    /// Automatically discovers workers from automation/generated/win*-worker-*.toml files
    pub fn new(
        config: SchedulerConfig,
        worker_manager: Arc<crate::worker_manager::WorkerManager>,
    ) -> Result<Self> {
        debug!("Initializing scheduler core");
        debug!("  Poll interval: {}s", config.poll_interval_seconds);
        debug!("  Max concurrent jobs: {}", config.max_concurrent_jobs);
        debug!("  Default timeout: {}s", config.default_timeout_seconds);

        // Create queues and pool
        let queue = JobQueue::new();
        let run_queue = RunQueue::new();
        let pool = WorkerPool::new(config.health_timeout_seconds);

        // Create round processor
        let round_processor = RoundProcessor::new();

        //Discover and register workers from automation/generated/*.toml
        // Returns list of discovered workers for syncing with WorkerManager
        let discovered_workers = Self::discover_and_register_workers(&pool)?;

        // Register same workers in WorkerManager (single source of truth)
        worker_manager.register_from_pool(discovered_workers)?;
        debug!("Worker registration synchronized between WorkerPool and WorkerManager");

        Ok(SchedulerCore {
            queue,
            run_queue,
            pool,
            worker_manager: Arc::clone(&worker_manager),
            round_processor,
            config,
        })
    }

    /// Discover workers from automation/generated/win*-worker-*.toml files
    /// returns Vec<WorkerConfig> for syncing with WorkerManager
    fn discover_and_register_workers(pool: &WorkerPool) -> Result<Vec<crate::worker_manager::WorkerConfig>> {
        let generated_dir = Path::new("automation/generated");

        if !generated_dir.exists() {
            warn!("automation/generated directory not found, no workers registered");
            warn!("Run 'automation/scripts/generate-configs.ps1' to create worker configs");
            return Ok(Vec::new());
        }

        // Find all win*-worker-*.toml files
        let entries = std::fs::read_dir(generated_dir)?;
        let mut worker_count = 0;
        let mut discovered_workers = Vec::new();

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Match pattern: win*-worker-*.toml
                if filename.starts_with("win") && filename.contains("-worker-") && filename.ends_with(".toml") {
                    match Self::load_worker_config(&path) {
                        Ok((worker_id, address)) => {
                            // Register in WorkerPool
                            pool.register_worker(worker_id.clone(), address.clone(), true)?;
                            info!("  Registered worker: {} at {}", worker_id, address);

                            // Add to discovered list for WorkerManager sync
                            discovered_workers.push(crate::worker_manager::WorkerConfig {
                                id: worker_id,
                                address,
                                enabled: true,
                            });

                            worker_count += 1;
                        }
                        Err(e) => {
                            warn!("Failed to load worker config {}: {}", filename, e);
                        }
                    }
                }
            }
        }

        if worker_count == 0 {
            warn!("No workers registered! Scheduler will not be able to execute jobs.");
            warn!("Create worker configs in automation/generated/ (e.g., win10-worker-01.toml)");
        } else {
            info!("Worker pool initialized with {} workers", worker_count);
        }

        Ok(discovered_workers)
    }

    /// Load worker configuration from individual worker TOML file
    /// Returns (worker_id, grpc_address)
    fn load_worker_config(path: &Path) -> Result<(String, String)> {
        let content = std::fs::read_to_string(path)?;
        let config: WorkerTomlConfig = toml::from_str(&content)?;

        // Worker gRPC address is worker IP + port 50052 (standard worker port)
        let address = format!("{}:50052", config.worker.ip_address); //todo put in config

        Ok((config.worker.worker_id, address))
    }

    /// Get reference to job queue (for external job submission)
    pub fn queue(&self) -> &JobQueue {
        &self.queue
    }

    /// Get reference to worker pool (for health checks)
    pub fn pool(&self) -> &WorkerPool {
        &self.pool
    }

    /// Get reference to worker manager (for job execution)
    pub fn worker_manager(&self) -> &Arc<crate::worker_manager::WorkerManager> {
        &self.worker_manager
    }

    /// Main scheduling loop
    /// Runs continuously until process exits
    pub async fn run(self: Arc<Self>) {
        debug!("Scheduler core started (poll interval: {}s)", self.config.poll_interval_seconds);
        debug!("Worker pool loaded: {} workers", self.pool.total_count());
        info!("Ready to accept jobs");

        loop {
            // 1. Check worker health (actively call HealthCheck RPC)
            // DISABLED: Legacy health checks replaced by bidirectional stream heartbeats
            // self.pool.check_worker_health().await;

            // 2. Check for available workers
            let available_workers = self.pool.get_available_workers();
            debug!("Poll iteration - available workers: {}", available_workers.len());

            if available_workers.is_empty() {
                debug!("No available workers, sleeping {}s", self.config.poll_interval_seconds);
                // No workers available, wait and retry
                sleep(Duration::from_secs(self.config.poll_interval_seconds)).await;
                continue;
            }

            // 3. Check if we can run more jobs
            let running_count = self.queue.running_count();
            debug!("Running jobs: {} / {} max", running_count, self.config.max_concurrent_jobs);
            if running_count >= self.config.max_concurrent_jobs {
                debug!("Max concurrent jobs reached, sleeping {}s", self.config.poll_interval_seconds);
                // Already at max concurrent jobs
                sleep(Duration::from_secs(self.config.poll_interval_seconds)).await;
                continue;
            }

            // 4. Get next queued job
            let job = match self.queue.pop_next() {
                Some(job) => {
                    debug!("Found queued job: {}", job.id);
                    job
                }
                None => {
                    debug!("No queued jobs, sleeping {}s", self.config.poll_interval_seconds);
                    // No jobs in queue
                    sleep(Duration::from_secs(self.config.poll_interval_seconds)).await;
                    continue;
                }
            };

            info!("Processing job: {} (template: {})", job.id, job.template_name);

            // 5. Process job (iterative rounds)
            let job_id = job.id.clone();
            let queue = self.queue.clone();
            let pool = self.pool.clone();
            let round_processor = self.round_processor.clone();
            let worker_manager = Arc::clone(&self.worker_manager);
            let config = self.config.clone();

            // Spawn async task to process job
            tokio::spawn(async move {
                if let Err(e) = Self::process_job(job, queue, pool, round_processor, worker_manager, config).await {
                    error!("Job {} failed: {}", job_id, e);
                }
            });

            // 6. Wait before checking for next job
            sleep(Duration::from_secs(self.config.poll_interval_seconds)).await;
        }
    }

    /// Process a single job through iterative rounds
    ///
    /// # Round-Based Iteration Protocol
    /// 1. Mark job as running
    /// 2. Loop through rounds (1 to max_rounds):
    ///    a. Create new Round
    ///    b. Call RoundProcessor.process_round() (dual-run protocol)
    ///    c. Save round to job
    ///    d. Check stopping conditions (stop_on_evasion, stop_on_detection, max_rounds)
    /// 3. Mark job as completed
    async fn process_job(
        mut job: Job,
        queue: JobQueue,
        pool: WorkerPool,
        round_processor: RoundProcessor,
        worker_manager: std::sync::Arc<crate::worker_manager::WorkerManager>,
        _config: SchedulerConfig,
    ) -> Result<()> {
        info!("[{}] Starting job (max_rounds: {})", job.id, job.max_rounds);
        job.start_running();
        queue.update_job(&job)?;

        // Round iteration loop
        while job.should_continue() {
            let round_number = job.current_round + 1;

            info!("[{}][round-{}] Starting round {}/{}", job.id, round_number, round_number, job.max_rounds);

            // Create new round
            let mut round = Round::new(
                job.id.clone(),
                round_number,
            );
            let round_id = round.round_id.clone();

            // Start round in job
            job.start_round();
            queue.update_job(&job)?;

            // Process round (dual-run protocol)
            // Pass WorkerManager for artifact deployment and execution
            match round_processor.process_round(&mut round, &job, &pool, &worker_manager).await {
                Ok(summary) => {
                    info!("[{}][{}] Round complete: detected={}, behavior_match={}, evasion_score={:.2}",
                        job.id, round_id, summary.detected, summary.behavior_match, summary.evasion_score);

                    // Complete round in job
                    job.complete_round(summary);
                    queue.update_job(&job)?;

                    // Check stopping conditions
                    if job.stop_on_evasion && !round.feedback.as_ref().unwrap().detected {
                        info!("[{}] Stopping: artifact not detected (evasion success)", job.id);
                        break;
                    }

                    if job.stop_on_detection && round.feedback.as_ref().unwrap().detected {
                        info!("[{}] Stopping: artifact detected", job.id);
                        break;
                    }
                }
                Err(e) => {
                    error!("[{}][{}] Round failed: {}", job.id, round_id, e);
                    job.mark_failed(format!("Round {} error: {}", round_number, e));
                    queue.update_job(&job)?;
                    return Err(e);
                }
            }
        }

        // Mark job as completed
        job.mark_completed();
        queue.update_job(&job)?;

        info!("[{}] Job complete: {} rounds processed", job.id, job.rounds.len());

        Ok(())
    }

    /// Build artifact using build/emitter
    async fn build_artifact(job: &Job) -> Result<String> {
        info!("[{}] Build parameters:", job.id);
        info!("  Template: {}", job.template_name);
        info!("  Source: {}", job.source_file);
        info!("  Trace mode: {}", job.trace_mode);
        info!("  Mutations: {}", job.mutations.len());

        // Create builder with default config
        let builder_config = builder::BuilderConfig::default();
        let artifact_builder = builder::ArtifactBuilder::new(builder_config)?;

        // Convert scheduler mutations to builder mutations
        let mutations: Vec<builder::mutator::MutationSpec> = job
            .mutations
            .iter()
            .map(|m| {
                // Convert serde_json::Value params to HashMap<String, String>
                let params = m.params.as_ref()
                    .and_then(|v| {
                        v.as_object().map(|obj| {
                            obj.iter()
                                .filter_map(|(k, v)| {
                                    v.as_str().map(|s| (k.clone(), s.to_string()))
                                })
                                .collect::<std::collections::HashMap<String, String>>()
                        })
                    })
                    .unwrap_or_default(); // Empty HashMap if no params

                builder::mutator::MutationSpec {
                    id: m.id.clone(),
                    params,
                }
            })
            .collect();

        // Build the artifact
        let built = artifact_builder
            .build(builder::BuildInput::SourceFile {
                template_name: job.template_name.clone(),
                source_file: job.source_file.clone(),
                mutations,
                trace_mode: job.trace_mode.clone(),
            })
            .await?;

        Ok(built.artifact_id)
    }

    /// Deploy artifact to worker via gRPC
    async fn deploy_artifact(job: &Job, worker_address: &str) -> Result<()> {
        use crate::automutate::common::ArtifactChunk;
        use crate::automutate::worker::worker_agent_client::WorkerAgentClient;
        use futures::stream;

        info!("[{}] Deploying to worker: {}", job.id, worker_address);
        info!("  Artifact: {:?}", job.artifact_id);

        let artifact_id = job.artifact_id.as_ref().ok_or_else(|| anyhow!("No artifact ID"))?;

        // 1. Read artifact from disk
        let builder_config = builder::BuilderConfig::default();
        let artifact_path = builder_config
            .output_dir
            .join(format!("{}.exe", artifact_id));

        if !artifact_path.exists() {
            return Err(anyhow!("Artifact {} not found at {:?}", artifact_id, artifact_path));
        }

        let artifact_data = tokio::fs::read(&artifact_path).await?;

        info!("[{}] Read artifact: {} bytes", job.id, artifact_data.len());

        // 2. Connect to worker
        let worker_url = format!("http://{}", worker_address);
        let endpoint = tonic::transport::Endpoint::try_from(worker_url.clone())?;
        let mut client = WorkerAgentClient::connect(endpoint).await?;

        // 3. Split into chunks (4MB per chunk)
        let chunk_size = 4 * 1024 * 1024;
        let total_chunks = ((artifact_data.len() + chunk_size - 1) / chunk_size) as u32;
        let chunks: Vec<ArtifactChunk> = artifact_data
            .chunks(chunk_size)
            .enumerate()
            .map(|(i, chunk)| ArtifactChunk {
                artifact_id: artifact_id.clone(),
                data: chunk.to_vec(),
                chunk_index: i as u32,
                total_chunks,
                sha256: artifact_id.clone(), // SHA256 is the artifact_id
            })
            .collect();

        // 4. Stream chunks to worker
        let request_stream = stream::iter(chunks);
        client.send_artifact(request_stream).await?;

        info!("[{}] Deploy complete", job.id);

        Ok(())
    }

    /// Execute artifact on worker via gRPC (non-blocking)
    async fn execute_artifact(job: &Job, worker_address: &str) -> Result<()> {
        use crate::automutate::common::SampleRequest;
        use crate::automutate::worker::worker_agent_client::WorkerAgentClient;

        info!("[{}] Starting execution on worker: {}", job.id, worker_address);
        info!("  Run ID: {:?}", job.run_id);

        let artifact_id = job.artifact_id.as_ref().ok_or_else(|| anyhow!("No artifact ID"))?;
        let job_id = job.id.clone();

        // Connect to worker
        let worker_url = format!("http://{}", worker_address);
        let endpoint = tonic::transport::Endpoint::try_from(worker_url)?;
        let mut client = WorkerAgentClient::connect(endpoint).await?;

        // Execute artifact via RunSample (worker agent service)
        // NOTE: This is a BLOCKING call that waits for completion
        // We spawn it in a background task so the scheduler can continue
        let request = tonic::Request::new(SampleRequest {
            job_id: job_id.clone(),
            artifact_id: artifact_id.clone(),
            timeout_seconds: 60, // todo take from config
            enable_etw: true,
        });

        // Spawn execution in background task (fire-and-forget)
        // Worker will report status via StatusReport gRPC calls
        let job_id_clone = job_id.clone();
        tokio::spawn(async move {
            match client.run_sample(request).await {
                Ok(response) => {
                    info!("[{}] Worker execution completed: success={}", job_id_clone, response.get_ref().success);
                }
                Err(e) => {
                    error!("[{}] Worker execution RPC failed: {}", job_id_clone, e);
                }
            }
        });

        info!("[{}] Execution request sent to worker", job_id);

        Ok(())
    }
}

/// Helper to create a shared scheduler core instance
pub fn create_scheduler_core(
    config: SchedulerConfig,
    worker_manager: Arc<crate::worker_manager::WorkerManager>,
) -> Result<Arc<SchedulerCore>> {
    let core = SchedulerCore::new(config, worker_manager)?;
    Ok(Arc::new(core))
}
