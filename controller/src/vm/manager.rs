//! Target manager: registration, gRPC streams, state tracking, and artifact deployment.
//!
//! Manages the full lifecycle of worker VM targets, from TOML-based
//! registration through bidirectional gRPC streaming to graceful disconnect.
//! Spawns a [`VMExecutor`] per connected target
//! and provides artifact upload via chunked gRPC streaming.

use anyhow::{Result, anyhow};
use dashmap::DashMap;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, oneshot};
use tonic::Request;
use tonic::transport::{Channel, Endpoint};
use tracing::{debug, error, info, warn};

use crate::automutate::common::{
    ControllerMessage, DisconnectNotice, Heartbeat, WorkerMessage, controller_message,
    worker_message,
};
use crate::automutate::worker::{
    WorkerInfoRequest, WorkerInfoResponse, worker_agent_client::WorkerAgentClient,
};
use crate::dispatch::{
    ArtifactSender, JobId, RemoteRunResult, RunId, RunPool, TargetId, VMExecutor, VMInfo,
};

// ============================================================================
// Types
// ============================================================================

/// Lifecycle state of a worker VM target.
///
/// State machine transitions:
/// - `Offline` -> `Available` (on successful stream establishment)
/// - `Available` -> `Busy` (on job reservation)
/// - `Busy` -> `Available` (on job release)
/// - `Available` | `Busy` -> `Offline` (on disconnect or stream failure)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetStatus {
    /// Target is connected and ready to accept work.
    Available,
    /// Target is currently executing a job.
    Busy,
    /// Target is not reachable (no active gRPC stream).
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

/// How a target was registered with the controller.
///
/// - `Static` — loaded from a TOML file in `automation/generated/` at startup.
/// - `Dynamic` — registered at runtime via a gRPC registration message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrationType {
    /// Target was loaded from a TOML configuration file at startup.
    Static,
    /// Target registered itself via gRPC at runtime.
    Dynamic,
}

/// Events emitted by TargetManager (for observability)
#[derive(Debug, Clone)]
pub enum TargetEvent {
    Connected {
        target_id: TargetId,
        os_version: String,
        capabilities: Vec<String>,
    },
    Disconnected {
        target_id: TargetId,
        reason: String,
    },
    Message {
        target_id: TargetId,
        msg: WorkerMessage,
    },
}

/// Minimal target descriptor used during initial registration.
#[derive(Debug, Clone)]
pub struct TargetConfig {
    pub id: TargetId,
    pub address: String,
    pub enabled: bool,
}

/// Runtime representation of a registered worker VM.
#[derive(Clone, Debug)]
pub struct Target {
    pub id: TargetId,
    pub address: String,
    pub os_version: String,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub tools: HashMap<String, String>,
    pub status: TargetStatus,
    pub enabled: bool,
    pub registration_type: RegistrationType,
    pub current_job: Option<JobId>,
    pub last_seen: SystemTime,
    pub connected_at: Option<SystemTime>,
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
    #[serde(default = "default_listen_port")]
    listen_port: u16,
}

fn default_listen_port() -> u16 {
    50052
}

/// Load target configuration from individual worker TOML file
fn load_target_config(path: &Path) -> Result<(String, String)> {
    let content = std::fs::read_to_string(path)?;
    let config: TargetTomlConfig = toml::from_str(&content)?;

    let address = format!("{}:{}", config.worker.ip_address, config.worker.listen_port);

    Ok((config.worker.worker_id, address))
}

impl Target {
    fn new(id: TargetId, address: String) -> Self {
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
            connected_at: None,
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

/// Thread-safe manager for all registered worker VM targets.
#[derive(Debug)]
pub struct TargetManager {
    targets: DashMap<TargetId, Target>,
    events_tx: mpsc::Sender<TargetEvent>,
    rpc_timeout: Duration,
    run_pool: Arc<RunPool>,
}

impl TargetManager {
    /// Create a new target manager with the given RPC timeout and event channel.
    pub fn new(
        rpc_timeout_secs: u64,
        events_tx: mpsc::Sender<TargetEvent>,
        run_pool: Arc<RunPool>,
    ) -> Self {
        Self {
            targets: DashMap::new(),
            events_tx,
            rpc_timeout: Duration::from_secs(rpc_timeout_secs),
            run_pool,
        }
    }

    /// Get the run pool.
    #[allow(dead_code)]
    pub fn run_pool(&self) -> Arc<RunPool> {
        Arc::clone(&self.run_pool)
    }

    // ========================================================================
    // Registration
    // ========================================================================

    /// Register a new target from configuration.
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

    /// Register or update a target with full metadata from a worker registration message.
    pub fn register_with_metadata(
        &self,
        id: impl Into<TargetId>,
        address: String,
        os_version: String,
        capabilities: Vec<String>,
        metadata: HashMap<String, String>,
        tools: HashMap<String, String>,
    ) -> Result<()> {
        let id: TargetId = id.into();
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
        info!(
            "Target {} updated: OS={}, caps={:?}",
            id, os_version, capabilities
        );
        Ok(())
    }

    // ========================================================================
    // Queries (public API for management/monitoring)
    // ========================================================================

    /// Look up a target by ID.
    pub fn get(&self, id: impl AsRef<str>) -> Option<Target> {
        self.targets.get(id.as_ref()).map(|t| t.clone())
    }

    /// List all registered target IDs.
    pub fn list_ids(&self) -> Vec<TargetId> {
        self.targets.iter().map(|e| e.key().clone()).collect()
    }

    /// List all registered targets.
    pub fn list_all(&self) -> Vec<Target> {
        self.targets.iter().map(|e| e.clone()).collect()
    }

    /// Return the number of registered targets.
    pub fn count(&self) -> usize {
        self.targets.len()
    }

    /// Return IDs of targets in [`TargetStatus::Available`] state.
    #[allow(dead_code)]
    pub fn get_available(&self) -> Vec<TargetId> {
        self.targets
            .iter()
            .filter(|t| t.status == TargetStatus::Available && t.enabled)
            .map(|t| t.id.clone())
            .collect()
    }

    /// Return available targets grouped by OS, filtered by required capabilities.
    pub fn get_available_by_os_and_capabilities(
        &self,
        required_capabilities: &[String],
    ) -> HashMap<String, Vec<TargetId>> {
        let mut by_os: HashMap<String, Vec<TargetId>> = HashMap::new();
        for entry in self.targets.iter() {
            let t = entry.value();
            if t.status != TargetStatus::Available || !t.enabled {
                continue;
            }
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
    // State Management
    // ========================================================================

    /// Transition a target to [`TargetStatus::Busy`].
    pub fn reserve(&self, id: impl AsRef<str>) -> Result<()> {
        let id = id.as_ref();
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

    /// Transition a target back to [`TargetStatus::Available`].
    pub fn release(&self, id: impl AsRef<str>) -> Result<()> {
        let id = id.as_ref();
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

    /// Mark a target as connected (transitions from Offline to Available).
    pub fn mark_connected(&self, id: impl AsRef<str>) -> Result<()> {
        let id = id.as_ref();
        let mut target = self
            .targets
            .get_mut(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;
        if target.status == TargetStatus::Offline {
            target.status = TargetStatus::Available;
            target.connected_at = Some(SystemTime::now());
        }
        target.touch();
        debug!("Target {} connected", id);
        Ok(())
    }

    /// Mark a target as offline and clear its connection state.
    pub fn mark_offline(&self, id: impl AsRef<str>) -> Result<()> {
        let id = id.as_ref();
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
        target.channel = None;
        debug!("Target {} offline", id);
        Ok(())
    }

    /// Record a health heartbeat for a target (updates last-seen timestamp).
    pub fn update_health(&self, id: impl AsRef<str>) -> Result<()> {
        let id = id.as_ref();
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

    async fn get_channel(&self, id: impl AsRef<str>) -> Result<Channel> {
        let id = id.as_ref();
        if let Some(target) = self.targets.get(id)
            && let Some(ref channel) = target.channel
        {
            return Ok(channel.clone());
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

    /// Establish a bidirectional gRPC stream with a target and spawn its [`VMExecutor`].
    ///
    /// Opens a dedicated channel (without request timeout), starts the stream
    /// handler for incoming messages, and defers [`VMExecutor`] creation until
    /// the worker sends its registration message (15 s timeout). A heartbeat
    /// task is also spawned to keep the stream alive.
    ///
    /// # Errors
    ///
    /// Returns an error if the target is not found, the gRPC endpoint cannot
    /// be connected, or the bidirectional stream handshake fails.
    pub async fn establish_stream(self: &Arc<Self>, id: impl AsRef<str>) -> Result<()> {
        let id = id.as_ref();
        debug!("Establishing stream with target {}", id);

        // Build a dedicated channel for the long-lived bidi stream — NO request timeout.
        // get_channel() has a 10s timeout meant for unary RPCs, which would kill this stream
        // after 10s of idle time between messages.
        let address = self
            .targets
            .get(id)
            .map(|t| t.address.clone())
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;
        let stream_endpoint = Endpoint::try_from(format!("http://{}", address))?
            .connect_timeout(Duration::from_secs(5))
            .tcp_keepalive(Some(Duration::from_secs(30)));
        let stream_channel = stream_endpoint.connect().await?;
        let mut client = WorkerAgentClient::new(stream_channel);

        // Create channel for outgoing messages to remote VM (commands, heartbeats)
        let (stream_tx, stream_rx) = mpsc::channel::<ControllerMessage>(128);
        let outgoing = tokio_stream::wrappers::ReceiverStream::new(stream_rx);

        // Establish stream
        let response = client.establish_stream(Request::new(outgoing)).await?;
        let incoming = response.into_inner();

        debug!("Stream established with target {}", id);

        self.mark_connected(id)?;

        // Store stream_tx in Target so send_command() can use it
        if let Some(mut target) = self.targets.get_mut(id) {
            target.stream_tx = Some(stream_tx.clone());
        }

        // Create channel for results from VM (run completions)
        let (result_tx, result_rx) = mpsc::channel::<RemoteRunResult>(128);

        // Oneshot channel: stream_handler sends VMInfo once registration arrives
        let (reg_tx, reg_rx) = oneshot::channel::<VMInfo>();

        // Spawn stream handler (passes reg_tx for registration signal)
        let target_id = TargetId::from(id);
        let events_tx = self.events_tx.clone();
        let targets = self.targets.clone();
        let result_tx_clone = result_tx.clone();

        tokio::spawn(async move {
            Self::stream_handler(
                target_id,
                incoming,
                events_tx,
                targets,
                result_tx_clone,
                Some(reg_tx),
            )
            .await;
        });

        // Create artifact sender
        let artifact_sender: Arc<dyn ArtifactSender + Send + Sync> =
            Arc::new(TargetArtifactSender {
                manager: Arc::clone(self),
            });

        // Spawn deferred VMExecutor: waits for registration, then starts
        let vm_id = id.to_string();
        let manager_ref = Arc::clone(self);
        let run_pool = Arc::clone(&self.run_pool);
        let stream_tx_exec = stream_tx.clone();

        tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_secs(15), reg_rx).await {
                Ok(Ok(vm_info)) => {
                    debug!(
                        "VM {} registration received (os={}, caps={:?}), starting executor",
                        vm_id, vm_info.os, vm_info.capabilities
                    );
                    let executor = VMExecutor::new(
                        vm_id,
                        vm_info,
                        manager_ref,
                        run_pool,
                        stream_tx_exec,
                        result_rx,
                        artifact_sender,
                    );
                    executor.run().await;
                }
                Ok(Err(_)) => {
                    warn!(
                        "VM {} stream closed before registration, executor not started",
                        vm_id
                    );
                }
                Err(_) => {
                    warn!(
                        "VM {} registration timeout (15s), executor not started",
                        vm_id
                    );
                }
            }
        });

        // Spawn heartbeat
        self.spawn_heartbeat(id, stream_tx);

        Ok(())
    }

    async fn stream_handler(
        id: TargetId,
        mut incoming: tonic::Streaming<WorkerMessage>,
        events_tx: mpsc::Sender<TargetEvent>,
        targets: DashMap<TargetId, Target>,
        result_tx: mpsc::Sender<RemoteRunResult>,
        mut reg_tx: Option<oneshot::Sender<VMInfo>>,
    ) {
        debug!("Stream handler started for target {}", id);
        let mut registration_received = false;

        while let Some(result) = incoming.next().await {
            match result {
                Ok(msg) => {
                    // Update last_seen
                    if let Some(mut t) = targets.get_mut(&id) {
                        t.touch();

                        // Handle registration message -> send Connected event
                        if !registration_received
                            && let Some(worker_message::Payload::Registration(ref reg)) =
                                msg.payload
                        {
                            t.capabilities = reg.capabilities.clone();
                            t.os_version = reg.os_version.clone();
                            registration_received = true;

                            // Signal VMExecutor spawn with live capabilities
                            if let Some(tx) = reg_tx.take() {
                                let _ = tx.send(VMInfo {
                                    id: id.as_str().to_string(),
                                    os: reg.os_version.clone(),
                                    capabilities: reg.capabilities.clone(),
                                });
                            }

                            let _ = events_tx
                                .send(TargetEvent::Connected {
                                    target_id: id.clone(),
                                    os_version: reg.os_version.clone(),
                                    capabilities: reg.capabilities.clone(),
                                })
                                .await;
                        }
                    }

                    // Handle SampleResponse -> forward to VMExecutor
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
                            elapsed_ms: response.elapsed_ms,
                            detection_verdict: response.detection_verdict.clone(),
                            last_checkpoint: response.last_checkpoint.clone(),
                        };
                        let _ = result_tx.send(result).await;
                    }

                    // Forward to Orchestrator (for telemetry/ES indexing)
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
            target.channel = None;
            target.status = TargetStatus::Offline;
        }

        // Send Disconnected event to Orchestrator
        let _ = events_tx
            .send(TargetEvent::Disconnected {
                target_id: id.clone(),
                reason: "Stream closed".to_string(),
            })
            .await;

        warn!("Target {} disconnected", id);
    }

    fn spawn_heartbeat(&self, id: impl AsRef<str>, tx: mpsc::Sender<ControllerMessage>) {
        let id = TargetId::from(id.as_ref());
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
                        timestamp: crate::storage::helpers::now_unix_secs(),
                    })),
                };

                if tx.send(heartbeat).await.is_err() {
                    break;
                }
            }

            debug!("Heartbeat stopped for target {}", id);
        });
    }

    /// Establish gRPC streams for all registered targets.
    pub async fn establish_all_streams(self: &Arc<Self>) -> HashMap<TargetId, Result<()>> {
        let ids = self.list_ids();
        let mut results = HashMap::new();
        for id in ids {
            let result = self.establish_stream(&id).await;
            results.insert(id, result);
        }
        results
    }

    /// Spawn a background task that periodically attempts to reconnect offline targets.
    pub fn spawn_reconnect_loop(self: &Arc<Self>, interval_secs: u64) {
        if interval_secs == 0 {
            info!("Reconnect loop disabled (interval=0)");
            return;
        }
        let manager = Arc::clone(self);
        info!("Reconnect loop started (interval={}s)", interval_secs);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            interval.tick().await; // skip first immediate tick (startup just ran)
            loop {
                interval.tick().await;
                let offline_ids: Vec<TargetId> = manager
                    .targets
                    .iter()
                    .filter(|e| e.value().status == TargetStatus::Offline && e.value().enabled)
                    .map(|e| e.key().clone())
                    .collect();
                if offline_ids.is_empty() {
                    continue;
                }
                info!(
                    "Reconnect: {} offline target(s): {:?}",
                    offline_ids.len(),
                    offline_ids
                );
                for id in &offline_ids {
                    info!("Reconnecting target {}...", id);
                    match manager.establish_stream(id).await {
                        Ok(()) => info!("Reconnected target {} successfully", id),
                        Err(e) => warn!("Reconnection failed for {}: {}", id, e),
                    }
                }
            }
        });
    }

    /// Send a control message to a specific target's gRPC stream.
    ///
    /// If the send fails (channel closed), the target is automatically
    /// transitioned to [`TargetStatus::Offline`] and a `Disconnected` event
    /// is emitted.
    ///
    /// # Errors
    ///
    /// Returns an error if the target is not connected or the channel send fails.
    pub async fn send_command(&self, id: impl AsRef<str>, msg: ControllerMessage) -> Result<()> {
        let id = id.as_ref();
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
                        target_id: TargetId::from(id),
                        reason: "Send failed".to_string(),
                    })
                    .await;
                Err(anyhow!("Target {} disconnected", id))
            }
        }
    }

    /// Broadcast a control message to all connected targets.
    pub async fn broadcast(&self, msg: ControllerMessage) -> usize {
        let ids = self.list_ids();
        let mut success = 0;
        for id in &ids {
            if self.send_command(id, msg.clone()).await.is_ok() {
                success += 1;
            }
        }
        success
    }

    /// Disconnect all targets, optionally disabling reconnection.
    pub async fn disconnect_all(&self, reason: &str, reconnect_allowed: bool) {
        info!("Disconnecting all targets: {}", reason);

        // Signal graceful shutdown to all VMExecutors via run pool
        self.run_pool.shutdown();

        // Give executors a moment to finish current runs
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send disconnect notice to VMs
        let msg = ControllerMessage {
            payload: Some(controller_message::Payload::Disconnect(DisconnectNotice {
                reason: reason.to_string(),
                reconnect_allowed,
            })),
        };
        let sent = self.broadcast(msg).await;
        debug!("Sent disconnect to {}/{} targets", sent, self.count());
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Mark all targets offline
        for mut target in self.targets.iter_mut() {
            target.stream_tx = None;
            target.channel = None;
            target.status = TargetStatus::Offline;
        }
    }

    /// Disconnect a single target by sending a `DisconnectNotice` and marking it offline.
    ///
    /// The disconnect notice is sent on a best-effort basis; the target is
    /// marked offline regardless of whether the send succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error if the target is not found or is already offline.
    pub async fn disconnect_one(&self, id: impl AsRef<str>, reason: &str) -> Result<()> {
        let id = id.as_ref();
        let target = self
            .targets
            .get(id)
            .ok_or_else(|| anyhow!("Target not found: {}", id))?;

        if target.status == TargetStatus::Offline {
            return Err(anyhow!("Target {} is already offline", id));
        }

        if target.status == TargetStatus::Busy {
            warn!(
                "Disconnecting target {} while busy (job: {:?})",
                id, target.current_job
            );
        }
        drop(target);

        // Send disconnect notice
        let msg = ControllerMessage {
            payload: Some(controller_message::Payload::Disconnect(DisconnectNotice {
                reason: reason.to_string(),
                reconnect_allowed: true,
            })),
        };

        // Best-effort send; mark offline regardless
        let _ = self.send_command(id, msg).await;
        self.mark_offline(id)?;

        info!("Disconnected target {}: {}", id, reason);
        Ok(())
    }

    // ========================================================================
    // Artifact Operations
    // ========================================================================

    /// Upload an artifact to a target via chunked gRPC streaming.
    ///
    /// Reads the file at `path`, splits it into chunks, and streams them to
    /// the worker's `SendArtifact` RPC endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, the gRPC channel cannot
    /// be established, or the streaming upload fails.
    pub async fn send_artifact(
        &self,
        id: impl AsRef<str>,
        artifact_id: &str,
        path: &Path,
    ) -> Result<()> {
        let id = id.as_ref();
        debug!("[{}] Sending artifact to target {}...", artifact_id, id);

        let data = tokio::fs::read(path).await?;
        debug!("[{}] Artifact size: {} bytes", artifact_id, data.len());

        let channel = self.get_channel(id).await?;
        let mut client = WorkerAgentClient::new(channel);

        let chunks = crate::dispatch::types::chunk_artifact(artifact_id, &data);

        debug!(
            "[{}] Sending {} chunks to target {}",
            artifact_id,
            chunks.len(),
            id
        );
        client.send_artifact(stream::iter(chunks)).await?;
        debug!("[{}] Artifact deployed to target {}", artifact_id, id);
        Ok(())
    }

    // ========================================================================
    // Info Queries
    // ========================================================================

    /// Query a target for its worker info via unary RPC.
    ///
    /// Uses the configured `rpc_timeout` for the request.
    ///
    /// # Errors
    ///
    /// Returns an error if the target is not found, the gRPC channel cannot be
    /// established, the RPC times out, or the remote call returns an error.
    pub async fn get_worker_info(&self, id: impl AsRef<str>) -> Result<WorkerInfoResponse> {
        let id = id.as_ref();
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

    /// Query all registered targets for their worker info.
    pub async fn query_all_info(&self) -> HashMap<TargetId, WorkerInfoResponse> {
        let ids = self.list_ids();
        let mut results = HashMap::new();
        for id in ids {
            match self.get_worker_info(&id).await {
                Ok(info) => {
                    // Update target with queried info (so establish_stream has correct values)
                    if let Some(mut target) = self.targets.get_mut(&id) {
                        target.os_version = info.os_version.clone();
                        target.capabilities = info.capabilities.clone();
                        debug!(
                            "Updated target {} info: os={}, caps={:?}",
                            id, info.os_version, info.capabilities
                        );
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

    /// Discover and register targets from `automation/generated/win*-worker-*.toml` files.
    ///
    /// Scans the `automation/generated/` directory for worker TOML configs,
    /// validates for duplicate IPs, and registers each unique target. Logs
    /// warnings for missing directories, parse failures, and duplicates.
    pub async fn discover_and_register_targets(&self) {
        use std::collections::HashMap as StdHashMap;
        use std::path::Path;

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
            if filename.starts_with("win")
                && filename.contains("-worker-")
                && filename.ends_with(".toml")
            {
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
                            id: TargetId::from(target_id.as_str()),
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
            warn!(
                "{} duplicate target config(s) were skipped (same IP)",
                duplicate_count
            );
        }

        if target_count == 0 {
            warn!("No targets registered! Scheduler will not be able to execute jobs.");
            warn!("Create target configs in automation/generated/ (e.g., win10-worker-01.toml)");
        } else {
            info!(
                "Target pool initialized with {} unique targets",
                target_count
            );
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
        Box::pin(async move {
            self.manager
                .send_artifact(&worker_id, &artifact_id, &path)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager() -> TargetManager {
        let (events_tx, _) = mpsc::channel(100);
        let run_pool = Arc::new(RunPool::new());
        TargetManager::new(30, events_tx, run_pool)
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
                id: TargetId::from("test-1"),
                address: "127.0.0.1:50052".to_string(),
                enabled: true,
            })
            .unwrap();

        let target = manager.get("test-1").unwrap();
        assert_eq!(target.id.as_str(), "test-1");
        assert_eq!(target.status, TargetStatus::Offline);
    }

    #[tokio::test]
    async fn test_reserve_and_release() {
        let manager = create_test_manager();

        manager
            .register(TargetConfig {
                id: TargetId::from("test-1"),
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
