// Worker pool management for scheduler

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, Duration};
use tokio::sync::RwLock;
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
    /// Shared state containing all workers (async RwLock for concurrent read access)
    state: Arc<RwLock<PoolState>>,
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
            state: Arc::new(RwLock::new(PoolState {
                workers: HashMap::new(),
            })),
            health_timeout: Duration::from_secs(health_timeout_secs),
        }
    }

    /// Register a worker from configuration (backward compatible - static registration)
    pub async fn register_worker(&self, id: String, address: String, enabled: bool) -> Result<()> {
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
        ).await
    }

    /// Register a worker with full metadata (for dynamic registration)
    pub async fn register_worker_with_metadata(
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
        ).await
    }

    /// Internal method to register a worker (used by both static and dynamic registration)
    async fn register_worker_internal(
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

        let mut state = self.state.write().await;

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
            address: address.clone(),
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

        info!("Registering worker {} with address: '{}'", id, address);
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
    pub async fn get_available_workers(&self) -> Vec<String> {
        let state = self.state.read().await;

        state
            .workers
            .values()
            // Only return workers that are available, enabled, AND connected
            .filter(|w| w.status == WorkerStatus::Available && w.enabled && w.connected)
            .map(|w| w.id.clone())
            .collect()
    }

    /// Get available workers grouped by OS, filtered by required capabilities
    ///
    /// # Parameters
    /// - `required_capabilities`: List of capabilities the worker must have (e.g., ["mde"] or ["cortex"])
    ///   If empty, no capability filtering is applied.
    ///
    /// # Returns
    /// HashMap<os_version, Vec<worker_id>> - workers grouped by OS that have ALL required capabilities
    ///
    /// # Example
    /// ```ignore
    /// // Get workers with MDE capability, grouped by OS
    /// let workers = pool.get_available_workers_by_os_and_capabilities(&["mde".to_string()]).await;
    /// // Returns: {"win10": ["worker-01"], "win11": ["worker-02"]}
    /// ```
    pub async fn get_available_workers_by_os_and_capabilities(
        &self,
        required_capabilities: &[String],
    ) -> HashMap<String, Vec<String>> {
        let state = self.state.read().await;
        let mut by_os: HashMap<String, Vec<String>> = HashMap::new();

        for worker in state.workers.values() {
            // Worker must be available, enabled, AND connected
            if worker.status != WorkerStatus::Available || !worker.enabled || !worker.connected {
                continue;
            }

            // Check capabilities if any are required
            if !required_capabilities.is_empty() {
                let has_all_capabilities = required_capabilities.iter().all(|req| {
                    worker.capabilities.iter().any(|cap| cap.eq_ignore_ascii_case(req))
                });
                if !has_all_capabilities {
                    continue;
                }
            }

            // Group by OS version
            by_os.entry(worker.os_version.clone())
                .or_insert_with(Vec::new)
                .push(worker.id.clone());
        }

        by_os
    }

    /// Release a worker (marks worker as available)
    pub async fn release_worker(&self, worker_id: &str) -> Result<()> {
        let mut state = self.state.write().await;

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        worker.status = WorkerStatus::Available;
        worker.current_job = None;

        Ok(())
    }

    /// Mark worker as offline (for graceful deregistration)
    pub async fn mark_worker_offline(&self, worker_id: &str) -> Result<()> {
        use tracing::{info, warn};

        let mut state = self.state.write().await;

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
    pub async fn mark_connected(&self, worker_id: &str) -> Result<()> {
        let mut state = self.state.write().await;

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        worker.connected = true;
        info!("Worker {} marked connected", worker_id);
        Ok(())
    }

    /// Mark worker as disconnected (WorkerManager lost gRPC stream)
    pub async fn mark_disconnected(&self, worker_id: &str) -> Result<()> {
        let mut state = self.state.write().await;

        let worker = state
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker not found: {}", worker_id))?;

        worker.connected = false;
        info!("Worker {} marked disconnected", worker_id);
        Ok(())
    }

    /// Update worker health check timestamp
    pub async fn update_health(&self, worker_id: &str) -> Result<()> {
        let mut state = self.state.write().await;

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

    /// Get worker state by ID
    pub async fn get_worker(&self, worker_id: &str) -> Option<WorkerState> {
        let state = self.state.read().await;
        state.workers.get(worker_id).cloned()
    }

    /// List all workers
    pub async fn list_workers(&self) -> Vec<WorkerState> {
        let state = self.state.read().await;
        state.workers.values().cloned().collect()
    }

    /// Get worker count by status
    pub async fn count_by_status(&self, status: WorkerStatus) -> usize {
        let state = self.state.read().await;
        state.workers.values().filter(|w| w.status == status).count()
    }

    /// Get total worker count
    pub async fn total_count(&self) -> usize {
        let state = self.state.read().await;
        state.workers.len()
    }
}

#[cfg(test)]
mod tests;
