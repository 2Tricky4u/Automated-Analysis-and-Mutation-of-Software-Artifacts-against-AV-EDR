// Scheduler core - main scheduling loop
// Phase 1: Simple FIFO scheduling with single job execution

use crate::job::{Job, JobStatus};
use crate::queue::JobQueue;
use crate::worker_pool::WorkerPool;
use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

/// Scheduler configuration
#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    /// Poll interval in seconds
    pub poll_interval_seconds: u64,
    /// Maximum concurrent jobs (Phase 1: 1, Phase 2+: configurable)
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
            max_concurrent_jobs: 1, // Phase 1: single job at a time
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
    /// Job queue
    queue: JobQueue,
    /// Worker pool
    pool: WorkerPool,
    /// Scheduler configuration
    config: SchedulerConfig,
}

impl SchedulerCore {
    /// Create a new scheduler core
    /// Automatically discovers workers from automation/generated/win*-worker-*.toml files
    pub fn new(config: SchedulerConfig) -> Result<Self> {
        info!("Initializing scheduler core");
        info!("  Poll interval: {}s", config.poll_interval_seconds);
        info!("  Max concurrent jobs: {}", config.max_concurrent_jobs);
        info!("  Default timeout: {}s", config.default_timeout_seconds);

        // Create queue and pool
        let queue = JobQueue::new();
        let pool = WorkerPool::new(config.health_timeout_seconds);

        // Discover and register workers from automation/generated/*.toml
        Self::discover_and_register_workers(&pool)?;

        Ok(SchedulerCore {
            queue,
            pool,
            config,
        })
    }

    /// Discover workers from automation/generated/win*-worker-*.toml files
    fn discover_and_register_workers(pool: &WorkerPool) -> Result<()> {
        let generated_dir = Path::new("automation/generated");

        if !generated_dir.exists() {
            warn!("automation/generated directory not found, no workers registered");
            warn!("Run 'automation/scripts/generate-configs.ps1' to create worker configs");
            return Ok(());
        }

        // Find all win*-worker-*.toml files
        let entries = std::fs::read_dir(generated_dir)?;
        let mut worker_count = 0;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Match pattern: win*-worker-*.toml
                if filename.starts_with("win") && filename.contains("-worker-") && filename.ends_with(".toml") {
                    match Self::load_worker_config(&path) {
                        Ok((worker_id, address)) => {
                            pool.register_worker(worker_id.clone(), address.clone(), true)?;
                            info!("  Registered worker: {} at {}", worker_id, address);
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

        Ok(())
    }

    /// Load worker configuration from individual worker TOML file
    /// Returns (worker_id, grpc_address)
    fn load_worker_config(path: &Path) -> Result<(String, String)> {
        let content = std::fs::read_to_string(path)?;
        let config: WorkerTomlConfig = toml::from_str(&content)?;

        // Worker gRPC address is worker IP + port 50052 (standard worker port)
        let address = format!("{}:50052", config.worker.ip_address);

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

    /// Main scheduling loop
    /// Runs continuously until process exits
    pub async fn run(self: Arc<Self>) {
        info!("Scheduler core started (poll interval: {}s)", self.config.poll_interval_seconds);
        info!("Worker pool loaded: {} workers", self.pool.total_count());
        info!("Ready to accept jobs");

        loop {
            // 1. Check worker health (actively call HealthCheck RPC)
            self.pool.check_worker_health().await;

            // 2. Check for available workers
            let available_workers = self.pool.get_available_workers();

            if available_workers.is_empty() {
                // No workers available, wait and retry
                sleep(Duration::from_secs(self.config.poll_interval_seconds)).await;
                continue;
            }

            // 3. Check if we can run more jobs (Phase 1: max 1 concurrent)
            let running_count = self.queue.running_count();
            if running_count >= self.config.max_concurrent_jobs {
                // Already at max concurrent jobs
                sleep(Duration::from_secs(self.config.poll_interval_seconds)).await;
                continue;
            }

            // 4. Get next queued job
            let job = match self.queue.pop_next() {
                Some(job) => job,
                None => {
                    // No jobs in queue
                    sleep(Duration::from_secs(self.config.poll_interval_seconds)).await;
                    continue;
                }
            };

            info!("Processing job: {} (template: {})", job.id, job.template_name);

            // 5. Process job (build → deploy → execute)
            let job_id = job.id.clone();
            let queue = self.queue.clone();
            let pool = self.pool.clone();
            let config = self.config.clone();

            // Spawn async task to process job
            tokio::spawn(async move {
                if let Err(e) = Self::process_job(job, queue, pool, config).await {
                    error!("Job {} failed: {}", job_id, e);
                }
            });

            // 6. Wait before checking for next job
            sleep(Duration::from_secs(self.config.poll_interval_seconds)).await;
        }
    }

    /// Process a single job through all phases
    async fn process_job(
        mut job: Job,
        queue: JobQueue,
        pool: WorkerPool,
        _config: SchedulerConfig,
    ) -> Result<()> {
        // Phase 1: Build artifact
        info!("[{}] Building artifact", job.id);
        job.start_building();
        queue.update_job(&job)?;

        match Self::build_artifact(&job).await {
            Ok(artifact_id) => {
                job.mark_deployed(artifact_id.clone());
                queue.update_job(&job)?;
                info!("[{}] Build complete: {}", job.id, artifact_id);
            }
            Err(e) => {
                error!("[{}] Build failed: {}", job.id, e);
                job.mark_failed(format!("Build error: {}", e));
                queue.update_job(&job)?;
                return Err(e);
            }
        }

        // Phase 2: Select worker
        let available_workers = pool.get_available_workers();
        if available_workers.is_empty() {
            let err = "No available workers";
            error!("[{}] {}", job.id, err);
            job.mark_failed(err.to_string());
            queue.update_job(&job)?;
            return Err(anyhow!(err));
        }

        // Select first available worker (FIFO)
        let worker_id = available_workers[0].clone();

        // Assign worker
        let worker_address = match pool.assign_worker(&worker_id, &job.id) {
            Ok(addr) => addr,
            Err(e) => {
                error!("[{}] Failed to assign worker: {}", job.id, e);
                job.mark_failed(format!("Worker assignment error: {}", e));
                queue.update_job(&job)?;
                return Err(e);
            }
        };

        info!("[{}] Assigned to worker: {} ({})", job.id, worker_id, worker_address);

        // Phase 3: Deploy artifact
        info!("[{}] Deploying artifact to worker", job.id);

        match Self::deploy_artifact(&job, &worker_address).await {
            Ok(_) => {
                info!("[{}] Deploy complete", job.id);
            }
            Err(e) => {
                error!("[{}] Deploy failed: {}", job.id, e);
                pool.release_worker(&worker_id)?;
                job.mark_failed(format!("Deploy error: {}", e));
                queue.update_job(&job)?;
                return Err(e);
            }
        }

        // Phase 4: Execute artifact
        info!("[{}] Executing artifact on worker", job.id);

        let run_id = uuid::Uuid::new_v4().to_string();
        job.mark_running(worker_id.clone(), run_id.clone());
        queue.update_job(&job)?;

        match Self::execute_artifact(&job, &worker_address).await {
            Ok(_) => {
                info!("[{}] Execution started successfully", job.id);
                // Note: Worker will send StatusReport updates (heartbeat, success, error, timeout)
                // Job status and worker release will be handled by ReportStatus in main.rs
            }
            Err(e) => {
                error!("[{}] Failed to start execution: {}", job.id, e);
                job.mark_failed(format!("Execution start error: {}", e));
                queue.update_job(&job)?;
                // Release worker since execution never started
                pool.release_worker(&worker_id)?;
            }
        }

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
        use crate::edr::worker::{ArtifactChunk, worker_agent_client::WorkerAgentClient};
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
        use crate::edr::worker::{SampleRequest, worker_agent_client::WorkerAgentClient};

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
            timeout_seconds: 60, // From config
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
pub fn create_scheduler_core(config: SchedulerConfig) -> Result<Arc<SchedulerCore>> {
    let core = SchedulerCore::new(config)?;
    Ok(Arc::new(core))
}
