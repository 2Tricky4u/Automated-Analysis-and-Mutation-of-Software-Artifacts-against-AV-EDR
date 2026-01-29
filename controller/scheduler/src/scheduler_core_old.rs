// Scheduler core - main scheduling loop

use crate::job::Job;
use crate::queue::JobQueue;
use crate::round::Round;
use crate::round_processor::RoundProcessor;
use crate::target_manager::{TargetConfig, TargetManager};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

/// Scheduler configuration
#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    /// Poll interval in milliseconds
    pub poll_interval_ms: u64,
    /// Maximum concurrent jobs
    pub max_concurrent_jobs: usize,
    /// Default timeout in seconds
    pub default_timeout_seconds: u64,
    /// Health check timeout for targets in seconds
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

/// Target configuration from individual worker TOML files
#[derive(Debug, Clone, Deserialize)]
struct TargetTomlConfig {
    worker: TargetInfo,
}

#[derive(Debug, Clone, Deserialize)]
struct TargetInfo {
    worker_id: String,
    ip_address: String,
}

/// Scheduler core managing the scheduling loop
pub struct SchedulerCore {
    /// Job queue
    queue: JobQueue,
    /// Target manager (unified worker pool + connection manager)
    targets: Arc<TargetManager>,
    /// Round processor for iterative mutation
    round_processor: RoundProcessor,
    /// Scheduler configuration
    config: SchedulerConfig,
}

impl SchedulerCore {
    /// Create a new scheduler core
    /// Automatically discovers targets from automation/generated/win*-worker-*.toml files
    pub async fn new(config: SchedulerConfig, targets: Arc<TargetManager>) -> Result<Self> {
        debug!("Initializing scheduler core");
        debug!("  Poll interval: {}ms", config.poll_interval_ms);
        debug!("  Max concurrent jobs: {}", config.max_concurrent_jobs);
        debug!("  Default timeout: {}s", config.default_timeout_seconds);

        // Create queue
        let queue = JobQueue::new();

        // Create round processor
        let round_processor = RoundProcessor::new();

        // Discover and register targets from automation/generated/*.toml
        Self::discover_and_register_targets(&targets).await?;

        Ok(SchedulerCore {
            queue,
            targets,
            round_processor,
            config,
        })
    }

    /// Discover targets from automation/generated/win*-worker-*.toml files
    async fn discover_and_register_targets(targets: &TargetManager) -> Result<()> {
        let generated_dir = Path::new("automation/generated");

        if !generated_dir.exists() {
            warn!("automation/generated directory not found, no targets registered");
            warn!("Run 'automation/scripts/generate-configs.ps1' to create target configs");
            return Ok(());
        }

        // Find all win*-worker-*.toml files
        let entries = std::fs::read_dir(generated_dir)?;
        let mut target_count = 0;
        let mut duplicate_count = 0;

        // Track registered IPs to detect duplicates
        let mut registered_ips: HashMap<String, (String, String)> = HashMap::new();

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Match pattern: win*-worker-*.toml
                if filename.starts_with("win")
                    && filename.contains("-worker-")
                    && filename.ends_with(".toml")
                {
                    match Self::load_target_config(&path) {
                        Ok((target_id, address)) => {
                            // Extract IP from address (address is "ip:port")
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
                            targets.register(TargetConfig {
                                id: target_id.clone(),
                                address: address.clone(),
                                enabled: true,
                            })?;
                            info!("  Registered target: {} at {}", target_id, address);

                            // Track this IP
                            registered_ips
                                .insert(ip, (target_id.clone(), filename.to_string()));
                            target_count += 1;
                        }
                        Err(e) => {
                            warn!("Failed to load target config {}: {}", filename, e);
                        }
                    }
                }
            }
        }

        if duplicate_count > 0 {
            warn!(
                "{} duplicate target config(s) were skipped (same IP)",
                duplicate_count
            );
        }

        if target_count == 0 {
            warn!("No targets registered! Scheduler will not be able to execute jobs.");
            warn!("Create target configs in automation/generated/ (e.g., win10-worker-01.toml)");
        } else {
            info!("Target pool initialized with {} unique targets", target_count);
        }

        Ok(())
    }

    /// Load target configuration from individual worker TOML file
    fn load_target_config(path: &Path) -> Result<(String, String)> {
        let content = std::fs::read_to_string(path)?;
        let config: TargetTomlConfig = toml::from_str(&content)?;

        // Target gRPC address is IP + port 50052 (standard worker port)
        let address = format!("{}:50052", config.worker.ip_address);

        Ok((config.worker.worker_id, address))
    }

    /// Get reference to job queue (for external job submission)
    pub fn queue(&self) -> &JobQueue {
        &self.queue
    }

    /// Get reference to target manager
    pub fn targets(&self) -> &Arc<TargetManager> {
        &self.targets
    }

    /// Main scheduling loop
    /// Runs continuously until process exits
    pub async fn run(self: Arc<Self>) {
        debug!(
            "Scheduler core started (poll interval: {}ms)",
            self.config.poll_interval_ms
        );
        debug!("Target pool loaded: {} targets", self.targets.count());
        debug!("Ready to accept jobs");

        loop {
            // 1. Check for available targets
            let available_targets = self.targets.get_available();

            if available_targets.is_empty() {
                // No targets available, wait and retry
                sleep(Duration::from_millis(self.config.poll_interval_ms)).await;
                continue;
            }

            // 2. Check if we can run more jobs
            let running_count = self.queue.running_count();
            if running_count >= self.config.max_concurrent_jobs {
                debug!(
                    "Max concurrent jobs reached, sleeping {}ms",
                    self.config.poll_interval_ms
                );
                sleep(Duration::from_millis(self.config.poll_interval_ms)).await;
                continue;
            }

            // 3. Get next queued job
            let job = match self.queue.pop_next() {
                Some(job) => {
                    debug!("Found queued job: {}", job.id);
                    job
                }
                None => {
                    // No jobs in queue
                    sleep(Duration::from_millis(self.config.poll_interval_ms)).await;
                    continue;
                }
            };

            info!(
                "Processing job: {} (template: {})",
                job.id, job.template_name
            );

            // 4. Process job (iterative rounds)
            let job_id = job.id.clone();
            let queue = self.queue.clone();
            let targets = Arc::clone(&self.targets);
            let round_processor = self.round_processor.clone();
            let config = self.config.clone();

            // Spawn async task to process job
            tokio::spawn(async move {
                if let Err(e) = Self::process_job(job, queue, targets, round_processor, config).await
                {
                    error!("Job {} failed: {}", job_id, e);
                }
            });

            // 5. Wait before checking for next job
            sleep(Duration::from_millis(self.config.poll_interval_ms)).await;
        }
    }

    /// Process a single job through iterative rounds
    async fn process_job(
        mut job: Job,
        queue: JobQueue,
        targets: Arc<TargetManager>,
        round_processor: RoundProcessor,
        _config: SchedulerConfig,
    ) -> Result<()> {
        debug!(
            "[{}] Starting job (max_rounds: {})",
            job.id, job.max_rounds
        );
        job.start_running();
        queue.update_job(&job)?;

        // Round iteration loop
        while job.should_continue() {
            let round_number = job.current_round + 1;

            info!(
                "[{}][round-{}] Starting round {}/{}",
                job.id, round_number, round_number, job.max_rounds
            );

            // Create new round
            let mut round = Round::new(job.id.clone(), round_number);
            let round_id = round.round_id.clone();

            // Start round in job
            job.start_round();
            queue.update_job(&job)?;

            // Process round (dual-run protocol)
            match round_processor.process_round(&mut round, &job, &targets).await {
                Ok(summary) => {
                    info!(
                        "[{}][{}] Round complete: detected={}, behavior_match={}, evasion_score={:.2}",
                        job.id, round_id, summary.detected, summary.behavior_match, summary.evasion_score
                    );

                    // Complete round in job
                    job.complete_round(summary);
                    queue.update_job(&job)?;

                    // Check stopping conditions
                    if job.stop_on_evasion && !round.feedback.as_ref().unwrap().detected {
                        info!(
                            "[{}] Stopping: artifact not detected (evasion success)",
                            job.id
                        );
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

        info!(
            "[{}] Job complete: {} rounds processed",
            job.id,
            job.rounds.len()
        );

        Ok(())
    }
}

/// Helper to create a shared scheduler core instance
pub async fn create_scheduler_core(
    config: SchedulerConfig,
    targets: Arc<TargetManager>,
) -> Result<Arc<SchedulerCore>> {
    let core = SchedulerCore::new(config, targets).await?;
    Ok(Arc::new(core))
}