// Worker Manager for Phase 1 & 2: Controller-initiated connections with bidirectional streaming
// Manages outbound gRPC connections from controller to workers

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::time::{Duration, timeout};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Streaming};
use tracing::{info, warn, error, debug};
use futures::stream::StreamExt;

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
    SampleRequest,
};

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

    // PHASE 2: Bidirectional stream fields
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
        info!("Connecting to worker {} at {}", self.id, worker_url);

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
    /// Worker connections indexed by worker ID
    connections: Arc<Mutex<HashMap<String, WorkerConnection>>>,
    /// Default RPC timeout
    rpc_timeout: Duration,
}

impl WorkerManager {
    /// Create new WorkerManager
    pub fn new(rpc_timeout_secs: u64) -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
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

        info!("Added worker {} at {}", config.id, config.address);
        Ok(())
    }

    /// Remove a worker from the manager
    pub fn remove_worker(&self, worker_id: &str) -> Result<()> {
        let mut connections = self.connections.lock().unwrap();

        if connections.remove(worker_id).is_some() {
            info!("Removed worker {}", worker_id);
            Ok(())
        } else {
            Err(anyhow!("Worker {} not found", worker_id))
        }
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
    pub fn is_worker_connected(&self, worker_id: &str) -> bool {
        let connections = self.connections.lock().unwrap();

        if let Some(connection) = connections.get(worker_id) {
            if connection.channel.is_some() {
                // Check if connection is recent (within last 5 minutes)
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
    pub fn list_workers(&self) -> Vec<String> {
        let connections = self.connections.lock().unwrap();
        connections.keys().cloned().collect()
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

    // ===== PHASE 2: Bidirectional Streaming Support =====

    /// Establish bidirectional stream with a worker
    pub async fn establish_stream(&self, worker_id: &str) -> Result<()> {
        info!("Establishing bidirectional stream with worker: {}", worker_id);

        // Get channel
        let channel = {
            let mut connections = self.connections.lock().unwrap();
            let connection = connections.get_mut(worker_id)
                .ok_or_else(|| anyhow!("Worker {} not found in manager", worker_id))?;

            connection.get_channel().await?
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

        info!("Stream established with worker {}", worker_id);

        // Spawn task to handle incoming messages from worker
        let worker_id_clone = worker_id.to_string();
        let connections_clone = Arc::clone(&self.connections);

        let handle = tokio::spawn(async move {
            info!("Starting stream message handler for worker {}", worker_id_clone);

            while let Some(result) = incoming.next().await {
                match result {
                    Ok(msg) => {
                        if let Err(e) = Self::handle_worker_message(&worker_id_clone, msg).await {
                            warn!("Error handling message from worker {}: {}", worker_id_clone, e);
                        }
                    }
                    Err(e) => {
                        error!("Stream error from worker {}: {}", worker_id_clone, e);
                        break;
                    }
                }
            }

            // Mark stream as inactive when it closes
            let mut connections = connections_clone.lock().unwrap();
            if let Some(connection) = connections.get_mut(&worker_id_clone) {
                connection.stream_active = false;
                connection.stream_tx = None;
                connection.stream_rx_handle = None;
            }

            warn!("Stream closed for worker {}", worker_id_clone);
        });

        // Store stream handles
        {
            let mut connections = self.connections.lock().unwrap();
            if let Some(connection) = connections.get_mut(worker_id) {
                connection.stream_tx = Some(tx);
                connection.stream_rx_handle = Some(handle);
                connection.stream_active = true;
            }
        }

        info!("Bidirectional stream fully established with worker {}", worker_id);
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
        }

        Ok(())
    }

    /// Send a command to worker via stream
    pub async fn send_command(&self, worker_id: &str, msg: ControllerMessage) -> Result<()> {
        let connections = self.connections.lock().unwrap();
        let connection = connections.get(worker_id)
            .ok_or_else(|| anyhow!("Worker {} not found", worker_id))?;

        if let Some(ref tx) = connection.stream_tx {
            tx.send(msg).await
                .map_err(|e| anyhow!("Failed to send command to worker {}: {}", worker_id, e))?;
            Ok(())
        } else {
            Err(anyhow!("Worker {} does not have active stream", worker_id))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_remove_worker() {
        let manager = WorkerManager::new(30);

        let config = WorkerConfig {
            id: "win10-worker-01".to_string(),
            address: "10.200.200.11:50052".to_string(),
            enabled: true,
        };

        manager.add_worker(config).unwrap();

        let workers = manager.list_workers();
        assert_eq!(workers.len(), 1);
        assert!(workers.contains(&"win10-worker-01".to_string()));

        manager.remove_worker("win10-worker-01").unwrap();

        let workers = manager.list_workers();
        assert_eq!(workers.len(), 0);
    }

    #[test]
    fn test_worker_stats() {
        let manager = WorkerManager::new(30);

        let config = WorkerConfig {
            id: "win10-worker-01".to_string(),
            address: "10.200.200.11:50052".to_string(),
            enabled: true,
        };

        manager.add_worker(config).unwrap();

        let stats = manager.get_worker_stats("win10-worker-01");
        assert!(stats.is_some());

        let (connected, retry_count) = stats.unwrap();
        assert_eq!(connected, false); // Not connected yet
        assert_eq!(retry_count, 0);
    }
}
