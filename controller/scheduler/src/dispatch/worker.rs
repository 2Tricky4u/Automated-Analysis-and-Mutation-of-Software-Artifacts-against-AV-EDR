//! Worker - thin dispatcher that owns a VM connection.
//!
//! Workers pull runs from their shared PoolGroup and dispatch to their VM.
//! Multiple workers can share a pool, enabling parallel execution.
//!
//! Responsibilities:
//! - Pull runs from shared pool (signal-driven, efficient)
//! - Upload artifacts to VM
//! - Send execution commands to VM
//! - Report results back to pool
//!
//! Graceful shutdown is handled via CancellationToken from PoolGroup.

use super::channels::{RemoteRunResult, WorkerEvent};
use super::pool_group::PoolGroup;
use super::types::{JobId, RunEnvelope, RunId, RunOutcome, WorkerId, WorkerInfo};
use crate::automutate::common::{controller_message, ControllerMessage, RunSampleCommand, SampleRequest};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

// ============================================================================
// ArtifactSender Trait
// ============================================================================

/// Trait for sending artifacts to remote VM.
///
/// Implemented by TargetManager to handle the actual file transfer.
pub trait ArtifactSender: std::fmt::Debug {
    fn send_artifact(
        &self,
        worker_id: &str,
        artifact_id: &str,
        path: &Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>;
}

// ============================================================================
// Worker
// ============================================================================

/// Worker is a thin dispatcher that owns a VM connection.
///
/// It pulls runs from the shared PoolGroup and dispatches them to its VM.
/// The worker does NOT own the run pool or perform artifact building -
/// that's handled by the PoolGroup.
///
/// Graceful shutdown is triggered via CancellationToken from the pool.
pub struct Worker {
    /// Worker identity
    id: WorkerId,
    info: WorkerInfo,

    /// Reference to shared pool group
    pool: Arc<PoolGroup>,

    /// Cancellation token for graceful shutdown (from pool)
    shutdown_token: CancellationToken,

    /// Events to orchestrator (for observability)
    event_tx: mpsc::Sender<WorkerEvent>,

    /// Channel to send commands to remote VM
    remote_tx: mpsc::Sender<ControllerMessage>,

    /// Channel to receive results from remote VM
    remote_rx: mpsc::Receiver<RemoteRunResult>,

    /// Artifact sender (for uploading before dispatch)
    artifact_sender: Arc<dyn ArtifactSender + Send + Sync>,

    /// Currently in-flight run (None = idle, Some = waiting for result)
    in_flight: Option<InFlightRun>,
}

/// Tracks a run that has been dispatched and is awaiting result.
#[derive(Debug)]
#[allow(dead_code)] // job_id kept for future correlation/logging
struct InFlightRun {
    run_id: RunId,
    job_id: JobId,
}

impl Worker {
    /// Create a new worker.
    ///
    /// The cancellation token is obtained from the pool for graceful shutdown.
    pub fn new(
        id: WorkerId,
        info: WorkerInfo,
        pool: Arc<PoolGroup>,
        event_tx: mpsc::Sender<WorkerEvent>,
        remote_tx: mpsc::Sender<ControllerMessage>,
        remote_rx: mpsc::Receiver<RemoteRunResult>,
        artifact_sender: Arc<dyn ArtifactSender + Send + Sync>,
    ) -> Self {
        let shutdown_token = pool.cancellation_token();
        Self {
            id,
            info,
            pool,
            shutdown_token,
            event_tx,
            remote_tx,
            remote_rx,
            artifact_sender,
            in_flight: None,
        }
    }

    /// Main worker loop.
    /// Signal-driven: waits for pool notification, VM result, or shutdown.
    pub async fn run(mut self) {
        info!(
            "[Worker:{}] Started (os={}, caps={:?}, pool={})",
            self.id, self.info.os, self.info.capabilities, self.pool.group_id()
        );

        // Signal availability
        self.emit_available().await;

        loop {
            tokio::select! {
                biased;

                // Priority 1: Graceful shutdown via cancellation token
                _ = self.shutdown_token.cancelled() => {
                    info!("[Worker:{}] Shutdown requested", self.id);
                    break;
                }

                // Priority 2: Results from remote VM (when in-flight)
                Some(result) = self.remote_rx.recv(), if self.in_flight.is_some() => {
                    self.on_result_received(result).await;

                    // Check shutdown before taking more work
                    if self.shutdown_token.is_cancelled() {
                        break;
                    }

                    // Immediately try to get more work (no wait needed)
                    if let Some(run) = self.pool.try_take_run().await {
                        if let Err(e) = self.dispatch_run(run).await {
                            error!("[Worker:{}] Dispatch failed: {}", self.id, e);
                        }
                    }
                }

                // Priority 3: Wait for pool signal when idle
                _ = self.pool.wait_for_runs(), if self.in_flight.is_none() => {
                    // Check shutdown after wakeup (pool.shutdown() also notifies)
                    if self.shutdown_token.is_cancelled() {
                        break;
                    }

                    // Woken by signal - try to grab a run
                    // Might fail if another worker grabbed it first
                    if let Some(run) = self.pool.try_take_run().await {
                        if let Err(e) = self.dispatch_run(run).await {
                            error!("[Worker:{}] Dispatch failed: {}", self.id, e);
                        }
                    }
                }
            }
        }

        info!("[Worker:{}] Stopped", self.id);
    }

    // ========================================================================
    // Dispatch
    // ========================================================================

    /// Dispatch a run to the remote VM.
    async fn dispatch_run(&mut self, envelope: RunEnvelope) -> anyhow::Result<()> {
        debug!(
            "[Worker:{}] Dispatching run {} (type={}, job={})",
            self.id, envelope.run_id, envelope.run_type, envelope.job_id
        );

        // Get artifact ID (SHA256)
        let artifact_id = envelope
            .artifact
            .sha256
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Artifact missing SHA256"))?;

        // Upload artifact to VM
        self.artifact_sender
            .send_artifact(&self.id.0, &artifact_id, &envelope.artifact.path)
            .await?;

        // Build command
        let command = ControllerMessage {
            payload: Some(controller_message::Payload::RunSample(RunSampleCommand {
                request_id: envelope.run_id.0.clone(),
                request: Some(SampleRequest {
                    job_id: envelope.job_id.0.clone(),
                    artifact_id: artifact_id.clone(),
                    trace_mode: envelope.run_type.trace_mode().to_string(),
                    timeout_seconds: envelope.timeout_seconds as i32,
                    ..Default::default()
                }),
            })),
        };

        // Track in-flight
        self.in_flight = Some(InFlightRun {
            run_id: envelope.run_id.clone(),
            job_id: envelope.job_id.clone(),
        });

        // Send to VM
        self.remote_tx
            .send(command)
            .await
            .map_err(|_| anyhow::anyhow!("Remote channel closed"))?;

        debug!(
            "[Worker:{}] Run {} dispatched to VM",
            self.id, envelope.run_id
        );

        Ok(())
    }

    // ========================================================================
    // Result Handling
    // ========================================================================

    /// Handle result received from VM.
    async fn on_result_received(&mut self, result: RemoteRunResult) {
        let in_flight = match self.in_flight.take() {
            Some(f) => f,
            None => {
                warn!(
                    "[Worker:{}] Received result but no run in-flight: {}",
                    self.id, result.run_id
                );
                return;
            }
        };

        // Verify run_id matches
        if in_flight.run_id != result.run_id {
            warn!(
                "[Worker:{}] Run ID mismatch: expected {}, got {}",
                self.id, in_flight.run_id, result.run_id
            );
        }

        debug!(
            "[Worker:{}] Run {} completed: detected={}, exit={}",
            self.id, result.run_id, result.detected, result.exit_code
        );

        // Build outcome
        let outcome = RunOutcome {
            detected: result.detected,
            exit_code: result.exit_code,
            error: result.error.clone(),
        };

        // Report to pool for aggregation
        self.pool
            .report_result(result.run_id.clone(), outcome.clone())
            .await;

        // Emit event for observability
        let _ = self
            .event_tx
            .send(WorkerEvent::RunCompleted {
                worker_id: self.id.clone(),
                run_id: result.run_id,
                outcome,
            })
            .await;
    }

    // ========================================================================
    // Events
    // ========================================================================

    /// Emit available event.
    async fn emit_available(&self) {
        let _ = self
            .event_tx
            .send(WorkerEvent::Available {
                worker_id: self.id.clone(),
            })
            .await;
    }

    // ========================================================================
    // Accessors (public API for introspection)
    // ========================================================================

    /// Get worker ID.
    #[allow(dead_code)]
    pub fn id(&self) -> &WorkerId {
        &self.id
    }

    /// Get worker info.
    #[allow(dead_code)]
    pub fn info(&self) -> &WorkerInfo {
        &self.info
    }

    /// Check if worker is idle (no in-flight run).
    #[allow(dead_code)]
    pub fn is_idle(&self) -> bool {
        self.in_flight.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::group_id::GroupId;
    use crate::dispatch::pool_group::PoolEvent;

    #[derive(Debug)]
    struct MockArtifactSender;

    impl ArtifactSender for MockArtifactSender {
        fn send_artifact(
            &self,
            _worker_id: &str,
            _artifact_id: &str,
            _path: &Path,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn test_worker_creation() {
        let (pool_event_tx, _) = mpsc::channel(100);
        let pool = Arc::new(PoolGroup::new(
            GroupId::new("windows-defender"),
            pool_event_tx,
        ));

        let (worker_event_tx, _) = mpsc::channel(64);
        let (remote_tx, _) = mpsc::channel(100);
        let (_, remote_rx) = mpsc::channel(100);

        let worker = Worker::new(
            WorkerId("test-worker".into()),
            WorkerInfo {
                id: WorkerId("test-worker".into()),
                os: "windows".into(),
                capabilities: vec!["defender".into()],
            },
            pool,
            worker_event_tx,
            remote_tx,
            remote_rx,
            Arc::new(MockArtifactSender),
        );

        assert_eq!(worker.id().0, "test-worker");
        assert!(worker.is_idle());
    }
}
