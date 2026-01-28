// Unified Target Manager - combines worker pool state and connection management
//
// Replaces both worker_pool.rs and worker_manager.rs with a single manager
// that handles:
// - Target registration (from TOML or dynamic)
// - gRPC connection/stream management
// - Target state (available, busy, offline)
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

/// Target status (worker availability)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetStatus {
    /// Ready to accept jobs
    Available,
    /// Currently running a job
    Busy,
    /// Not responding / disconnected
    Offline,
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
    /// Loaded from TOML config
    Static,
    /// Registered via RPC/stream
    Dynamic,
}

/// Events emitted by TargetManager to orchestration loop
#[derive(Debug, Clone)]
pub enum TargetEvent {
    /// Target connected and sent Registration
    Connected {
        target_id: String,
        os_version: String,
        capabilities: Vec<String>,
    },
    /// Target disconnected
    Disconnected { target_id: String, reason: String },
    /// Target sent a message
    Message { target_id: String, msg: WorkerMessage },
}

impl TargetEvent {
    pub fn connected(id: impl Into<String>, os: impl Into<String>, caps: Vec<String>) -> Self {
        TargetEvent::Connected {
            target_id: id.into(),
            os_version: os.into(),
            capabilities: caps,
        }
    }

    pub fn disconnected(id: impl Into<String>, reason: impl Into<String>) -> Self {
        TargetEvent::Disconnected {
            target_id: id.into(),
            reason: reason.into(),
        }
    }

    pub fn message(id: impl Into<String>, msg: WorkerMessage) -> Self {
        TargetEvent::Message {
            target_id: id.into(),
            msg,
        }
    }
}

/// Target configuration (for registration)
#[derive(Debug, Clone)]
pub struct TargetConfig {
    pub id: String,
    pub address: String,
    pub enabled: bool,
}

/// Complete target state (worker + connection)
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

    // State
    pub status: TargetStatus,
    pub enabled: bool,
    pub registration_type: RegistrationType,
    pub registered_at: SystemTime,

    // Job assignment
    pub current_job: Option<String>,

    // Connection state
    pub connected: bool,
    pub last_seen: SystemTime,
}

impl Target {
    fn new(id: String, address: String, registration_type: RegistrationType) -> Self {
        Self {
            id,
            address,
            os_version: "unknown".to_string(),
            capabilities: Vec::new(),
            metadata: HashMap::new(),
            tools: HashMap::new(),
            status: TargetStatus::Available,
            enabled: true,
            registration_type,
            registered_at: SystemTime::now(),
            current_job: None,
            connected: false,
            last_seen: SystemTime::now(),
        }
    }

    fn touch(&mut self) {
        self.last_seen = SystemTime::now();
    }
}

/// Internal: gRPC channel + stream state for a target
#[derive(Clone)]
struct TargetConnection {
    channel: Option<Channel>,
    stream_tx: Option<mpsc::Sender<ControllerMessage>>,
}

impl TargetConnection {
    fn new() -> Self {
        Self {
            channel: None,
            stream_tx: None,
        }
    }
}

// ============================================================================
// TargetManager
// ============================================================================

/// Unified manager for all target (worker) state and connections
pub struct TargetManager {
    /// Target state (id -> Target)
    targets: DashMap<String, Target>,

    /// Connection state (separate from Target for interior mutability)
    connections: DashMap<String, TargetConnection>,

    /// Event bus for target events
    events_tx: mpsc::Sender<TargetEvent>,

    /// Pending responses for stream-based execution
    pending_responses: Arc<Mutex<HashMap<String, oneshot::Sender<SampleResponse>>>>,

    /// RPC timeout
    rpc_timeout: Duration,

    /// Health check timeout
    health_timeout: Duration,
}

impl TargetManager {
    /// Create new TargetManager
    pub fn new(
        rpc_timeout_secs: u64,
        health_timeout_secs: u64,
        events_tx: mpsc::Sender<TargetEvent>,
    ) -> Self {
        Self {
            targets: DashMap::new(),
            connections: DashMap::new(),
            events_tx,
            pending_responses: Arc::new(Mutex::new(HashMap::new())),
            rpc_timeout: Duration::from_secs(rpc_timeout_secs),
            health_timeout: Duration::from_secs(health_timeout_secs),
        }
    }

    // ========================================================================
    // Registration
    // ========================================================================

    /// Register a target from TOML config (static registration)
    pub fn register(&self, config: TargetConfig) -> Result<()> {
        let id = config.id.clone();

        if self.targets.contains_key(&id) {
            info!("Target {} re-registering", id);
        }

        let mut target = Target::new(id.clone(), config.address.clone(), RegistrationType::Static);
        target.enabled = config.enabled;
        if !config.enabled {
            target.status = TargetStatus::Offline;
        }

        info!(
            "Registering target {} at {} (static)",
            id, config.address
        );

        self.targets.insert(id.clone(), target);
        self.connections.insert(id, TargetConnection::new());

        Ok(())
    }

    /// Register/update target with full metadata (dynamic registration from stream)
    pub fn register_with_metadata(
        &self,
        id: String,
        address: String,
        os_version: String,
        capabilities: Vec<String>,
        metadata: HashMap<String, String>,
        tools: HashMap<String, String>,
    ) -> Result<()> {
        // Preserve connected state if re-registering
        let (existing_connected, existing_status) = self
            .targets
            .get(&id)
            .map(|t| (t.connected, t.status))
            .unwrap_or((false, TargetStatus::Available));

        let mut target = Target::new(id.clone(), address.clone(), RegistrationType::Dynamic);
        target.os_version = os_version.clone();
        target.capabilities = capabilities.clone();
        target.metadata = metadata;
        target.tools = tools;
        target.connected = existing_connected;
        target.status = existing_status;

        info!(
            "Target {} registered dynamically: OS={}, caps={:?}",
            id, os_version, capabilities
        );

        self.targets.insert(id.clone(), target);

        // Ensure connection entry exists
        if !self.connections.contains_key(&id) {
            self.connections.insert(id, TargetConnection::new());
        }

        Ok(())
    }

    /// Register multiple targets from configs (bulk registration)
    pub fn register_all(&self, configs: Vec<TargetConfig>) -> Result<()> {
        for config in configs {
            self.register(config)?;
        }
        Ok(())
    }

    // ========================================================================
    // Queries
    // ========================================================================

    /// Get target by ID
    pub fn get(&self, id: &str) -> Option<Target> {
        self.targets.get(id).map(|t| t.clone())
    }

    /// List all target IDs
    pub fn list_ids(&self) -> Vec<String> {
        self.targets.iter().map(|e| e.key().clone()).collect()
    }

    /// List all targets
    pub fn list_all(&self) -> Vec<Target> {
        self.targets.iter().map(|e| e.clone()).collect()
    }

    /// Get count
    pub fn count(&self) -> usize {
        self.targets.len()
    }

    /// Get available targets (for job scheduling)
    pub fn get_available(&self) -> Vec<String> {
        self.targets
            .iter()
            .filter(|t| t.status == TargetStatus::Available && t.enabled && t.connected)
            .map(|t| t.id.clone())
            .collect()
    }

    /// Get available targets grouped by OS, filtered by required capabilities
    ///
    /// Returns HashMap<os_version, Vec<target_id>>
    /// Only includes targets that:
    /// - status == Available
    /// - enabled == true
    /// - connected == true
    /// - have ALL required capabilities (case-insensitive)
    pub fn get_available_by_os_and_capabilities(
        &self,
        required_capabilities: &[String],
    ) -> HashMap<String, Vec<String>> {
        let mut by_os: HashMap<String, Vec<String>> = HashMap::new();

        for entry in self.targets.iter() {
            let t = entry.value();

            // Must be available, enabled, and connected
            if t.status != TargetStatus::Available || !t.enabled || !t.connected {
                continue;
            }

            // Check capabilities if any required
            if !required_capabilities.is_empty() {
                let has_all = required_capabilities.iter().all(|req| {
                    t.capabilities
                        .iter()
                        .any(|cap| cap.eq_ignore_ascii_case(req))
                });
                if !has_all {
                    continue;
                }
            }

            by_os
                .entry(t.os_version.clone())
                .or_default()
                .push(t.id.clone());
        }

        by_os
    }

    // ========================================================================
    // State management
    // ========================================================================

    /// Reserve target for a job (marks busy)
    pub fn reserve(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        if target.status != TargetStatus::Available {
            return Err(anyhow!(
                "Target {} not available (status: {})",
                id,
                target.status
            ));
        }

        target.status = TargetStatus::Busy;
        info!("Target {} reserved", id);
        Ok(())
    }

    /// Release target (marks available)
    pub fn release(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        target.status = TargetStatus::Available;
        target.current_job = None;
        info!("Target {} released", id);
        Ok(())
    }

    /// Assign job to target
    pub fn assign_job(&self, target_id: &str, job_id: &str) -> Result<String> {
        let mut target = self
            .targets
            .get_mut(target_id)
            .ok_or_else(|| anyhow!("Target not found: {}", target_id))?;

        if target.status != TargetStatus::Available {
            return Err(anyhow!(
                "Target {} not available (status: {})",
                target_id,
                target.status
            ));
        }

        target.status = TargetStatus::Busy;
        target.current_job = Some(job_id.to_string());

        Ok(target.address.clone())
    }

    /// Mark target as connected
    pub fn mark_connected(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        target.connected = true;
        target.touch();
        info!("Target {} marked connected", id);
        Ok(())
    }

    /// Mark target as disconnected
    pub fn mark_disconnected(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        target.connected = false;
        info!("Target {} marked disconnected", id);
        Ok(())
    }

    /// Mark target as offline (for graceful deregistration)
    pub fn mark_offline(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        if target.status == TargetStatus::Busy {
            warn!(
                "Target {} going offline while busy (job: {:?})",
                id, target.current_job
            );
        }

        target.status = TargetStatus::Offline;
        target.current_job = None;
        info!("Target {} marked offline", id);
        Ok(())
    }

    /// Update target health (last_seen timestamp)
    pub fn update_health(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        target.touch();

        // If was offline and now responding, mark available
        if target.status == TargetStatus::Offline && target.enabled {
            target.status = TargetStatus::Available;
        }

        Ok(())
    }

    // ========================================================================
    // Connection management
    // ========================================================================

    /// Get or create gRPC channel for target
    async fn get_channel(&self, id: &str) -> Result<Channel> {
        // Check for existing channel
        if let Some(conn) = self.connections.get(id) {
            if let Some(ref channel) = conn.channel {
                return Ok(channel.clone());
            }
        }

        // Get target address
        let address = self
            .targets
            .get(id)
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
        if let Some(mut conn) = self.connections.get_mut(id) {
            conn.channel = Some(channel.clone());
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

        // Store stream sender
        if let Some(mut conn) = self.connections.get_mut(id) {
            conn.stream_tx = Some(tx.clone());
        }

        // Spawn message handler
        let id_clone = id.to_string();
        let events_tx = self.events_tx.clone();
        let targets = self.targets.clone();
        let connections = self.connections.clone();

        tokio::spawn(async move {
            info!("Stream handler started for target {}", id_clone);
            let mut registration_received = false;

            while let Some(result) = incoming.next().await {
                match result {
                    Ok(msg) => {
                        // Update last_seen
                        if let Some(mut t) = targets.get_mut(&id_clone) {
                            t.touch();

                            // Handle registration
                            if !registration_received {
                                if let Some(worker_message::Payload::Registration(ref reg)) =
                                    msg.payload
                                {
                                    t.capabilities = reg.capabilities.clone();
                                    t.os_version = reg.os_version.clone();
                                    registration_received = true;

                                    let _ = events_tx
                                        .send(TargetEvent::connected(
                                            &id_clone,
                                            &reg.os_version,
                                            reg.capabilities.clone(),
                                        ))
                                        .await;
                                }
                            }
                        }

                        // Forward message to event bus
                        if let Err(e) = events_tx
                            .send(TargetEvent::message(&id_clone, msg))
                            .await
                        {
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

            if let Some(mut conn) = connections.get_mut(&id_clone) {
                conn.stream_tx = None;
            }

            let _ = events_tx
                .send(TargetEvent::disconnected(&id_clone, "Stream closed"))
                .await;

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

                // Check if target still exists and is connected
                let exists = targets_hb
                    .get(&id_heartbeat)
                    .map(|t| t.connected)
                    .unwrap_or(false);

                if !exists {
                    info!("Target {} gone, stopping heartbeat", id_heartbeat);
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
        let tx = self
            .connections
            .get(id)
            .and_then(|conn| conn.stream_tx.clone())
            .ok_or_else(|| anyhow!("Target {} not connected or stream not established", id))?;

        match tx.send(msg).await {
            Ok(()) => Ok(()),
            Err(_) => {
                warn!("Send to target {} failed, marking disconnected", id);

                if let Some(mut conn) = self.connections.get_mut(id) {
                    conn.stream_tx = None;
                }

                let _ = self
                    .events_tx
                    .send(TargetEvent::disconnected(id, "Send failed"))
                    .await;

                Err(anyhow!("Target {} disconnected (send failed)", id))
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

        // Clear all stream senders
        for mut conn in self.connections.iter_mut() {
            conn.stream_tx = None;
        }
    }

    // ========================================================================
    // Artifact operations
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

        info!(
            "[{}] Sending {} chunks to target {}",
            artifact_id, total_chunks, id
        );

        client.send_artifact(stream::iter(chunks)).await?;

        info!("[{}] Artifact deployed to target {}", artifact_id, id);
        Ok(())
    }

    /// Execute artifact on target via stream
    ///
    /// Non-blocking: sends command, awaits response via channel
    pub async fn execute_artifact(
        &self,
        id: &str,
        request: SampleRequest,
    ) -> Result<SampleResponse> {
        info!(
            "[{}] Executing artifact {} on target {} via stream",
            request.job_id, request.artifact_id, id
        );

        // Generate run_id for response matching
        let run_id = format!(
            "{}-{}-{}",
            request.job_id,
            request.artifact_id,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
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

        info!(
            "[{}] Sent RunSampleCommand to target {} [run_id: {}]",
            request.job_id, id, run_id
        );

        // Await response
        let timeout_duration = Duration::from_secs(request.timeout_seconds as u64 + 30);

        match tokio::time::timeout(timeout_duration, rx).await {
            Ok(Ok(response)) => {
                info!(
                    "[{}] Execution complete: success={}, exit_code={}",
                    request.job_id, response.success, response.exit_code
                );
                Ok(response)
            }
            Ok(Err(_)) => Err(anyhow!(
                "Response channel closed for run_id: {} (target {} disconnected?)",
                run_id,
                id
            )),
            Err(_) => Err(anyhow!(
                "Timeout waiting for response from target {} [run_id: {}, timeout: {}s]",
                id,
                run_id,
                timeout_duration.as_secs()
            )),
        }
    }

    /// Register pending execution (for stream response routing)
    fn register_pending_execution(&self, run_id: String) -> oneshot::Receiver<SampleResponse> {
        let (tx, rx) = oneshot::channel();

        let mut pending = self.pending_responses.lock().unwrap();
        pending.insert(run_id.clone(), tx);

        debug!("Registered pending execution: {}", run_id);
        rx
    }

    /// Complete pending execution (called by event handler when SampleResponse arrives)
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
    // Info queries (legacy RPC, for initial metadata fetch)
    // ========================================================================

    /// Get worker info via RPC (for initial metadata fetch)
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

    /// Query metadata from all targets
    pub async fn query_all_info(&self) -> HashMap<String, WorkerInfoResponse> {
        let ids = self.list_ids();
        let mut results = HashMap::new();

        for id in ids {
            match self.get_worker_info(&id).await {
                Ok(info) => {
                    results.insert(id, info);
                }
                Err(e) => {
                    warn!("Failed to query target {}: {}", id, e);
                }
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

        manager
            .register(TargetConfig {
                id: "test-1".to_string(),
                address: "127.0.0.1:50052".to_string(),
                enabled: true,
            })
            .unwrap();

        let target = manager.get("test-1").unwrap();
        assert_eq!(target.id, "test-1");
        assert_eq!(target.status, TargetStatus::Available);
        assert!(!target.connected);
    }

    #[tokio::test]
    async fn test_reserve_and_release() {
        let (tx, _rx) = mpsc::channel(100);
        let manager = TargetManager::new(30, 30, tx);

        manager
            .register(TargetConfig {
                id: "test-1".to_string(),
                address: "127.0.0.1:50052".to_string(),
                enabled: true,
            })
            .unwrap();

        manager.reserve("test-1").unwrap();
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Busy);

        manager.release("test-1").unwrap();
        assert_eq!(
            manager.get("test-1").unwrap().status,
            TargetStatus::Available
        );
    }

    #[tokio::test]
    async fn test_get_available_by_os_and_caps() {
        let (tx, _rx) = mpsc::channel(100);
        let manager = TargetManager::new(30, 30, tx);

        // Register two targets
        manager
            .register(TargetConfig {
                id: "win10-1".to_string(),
                address: "127.0.0.1:50052".to_string(),
                enabled: true,
            })
            .unwrap();

        manager
            .register(TargetConfig {
                id: "win11-1".to_string(),
                address: "127.0.0.1:50053".to_string(),
                enabled: true,
            })
            .unwrap();

        // Update metadata and mark connected
        manager
            .register_with_metadata(
                "win10-1".to_string(),
                "127.0.0.1:50052".to_string(),
                "win10".to_string(),
                vec!["mde".to_string(), "rededr".to_string()],
                HashMap::new(),
                HashMap::new(),
            )
            .unwrap();
        manager.mark_connected("win10-1").unwrap();

        manager
            .register_with_metadata(
                "win11-1".to_string(),
                "127.0.0.1:50053".to_string(),
                "win11".to_string(),
                vec!["defender".to_string()],
                HashMap::new(),
                HashMap::new(),
            )
            .unwrap();
        manager.mark_connected("win11-1").unwrap();

        // Query with no caps - should get both
        let all = manager.get_available_by_os_and_capabilities(&[]);
        assert_eq!(all.len(), 2);

        // Query with mde cap - only win10
        let with_mde = manager.get_available_by_os_and_capabilities(&["mde".to_string()]);
        assert_eq!(with_mde.len(), 1);
        assert!(with_mde.contains_key("win10"));

        // Query with defender cap - only win11
        let with_defender =
            manager.get_available_by_os_and_capabilities(&["defender".to_string()]);
        assert_eq!(with_defender.len(), 1);
        assert!(with_defender.contains_key("win11"));
    }
}