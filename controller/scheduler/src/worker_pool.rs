// Worker pool management for scheduler
// Phase 1: Static worker list from config file with health checking

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, Duration};

/// Worker status states
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkerStatus {
    /// Ready to accept jobs
    Available,
    /// Currently running a job
    Busy,
    /// Not responding to health checks
    Offline,
}

/// Worker registration type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RegistrationType {
    /// Loaded from TOML file
    Static,
    /// Registered via RPC
    Dynamic,
}

impl std::fmt::Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerStatus::Available => write!(f, "available"),
            WorkerStatus::Busy => write!(f, "busy"),
            WorkerStatus::Offline => write!(f, "offline"),
        }
    }
}

/// Worker state information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerState {
    /// Worker ID (e.g., "worker-01")
    pub id: String,

    /// Worker gRPC address (e.g., "10.200.200.11:50052")
    pub address: String,

    /// Current worker status
    pub status: WorkerStatus,

    /// Currently assigned job ID (if busy)
    pub current_job: Option<String>,

    /// Last successful health check timestamp
    pub last_ping: SystemTime,

    /// Whether worker is enabled in config
    pub enabled: bool,

    // NEW FIELDS for dynamic registration
    /// Operating system version (e.g., "windows10", "windows11")
    pub os_version: String,

    /// Worker capabilities (e.g., ["rededr", "defender", "etw", "gpu"])
    pub capabilities: Vec<String>,

    /// Worker metadata (key-value pairs)
    pub metadata: HashMap<String, String>,

    /// Installed tool versions
    pub tools: HashMap<String, String>,

    /// When worker was registered
    pub registered_at: SystemTime,

    /// How worker was registered (Static TOML or Dynamic RPC)
    pub registration_type: RegistrationType,
}

/// Worker pool managing multiple workers
#[derive(Clone)]
pub struct WorkerPool {
    /// Shared state containing all workers
    state: Arc<Mutex<PoolState>>,
    /// Health check timeout (seconds)
    health_timeout: Duration,
}

struct PoolState {
    /// Workers indexed by ID
    workers: HashMap<String, WorkerState>,
}

impl WorkerPool {
    /// Create a new worker pool
    pub fn new(health_timeout_secs: u64) -> Self {
        WorkerPool {
            state: Arc::new(Mutex::new(PoolState {
                workers: HashMap::new(),
            })),
            health_timeout: Duration::from_secs(health_timeout_secs),
        }
    }

    /// Register a worker from configuration (backward compatible - static registration)
    pub fn register_worker(&self, id: String, address: String, enabled: bool) -> Result<()> {
        // Delegate to internal method with default values and Static type
        self.register_worker_internal(
            id,
            address,
            enabled,
            "unknown".to_string(),  // os_version (not specified in TOML)
            vec![],                 // capabilities (empty for static)
            HashMap::new(),         // metadata (empty)
            HashMap::new(),         // tools (empty)
            RegistrationType::Static,
        )
    }

    /// Register a worker with full metadata (for dynamic registration)
    pub fn register_worker_with_metadata(
        &self,
        id: String,
        address: String,
        enabled: bool,
        os_version: String,
        capabilities: Vec<String>,
        metadata: HashMap<String, String>,
        tools: HashMap<String, String>,
    ) -> Result<()> {
        self.register_worker_internal(
            id,
            address,
            enabled,
            os_version,
            capabilities,
            metadata,
            tools,
            RegistrationType::Dynamic,
        )
    }

    /// Internal method to register a worker (used by both static and dynamic registration)
    fn register_worker_internal(
        &self,
        id: String,
        address: String,
        enabled: bool,
        os_version: String,
        capabilities: Vec<String>,
        metadata: HashMap<String, String>,
        tools: HashMap<String, String>,
        registration_type: RegistrationType,
    ) -> Result<()> {
        use tracing::info;

        let mut state = self.state.lock().unwrap();

        // Check if worker already exists (allow re-registration)
        if let Some(existing) = state.workers.get(&id) {
            info!("Worker {} re-registering (was: {:?})", id, existing.status);
        }

        let worker = WorkerState {
            id: id.clone(),
            address,
            status: if enabled { WorkerStatus::Available } else { WorkerStatus::Offline },
            current_job: None,
            last_ping: SystemTime::now(),
            enabled,
            os_version,
            capabilities: capabilities.clone(),
            metadata,
            tools,
            registered_at: SystemTime::now(),
            registration_type: registration_type.clone(),
        };

        state.workers.insert(id.clone(), worker);

        match registration_type {
            RegistrationType::Dynamic => {
                info!(
                    "Worker {} registered dynamically with capabilities: {:?}",
                    id, capabilities
                );
            }
            RegistrationType::Static => {
                info!("Worker {} registered from TOML configuration", id);
            }
        }

        Ok(())
    }

    /// Get list of available workers
    pub fn get_available_workers(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();

        state
            .workers
            .values()
            .filter(|w| w.status == WorkerStatus::Available && w.enabled)
            .map(|w| w.id.clone())
            .collect()
    }

    /// Get available workers grouped by OS (extracted from worker name)
    ///
    /// # Example
    /// Worker IDs like "win10-worker-01", "win11-worker-02" are grouped by OS prefix
    /// Returns: HashMap<"win10", vec!["win10-worker-01", "win10-worker-02"]>
    pub fn get_available_workers_by_os(&self) -> std::collections::HashMap<String, Vec<String>> {
        use std::collections::HashMap;

        let state = self.state.lock().unwrap();
        let mut by_os: HashMap<String, Vec<String>> = HashMap::new();

        for worker in state.workers.values() {
            if worker.status == WorkerStatus::Available && worker.enabled {
                // Extract OS from worker ID (e.g., "win10" from "win10-worker-01")
                let os = Self::extract_os_from_worker_id(&worker.id);
                by_os.entry(os).or_insert_with(Vec::new).push(worker.id.clone());
            }
        }

        by_os
    }

    /// Extract OS identifier from worker ID
    ///
    /// # Examples
    /// - "win10-worker-01" → "win10"
    /// - "win11-worker-02" → "win11"
    /// - "ubuntu-worker-01" → "ubuntu"
    /// - "worker-01" → "unknown" (fallback)
    fn extract_os_from_worker_id(worker_id: &str) -> String {
        // Split by '-' and take first part as OS identifier
        worker_id.split('-')
            .next()
            .filter(|s| s.len() >= 3 && s.len() <= 10) // Sanity check
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Get available workers with specific capabilities
    /// Returns workers that have ALL required capabilities
    pub fn get_available_workers_with_capabilities(
        &self,
        required_capabilities: &[String],
    ) -> Vec<String> {
        let state = self.state.lock().unwrap();

        state.workers.values()
            .filter(|w| {
                // Worker must be available and enabled
                if w.status != WorkerStatus::Available || !w.enabled {
                    return false;
                }

                // If no capabilities required, any worker is fine
                if required_capabilities.is_empty() {
                    return true;
                }

                // Worker must have ALL required capabilities
                required_capabilities.iter().all(|req| {
                    w.capabilities.iter().any(|cap| cap == req)
                })
            })
            .map(|w| w.id.clone())
            .collect()
    }

    /// Get workers by OS version
    pub fn get_available_workers_by_os_version(&self, os: &str) -> Vec<String> {
        let state = self.state.lock().unwrap();

        state.workers.values()
            .filter(|w| {
                w.status == WorkerStatus::Available
                    && w.enabled
                    && w.os_version == os
            })
            .map(|w| w.id.clone())
            .collect()
    }

    /// Assign a job to a worker (marks worker as busy)
    pub fn assign_worker(&self, worker_id: &str, job_id: &str) -> Result<String> {
        let mut state = self.state.lock().unwrap();

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        if worker.status != WorkerStatus::Available {
            return Err(anyhow!(
                "Worker {} is not available (status: {})",
                worker_id,
                worker.status
            ));
        }

        worker.status = WorkerStatus::Busy;
        worker.current_job = Some(job_id.to_string());

        Ok(worker.address.clone())
    }

    /// Release a worker (marks worker as available)
    pub fn release_worker(&self, worker_id: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        worker.status = WorkerStatus::Available;
        worker.current_job = None;

        Ok(())
    }

    /// Mark worker as offline (for graceful deregistration)
    pub fn mark_worker_offline(&self, worker_id: &str) -> Result<()> {
        use tracing::{info, warn};

        let mut state = self.state.lock().unwrap();

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        // If worker was busy, log warning
        if worker.status == WorkerStatus::Busy {
            warn!(
                "Worker {} deregistering while busy (job: {:?})",
                worker_id, worker.current_job
            );
        }

        worker.status = WorkerStatus::Offline;
        worker.current_job = None;

        info!("Worker {} marked offline", worker_id);
        Ok(())
    }

    /// Update worker health check timestamp
    pub fn update_health(&self, worker_id: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        worker.last_ping = SystemTime::now();

        // If worker was offline and now responding, mark as available
        if worker.status == WorkerStatus::Offline && worker.enabled {
            worker.status = WorkerStatus::Available;
        }

        Ok(())
    }

    /// Actively check worker health by calling HealthCheck RPC
    /// This should be called periodically by the scheduler loop
    pub async fn check_worker_health(&self) {
        use crate::edr::worker::{HealthRequest, worker_agent_client::WorkerAgentClient};

        // Get list of workers to check (clone to avoid holding lock during async calls)
        let workers_to_check: Vec<(String, String)> = {
            let state = self.state.lock().unwrap();
            state.workers.iter()
                .filter(|(_, w)| w.enabled)
                .map(|(id, w)| (id.clone(), w.address.clone()))
                .collect()
        };

        // Check each worker concurrently
        let health_timeout = self.health_timeout;
        let state_clone = self.state.clone();

        for (worker_id, worker_address) in workers_to_check {
            let state = state_clone.clone();
            let timeout = health_timeout;

            tokio::spawn(async move {
                // Try to call HealthCheck RPC with timeout
                let worker_url = format!("http://{}", worker_address);
                let worker_id_for_request = worker_id.clone();

                match tokio::time::timeout(
                    timeout,
                    async {
                        let endpoint = tonic::transport::Endpoint::try_from(worker_url)
                            .map_err(|e| anyhow::anyhow!("Invalid endpoint: {}", e))?;
                        let mut client = WorkerAgentClient::connect(endpoint).await
                            .map_err(|e| anyhow::anyhow!("Connection failed: {}", e))?;
                        client.health_check(tonic::Request::new(HealthRequest {
                            worker_id: worker_id_for_request,
                        })).await
                            .map_err(|e| anyhow::anyhow!("Health check RPC failed: {}", e))
                    }
                ).await {
                    Ok(Ok(_response)) => {
                        // Health check succeeded - update last_ping
                        let mut state = state.lock().unwrap();
                        if let Some(worker) = state.workers.get_mut(&worker_id) {
                            worker.last_ping = SystemTime::now();
                            // If worker was offline, mark as available
                            if worker.status == WorkerStatus::Offline && worker.enabled {
                                worker.status = WorkerStatus::Available;
                                eprintln!("[INFO] Worker {} is back online", worker_id);
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        // RPC error - mark worker as offline
                        let mut state = state.lock().unwrap();
                        if let Some(worker) = state.workers.get_mut(&worker_id) {
                            if worker.status != WorkerStatus::Offline {
                                eprintln!(
                                    "[WARN] Worker {} health check failed: {} - marking offline",
                                    worker_id, e
                                );
                                worker.status = WorkerStatus::Offline;
                                worker.current_job = None;
                            }
                        }
                    }
                    Err(_) => {
                        // Timeout - mark worker as offline
                        let mut state = state.lock().unwrap();
                        if let Some(worker) = state.workers.get_mut(&worker_id) {
                            if worker.status != WorkerStatus::Offline {
                                eprintln!(
                                    "[WARN] Worker {} health check timeout (>{:?}) - marking offline",
                                    worker_id, timeout
                                );
                                worker.status = WorkerStatus::Offline;
                                worker.current_job = None;
                            }
                        }
                    }
                }
            });
        }

        // Wait a bit for health checks to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    /// Get worker state by ID
    pub fn get_worker(&self, worker_id: &str) -> Option<WorkerState> {
        let state = self.state.lock().unwrap();
        state.workers.get(worker_id).cloned()
    }

    /// List all workers
    pub fn list_workers(&self) -> Vec<WorkerState> {
        let state = self.state.lock().unwrap();
        state.workers.values().cloned().collect()
    }

    /// Get worker count by status
    pub fn count_by_status(&self, status: WorkerStatus) -> usize {
        let state = self.state.lock().unwrap();
        state.workers.values().filter(|w| w.status == status).count()
    }

    /// Get total worker count
    pub fn total_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.workers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_registration() {
        let pool = WorkerPool::new(30);

        pool.register_worker(
            "worker-01".to_string(),
            "10.200.200.11:50052".to_string(),
            true,
        )
        .unwrap();

        assert_eq!(pool.total_count(), 1);

        let worker = pool.get_worker("worker-01").unwrap();
        assert_eq!(worker.status, WorkerStatus::Available);
    }

    #[test]
    fn test_worker_assignment() {
        let pool = WorkerPool::new(30);

        pool.register_worker(
            "worker-01".to_string(),
            "10.200.200.11:50052".to_string(),
            true,
        )
        .unwrap();

        // Assign job
        let address = pool.assign_worker("worker-01", "job-000001").unwrap();
        assert_eq!(address, "10.200.200.11:50052");

        let worker = pool.get_worker("worker-01").unwrap();
        assert_eq!(worker.status, WorkerStatus::Busy);
        assert_eq!(worker.current_job, Some("job-000001".to_string()));

        // Cannot assign another job to busy worker
        assert!(pool.assign_worker("worker-01", "job-000002").is_err());

        // Release worker
        pool.release_worker("worker-01").unwrap();

        let worker = pool.get_worker("worker-01").unwrap();
        assert_eq!(worker.status, WorkerStatus::Available);
        assert_eq!(worker.current_job, None);
    }

    #[test]
    fn test_available_workers() {
        let pool = WorkerPool::new(30);

        pool.register_worker(
            "worker-01".to_string(),
            "10.200.200.11:50052".to_string(),
            true,
        )
        .unwrap();
        pool.register_worker(
            "worker-02".to_string(),
            "10.200.200.12:50052".to_string(),
            true,
        )
        .unwrap();
        pool.register_worker(
            "worker-03".to_string(),
            "10.200.200.13:50052".to_string(),
            false, // disabled
        )
        .unwrap();

        let available = pool.get_available_workers();
        assert_eq!(available.len(), 2);
        assert!(available.contains(&"worker-01".to_string()));
        assert!(available.contains(&"worker-02".to_string()));

        // Assign one worker
        pool.assign_worker("worker-01", "job-000001").unwrap();

        let available = pool.get_available_workers();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0], "worker-02");
    }

    #[test]
    fn test_health_check() {
        let pool = WorkerPool::new(2); // 2 second timeout

        pool.register_worker(
            "worker-01".to_string(),
            "10.200.200.11:50052".to_string(),
            true,
        )
        .unwrap();

        // Initially available
        assert_eq!(pool.count_by_status(WorkerStatus::Available), 1);

        // Simulate no health check (worker should still be available within timeout)
        pool.check_worker_health();
        assert_eq!(pool.count_by_status(WorkerStatus::Available), 1);

        // Note: In real scenario, would wait 3 seconds and then check_worker_health
        // would mark worker as offline. Can't easily test without sleep in unit test.
    }
}
