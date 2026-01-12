//! Phase 2: Bidirectional stream handler for worker-controller communication
//!
//! This module handles the bidirectional gRPC stream established by the controller.
//! The stream allows real-time communication in both directions:
//! - Controller → Worker: commands (run sample, health checks, heartbeats)
//! - Worker → Controller: registration, status updates, telemetry, results

use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tonic::{Status, Streaming};
use tracing::{debug, error, info, warn};

// Import generated protobuf types
use crate::automutate::common::{
    Ack, ControllerMessage, DisconnectNotice, ExecutionStatusReport, HealthCheckRequest, Heartbeat, RunSampleCommand,
    SampleResponse, StatusReport, TelemetryBatch, WorkerMessage, WorkerRegistration,
    controller_message, worker_message,
};

use crate::capabilities::WorkerState;

/// Handles incoming controller messages and sends worker responses via bidirectional stream
pub struct StreamHandler {
    /// Shared worker state
    worker_state: Arc<RwLock<WorkerState>>,

    /// Channel for sending messages to controller
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,
}

impl StreamHandler {
    /// Create a new stream handler
    ///
    /// Returns the handler and a receiver for outgoing messages
    pub fn new(
        worker_state: Arc<RwLock<WorkerState>>,
    ) -> (Self, mpsc::Receiver<Result<WorkerMessage, Status>>) {
        let (tx, rx) = mpsc::channel(100);

        let handler = StreamHandler { worker_state, tx };

        (handler, rx)
    }

    /// Handle incoming messages from controller via stream
    ///
    /// This runs in a loop processing controller messages until the stream closes
    pub async fn handle_stream(
        &self,
        mut stream: Streaming<ControllerMessage>,
    ) -> Result<(), Status> {
        info!("Starting bidirectional stream message handler");

        while let Some(msg) = stream.message().await? {
            if let Err(e) = self.process_message(msg).await {
                warn!("Error processing controller message: {}", e);
                // Continue processing other messages even if one fails
            }
        }

        info!("Controller stream closed");
        Ok(())
    }

    /// Process a single message from the controller
    async fn process_message(&self, msg: ControllerMessage) -> Result<()> {
        match msg.payload {
            Some(controller_message::Payload::RunSample(cmd)) => {
                self.handle_run_sample(cmd).await?;
            }
            Some(controller_message::Payload::HealthCheck(req)) => {
                self.handle_health_check(req).await?;
            }
            Some(controller_message::Payload::Heartbeat(hb)) => {
                self.handle_heartbeat(hb).await?;
            }
            Some(controller_message::Payload::Disconnect(notice)) => {
                self.handle_disconnect(notice).await?;
            }
            Some(controller_message::Payload::Ack(ack)) => {
                debug!("Received ack for request: {}", ack.request_id);
            }
            Some(controller_message::Payload::ArtifactChunks(_chunks)) => {
                warn!("Artifact chunks via stream not yet implemented");
                // TODO: Implement artifact transfer via stream in future
            }
            None => {
                warn!("Received empty controller message");
            }
        }

        Ok(())
    }

    /// Handle RunSample command from controller
    async fn handle_run_sample(&self, cmd: RunSampleCommand) -> Result<()> {
        let request_id = cmd.request_id.clone();
        let sample_request = cmd
            .request
            .ok_or_else(|| anyhow!("RunSampleCommand missing request"))?;

        info!(
            "Received RunSample command (request_id: {}, job_id: {})",
            request_id, sample_request.job_id
        );

        // Send immediate acknowledgement
        self.send_ack(&request_id, true, "").await?;

        // Execute sample asynchronously and send response via stream
        let tx = self.tx.clone();
        let worker_state = self.worker_state.clone();

        tokio::spawn(async move {
            // Update worker state to indicate job is running
            {
                let mut state = worker_state.write().await;
                state.current_job_id = Some(sample_request.job_id.clone());
            }

            // Execute the sample
            // TODO: Call actual execution logic from main.rs or execution module
            info!("Executing sample: {}", sample_request.job_id);

            // Placeholder: simulate execution
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

            let result = SampleResponse {
                job_id: sample_request.job_id.clone(),
                success: true,
                exit_code: 0,
                output: "Sample executed successfully".to_string(),
                telemetry_ids: vec![],
            };

            // Update worker state
            {
                let mut state = worker_state.write().await;
                state.current_job_id = None;
            }

            // Send response via stream
            if let Err(e) = tx
                .send(Ok(WorkerMessage {
                    payload: Some(worker_message::Payload::SampleResponse(result)),
                }))
                .await
            {
                error!("Failed to send sample response: {}", e);
            } else {
                info!("Sample execution completed: {}", sample_request.job_id);
            }
        });

        Ok(())
    }

    /// Handle health check request
    async fn handle_health_check(&self, req: HealthCheckRequest) -> Result<()> {
        debug!("Received health check request: {}", req.request_id);

        let state = self.worker_state.read().await;

        let status_report = StatusReport {
            worker_id: state.worker_id.clone(),
            worker_ip: state
                .metadata
                .get("ip_address")
                .cloned()
                .unwrap_or_default(),
            cpu_percent: state.health.cpu_percent,
            memory_mb: state.health.memory_percent, // Note: API mismatch, should be MB
            active_jobs: if state.current_job_id.is_some() { 1 } else { 0 },
            event_type: "health_check".to_string(),
            current_job_id: state.current_job_id.clone().unwrap_or_default(),
        };

        self.tx
            .send(Ok(WorkerMessage {
                payload: Some(worker_message::Payload::Status(status_report)),
            }))
            .await
            .map_err(|e| anyhow!("Failed to send status report: {}", e))?;

        debug!("Sent health check response");
        Ok(())
    }

    /// Handle heartbeat from controller
    async fn handle_heartbeat(&self, hb: Heartbeat) -> Result<()> {
        debug!(
            "Received heartbeat from controller at timestamp: {}",
            hb.timestamp
        );

        // Update last heartbeat time in worker state
        {
            let mut state = self.worker_state.write().await;
            state.last_controller_heartbeat = Some(hb.timestamp);
        }

        Ok(())
    }

    /// Handle disconnect notification from controller
    async fn handle_disconnect(&self, notice: DisconnectNotice) -> Result<()> {
        if notice.reconnect_allowed {
            info!(
                "Controller disconnecting (reconnect allowed): {}",
                notice.reason
            );
        } else {
            warn!(
                "Controller disconnecting (reconnect NOT allowed): {}",
                notice.reason
            );
        }

        // Update worker state to mark controller as disconnected
        {
            let mut state = self.worker_state.write().await;
            state.controller_disconnected = true;
            state.disconnect_reason = Some(notice.reason.clone());
            state.reconnect_allowed = notice.reconnect_allowed;
        }

        Ok(())
    }

    /// Send acknowledgement message
    async fn send_ack(&self, request_id: &str, success: bool, error: &str) -> Result<()> {
        self.tx
            .send(Ok(WorkerMessage {
                payload: Some(worker_message::Payload::Ack(Ack {
                    request_id: request_id.to_string(),
                    success,
                    error: error.to_string(),
                })),
            }))
            .await
            .map_err(|e| anyhow!("Failed to send ack: {}", e))?;

        Ok(())
    }

    /// Send telemetry batch via stream
    ///
    /// Called by telemetry collectors to stream events to controller
    pub async fn send_telemetry(&self, batch: TelemetryBatch) -> Result<()> {
        info!(
            "Sending telemetry batch: {} events for job {}",
            batch.events.len(),
            batch.job_id
        );

        self.tx
            .send(Ok(WorkerMessage {
                payload: Some(worker_message::Payload::Telemetry(batch)),
            }))
            .await
            .map_err(|e| anyhow!("Failed to send telemetry: {}", e))?;

        Ok(())
    }

    /// Send periodic status updates (called by background task)
    pub async fn send_status_update(&self, event_type: &str) -> Result<()> {
        let state = self.worker_state.read().await;

        let status_report = StatusReport {
            worker_id: state.worker_id.clone(),
            worker_ip: state
                .metadata
                .get("ip_address")
                .cloned()
                .unwrap_or_default(),
            cpu_percent: state.health.cpu_percent,
            memory_mb: state.health.memory_percent,
            active_jobs: if state.current_job_id.is_some() { 1 } else { 0 },
            event_type: event_type.to_string(),
            current_job_id: state.current_job_id.clone().unwrap_or_default(),
        };

        self.tx
            .send(Ok(WorkerMessage {
                payload: Some(worker_message::Payload::Status(status_report)),
            }))
            .await
            .map_err(|e| anyhow!("Failed to send status update: {}", e))?;

        Ok(())
    }

    /// Send detailed execution status from ExecutionMonitor
    /// This sends comprehensive execution metrics including job details, process state, and telemetry counts
    pub async fn send_execution_status(
        &self,
        job_id: String,
        run_id: String,
        artifact_name: String,
        pid: i32,
        elapsed_seconds: i32,
        process_alive: bool,
        telemetry_events_count: i32,
        event_type: String,
        cpu_percent: i32,
        memory_mb: i32,
        details: String,
    ) -> Result<()> {
        let state = self.worker_state.read().await;

        let execution_status = ExecutionStatusReport {
            worker_id: state.worker_id.clone(),
            worker_ip: state
                .metadata
                .get("ip_address")
                .cloned()
                .unwrap_or_default(),
            job_id,
            run_id,
            artifact_name,
            pid,
            elapsed_seconds,
            process_alive,
            telemetry_events_count,
            event_type,
            cpu_percent,
            memory_mb,
            details,
        };

        self.tx
            .send(Ok(WorkerMessage {
                payload: Some(worker_message::Payload::ExecutionStatus(execution_status)),
            }))
            .await
            .map_err(|e| anyhow!("Failed to send execution status: {}", e))?;

        Ok(())
    }

    /// Send worker registration (called once on stream establishment)
    pub async fn send_registration(&self) -> Result<()> {
        let state = self.worker_state.read().await;

        let registration = WorkerRegistration {
            worker_id: state.worker_id.clone(),
            ip_address: state
                .metadata
                .get("ip_address")
                .cloned()
                .unwrap_or_default(),
            os_version: state
                .metadata
                .get("os_version")
                .cloned()
                .unwrap_or_default(),
            capabilities: state.capabilities.clone(),
            metadata: state.metadata.clone(),
            tools: state.tools.clone(),
        };

        self.tx
            .send(Ok(WorkerMessage {
                payload: Some(worker_message::Payload::Registration(registration)),
            }))
            .await
            .map_err(|e| anyhow!("Failed to send registration: {}", e))?;

        info!("Sent worker registration to controller");
        Ok(())
    }
}

/// Background task to send periodic heartbeat status updates
pub async fn heartbeat_loop(handler: Arc<StreamHandler>, interval_secs: u64) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));

    loop {
        interval.tick().await;

        // Check if controller has explicitly disconnected
        let disconnected = {
            let state = handler.worker_state.read().await;
            state.controller_disconnected
        };

        if let Err(e) = handler.send_status_update("heartbeat").await {
            if disconnected {
                // Controller explicitly disconnected - log at debug level
                debug!("Heartbeat failed (controller disconnected): {}", e);
            } else {
                // Unexpected disconnect - log at warn level
                warn!("Failed to send heartbeat status: {}", e);
            }
            // Continue trying regardless (user wants continuous retries)
        } else {
            debug!("Sent heartbeat status update");

            // If we successfully sent after disconnect, controller reconnected
            if disconnected {
                info!("Controller reconnected - heartbeat successful");
                let mut state = handler.worker_state.write().await;
                state.controller_disconnected = false;
                state.disconnect_reason = None;
            }
        }
    }
}
