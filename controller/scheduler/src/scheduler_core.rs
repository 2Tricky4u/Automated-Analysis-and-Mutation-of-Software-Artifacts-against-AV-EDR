// Scheduler core - main scheduling loop

use crate::dispatch_coordinator::DispatchCoordinator;
use crate::dispatcher;
use crate::executor::ProductionExecutor;
use crate::job::Job;
use crate::queue::JobQueue;
use crate::round::Round;
use crate::round_processor::RoundProcessor;
use crate::worker_pool::WorkerPool;
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
// sleep/Duration removed - now using signal-based scheduling
use tracing::{debug, error, info, warn};

/// Scheduler configuration
#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    /// Poll interval in seconds
    pub poll_interval_ms: u64,  //TODO not needed anymore
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
            poll_interval_ms: 500,
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
}

#[derive(Debug, Clone, Deserialize)]
struct WorkerInfo {
    worker_id: String,
    ip_address: String,
}

/// Scheduler core managing the scheduling loop
pub struct SchedulerCore {
    /// Job queue (high-level: multi-round tasks)
    queue: JobQueue,
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
    pub async fn new(
        config: SchedulerConfig,
        worker_manager: Arc<crate::worker_manager::WorkerManager>,
    ) -> Result<Self> {
        debug!("Initializing scheduler core");
        debug!("  Max concurrent jobs: {}", config.max_concurrent_jobs);
        debug!("  Default timeout: {}s", config.default_timeout_seconds);

        let queue = JobQueue::new();
        let pool = WorkerPool::new(config.health_timeout_seconds);

        let round_processor = RoundProcessor::new();

        let discovered_workers = Self::discover_and_register_workers(&pool).await?;

        worker_manager.register_from_pool(discovered_workers)?;
        debug!("Worker registration synchronized between WorkerPool and WorkerManager");

        Ok(SchedulerCore {
            queue,
            pool,
            worker_manager: Arc::clone(&worker_manager),
            round_processor,
            config,
        })
    }

    /// Discover workers from automation/generated/win*-worker-*.toml files
    async fn discover_and_register_workers(
        pool: &WorkerPool,
    ) -> Result<Vec<crate::worker_manager::WorkerConfig>> {
        let generated_dir = Path::new("automation/generated");

        if !generated_dir.exists() {
            warn!("automation/generated directory not found, no workers registered");
            warn!("Run 'automation/scripts/generate-configs.ps1' to create worker configs");
            return Ok(Vec::new());
        }

        let entries = std::fs::read_dir(generated_dir)?;
        let mut worker_count = 0;
        let mut duplicate_count = 0;
        let mut discovered_workers = Vec::new();

        let mut registered_ips: HashMap<String, (String, String)> = HashMap::new();

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.starts_with("win")
                    && filename.contains("-worker-")
                    && filename.ends_with(".toml")
                {
                    match Self::load_worker_config(&path) {
                        Ok((worker_id, address)) => {
                            let ip = address.split(':').next().unwrap_or(&address).to_string();

                            if let Some((existing_id, existing_file)) = registered_ips.get(&ip) {
                                warn!(
                                    "Duplicate IP detected: {} in '{}' (worker: {}) - already registered from '{}' (worker: {}). Skipping.",
                                    ip, filename, worker_id, existing_file, existing_id
                                );
                                duplicate_count += 1;
                                continue;
                            }

                            pool.register_worker(worker_id.clone(), address.clone(), true)
                                .await?;
                            info!("  Registered worker: {} at {}", worker_id, address);

                            registered_ips
                                .insert(ip, (worker_id.clone(), filename.to_string()));

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

        if duplicate_count > 0 {
            warn!(
                "{} duplicate worker config(s) were skipped (same IP)",
                duplicate_count
            );
        }

        if worker_count == 0 {
            warn!("No workers registered! Scheduler will not be able to execute jobs.");
            warn!("Create worker configs in automation/generated/ (e.g., win10-worker-01.toml)");
        } else {
            info!("Worker pool initialized with {} unique workers", worker_count);
        }

        Ok(discovered_workers)
    }

    /// Load worker configuration from individual worker TOML file
    fn load_worker_config(path: &Path) -> Result<(String, String)> {
        let content = std::fs::read_to_string(path)?;
        let config: WorkerTomlConfig = toml::from_str(&content)?;

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

    /// Get reference to worker manager (for job execution)
    pub fn worker_manager(&self) -> &Arc<crate::worker_manager::WorkerManager> {
        &self.worker_manager
    }

    /// Main scheduling loop (signal-based)
    ///
    /// Spawns the dispatcher loop and processes jobs through rounds.
    /// The dispatcher handles run-to-worker matching via signals.
    /// The scheduler waits for job_submitted signals instead of polling.
    pub async fn run(self: Arc<Self>, coordinator: Arc<DispatchCoordinator>) {
        info!("Scheduler started (signal-based)");
        debug!(
            "Worker pool loaded: {} workers",
            self.pool.total_count().await
        );

        // Spawn dispatcher loop for run-to-worker matching
        let dispatcher_coord = Arc::clone(&coordinator);
        let worker_manager = Arc::clone(&self.worker_manager);
        let executor = Arc::new(ProductionExecutor::new());

        tokio::spawn(async move {
            dispatcher::run_dispatcher(dispatcher_coord, worker_manager, executor).await;
        });

        info!("Dispatcher loop spawned");

        loop {
            // Wait for job signal (no polling!)
            coordinator.wait_for_job().await;

            // Process all available jobs after waking
            self.process_pending_jobs(&coordinator).await;
        }
    }

    /// Process all pending jobs that can be started
    async fn process_pending_jobs(self: &Arc<Self>, coordinator: &Arc<DispatchCoordinator>) {
        loop {
            // Check concurrent job limit
            let running_count = self.queue.running_count();
            if running_count >= self.config.max_concurrent_jobs {
                debug!(
                    "Max concurrent jobs reached ({}/{}), waiting for completion",
                    running_count, self.config.max_concurrent_jobs
                );
                return;
            }

            // Try to get next job
            let job = match self.queue.pop_next() {
                Some(job) => {
                    debug!("Found queued job: {}", job.id);
                    job
                }
                None => {
                    // No more jobs to process
                    return;
                }
            };

            info!(
                "Processing job: {} (template: {})",
                job.id, job.template_name
            );

            let job_id = job.id.clone();
            let queue = self.queue.clone();
            let round_processor = self.round_processor.clone();
            let coord = Arc::clone(coordinator);

            tokio::spawn(async move {
                if let Err(e) =
                    Self::process_job_with_coordinator(job, queue, round_processor, &coord).await
                {
                    error!("Job {} failed: {}", job_id, e);
                }
            });
        }
    }

    /// Process a single job through iterative rounds using the coordinator
    async fn process_job_with_coordinator(
        mut job: Job,
        queue: JobQueue,
        round_processor: RoundProcessor,
        coordinator: &DispatchCoordinator,
    ) -> Result<()> {
        debug!("[{}] Starting job (max_rounds: {})", job.id, job.max_rounds);
        job.start_running();
        queue.update_job(&job)?;

        while job.should_continue() {
            let round_number = job.current_round + 1;

            info!(
                "[{}][round-{}] Starting round {}/{}",
                job.id, round_number, round_number, job.max_rounds
            );

            let mut round = Round::new(job.id.clone(), round_number);
            let round_id = round.id.clone();

            job.start_round();
            queue.update_job(&job)?;

            match round_processor
                .process_round(&mut round, &job, coordinator)
                .await
            {
                Ok(summary) => {
                    info!(
                        "[{}][{}] Round complete: detected={}, behavior_match={}, evasion_score={:.2}",
                        job.id, round_id, summary.detected, summary.behavior_match, summary.evasion_score
                    );

                    job.complete_round(summary);
                    queue.update_job(&job)?;

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

        job.mark_completed();
        queue.update_job(&job)?;

        info!("[{}] Job complete: {} rounds processed", job.id, job.rounds.len());

        Ok(())
    }
}

/// Helper to create a shared scheduler core instance
pub async fn create_scheduler_core(
    config: SchedulerConfig,
    worker_manager: Arc<crate::worker_manager::WorkerManager>,
) -> Result<Arc<SchedulerCore>> {
    let core = SchedulerCore::new(config, worker_manager).await?;
    Ok(Arc::new(core))
}