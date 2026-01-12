// Worker pool management for scheduler

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, Duration};
use tracing::{info, warn};


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

    /// Network connectivity status from WorkerManager
    /// true = WorkerManager has active gRPC stream, false = no connection
    pub connected: bool,
}

/// Worker pool managing multiple workers
#[derive(Clone)]
pub struct WorkerPool {
    /// Shared state containing all workers
    state: Arc<Mutex<PoolState>>,
    /// Health check timeout (seconds)
    health_timeout: Duration,
    // WorkerManager handles connectivity events instead
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
        // Preserve connected flag if worker is re-registering
        let existing_connected = if let Some(existing) = state.workers.get(&id) {
            info!("Worker {} re-registering (was: {:?})", id, existing.status);
            existing.connected  // Preserve existing connected state
        } else {
            false  // New worker, initially disconnected
        };

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
            connected: existing_connected,  // Preserve connected flag from existing worker
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
            // Only return workers that are available, enabled, AND connected
            .filter(|w| w.status == WorkerStatus::Available && w.enabled && w.connected)
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
            // Only return workers that are available, enabled, AND connected
            if worker.status == WorkerStatus::Available && worker.enabled && worker.connected {
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
                // Worker must be available, enabled, AND connected
                if w.status != WorkerStatus::Available || !w.enabled || !w.connected {
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
                // Worker must be available, enabled, connected, AND match OS
                w.status == WorkerStatus::Available
                    && w.enabled
                    && w.connected
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

    /// Mark worker as connected (WorkerManager has active gRPC stream)
    pub fn mark_connected(&self, worker_id: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        worker.connected = true;
        info!("Worker {} marked connected", worker_id);
        Ok(())
    }

    /// Mark worker as disconnected (WorkerManager lost gRPC stream)
    pub fn mark_disconnected(&self, worker_id: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        worker.connected = false;
        info!("Worker {} marked disconnected", worker_id);
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
    /// LEGACY TODO transfer to new stream way
    pub async fn check_worker_health(&self) {
        use crate::automutate::worker::{HealthRequest, worker_agent_client::WorkerAgentClient};

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
            let timeout = health_timeout + Duration::from_secs(1);

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
                        let came_back_online = {
                            let mut state = state.lock().unwrap();
                            let mut status_changed = false;
                            if let Some(worker) = state.workers.get_mut(&worker_id) {
                                worker.last_ping = SystemTime::now();
                                // If worker was offline, mark as available
                                if worker.status == WorkerStatus::Offline && worker.enabled {
                                    worker.status = WorkerStatus::Available;
                                    status_changed = true;
                                    info!("Worker {} is back online", worker_id);
                                }
                            }
                            status_changed
                        };

                        if came_back_online {
                            info!("Worker {} came back online", worker_id);
                        }
                    }
                    Ok(Err(e)) => {
                        // RPC error - mark worker as offline
                        let went_offline = {
                            let mut state = state.lock().unwrap();
                            let mut status_changed = false;
                            if let Some(worker) = state.workers.get_mut(&worker_id) {
                                if worker.status != WorkerStatus::Offline {
                                    warn!(
                                        "Worker {} health check failed: {} - marking offline",
                                        worker_id, e
                                    );
                                    worker.status = WorkerStatus::Offline;
                                    worker.current_job = None;
                                    status_changed = true;
                                }
                            }
                            status_changed
                        };

                        if went_offline {
                            info!("Worker {} went offline", worker_id);
                        }
                    }
                    Err(_) => {
                        // Timeout - mark worker as offline
                        let went_offline = {
                            let mut state = state.lock().unwrap();
                            let mut status_changed = false;
                            if let Some(worker) = state.workers.get_mut(&worker_id) {
                                if worker.status != WorkerStatus::Offline {
                                    warn!(
                                        "Worker {} health check timeout (>{:?}) - marking offline",
                                        worker_id, timeout
                                    );
                                    worker.status = WorkerStatus::Offline;
                                    worker.current_job = None;
                                    status_changed = true;
                                }
                            }
                            status_changed
                        };

                        if went_offline {
                            info!("Worker {} went offline", worker_id);
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
mod tests;
