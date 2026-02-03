// Unified Target Manager - connection management and Worker spawning
//
// Handles:
// - Target registration (from TOML or dynamic)
// - gRPC connection/stream management
// - Target state (Available/Busy/Offline)
// - Spawning Worker tasks on connection
// - Artifact deployment

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tonic::transport::{Channel, Endpoint};
use tonic::Request;
use tracing::{debug, error, info, warn};

use crate::automutate::common::{
    controller_message, worker_message, ArtifactChunk, ControllerMessage, DisconnectNotice,
    Heartbeat, WorkerMessage,
};
use crate::automutate::worker::{
    worker_agent_client::WorkerAgentClient, WorkerInfoRequest, WorkerInfoResponse,
};
use crate::dispatch::{
    ArtifactSender, OrchestratorEvent, RemoteRunResult, RunId, Worker, WorkerCommand, WorkerEvent,
    WorkerId, WorkerInfo,
};
use crate::dispatch::pool_group::PoolGroupRegistry;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetStatus {
    Available,
    Busy,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrationType {
    Static,
    Dynamic,
}

/// Events emitted by TargetManager (for observability)
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

#[derive(Debug, Clone)]
pub struct TargetConfig {
    pub id: String,
    pub address: String,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub address: String,
    pub os_version: String,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub tools: HashMap<String, String>,
    pub status: TargetStatus,
    pub enabled: bool,
    pub registration_type: RegistrationType,
    pub current_job: Option<String>,
    pub last_seen: SystemTime,
    channel: Option<Channel>,
    stream_tx: Option<mpsc::Sender<ControllerMessage>>,
}

// ============================================================================
// Target Discovery
// ============================================================================

/// Target configuration from individual worker TOML files
#[derive(Debug, Clone, serde::Deserialize)]
struct TargetTomlConfig {
    worker: TargetInfo,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct TargetInfo {
    worker_id: String,
    ip_address: String,
}

/// Load target configuration from individual worker TOML file
fn load_target_config(path: &std::path::Path) -> anyhow::Result<(String, String)> {
    let content = std::fs::read_to_string(path)?;
    let config: TargetTomlConfig = toml::from_str(&content)?;

    // Target gRPC address is IP + port 50052 (standard worker port)
    let address = format!("{}:50052", config.worker.ip_address);

    Ok((config.worker.worker_id, address))
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
            status: TargetStatus::Offline,
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
}

// ============================================================================
// TargetManager
// ============================================================================

#[derive(Debug)]
pub struct TargetManager {
    targets: DashMap<String, Target>,
    events_tx: mpsc::Sender<TargetEvent>,
    orchestrator_tx: mpsc::Sender<OrchestratorEvent>,
    rpc_timeout: Duration,
    /// Shared pool group registry (workers with same capabilities share a pool)
    pool_registry: Arc<PoolGroupRegistry>,
}

impl TargetManager {
    pub fn new(
        rpc_timeout_secs: u64,
        events_tx: mpsc::Sender<TargetEvent>,
        orchestrator_tx: mpsc::Sender<OrchestratorEvent>,
        pool_registry: Arc<PoolGroupRegistry>,
    ) -> Self {
        Self {
            targets: DashMap::new(),
            events_tx,
            orchestrator_tx,
            rpc_timeout: Duration::from_secs(rpc_timeout_secs),
            pool_registry,
        }
    }

    /// Get the pool registry (for Orchestrator to use).
    #[allow(dead_code)]
    pub fn pool_registry(&self) -> Arc<PoolGroupRegistry> {
        Arc::clone(&self.pool_registry)
    }

    // ========================================================================
    // Registration
    // ========================================================================

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

    pub fn register_with_metadata(
        &self,
        id: String,
        address: String,
        os_version: String,
        capabilities: Vec<String>,
        metadata: HashMap<String, String>,
        tools: HashMap<String, String>,
    ) -> Result<()> {
        if let Some(mut target) = self.targets.get_mut(&id) {
            target.os_version = os_version.clone();
            target.capabilities = capabilities.clone();
            target.metadata = metadata;
            target.tools = tools;
            target.registration_type = RegistrationType::Dynamic;
            target.touch();
        } else {
            let mut target = Target::new(id.clone(), address);
            target.os_version = os_version.clone();
            target.capabilities = capabilities.clone();
            target.metadata = metadata;
            target.tools = tools;
            target.registration_type = RegistrationType::Dynamic;
            self.targets.insert(id.clone(), target);
        }
        info!("Target {} updated: OS={}, caps={:?}", id, os_version, capabilities);
        Ok(())
    }

    // ========================================================================
    // Queries (public API for management/monitoring)
    // ========================================================================

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub fn get_available(&self) -> Vec<String> {
        self.targets
            .iter()
            .filter(|t| t.status == TargetStatus::Available && t.enabled)
            .map(|t| t.id.clone())
            .collect()
    }

    #[allow(dead_code)]
    pub fn get_available_by_os_and_capabilities(
        &self,
        required_capabilities: &[String],
    ) -> HashMap<String, Vec<String>> {
        let mut by_os: HashMap<String, Vec<String>> = HashMap::new();
        for entry in self.targets.iter() {
            let t = entry.value();
            if t.status != TargetStatus::Available || !t.enabled {
                continue;
            }
            if !required_capabilities.is_empty() {
                let has_all = required_capabilities
                    .iter()
                    .all(|req| t.capabilities.iter().any(|cap| cap.eq_ignore_ascii_case(req)));
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
    // State Management
    // ========================================================================

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
        debug!("Target {} reserved", id);
        Ok(())
    }

    pub fn release(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;
        if target.status == TargetStatus::Offline {
            return Err(anyhow!("Target {} is offline, cannot release", id));
        }
        target.status = TargetStatus::Available;
        target.current_job = None;
        debug!("Target {} released", id);
        Ok(())
    }

    pub fn mark_connected(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;
        if target.status == TargetStatus::Offline {
            target.status = TargetStatus::Available;
        }
        target.touch();
        debug!("Target {} connected", id);
        Ok(())
    }

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
        target.stream_tx = None;
        debug!("Target {} offline", id);
        Ok(())
    }

    pub fn mark_disconnected(&self, id: &str) -> Result<()> {
        self.mark_offline(id)
    }

    pub fn update_health(&self, id: &str) -> Result<()> {
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;
        target.touch();
        Ok(())
    }

    // ========================================================================
    // Connection Management
    // ========================================================================

    async fn get_channel(&self, id: &str) -> Result<Channel> {
        if let Some(target) = self.targets.get(id) {
            if let Some(ref channel) = target.channel {
                return Ok(channel.clone());
            }
        }

        let address = self
            .targets
            .get(id)
            .map(|t| t.address.clone())
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        let url = format!("http://{}", address);
        debug!("Connecting to target {} at {}", id, url);

        let endpoint = Endpoint::try_from(url)?
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .tcp_keepalive(Some(Duration::from_secs(30)));

        let channel = endpoint.connect().await?;

        if let Some(mut target) = self.targets.get_mut(id) {
            target.channel = Some(channel.clone());
        }

        debug!("Connected to target {}", id);
        Ok(channel)
    }

    /// Establish bidirectional stream and spawn Worker task
    pub async fn establish_stream(self: &Arc<Self>, id: &str) -> Result<()> {
        debug!("Establishing stream with target {}", id);

        let channel = self.get_channel(id).await?;
        let mut client = WorkerAgentClient::new(channel);

        // Create channel for outgoing messages to remote VM
        let (stream_tx, stream_rx) = mpsc::channel::<ControllerMessage>(100);
        let outgoing = tokio_stream::wrappers::ReceiverStream::new(stream_rx);

        // Establish stream
        let response = client.establish_stream(Request::new(outgoing)).await?;
        let incoming = response.into_inner();

        debug!("Stream established with target {}", id);

        // Store stream sender
        if let Some(mut target) = self.targets.get_mut(id) {
            target.stream_tx = Some(stream_tx.clone());
            if target.status == TargetStatus::Offline {
                target.status = TargetStatus::Available;
            }
        }

        // Create channels for Worker
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCommand>(16);
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>(64);
        let (result_tx, result_rx) = mpsc::channel::<RemoteRunResult>(64);

        // Get worker info
        let worker_info = {
            let target = self.targets.get(id).ok_or_else(|| anyhow!("Target not found"))?;
            WorkerInfo {
                id: WorkerId(id.to_string()),
                os: target.os_version.clone(),
                capabilities: target.capabilities.clone(),
            }
        };

        // Spawn stream handler
        let id_clone = id.to_string();
        let events_tx = self.events_tx.clone();
        let targets = self.targets.clone();
        let result_tx_clone = result_tx.clone();

        tokio::spawn(async move {
            Self::stream_handler(id_clone, incoming, events_tx, targets, result_tx_clone).await;
        });

        // Create artifact sender
        let artifact_sender: Arc<dyn ArtifactSender + Send + Sync> =
            Arc::new(TargetArtifactSender {
                manager: Arc::clone(self),
            });

        // Get or create pool group for this worker
        let pool = self.pool_registry.get_or_create(&worker_info).await;
        debug!(
            "Worker {} assigned to pool group {}",
            id,
            pool.group_id()
        );

        // Spawn Worker task
        let worker = Worker::new(
            WorkerId(id.to_string()),
            worker_info.clone(),
            pool,
            cmd_rx,
            event_tx,
            stream_tx.clone(),
            result_rx,
            artifact_sender,
        );
        tokio::spawn(worker.run());

        // Notify Orchestrator
        let _ = self
            .orchestrator_tx
            .send(OrchestratorEvent::WorkerConnected {
                worker_id: WorkerId(id.to_string()),
                info: worker_info,
                cmd_tx,
                event_rx,
            })
            .await;

        // Spawn heartbeat
        self.spawn_heartbeat(id, stream_tx);

        Ok(())
    }

    async fn stream_handler(
        id: String,
        mut incoming: tonic::Streaming<WorkerMessage>,
        events_tx: mpsc::Sender<TargetEvent>,
        targets: DashMap<String, Target>,
        result_tx: mpsc::Sender<RemoteRunResult>,
    ) {
        debug!("Stream handler started for target {}", id);
        let mut registration_received = false;

        while let Some(result) = incoming.next().await {
            match result {
                Ok(msg) => {
                    // Update last_seen
                    if let Some(mut t) = targets.get_mut(&id) {
                        t.touch();

                        // Handle registration message
                        if !registration_received {
                            if let Some(worker_message::Payload::Registration(ref reg)) = msg.payload
                            {
                                t.capabilities = reg.capabilities.clone();
                                t.os_version = reg.os_version.clone();
                                registration_received = true;

                                let _ = events_tx
                                    .send(TargetEvent::Connected {
                                        target_id: id.clone(),
                                        os_version: reg.os_version.clone(),
                                        capabilities: reg.capabilities.clone(),
                                    })
                                    .await;
                            }
                        }
                    }

                    // Handle SampleResponse -> forward to Worker
                    if let Some(worker_message::Payload::SampleResponse(ref response)) = msg.payload
                    {
                        let result = RemoteRunResult {
                            run_id: RunId(response.run_id.clone()),
                            detected: response.detected,
                            exit_code: response.exit_code,
                            success: response.success,
                            error: if response.error.is_empty() {
                                None
                            } else {
                                Some(response.error.clone())
                            },
                        };
                        let _ = result_tx.send(result).await;
                    }

                    // Forward to event bus
                    let _ = events_tx
                        .send(TargetEvent::Message {
                            target_id: id.clone(),
                            msg,
                        })
                        .await;
                }
                Err(e) => {
                    error!("Stream error from target {}: {}", id, e);
                    break;
                }
            }
        }

        // Cleanup
        debug!("Stream closed for target {}", id);
        if let Some(mut target) = targets.get_mut(&id) {
            target.stream_tx = None;
            target.status = TargetStatus::Offline;
        }

        let _ = events_tx
            .send(TargetEvent::Disconnected {
                target_id: id.clone(),
                reason: "Stream closed".to_string(),
            })
            .await;

        // TODO: Send OrchestratorEvent::WorkerDisconnected to orchestrator_tx
        // Currently Orchestrator has on_worker_disconnected() but it's never called
        

        warn!("Target {} disconnected", id);
    }

    fn spawn_heartbeat(&self, id: &str, tx: mpsc::Sender<ControllerMessage>) {
        let id = id.to_string();
        let targets = self.targets.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            debug!("Heartbeat started for target {}", id);

            loop {
                interval.tick().await;

                let connected = targets
                    .get(&id)
                    .map(|t| t.status != TargetStatus::Offline)
                    .unwrap_or(false);

                if !connected {
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

                if tx.send(heartbeat).await.is_err() {
                    break;
                }
            }

            debug!("Heartbeat stopped for target {}", id);
        });
    }

    pub async fn establish_all_streams(self: &Arc<Self>) -> HashMap<String, Result<()>> {
        let ids = self.list_ids();
        let mut results = HashMap::new();
        for id in ids {
            let result = self.establish_stream(&id).await;
            results.insert(id, result);
        }
        results
    }

    pub async fn send_command(&self, id: &str, msg: ControllerMessage) -> Result<()> {
        let tx = self
            .targets
            .get(id)
            .and_then(|t| t.stream_tx.clone())
            .ok_or_else(|| anyhow!("Target {} not connected", id))?;

        match tx.send(msg).await {
            Ok(()) => Ok(()),
            Err(_) => {
                warn!("Send to target {} failed", id);
                if let Some(mut target) = self.targets.get_mut(id) {
                    target.stream_tx = None;
                    target.status = TargetStatus::Offline;
                }
                let _ = self
                    .events_tx
                    .send(TargetEvent::Disconnected {
                        target_id: id.to_string(),
                        reason: "Send failed".to_string(),
                    })
                    .await;
                Err(anyhow!("Target {} disconnected", id))
            }
        }
    }

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

    pub async fn disconnect_all(&self, reason: &str, reconnect_allowed: bool) {
        info!("Disconnecting all targets: {}", reason);
        let msg = ControllerMessage {
            payload: Some(controller_message::Payload::Disconnect(DisconnectNotice {
                reason: reason.to_string(),
                reconnect_allowed,
            })),
        };
        let sent = self.broadcast(msg).await;
        debug!("Sent disconnect to {}/{} targets", sent, self.count());
        tokio::time::sleep(Duration::from_millis(200)).await;
        for mut target in self.targets.iter_mut() {
            target.stream_tx = None;
            target.status = TargetStatus::Offline;
        }
    }

    // ========================================================================
    // Artifact Operations
    // ========================================================================

    pub async fn send_artifact(&self, id: &str, artifact_id: &str, path: &Path) -> Result<()> {
        debug!("[{}] Sending artifact to target {}...", artifact_id, id);

        let data = tokio::fs::read(path).await?;
        debug!("[{}] Artifact size: {} bytes", artifact_id, data.len());

        let channel = self.get_channel(id).await?;
        let mut client = WorkerAgentClient::new(channel);

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

        debug!(
            "[{}] Sending {} chunks to target {}",
            artifact_id, total_chunks, id
        );
        client.send_artifact(stream::iter(chunks)).await?;
        debug!("[{}] Artifact deployed to target {}", artifact_id, id);
        Ok(())
    }

    // ========================================================================
    // Info Queries
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
                Ok(info) => {
                    // Update target with queried info (so establish_stream has correct values)
                    if let Some(mut target) = self.targets.get_mut(&id) {
                        target.os_version = info.os_version.clone();
                        target.capabilities = info.capabilities.clone();
                        debug!("Updated target {} info: os={}, caps={:?}",
                            id, info.os_version, info.capabilities);
                    }
                    results.insert(id, info);
                }
                Err(e) => {
                    warn!("Failed to query target {}: {}", id, e);
                }
            }
        }
        results
    }


    /// Discover targets from automation/generated/win*-worker-*.toml files
    pub async fn discover_and_register_targets(&self) {
        use std::path::Path;
        use std::collections::HashMap as StdHashMap;

        let generated_dir = Path::new("automation/generated");

        if !generated_dir.exists() {
            warn!("automation/generated directory not found, no targets registered");
            warn!("Run 'automation/scripts/generate-configs.ps1' to create target configs");
            return;
        }

        let entries = match std::fs::read_dir(generated_dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to read automation/generated: {}", e);
                return;
            }
        };

        let mut target_count = 0;
        let mut duplicate_count = 0;
        let mut registered_ips: StdHashMap<String, (String, String)> = StdHashMap::new();

        for entry in entries.flatten() {
            let path = entry.path();
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Match pattern: win*-worker-*.toml
            if filename.starts_with("win") && filename.contains("-worker-") && filename.ends_with(".toml") {
                match load_target_config(&path) {
                    Ok((target_id, address)) => {
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
                        if let Err(e) = self.register(TargetConfig {
                            id: target_id.clone(),
                            address: address.clone(),
                            enabled: true,
                        }) {
                            warn!("Failed to register target {}: {}", target_id, e);
                            continue;
                        }

                        debug!("  Registered target: {} at {}", target_id, address);
                        registered_ips.insert(ip, (target_id.clone(), filename.to_string()));
                        target_count += 1;
                    }
                    Err(e) => {
                        warn!("Failed to load target config {}: {}", filename, e);
                    }
                }
            }
        }

        if duplicate_count > 0 {
            warn!("{} duplicate target config(s) were skipped (same IP)", duplicate_count);
        }

        if target_count == 0 {
            warn!("No targets registered! Scheduler will not be able to execute jobs.");
            warn!("Create target configs in automation/generated/ (e.g., win10-worker-01.toml)");
        } else {
            info!("Target pool initialized with {} unique targets", target_count);
        }
    }
}

// ============================================================================
// ArtifactSender implementation for TargetManager
// ============================================================================

#[derive(Debug)]
struct TargetArtifactSender {
    manager: Arc<TargetManager>,
}

impl ArtifactSender for TargetArtifactSender {
    fn send_artifact(
        &self,
        worker_id: &str,
        artifact_id: &str,
        path: &Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let worker_id = worker_id.to_string();
        let artifact_id = artifact_id.to_string();
        let path = path.to_path_buf();
        Box::pin(async move { self.manager.send_artifact(&worker_id, &artifact_id, &path).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::pool_group::PoolEvent;

    fn create_test_manager() -> TargetManager {
        let (events_tx, _) = mpsc::channel(100);
        let (orch_tx, _) = mpsc::channel(100);
        let (pool_event_tx, _) = mpsc::channel(100);
        let pool_registry = Arc::new(PoolGroupRegistry::new(pool_event_tx));
        TargetManager::new(30, events_tx, orch_tx, pool_registry)
    }

    #[test]
    fn test_target_status_display() {
        assert_eq!(TargetStatus::Available.to_string(), "available");
        assert_eq!(TargetStatus::Busy.to_string(), "busy");
        assert_eq!(TargetStatus::Offline.to_string(), "offline");
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let manager = create_test_manager();

        manager
            .register(TargetConfig {
                id: "test-1".to_string(),
                address: "127.0.0.1:50052".to_string(),
                enabled: true,
            })
            .unwrap();

        let target = manager.get("test-1").unwrap();
        assert_eq!(target.id, "test-1");
        assert_eq!(target.status, TargetStatus::Offline);
    }

    #[tokio::test]
    async fn test_reserve_and_release() {
        let manager = create_test_manager();

        manager
            .register(TargetConfig {
                id: "test-1".to_string(),
                address: "127.0.0.1:50052".to_string(),
                enabled: true,
            })
            .unwrap();

        manager.mark_connected("test-1").unwrap();
        assert_eq!(
            manager.get("test-1").unwrap().status,
            TargetStatus::Available
        );

        manager.reserve("test-1").unwrap();
        assert_eq!(manager.get("test-1").unwrap().status, TargetStatus::Busy);

        manager.release("test-1").unwrap();
        assert_eq!(
            manager.get("test-1").unwrap().status,
            TargetStatus::Available
        );
    }
}