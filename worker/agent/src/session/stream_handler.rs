//! Bidirectional stream handler for worker-controller communication.
//!
//! This module handles the bidirectional gRPC stream established by the controller.
//! The stream allows real-time communication in both directions:
//! - Controller -> Worker: commands (run sample, health checks, heartbeats)
//! - Worker -> Controller: registration, status updates, telemetry, results
//!
//! NOTE: This struct does NOT hold Arc<WorkerAgentService> to avoid a reference
//! cycle (WorkerAgentService -> StreamHandler -> WorkerAgentService).
//! Instead it stores the individual fields it needs.

use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, mpsc};
use tonic::{Status, Streaming};
use tracing::{debug, error, info, warn};

// Import generated protobuf types
use crate::automutate::common::{
    Ack, ControllerMessage, DisconnectNotice, HealthCheckRequest, Heartbeat, RunSampleCommand,
    StatusReport, TelemetryBatch, WorkerMessage, WorkerRegistration,
    controller_message, worker_message,
};

use crate::dispatch::state::ExecutionState;
use crate::session::worker_state::WorkerState;
use edr_config::WorkerConfig;

/// Handles incoming controller messages and sends worker responses via bidirectional stream.
///
/// Does NOT hold a reference to WorkerAgentService (breaks Arc cycle).
pub struct StreamHandler {
    /// Shared worker state
    pub worker_state: Arc<RwLock<WorkerState>>,

    /// Channel for sending messages to controller
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,

    /// Worker identity and config (extracted from WorkerAgentService)
    worker_id: String,
    config: WorkerConfig,

    /// Execution lock shared with the service
    execution_lock: Arc<Mutex<ExecutionState>>,
}

impl StreamHandler {
    /// Create a new stream handler.
    ///
    /// Returns the handler and a receiver for outgoing messages.
    pub fn new(
        worker_state: Arc<RwLock<WorkerState>>,
        worker_id: String,
        config: WorkerConfig,
        execution_lock: Arc<Mutex<ExecutionState>>,
    ) -> (Self, mpsc::Receiver<Result<WorkerMessage, Status>>) {
        let (tx, rx) = mpsc::channel(100);

        let handler = StreamHandler {
            worker_state,
            tx,
            worker_id,
            config,
            execution_lock,
        };

        (handler, rx)
    }

    /// Get a reference to the outgoing message sender.
    /// Used by external code to build a ControlPlaneSink.
    pub fn sender(&self) -> &mpsc::Sender<Result<WorkerMessage, Status>> {
        &self.tx
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
            }
            None => {
                warn!("Received empty controller message");
            }
        }

        Ok(())
    }

    /// Handle RunSample command from controller.
    ///
    /// Calls engine::execute_run() directly instead of going through
    /// api::run::run_sample(), avoiding the need to hold Arc<WorkerAgentService>.
    async fn handle_run_sample(&self, cmd: RunSampleCommand) -> Result<()> {
        use crate::dispatch::engine;
        use crate::dispatch::state::ExecutionLockGuard;
        use crate::dispatch::types::{RunContext, RunRequest, sample_response_error, sample_response_ok};

        let request_id = cmd.request_id.clone();
        let sample_request = cmd
            .request
            .ok_or_else(|| anyhow!("RunSampleCommand missing request"))?;

        debug!(
            "Received RunSample command (request_id: {}, job_id: {})",
            request_id, sample_request.job_id
        );

        // Send immediate acknowledgement
        self.send_ack(&request_id, true, "").await?;

        // Clone fields needed by the spawned task
        let tx = self.tx.clone();
        let worker_state = self.worker_state.clone();
        let worker_id = self.worker_id.clone();
        let config = self.config.clone();
        let execution_lock = self.execution_lock.clone();

        tokio::spawn(async move {
            let job_id = sample_request.job_id.clone();
            let artifact_name = format!("{}.exe", sample_request.artifact_id);
            let run_id = request_id.clone();

            // Update worker state to indicate job is running
            {
                let mut state = worker_state.write().await;
                state.current_job_id = Some(job_id.clone());
                state.current_run_id = Some(run_id.clone());
            }

            // Acquire execution lock
            let _execution_lock = {
                let mut state = execution_lock.lock().await;
                if let Err(e) = state.acquire(job_id.clone(), artifact_name.clone(), run_id.clone())
                {
                    warn!("[ERROR] REJECTED: {}", e);
                    // Send error response
                    let _ = tx
                        .send(Ok(WorkerMessage {
                            payload: Some(worker_message::Payload::SampleResponse(
                                sample_response_error(&job_id, &run_id, &e),
                            )),
                        }))
                        .await;
                    // Clear worker state
                    let mut ws = worker_state.write().await;
                    ws.current_job_id = None;
                    ws.current_run_id = None;
                    return;
                }
                ExecutionLockGuard::new(execution_lock.clone())
            };

            // Build typed request and context
            let artifacts_base = std::path::Path::new(&config.storage.artifacts_path);
            let artifact_path = artifacts_base.join(format!("{}.exe", sample_request.artifact_id));
            let telemetry_dir =
                artifacts_base.join(format!("telemetry_{}", sample_request.artifact_id));

            let run_request = RunRequest {
                job_id: job_id.clone(),
                artifact_id: sample_request.artifact_id.clone(),
                timeout_seconds: sample_request.timeout_seconds as u32,
                run_id: run_id.clone(),
            };

            let run_context = RunContext {
                worker_id,
                config,
                telemetry_dir,
                artifact_path,
                artifact_name: artifact_name.clone(),
            };

            // Build sink from tx channel (no Arc<StreamHandler> needed)
            let sink = crate::dispatch::sink::build_sink(Some(&tx));

            // Execute via engine
            let result = match engine::execute_run(&run_request, &run_context, sink).await {
                Ok(outcome) => {
                    let output = crate::api::run::format_output(
                        &outcome,
                        sample_request.timeout_seconds as u32,
                    );
                    sample_response_ok(&job_id, &run_id, &outcome, output)
                }
                Err(e) => {
                    error!("Sample execution failed: {}", e);
                    sample_response_error(&job_id, &run_id, &e)
                }
            };

            // Update worker state
            {
                let mut state = worker_state.write().await;
                state.current_job_id = None;
                state.current_run_id = None;
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
                debug!("Sample execution completed: {}", job_id);
            }
        });

        Ok(())
    }

    /// Handle health check request
    async fn handle_health_check(&self, req: HealthCheckRequest) -> Result<()> {
        debug!("Received health check request: {}", req.request_id);
        self.send_status_update("health_check").await?;
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
        debug!(
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

        let execution_status = crate::automutate::common::ExecutionStatusReport {
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

        let ip_address = format!(
            "{}:{}",
            self.config.worker.ip_address, self.config.worker.listen_port
        );

        let registration = WorkerRegistration {
            worker_id: state.worker_id.clone(),
            ip_address: ip_address.clone(),
            os_version: self.config.worker.os_version.clone(),
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

        debug!(
            "Sent worker registration to controller (IP: {})",
            ip_address
        );
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
