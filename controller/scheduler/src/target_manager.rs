// Unified Target Manager - combines worker pool state and connection management
//
// Handles:
// - Target registration (from TOML or dynamic)
// - gRPC connection/stream management
// - Target state (Available/Busy/Offline)
// - Artifact deployment and execution
// - Event emission for orchestration loop

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};
use tonic::transport::{Channel, Endpoint};
use tonic::Request;
use tracing::{debug, error, info, warn};

use crate::automutate::common::{
    controller_message, ArtifactChunk, ControllerMessage, DisconnectNotice, Heartbeat,
    RunSampleCommand, SampleRequest, SampleResponse, WorkerMessage, worker_message,
};
use crate::automutate::worker::{worker_agent_client::WorkerAgentClient, WorkerInfoRequest, WorkerInfoResponse};

// ============================================================================
// Types
// ============================================================================

/// Target status - single source of truth for availability
/// Offline = not connected (no separate `connected` bool)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetStatus {
    Available,  // Connected and ready
    Busy,       // Connected but running a job
    Offline,    // Not connected
}

impl std::fmt::Display for TargetStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetStatus::Available => write!(f, "available"),
            TargetStatus::Busy => write!(f, "busy"),
            TargetStatus::Offline => write!(f, "offline"),
        }
    }
}

/// How target was registered
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrationType {
    Static,   // From TOML config
    Dynamic,  // From stream registration
}

/// Events emitted by TargetManager to orchestration loop
#[derive(Debug, Clone)]
pub enum TargetEvent {
    Connected {
        target_id: String,
        os_version: String,
        capabilities: Vec<String>,
    },
    Disconnected {
        target_id: String,
        reason: String,
    },
    Message {
        target_id: String,
        msg: WorkerMessage,
    },
}

/// Target configuration (for static registration)
#[derive(Debug, Clone)]
pub struct TargetConfig {
    pub id: String,
    pub address: String,
    pub enabled: bool,
}

/// Complete target state (worker info + connection state in one struct)
#[derive(Clone)]
pub struct Target {
    // Identity
    pub id: String,
    pub address: String,

    // Metadata (populated from registration message)
    pub os_version: String,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub tools: HashMap<String, String>,

    // State (single source of truth)
    pub status: TargetStatus,
    pub enabled: bool,
    pub registration_type: RegistrationType,
    pub current_job: Option<String>,
    pub last_seen: SystemTime,

    // Connection (embedded - no separate map)
    channel: Option<Channel>,
    stream_tx: Option<mpsc::Sender<ControllerMessage>>,
}

impl Target {
    fn new(id: String, address: String) -> Self {
        Self {
            id,
            address,
            os_version: "unknown".to_string(),
            capabilities: Vec::new(),
            metadata: HashMap::new(),
            tools: HashMap::new(),
            status: TargetStatus::Offline, // Start offline until connected
            enabled: true,
            registration_type: RegistrationType::Static,
            current_job: None,
            last_seen: SystemTime::now(),
            channel: None,
            stream_tx: None,
        }
    }

    fn touch(&mut self) {
        self.last_seen = SystemTime::now();
    }

    fn is_connected(&self) -> bool {
        self.status != TargetStatus::Offline
    }
}

// ============================================================================
// TargetManager
// ============================================================================

/// Unified manager for all target state and connections
pub struct TargetManager {
    /// Single map: id -> Target (state + connection)
    targets: DashMap<String, Target>,

    /// Event bus for target events
    events_tx: mpsc::Sender<TargetEvent>,

    /// Pending responses for stream-based execution
    pending_responses: Arc<Mutex<HashMap<String, oneshot::Sender<SampleResponse>>>>,

    /// RPC timeout for initial queries
    rpc_timeout: Duration,
}

impl TargetManager {
    pub fn new(
        rpc_timeout_secs: u64,
        _health_timeout_secs: u64, // kept for API compat, unused
        events_tx: mpsc::Sender<TargetEvent>,
    ) -> Self {
        Self {
            targets: DashMap::new(),
            events_tx,
            pending_responses: Arc::new(Mutex::new(HashMap::new())),
            rpc_timeout: Duration::from_secs(rpc_timeout_secs),
        }
    }

    // ========================================================================
    // Registration
    // ========================================================================

    /// Register a target from TOML config
    pub fn register(&self, config: TargetConfig) -> Result<()> {
        let id = config.id.clone();

        let mut target = Target::new(id.clone(), config.address.clone());
        target.enabled = config.enabled;
        if !config.enabled {
            target.status = TargetStatus::Offline;
        }

        info!("Registering target {} at {}", id, config.address);
        self.targets.insert(id, target);
        Ok(())
    }

    /// Update target with metadata from stream registration
    pub fn register_with_metadata(
        &self,
        id: String,
        address: String,
        os_version: String,
        capabilities: Vec<String>,
        metadata: HashMap<String, String>,
        tools: HashMap<String, String>,
    ) -> Result<()> {
        // Update existing or create new
        if let Some(mut target) = self.targets.get_mut(&id) {
            // Update in place, preserve status
            target.os_version = os_version.clone();
            target.capabilities = capabilities.clone();
            target.metadata = metadata;
            target.tools = tools;
            target.registration_type = RegistrationType::Dynamic;
            target.touch();
        } else {
            // Create new
            let mut target = Target::new(id.clone(), address);
            target.os_version = os_version.clone();
            target.capabilities = capabilities.clone();
            target.metadata = metadata;
            target.tools = tools;
            target.registration_type = RegistrationType::Dynamic;
            self.targets.insert(id.clone(), target);
        }

        info!(
            "Target {} updated: OS={}, caps={:?}",
            id, os_version, capabilities
        );
        Ok(())
    }

    // ========================================================================
    // Queries
    // ========================================================================

    pub fn get(&self, id: &str) -> Option<Target> {
        self.targets.get(id).map(|t| t.clone())
    }

    pub fn list_ids(&self) -> Vec<String> {
        self.targets.iter().map(|e| e.key().clone()).collect()
    }

    pub fn list_all(&self) -> Vec<Target> {
        self.targets.iter().map(|e| e.clone()).collect()
    }

    pub fn count(&self) -> usize {
        self.targets.len()
    }

    /// Get available (connected + not busy) target IDs
    pub fn get_available(&self) -> Vec<String> {
        self.targets
            .iter()
            .filter(|t| t.status == TargetStatus::Available && t.enabled)
            .map(|t| t.id.clone())
            .collect()
    }

    /// Get available targets grouped by OS, filtered by required capabilities
    pub fn get_available_by_os_and_capabilities(
        &self,
        required_capabilities: &[String],
    ) -> HashMap<String, Vec<String>> {
        let mut by_os: HashMap<String, Vec<String>> = HashMap::new();

        for entry in self.targets.iter() {
            let t = entry.value();

            // Must be available and enabled
            if t.status != TargetStatus::Available || !t.enabled {
                continue;
            }

            // Check capabilities if any required
            if !required_capabilities.is_empty() {
                let has_all = required_capabilities.iter().all(|req| {
                    t.capabilities.iter().any(|cap| cap.eq_ignore_ascii_case(req))
                });
                if !has_all {
                    continue;
                }
            }

            by_os.entry(t.os_version.clone()).or_default().push(t.id.clone());
        }

        by_os
    }

    // ========================================================================
    // State Management (consolidated)
    // ========================================================================

    /// Reserve target for a job (Available -> Busy)
    pub fn reserve(&self, id: &str) -> Result<()> {
        let mut target = self.targets.get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        if target.status != TargetStatus::Available {
            return Err(anyhow!("Target {} not available (status: {})", id, target.status));
        }

        target.status = TargetStatus::Busy;
        info!("Target {} reserved", id);
        Ok(())
    }

    /// Release target (Busy -> Available)
    pub fn release(&self, id: &str) -> Result<()> {
        let mut target = self.targets.get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        if target.status == TargetStatus::Offline {
            return Err(anyhow!("Target {} is offline, cannot release", id));
        }

        target.status = TargetStatus::Available;
        target.current_job = None;
        info!("Target {} released", id);
        Ok(())
    }

    /// Mark target as connected (Offline -> Available)
    pub fn mark_connected(&self, id: &str) -> Result<()> {
        let mut target = self.targets.get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        if target.status == TargetStatus::Offline {
            target.status = TargetStatus::Available;
        }
        target.touch();
        info!("Target {} connected", id);
        Ok(())
    }

    /// Mark target as offline (Any -> Offline), clears stream
    pub fn mark_offline(&self, id: &str) -> Result<()> {
        let mut target = self.targets.get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        if target.status == TargetStatus::Busy {
            warn!("Target {} going offline while busy (job: {:?})", id, target.current_job);
        }

        target.status = TargetStatus::Offline;
        target.current_job = None;
        target.stream_tx = None;
        info!("Target {} offline", id);
        Ok(())
    }

    /// Alias for mark_offline (API compat)
    pub fn mark_disconnected(&self, id: &str) -> Result<()> {
        self.mark_offline(id)
    }

    /// Update health timestamp
    pub fn update_health(&self, id: &str) -> Result<()> {
        let mut target = self.targets.get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;
        target.touch();
        Ok(())
    }

    // ========================================================================
    // Connection Management
    // ========================================================================

    /// Get or create gRPC channel for target
    async fn get_channel(&self, id: &str) -> Result<Channel> {
        // Check for existing channel
        if let Some(target) = self.targets.get(id) {
            if let Some(ref channel) = target.channel {
                return Ok(channel.clone());
            }
        }

        // Get address
        let address = self.targets.get(id)
            .map(|t| t.address.clone())
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        // Create new channel
        let url = format!("http://{}", address);
        debug!("Connecting to target {} at {}", id, url);

        let endpoint = Endpoint::try_from(url)?
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .tcp_keepalive(Some(Duration::from_secs(30)));

        let channel = endpoint.connect().await?;

        // Store channel
        if let Some(mut target) = self.targets.get_mut(id) {
            target.channel = Some(channel.clone());
        }

        info!("Connected to target {}", id);
        Ok(channel)
    }

    /// Establish bidirectional stream with target
    pub async fn establish_stream(&self, id: &str) -> Result<()> {
        debug!("Establishing stream with target {}", id);

        let channel = self.get_channel(id).await?;
        let mut client = WorkerAgentClient::new(channel);

        // Create channel for outgoing messages
        let (tx, rx) = mpsc::channel::<ControllerMessage>(100);
        let outgoing = tokio_stream::wrappers::ReceiverStream::new(rx);

        // Establish stream
        let response = client.establish_stream(Request::new(outgoing)).await?;
        let mut incoming = response.into_inner();

        debug!("Stream established with target {}", id);

        // Store stream sender and mark available
        if let Some(mut target) = self.targets.get_mut(id) {
            target.stream_tx = Some(tx.clone());
            if target.status == TargetStatus::Offline {
                target.status = TargetStatus::Available;
            }
        }

        // Spawn message handler
        let id_clone = id.to_string();
        let events_tx = self.events_tx.clone();
        let targets = self.targets.clone();

        tokio::spawn(async move {
            info!("Stream handler started for target {}", id_clone);
            let mut registration_received = false;

            while let Some(result) = incoming.next().await {
                match result {
                    Ok(msg) => {
                        // Update last_seen
                        if let Some(mut t) = targets.get_mut(&id_clone) {
                            t.touch();

                            // Handle first registration message
                            if !registration_received {
                                if let Some(worker_message::Payload::Registration(ref reg)) = msg.payload {
                                    t.capabilities = reg.capabilities.clone();
                                    t.os_version = reg.os_version.clone();
                                    registration_received = true;

                                    let _ = events_tx.send(TargetEvent::Connected {
                                        target_id: id_clone.clone(),
                                        os_version: reg.os_version.clone(),
                                        capabilities: reg.capabilities.clone(),
                                    }).await;
                                }
                            }
                        }

                        // Forward message to event bus
                        if let Err(e) = events_tx.send(TargetEvent::Message {
                            target_id: id_clone.clone(),
                            msg,
                        }).await {
                            error!("Failed to forward message: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Stream error from target {}: {}", id_clone, e);
                        break;
                    }
                }
            }

            // Cleanup on disconnect
            info!("Stream closed for target {}", id_clone);

            if let Some(mut target) = targets.get_mut(&id_clone) {
                target.stream_tx = None;
                target.status = TargetStatus::Offline;
            }

            let _ = events_tx.send(TargetEvent::Disconnected {
                target_id: id_clone.clone(),
                reason: "Stream closed".to_string(),
            }).await;

            warn!("Target {} disconnected", id_clone);
        });

        // Spawn heartbeat task
        let id_heartbeat = id.to_string();
        let tx_heartbeat = tx.clone();
        let targets_hb = self.targets.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            info!("Heartbeat started for target {}", id_heartbeat);

            loop {
                interval.tick().await;

                // Check if target still connected
                let connected = targets_hb.get(&id_heartbeat)
                    .map(|t| t.status != TargetStatus::Offline)
                    .unwrap_or(false);

                if !connected {
                    info!("Target {} offline, stopping heartbeat", id_heartbeat);
                    break;
                }

                let heartbeat = ControllerMessage {
                    payload: Some(controller_message::Payload::Heartbeat(Heartbeat {
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64,
                    })),
                };

                if tx_heartbeat.send(heartbeat).await.is_err() {
                    warn!("Heartbeat failed for target {}", id_heartbeat);
                    break;
                }

                debug!("Sent heartbeat to target {}", id_heartbeat);
            }

            info!("Heartbeat stopped for target {}", id_heartbeat);
        });

        Ok(())
    }

    /// Establish streams with all registered targets
    pub async fn establish_all_streams(&self) -> HashMap<String, Result<()>> {
        let ids = self.list_ids();
        let mut results = HashMap::new();

        for id in ids {
            let result = self.establish_stream(&id).await;
            results.insert(id, result);
        }

        results
    }

    /// Send command to target via stream
    pub async fn send_command(&self, id: &str, msg: ControllerMessage) -> Result<()> {
        let tx = self.targets.get(id)
            .and_then(|t| t.stream_tx.clone())
            .ok_or_else(|| anyhow!("Target {} not connected", id))?;

        match tx.send(msg).await {
            Ok(()) => Ok(()),
            Err(_) => {
                warn!("Send to target {} failed", id);

                // Mark offline
                if let Some(mut target) = self.targets.get_mut(id) {
                    target.stream_tx = None;
                    target.status = TargetStatus::Offline;
                }

                let _ = self.events_tx.send(TargetEvent::Disconnected {
                    target_id: id.to_string(),
                    reason: "Send failed".to_string(),
                }).await;

                Err(anyhow!("Target {} disconnected", id))
            }
        }
    }

    /// Broadcast message to all connected targets
    pub async fn broadcast(&self, msg: ControllerMessage) -> usize {
        let ids = self.list_ids();
        let mut success = 0;

        for id in ids {
            if self.send_command(&id, msg.clone()).await.is_ok() {
                success += 1;
            }
        }

        success
    }

    /// Gracefully disconnect all targets
    pub async fn disconnect_all(&self, reason: &str, reconnect_allowed: bool) {
        info!("Disconnecting all targets: {}", reason);

        let msg = ControllerMessage {
            payload: Some(controller_message::Payload::Disconnect(DisconnectNotice {
                reason: reason.to_string(),
                reconnect_allowed,
            })),
        };

        let sent = self.broadcast(msg).await;
        info!("Sent disconnect to {}/{} targets", sent, self.count());

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Clear all streams
        for mut target in self.targets.iter_mut() {
            target.stream_tx = None;
            target.status = TargetStatus::Offline;
        }
    }

    // ========================================================================
    // Artifact Operations
    // ========================================================================

    /// Send artifact to target
    pub async fn send_artifact(&self, id: &str, artifact_id: &str, path: &Path) -> Result<()> {
        info!("[{}] Sending artifact to target {}...", artifact_id, id);

        let data = tokio::fs::read(path).await?;
        info!("[{}] Artifact size: {} bytes", artifact_id, data.len());

        let channel = self.get_channel(id).await?;
        let mut client = WorkerAgentClient::new(channel);

        // Chunk into 4MB pieces
        let chunk_size = 4 * 1024 * 1024;
        let total_chunks = ((data.len() + chunk_size - 1) / chunk_size) as u32;

        let chunks: Vec<ArtifactChunk> = data
            .chunks(chunk_size)
            .enumerate()
            .map(|(i, chunk)| ArtifactChunk {
                artifact_id: artifact_id.to_string(),
                data: chunk.to_vec(),
                chunk_index: i as u32,
                total_chunks,
                sha256: artifact_id.to_string(),
            })
            .collect();

        info!("[{}] Sending {} chunks to target {}", artifact_id, total_chunks, id);

        client.send_artifact(stream::iter(chunks)).await?;

        info!("[{}] Artifact deployed to target {}", artifact_id, id);
        Ok(())
    }

    /// Execute artifact on target via stream
    pub async fn execute_artifact(&self, id: &str, request: SampleRequest) -> Result<SampleResponse> {
        info!("[{}] Executing artifact {} on target {}", request.job_id, request.artifact_id, id);

        // Generate run_id for response matching
        let run_id = format!(
            "{}-{}-{}",
            request.job_id,
            request.artifact_id,
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
        );

        // Register pending execution
        let rx = self.register_pending_execution(run_id.clone());

        // Send command
        let command = ControllerMessage {
            payload: Some(controller_message::Payload::RunSample(RunSampleCommand {
                request_id: run_id.clone(),
                request: Some(request.clone()),
            })),
        };

        self.send_command(id, command).await?;

        info!("[{}] Sent RunSampleCommand [run_id: {}]", request.job_id, run_id);

        // Await response
        let timeout_duration = Duration::from_secs(request.timeout_seconds as u64 + 30);

        match tokio::time::timeout(timeout_duration, rx).await {
            Ok(Ok(response)) => {
                info!("[{}] Execution complete: success={}, exit_code={}",
                    request.job_id, response.success, response.exit_code);
                Ok(response)
            }
            Ok(Err(_)) => Err(anyhow!(
                "Response channel closed for run_id: {} (target {} disconnected?)",
                run_id, id
            )),
            Err(_) => Err(anyhow!(
                "Timeout waiting for response from target {} [run_id: {}, timeout: {}s]",
                id, run_id, timeout_duration.as_secs()
            )),
        }
    }

    fn register_pending_execution(&self, run_id: String) -> oneshot::Receiver<SampleResponse> {
        let (tx, rx) = oneshot::channel();
        let mut pending = self.pending_responses.lock().unwrap();
        pending.insert(run_id.clone(), tx);
        debug!("Registered pending execution: {}", run_id);
        rx
    }

    pub fn complete_pending_execution(&self, run_id: &str, response: SampleResponse) -> Result<()> {
        let mut pending = self.pending_responses.lock().unwrap();

        if let Some(tx) = pending.remove(run_id) {
            let _ = tx.send(response);
            debug!("Completed pending execution: {}", run_id);
            Ok(())
        } else {
            debug!("No pending execution for run_id: {}", run_id);
            Err(anyhow!("No pending execution for run_id: {}", run_id))
        }
    }

    // ========================================================================
    // Info Queries (legacy RPC)
    // ========================================================================

    pub async fn get_worker_info(&self, id: &str) -> Result<WorkerInfoResponse> {
        let channel = self.get_channel(id).await?;
        let mut client = WorkerAgentClient::new(channel);

        let response = tokio::time::timeout(
            self.rpc_timeout,
            client.get_worker_info(Request::new(WorkerInfoRequest {})),
        )
        .await
        .map_err(|_| anyhow!("GetWorkerInfo timeout for target {}", id))?
        .map_err(|e| anyhow!("GetWorkerInfo failed for target {}: {}", id, e))?;

        Ok(response.into_inner())
    }

    pub async fn query_all_info(&self) -> HashMap<String, WorkerInfoResponse> {
        let ids = self.list_ids();
        let mut results = HashMap::new();

        for id in ids {
            match self.get_worker_info(&id).await {
                Ok(info) => { results.insert(id, info); }
                Err(e) => { warn!("Failed to query target {}: {}", id, e); }
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_status_display() {
        assert_eq!(TargetStatus::Available.to_string(), "available");
        assert_eq!(TargetStatus::Busy.to_string(), "busy");
        assert_eq!(TargetStatus::Offline.to_string(), "offline");
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let (tx, _rx) = mpsc::channel(100);
        let manager = TargetManager::new(30, 30, tx);

        manager.register(TargetConfig {
            id: "test-1".to_string(),
            address: "127.0.0.1:50052".to_string(),
            enabled: true,
        }).unwrap();

        let target = manager.get("test-1").unwrap();
        assert_eq!(target.id, "test-1");
        assert_eq!(target.status, TargetStatus::Offline); // Starts offline
    }

    #[tokio::test]
    async fn test_reserve_and_release() {
        let (tx, _rx) = mpsc::channel(100);
        let manager = TargetManager::new(30, 30, tx);

        manager.register(TargetConfig {
            id: "test-1".to_string(),
            address: "127.0.0.1:50052".to_string(),
            enabled: true,
        }).unwrap();

        // Must be connected first
        manager.mark_connected("test-1").unwrap();
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Available);

        manager.reserve("test-1").unwrap();
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Busy);

        manager.release("test-1").unwrap();
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Available);
    }

    #[tokio::test]
    async fn test_get_available_by_os_and_caps() {
        let (tx, _rx) = mpsc::channel(100);
        let manager = TargetManager::new(30, 30, tx);

        // Register and connect two targets
        manager.register(TargetConfig {
            id: "win10-1".to_string(),
            address: "127.0.0.1:50052".to_string(),
            enabled: true,
        }).unwrap();

        manager.register(TargetConfig {
            id: "win11-1".to_string(),
            address: "127.0.0.1:50053".to_string(),
            enabled: true,
        }).unwrap();

        // Update metadata
        manager.register_with_metadata(
            "win10-1".to_string(),
            "127.0.0.1:50052".to_string(),
            "win10".to_string(),
            vec!["mde".to_string(), "rededr".to_string()],
            HashMap::new(),
            HashMap::new(),
        ).unwrap();
        manager.mark_connected("win10-1").unwrap();

        manager.register_with_metadata(
            "win11-1".to_string(),
            "127.0.0.1:50053".to_string(),
            "win11".to_string(),
            vec!["defender".to_string()],
            HashMap::new(),
            HashMap::new(),
        ).unwrap();
        manager.mark_connected("win11-1").unwrap();

        // Query with no caps - should get both
        let all = manager.get_available_by_os_and_capabilities(&[]);
        assert_eq!(all.len(), 2);

        // Query with mde cap - only win10
        let with_mde = manager.get_available_by_os_and_capabilities(&["mde".to_string()]);
        assert_eq!(with_mde.len(), 1);
        assert!(with_mde.contains_key("win10"));

        // Query with defender cap - only win11
        let with_defender = manager.get_available_by_os_and_capabilities(&["defender".to_string()]);
        assert_eq!(with_defender.len(), 1);
        assert!(with_defender.contains_key("win11"));
    }

    #[tokio::test]
    async fn test_offline_means_disconnected() {
        let (tx, _rx) = mpsc::channel(100);
        let manager = TargetManager::new(30, 30, tx);

        manager.register(TargetConfig {
            id: "test-1".to_string(),
            address: "127.0.0.1:50052".to_string(),
            enabled: true,
        }).unwrap();

        // Starts offline
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Offline);

        // Connect
        manager.mark_connected("test-1").unwrap();
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Available);

        // Disconnect (via mark_offline)
        manager.mark_offline("test-1").unwrap();
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Offline);

        // mark_disconnected is alias
        manager.mark_connected("test-1").unwrap();
        manager.mark_disconnected("test-1").unwrap();
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Offline);
    }
}