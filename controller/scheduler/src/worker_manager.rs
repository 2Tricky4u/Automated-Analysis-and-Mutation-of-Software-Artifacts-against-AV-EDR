// Manages outbound gRPC connections from controller to workers

use anyhow::{Result, anyhow, Error};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use std::path::Path;
use tokio::time::{Duration, timeout};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Streaming};
use tracing::{info, warn, error, debug};
use futures::stream::{self, StreamExt};

// Import generated protobuf types
use crate::automutate::worker::{
    worker_agent_client::WorkerAgentClient,
    WorkerInfoRequest,
    WorkerInfoResponse,
    TelemetryRequest,
};
use crate::automutate::common::{
    TelemetryData,
    ControllerMessage,
    WorkerMessage,
    controller_message,
    worker_message,
    RunSampleCommand,
    Heartbeat,
    DisconnectNotice,
    SampleRequest,
    SampleResponse,
    ArtifactChunk,
};
use crate::automutate::common::worker_message::Payload;

/// Events emitted by WorkerManager to central orchestration loop
/// Mirror of worker's outgoing message types
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// Worker connected and sent Registration
    Connected {
        worker_id: String,
        os_version: String,
        capabilities: Vec<String>,
    },

    /// Worker disconnected (stream closed or send failed)
    Disconnected {
        worker_id: String,
        reason: String,
    },

    /// Worker sent a message
    Message {
        worker_id: String,
        msg: WorkerMessage,
    },
}

impl WorkerEvent {
    /// Helper: Create Connected event
    pub fn connected(worker_id: impl Into<String>, os_version: impl Into<String>, capabilities: Vec<String>) -> Self {
        WorkerEvent::Connected {
            worker_id: worker_id.into(),
            os_version: os_version.into(),
            capabilities,
        }
    }

    /// Helper: Create Disconnected event
    pub fn disconnected(worker_id: impl Into<String>, reason: impl Into<String>) -> Self {
        WorkerEvent::Disconnected {
            worker_id: worker_id.into(),
            reason: reason.into(),
        }
    }

    /// Helper: Create Message event
    pub fn message(worker_id: impl Into<String>, msg: WorkerMessage) -> Self {
        WorkerEvent::Message {
            worker_id: worker_id.into(),
            msg,
        }
    }
}

/// Configuration for a single worker connection
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Worker ID (e.g., "win10-worker-01")
    pub id: String,
    /// Worker listen address (e.g., "10.200.200.11:50052")
    pub address: String,
    /// Whether worker is enabled
    pub enabled: bool,
}

/// Active session handle (simplified from WorkerConnection)
/// Stores only stream-specific state
#[derive(Clone)]
pub struct SessionHandle {
    pub worker_id: String,
    pub tx: mpsc::Sender<ControllerMessage>,
    pub last_seen: std::time::SystemTime,
    pub capabilities: Vec<String>,
}

impl SessionHandle {
    pub fn new(worker_id: String, tx: mpsc::Sender<ControllerMessage>) -> Self {
        Self {
            worker_id,
            tx,
            last_seen: std::time::SystemTime::now(),
            capabilities: Vec::new(),
        }
    }

    pub fn touch(&mut self) {
        self.last_seen = std::time::SystemTime::now();
    }

    pub fn is_stale(&self, timeout_secs: u64) -> bool {
        std::time::SystemTime::now()
            .duration_since(self.last_seen)
            .map(|d| d.as_secs() > timeout_secs)
            .unwrap_or(true)
    }
}

/// Manages a single worker connection
struct WorkerConnection {
    /// Worker ID
    id: String,
    /// Worker gRPC address
    address: String,
    /// Persistent gRPC channel (connection reuse)
    channel: Option<Channel>,
    /// Last successful connection time
    last_connected: Option<std::time::SystemTime>,
    /// Connection retry count
    retry_count: u32,
    /// Channel for sending messages to worker via stream
    stream_tx: Option<mpsc::Sender<ControllerMessage>>,
    /// Handle for the stream receiver task
    stream_rx_handle: Option<JoinHandle<()>>,
    /// Whether stream is currently active
    stream_active: bool,
}

impl WorkerConnection {
    fn new(id: String, address: String) -> Self {
        Self {
            id,
            address,
            channel: None,
            last_connected: None,
            retry_count: 0,
            stream_tx: None,
            stream_rx_handle: None,
            stream_active: false,
        }
    }

    /// Establish or return existing gRPC channel
    async fn get_channel(&mut self) -> Result<Channel> {
        // Return existing channel if valid
        if let Some(ref channel) = self.channel {
            return Ok(channel.clone());
        }

        // Create new connection
        let worker_url = format!("http://{}", self.address);
        debug!("Connecting to worker {} at {}", self.id, worker_url);

        let endpoint = Endpoint::try_from(worker_url)
            .map_err(|e| anyhow!("Invalid endpoint for worker {}: {}", self.id, e))?
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .tcp_keepalive(Some(Duration::from_secs(30)));

        let channel = endpoint.connect().await
            .map_err(|e| anyhow!("Failed to connect to worker {}: {}", self.id, e))?;

        self.channel = Some(channel.clone());
        self.last_connected = Some(std::time::SystemTime::now());
        self.retry_count = 0;

        info!("Successfully connected to worker {}", self.id);
        Ok(channel)
    }

    /// Clear channel on error (forces reconnection)
    fn clear_channel(&mut self) {
        self.channel = None;
        self.retry_count += 1;
    }
}

/// Worker Manager - Manages all outbound worker connections
pub struct WorkerManager {
    /// Legacy connections for unary RPCs (backwards compat)
    /// TODO: Remove after full migration to streaming
    connections: Arc<Mutex<HashMap<String, WorkerConnection>>>,

    /// Active worker sessions (streaming)
    sessions: DashMap<String, SessionHandle>,

    /// Event bus for worker events
    events_tx: mpsc::Sender<WorkerEvent>,

    /// Default RPC timeout
    rpc_timeout: Duration,
}

impl WorkerManager {
    /// Create new WorkerManager with event bus
    pub fn new(rpc_timeout_secs: u64, events_tx: mpsc::Sender<WorkerEvent>) -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            sessions: DashMap::new(),
            events_tx,
            rpc_timeout: Duration::from_secs(rpc_timeout_secs),
        }
    }

    /// Add a worker to the manager
    pub fn add_worker(&self, config: WorkerConfig) -> Result<()> {
        let mut connections = self.connections.lock().unwrap();

        if connections.contains_key(&config.id) {
            warn!("Worker {} already exists, replacing configuration", config.id);
        }

        let connection = WorkerConnection::new(config.id.clone(), config.address.clone());
        connections.insert(config.id.clone(), connection);

        debug!("Added worker {} at {}", config.id, config.address);
        Ok(())
    }

    /// Remove a worker from the manager
    pub async fn remove_worker(&self, worker_id: &str) -> Result<()> {
        // Send disconnect notification first (if stream is active)
        let disconnect_msg = ControllerMessage {
            payload: Some(controller_message::Payload::Disconnect(DisconnectNotice {
                reason: "Worker removal requested".to_string(),
                reconnect_allowed: false,
            })),
        };

        // Try to send disconnect notification (best effort, don't fail if stream is already dead)
        if let Err(e) = self.send_command(worker_id, disconnect_msg).await {
            warn!("Could not send disconnect notification to worker {}: {}", worker_id, e);
        } else {
            info!("Sent disconnect notification to worker {}", worker_id);
            // Give worker a moment to process the disconnect
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Remove session (this will cause heartbeat to stop)
        self.sessions.remove(worker_id);

        // Remove from connections
        let mut connections = self.connections.lock().unwrap();

        if connections.remove(worker_id).is_some() {
            info!("Removed worker {}", worker_id);
            Ok(())
        } else {
            Err(anyhow!("Worker {} not found", worker_id))
        }
    }

    /// Register workers from SchedulerCore discovery
    /// Called by SchedulerCore after discovering workers from automation/generated/*.toml
    /// This ensures single source of truth for worker registration
    pub fn register_from_pool(&self, workers: Vec<WorkerConfig>) -> Result<()> {
        debug!("Registering {} workers from SchedulerCore discovery", workers.len());

        for worker in workers {
            self.add_worker(worker)?;
        }

        Ok(())
    }

    /// Get list of registered worker addresses for discovery sync
    /// Returns (worker_id, address) pairs
    pub fn get_worker_addresses(&self) -> Vec<(String, String)> {
        let connections = self.connections.lock().unwrap();
        connections.iter()
            .map(|(id, conn)| (id.clone(), conn.address.clone()))
            .collect()
    }

    /// Get worker metadata by calling GetWorkerInfo RPC
    pub async fn get_worker_info(&self, worker_id: &str) -> Result<WorkerInfoResponse> {
        // Get or create channel
        let channel = {
            let mut connections = self.connections.lock().unwrap();
            let connection = connections.get_mut(worker_id)
                .ok_or_else(|| anyhow!("Worker {} not found in manager", worker_id))?;

            connection.get_channel().await?
        };

        // Create client and make RPC call
        let mut client = WorkerAgentClient::new(channel.clone());

        let request = tonic::Request::new(WorkerInfoRequest {});

        let response = timeout(
            self.rpc_timeout,
            client.get_worker_info(request)
        ).await
            .map_err(|_| anyhow!("GetWorkerInfo RPC timeout for worker {}", worker_id))?
            .map_err(|e| anyhow!("GetWorkerInfo RPC failed for worker {}: {}", worker_id, e))?;

        Ok(response.into_inner())
    }

    /// Get telemetry from worker by calling GetTelemetry RPC (streaming)
    pub async fn get_telemetry(
        &self,
        worker_id: &str,
        job_id: &str,
        since_timestamp: i64,
        max_events: i32,
    ) -> Result<Vec<TelemetryData>> {
        // Get or create channel
        let channel = {
            let mut connections = self.connections.lock().unwrap();
            let connection = connections.get_mut(worker_id)
                .ok_or_else(|| anyhow!("Worker {} not found in manager", worker_id))?;

            connection.get_channel().await?
        };

        // Create client and make RPC call
        let mut client = WorkerAgentClient::new(channel.clone());

        let request = tonic::Request::new(TelemetryRequest {
            job_id: job_id.to_string(),
            since_timestamp,
            max_events,
        });

        // Stream telemetry data
        let mut stream = timeout(
            self.rpc_timeout,
            client.get_telemetry(request)
        ).await
            .map_err(|_| anyhow!("GetTelemetry RPC timeout for worker {}", worker_id))?
            .map_err(|e| anyhow!("GetTelemetry RPC failed for worker {}: {}", worker_id, e))?
            .into_inner();

        // Collect all telemetry events from stream
        let mut telemetry_events = Vec::new();

        while let Some(event) = stream.message().await
            .map_err(|e| anyhow!("Error reading telemetry stream from worker {}: {}", worker_id, e))? {
            telemetry_events.push(event);
        }

        info!("Collected {} telemetry events from worker {} for job {}",
              telemetry_events.len(), worker_id, job_id);

        Ok(telemetry_events)
    }

    /// Query worker metadata from all managed workers
    /// Returns map of worker_id -> WorkerInfoResponse
    pub async fn query_all_workers(&self) -> HashMap<String, WorkerInfoResponse> {
        let worker_ids: Vec<String> = {
            let connections = self.connections.lock().unwrap();
            connections.keys().cloned().collect()
        };

        let mut results = HashMap::new();

        for worker_id in worker_ids {
            match self.get_worker_info(&worker_id).await {
                Ok(info) => {
                    results.insert(worker_id.clone(), info);
                }
                Err(e) => {
                    warn!("Failed to query worker {}: {}", worker_id, e);
                    // Clear channel on error
                    let mut connections = self.connections.lock().unwrap();
                    if let Some(connection) = connections.get_mut(&worker_id) {
                        connection.clear_channel();
                    }
                }
            }
        }

        results
    }

    /// Check if worker connection is healthy (channel exists and recent)
    /// Check if worker has active session (streaming connection)
    ///
    /// # Staleness Check
    /// Returns false if session exists but hasn't sent messages in 5 minutes
    pub fn is_worker_connected(&self, worker_id: &str) -> bool {
        // Check for active session first
        if let Some(session) = self.sessions.get(worker_id) {
            return !session.is_stale(300);  // 5 min timeout
        }

        // Fallback: Check legacy connection
        let connections = self.connections.lock().unwrap();
        if let Some(connection) = connections.get(worker_id) {
            if connection.channel.is_some() {
                if let Some(last_connected) = connection.last_connected {
                    let elapsed = std::time::SystemTime::now()
                        .duration_since(last_connected)
                        .unwrap_or(Duration::from_secs(999));
                    return elapsed < Duration::from_secs(300);
                }
            }
        }

        false
    }

    /// Get list of all worker IDs
    ///
    /// Returns union of streaming sessions and legacy connections
    pub fn list_workers(&self) -> Vec<String> {
        // Return workers with active sessions (streaming)
        let mut workers: Vec<String> = self.sessions.iter()
            .map(|entry| entry.key().clone())
            .collect();

        // Add legacy connections not yet migrated to streams
        let legacy_connections = self.connections.lock().unwrap();
        for worker_id in legacy_connections.keys() {
            if !workers.contains(worker_id) {
                workers.push(worker_id.clone());
            }
        }

        workers
    }

    /// Get connection stats for a worker
    pub fn get_worker_stats(&self, worker_id: &str) -> Option<(bool, u32)> {
        let connections = self.connections.lock().unwrap();

        connections.get(worker_id).map(|conn| {
            let connected = conn.channel.is_some();
            let retry_count = conn.retry_count;
            (connected, retry_count)
        })
    }

    /// Force reconnection for a worker (clears existing channel)
    pub fn force_reconnect(&self, worker_id: &str) -> Result<()> {
        let mut connections = self.connections.lock().unwrap();

        let connection = connections.get_mut(worker_id)
            .ok_or_else(|| anyhow!("Worker {} not found", worker_id))?;

        connection.clear_channel();
        info!("Cleared connection for worker {} - will reconnect on next RPC", worker_id);

        Ok(())
    }

    // ===== Bidirectional Streaming Support =====

    /// Establish bidirectional stream with a worker
    pub async fn establish_stream(&self, worker_id: &str) -> Result<()> {
        debug!("Establishing bidirectional stream with worker: {}", worker_id);

        // Get channel - split lock acquisition from async operation
        let channel = {
            // Get worker address from lock
            let worker_address = {
                let connections = self.connections.lock().unwrap();
                let connection = connections.get(worker_id)
                    .ok_or_else(|| anyhow!("Worker {} not found in manager", worker_id))?;
                connection.address.clone()
            };  // Lock dropped here

            // Check if channel exists, or create new one (with await, no lock held)
            let existing_channel = {
                let connections = self.connections.lock().unwrap();
                connections.get(worker_id).and_then(|c| c.channel.clone())
            };  // Lock dropped here

            if let Some(ch) = existing_channel {
                ch
            } else {
                // Create new connection (async operation, no lock held)
                let worker_url = format!("http://{}", worker_address);
                debug!("Connecting to worker {} at {}", worker_id, worker_url);

                let endpoint = Endpoint::try_from(worker_url)
                    .map_err(|e| anyhow!("Invalid endpoint for worker {}: {}", worker_id, e))?
                    .timeout(Duration::from_secs(10))
                    .connect_timeout(Duration::from_secs(5))
                    .tcp_keepalive(Some(Duration::from_secs(30)));

                let channel = endpoint.connect().await
                    .map_err(|e| anyhow!("Failed to connect to worker {}: {}", worker_id, e))?;

                // Store channel (brief lock, no await)
                {
                    let mut connections = self.connections.lock().unwrap();
                    if let Some(connection) = connections.get_mut(worker_id) {
                        connection.channel = Some(channel.clone());
                        connection.last_connected = Some(std::time::SystemTime::now());
                        connection.retry_count = 0;
                    }
                }  // Lock dropped here

                debug!("Successfully connected to worker {}", worker_id);
                channel
            }
        };

        // Create client
        let mut client = WorkerAgentClient::new(channel.clone());

        // Create channel for outgoing messages (controller -> worker)
        let (tx, rx) = mpsc::channel::<ControllerMessage>(100);

        // Convert receiver to stream
        let outgoing = tokio_stream::wrappers::ReceiverStream::new(rx);

        // Establish stream
        let response = client.establish_stream(Request::new(outgoing)).await
            .map_err(|e| anyhow!("Failed to establish stream with worker {}: {}", worker_id, e))?;

        let mut incoming = response.into_inner();

        debug!("Stream established with worker {}", worker_id);

        // Register session in DashMap early
        self.sessions.insert(worker_id.to_string(), SessionHandle::new(
            worker_id.to_string(),
            tx.clone(),
        ));

        // Spawn task to handle incoming messages from worker
        let worker_id_clone = worker_id.to_string();
        let connections_clone = Arc::clone(&self.connections);
        let events_tx = self.events_tx.clone();
        let sessions = self.sessions.clone();

        let handle = tokio::spawn(async move {
            info!("Stream message handler started for worker {}", worker_id_clone);

            // Track first message (Registration)
            let mut registration_received = false;

            while let Some(result) = incoming.next().await {
                match result {
                    Ok(msg) => {
                        // Update last_seen timestamp
                        if let Some(mut session) = sessions.get_mut(&worker_id_clone) {
                            session.touch();

                            // Extract capabilities from Registration
                            if !registration_received {
                                if let Some(worker_message::Payload::Registration(ref reg)) = msg.payload {
                                    session.capabilities = reg.capabilities.clone();
                                    registration_received = true;

                                    let _ = events_tx.send(WorkerEvent::connected(
                                        &worker_id_clone,
                                        &reg.os_version,
                                        reg.capabilities.clone(),
                                    )).await;

                                    debug!("Worker {} connected (OS: {}, Caps: {:?})",
                                        worker_id_clone, reg.os_version, reg.capabilities);
                                }
                            }
                        }

                        // Forward ALL messages to event bus
                        if let Err(e) = events_tx.send(WorkerEvent::message(&worker_id_clone, msg)).await {
                            error!("Failed to forward message to event bus: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Stream error from worker {}: {}", worker_id_clone, e);
                        break;
                    }
                }
            }

            // Cleanup on disconnect
            info!("Stream closed for worker {}", worker_id_clone);
            sessions.remove(&worker_id_clone);

            // Emit Disconnected event
            let _ = events_tx.send(WorkerEvent::disconnected(
                &worker_id_clone,
                "Stream closed"
            )).await;

            // Mark stream as inactive in legacy connections
            let mut connections = connections_clone.lock().unwrap();
            if let Some(connection) = connections.get_mut(&worker_id_clone) {
                connection.stream_active = false;
                connection.stream_tx = None;
                connection.stream_rx_handle = None;
            }

            warn!("Worker {} disconnected", worker_id_clone);
        });

        // Spawn automatic heartbeat task
        let worker_id_heartbeat = worker_id.to_string();
        let tx_heartbeat = tx.clone();
        let sessions_heartbeat = self.sessions.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            info!("Heartbeat task started for worker {}", worker_id_heartbeat);

            loop {
                interval.tick().await;

                // Check if session still exists
                if !sessions_heartbeat.contains_key(&worker_id_heartbeat) {
                    info!("Session removed, stopping heartbeat for {}", worker_id_heartbeat);
                    break;
                }

                // Send heartbeat
                let heartbeat_msg = ControllerMessage {
                    payload: Some(controller_message::Payload::Heartbeat(Heartbeat {
                        timestamp: std::time::SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64,
                    })),
                };

                if tx_heartbeat.send(heartbeat_msg).await.is_err() {
                    warn!("Heartbeat send failed for {}, stream likely closed", worker_id_heartbeat);
                    break;
                }

                debug!("Sent heartbeat to worker {}", worker_id_heartbeat);
            }

            info!("Heartbeat task stopped for worker {}", worker_id_heartbeat);
        });

        // Store legacy stream handles (for backward compat)
        {
            let mut connections = self.connections.lock().unwrap();
            if let Some(connection) = connections.get_mut(worker_id) {
                connection.stream_tx = Some(tx);
                connection.stream_rx_handle = Some(handle);
                connection.stream_active = true;
            }
        }

        debug!("Bidirectional stream fully established with worker {}", worker_id);
        Ok(())
    }

    /// Handle incoming message from worker via stream
    async fn handle_worker_message(worker_id: &str, msg: WorkerMessage) -> Result<()> {
        match msg.payload {
            Some(worker_message::Payload::Registration(reg)) => {
                info!(
                    "Worker {} registered - OS: {}, Capabilities: {:?}",
                    worker_id, reg.os_version, reg.capabilities
                );
                // TODO: Store worker metadata in state
            }
            Some(worker_message::Payload::Status(status)) => {
                debug!(
                    "Status from {}: CPU {}%, Memory {}MB, Jobs: {}",
                    worker_id, status.cpu_percent, status.memory_mb, status.active_jobs
                );
                // TODO: Update worker health metrics
            }
            Some(worker_message::Payload::Telemetry(batch)) => {
                info!(
                    "Received {} telemetry events from {} (job: {}, final: {})",
                    batch.events.len(), worker_id, batch.job_id, batch.is_final
                );
                // TODO: Forward to Elasticsearch
            }
            Some(worker_message::Payload::SampleResponse(response)) => {
                info!(
                    "Sample execution completed on {}: job_id={}, success={}",
                    worker_id, response.job_id, response.success
                );
                // TODO: Notify job completion handler
            }
            Some(worker_message::Payload::Ack(ack)) => {
                debug!("Ack from {}: request_id={}, success={}", worker_id, ack.request_id, ack.success);
            }
            None => {
                warn!("Received empty message from worker {}", worker_id);
            }
            _ => {}
        }

        Ok(())
    }

    /// Send a command to worker via stream
    /// Send command to worker via stream
    ///
    /// # Error Handling
    /// - Returns error if worker not found or stream closed
    /// - Automatically removes dead sessions on send failure
    /// - Emits Disconnected event for orchestration loop
    pub async fn send_command(&self, worker_id: &str, msg: ControllerMessage) -> Result<()> {
        // Get session (no lock held after this line)
        let session = self.sessions.get(worker_id)
            .ok_or_else(|| anyhow!("Worker {} not found or stream not established", worker_id))?;

        // Clone tx to release DashMap read lock before .await
        let tx = session.tx.clone();
        drop(session);  // Explicitly release read lock

        // Send message (no locks held)
        match tx.send(msg).await {
            Ok(()) => Ok(()),
            Err(_) => {
                // Send failed → worker disconnected, remove session
                warn!("Send to worker {} failed, removing session", worker_id);
                self.sessions.remove(worker_id);

                // Emit Disconnected event
                let _ = self.events_tx.send(WorkerEvent::disconnected(
                    worker_id,
                    "Send failed"
                )).await;

                Err(anyhow!("Worker {} disconnected (send failed)", worker_id))
            }
        }
    }

    /// Send RunSample command to worker via stream
    pub async fn send_run_sample(&self, worker_id: &str, request: SampleRequest) -> Result<()> {
        let request_id = uuid::Uuid::new_v4().to_string();

        info!("Sending RunSample command to worker {} (request_id: {}, job_id: {})",
              worker_id, request_id, request.job_id);

        let cmd = ControllerMessage {
            payload: Some(controller_message::Payload::RunSample(RunSampleCommand {
                request_id,
                request: Some(request),
            })),
        };

        self.send_command(worker_id, cmd).await
    }

    /// Send heartbeat to worker via stream
    pub async fn send_heartbeat(&self, worker_id: &str) -> Result<()> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let heartbeat = ControllerMessage {
            payload: Some(controller_message::Payload::Heartbeat(Heartbeat {
                timestamp,
            })),
        };

        self.send_command(worker_id, heartbeat).await
    }

    /// Broadcast message to all connected workers
    ///
    /// # Behavior
    /// - Sends to all workers with active sessions
    /// - Does NOT fail if some workers are unreachable
    /// - Logs warnings for failed sends
    /// - Returns number of successful sends
    pub async fn broadcast(&self, msg: ControllerMessage) -> usize {
        // Collect worker IDs (avoid holding iterator across .await)
        let worker_ids: Vec<String> = self.sessions.iter()
            .map(|entry| entry.key().clone())
            .collect();

        let mut success_count = 0;

        for worker_id in worker_ids {
            match self.send_command(&worker_id, msg.clone()).await {
                Ok(()) => {
                    success_count += 1;
                }
                Err(e) => {
                    warn!("Broadcast to worker {} failed: {}", worker_id, e);
                    // Continue broadcasting to other workers
                }
            }
        }

        info!("Broadcast completed: {}/{} workers reachable",
            success_count, self.sessions.len());

        success_count
    }

    /// Gracefully disconnect all workers
    ///
    /// Sends disconnect notification to all workers before shutting down
    pub async fn disconnect_all(&self, reason: &str, reconnect_allowed: bool) {
        info!("Disconnecting all workers - reason: {}", reason);

        let disconnect_msg = ControllerMessage {
            payload: Some(controller_message::Payload::Disconnect(DisconnectNotice {
                reason: reason.to_string(),
                reconnect_allowed,
            })),
        };

        let sent_count = self.broadcast(disconnect_msg).await;
        info!("Sent disconnect notification to {}/{} workers", sent_count, self.sessions.len());

        // Give workers a moment to process the disconnect
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Clear all sessions
        self.sessions.clear();
    }

    /// Check if worker has active stream
    pub fn has_active_stream(&self, worker_id: &str) -> bool {
        let connections = self.connections.lock().unwrap();
        connections.get(worker_id)
            .map(|conn| conn.stream_active)
            .unwrap_or(false)
    }

    /// Establish streams with all workers
    pub async fn establish_all_streams(&self) -> HashMap<String, Result<()>> {
        let worker_ids = self.list_workers();
        let mut results = HashMap::new();

        for worker_id in worker_ids {
            let result = self.establish_stream(&worker_id).await;
            results.insert(worker_id, result);
        }

        results
    }

    /// Send artifact to worker via gRPC streaming
    ///
    /// Reuses existing connection (no new client creation).
    /// Chunks artifact into 4MB pieces for efficient transfer.
    ///
    /// # Arguments
    /// * `worker_id` - Target worker ID
    /// * `artifact_id` - SHA256 hash of the artifact
    /// * `artifact_path` - Path to artifact binary on controller filesystem
    ///
    /// # Returns
    /// * `Ok(())` - Artifact successfully transferred
    /// * `Err(_)` - Transfer failed (network, file read, or worker error)
    pub async fn send_artifact(&self, worker_id: &str, artifact_id: &str, artifact_path: &Path) -> Result<()> {
        info!("[{}] Sending artifact {} to worker...", artifact_id, worker_id);

        // Read artifact file
        let artifact_data = tokio::fs::read(artifact_path).await
            .map_err(|e| anyhow!("Failed to read artifact file {:?}: {}", artifact_path, e))?;

        info!("[{}] Artifact size: {} bytes", artifact_id, artifact_data.len());

        // Get worker channel (reuse existing connection)
        // Split lock acquisition from async operations to avoid Send bound issues
        let channel = self.get_channel_from_worker(worker_id).await?;

        // Create client from existing channel
        let mut client = WorkerAgentClient::new(channel.clone());

        // Chunk artifact into 4MB pieces
        let chunk_size = 4 * 1024 * 1024;
        let total_chunks = ((artifact_data.len() + chunk_size - 1) / chunk_size) as u32;

        let chunks: Vec<ArtifactChunk> = artifact_data
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

        info!("[{}] Sending {} chunks to worker {}", artifact_id, total_chunks, worker_id);

        // Send chunks via streaming RPC
        client.send_artifact(stream::iter(chunks)).await
            .map_err(|e| anyhow!("Artifact transfer to worker {} failed: {}", worker_id, e))?;

        info!("[{}] Artifact successfully deployed to worker {}", artifact_id, worker_id);
        Ok(())
    }

    async fn get_channel_from_worker(&self, worker_id: &str) -> Result<Channel, Error> {
        // Get worker address (brief lock, no await)
        let worker_address = {
            let connections = self.connections.lock().unwrap();
            connections.get(worker_id)
                .ok_or_else(|| anyhow!("Worker {} not found in manager", worker_id))?
                .address.clone()
        };  // Lock dropped

        // Check for existing channel (brief lock, no await)
        let existing_channel = {
            let connections = self.connections.lock().unwrap();
            connections.get(worker_id).and_then(|c| c.channel.clone())
        };  // Lock dropped

        // Create channel if needed (async, no lock held)
        Ok(if let Some(ch) = existing_channel {
            ch
        } else {
            let endpoint = Endpoint::try_from(format!("http://{}", worker_address))
                .map_err(|e| anyhow!("Invalid endpoint for worker {}: {}", worker_id, e))?;
            let channel = endpoint.connect().await?;

            // Store channel (brief lock, no await)
            {
                let mut connections = self.connections.lock().unwrap();
                if let Some(conn) = connections.get_mut(worker_id) {
                    conn.channel = Some(channel.clone());
                }
            }  // Lock dropped

            channel
        })
    }

    /// Execute artifact on worker and return execution result
    ///
    /// Makes a blocking RPC call (waits for execution to complete).
    /// Reuses existing connection (no new client creation).
    ///
    /// # Arguments
    /// * `worker_id` - Target worker ID
    /// * `request` - SampleRequest containing job_id, artifact_id, timeout, etc.
    ///
    /// # Returns
    /// * `Ok(SampleResponse)` - Execution result with success, exit_code, telemetry_ids
    /// * `Err(_)` - Execution failed (network, timeout, or worker error)
    pub async fn execute_artifact(&self, worker_id: &str, request: SampleRequest) -> Result<SampleResponse> {
        info!("[{}] Executing artifact {} on worker {}...",
            request.job_id, request.artifact_id, worker_id);

        // Get worker channel (reuse existing connection)
        // Split lock acquisition from async operations to avoid Send bound issues
        let channel = self.get_channel_from_worker(worker_id).await?;

        // Create client from existing channel
        let mut client = WorkerAgentClient::new(channel.clone());

        // Make blocking RPC call (waits for execution to complete)
        let response = client.run_sample(tonic::Request::new(request.clone())).await
            .map_err(|e| anyhow!("Execution RPC to worker {} failed: {}", worker_id, e))?;

        let exec_result = response.into_inner();

        info!("[{}] Execution complete on worker {}: success={}, exit_code={}, telemetry_events={}",
            request.job_id, worker_id, exec_result.success, exec_result.exit_code, exec_result.telemetry_ids.len());

        Ok(exec_result)
    }
}

#[cfg(test)]
mod tests;