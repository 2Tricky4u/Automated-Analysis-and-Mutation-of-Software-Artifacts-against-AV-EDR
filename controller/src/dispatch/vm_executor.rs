//! VMExecutor - thin dispatcher that executes runs on a VM.
//!
//! Architecture:
//! - Pulls runs from shared RunPool (capability-filtered)
//! - Reserves VM via TargetManager before dispatch
//! - Uploads artifacts to VM
//! - Dispatches execution commands to VM
//! - Routes results back to originating JobWorker via RunPool
//! - Releases VM after result received
//!
//! VMExecutor is "dumb" - it doesn't know about jobs or rounds,
//! it just takes runs and returns results.

use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::automutate::common::{
    ControllerMessage, RunSampleCommand, SampleRequest, controller_message,
};
use crate::vm::TargetManager;

use super::channels::{JobRunResult, RemoteRunResult};
use super::run_pool::RunPool;
use super::types::{RunEnvelope, RunOutcome, VMInfo};

// ============================================================================
// ArtifactSender Trait
// ============================================================================

/// Trait for sending artifacts to remote VM.
///
/// Implemented by TargetManager to handle the actual file transfer.
pub trait ArtifactSender: std::fmt::Debug {
    fn send_artifact(
        &self,
        vm_id: &str,
        artifact_id: &str,
        path: &Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>;
}

// ============================================================================
// VMExecutor
// ============================================================================

/// VMExecutor is a thin dispatcher that owns a VM connection.
///
/// It pulls runs from the shared RunPool and dispatches them to its VM.
/// Results are routed back via the RunPool to the originating JobWorker.
///
/// Unlike the old Worker, VMExecutor:
/// - Doesn't know about jobs or rounds (just runs)
/// - Doesn't report to a specific pool group
/// - Routes results back via RunPool (which knows which JobWorker to notify)
pub struct VMExecutor {
    /// VM identity
    id: String,
    info: VMInfo,

    /// Target manager for reserve/release
    targets: Arc<TargetManager>,

    /// Shared run pool
    run_pool: Arc<RunPool>,

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
struct InFlightRun {
    /// The full envelope (needed for routing result back)
    envelope: RunEnvelope,
}

impl VMExecutor {
    /// Create a new VMExecutor.
    pub fn new(
        id: String,
        info: VMInfo,
        targets: Arc<TargetManager>,
        run_pool: Arc<RunPool>,
        remote_tx: mpsc::Sender<ControllerMessage>,
        remote_rx: mpsc::Receiver<RemoteRunResult>,
        artifact_sender: Arc<dyn ArtifactSender + Send + Sync>,
    ) -> Self {
        Self {
            id,
            info,
            targets,
            run_pool,
            remote_tx,
            remote_rx,
            artifact_sender,
            in_flight: None,
        }
    }

    /// Main executor loop.
    /// Signal-driven: waits for pool notification, VM result, or shutdown.
    pub async fn run(mut self) {
        info!(
            "[VM:{}] Executor started (os={}, caps={:?})",
            self.id, self.info.os, self.info.capabilities
        );

        let shutdown_token = self.run_pool.cancellation_token();

        // Pick up any runs already queued (e.g. after reconnection)
        if !shutdown_token.is_cancelled()
            && let Some(run) = self
                .run_pool
                .take_run(&self.info.os, &self.info.capabilities)
                .await
            {
                self.dispatch(run).await;
            }

        loop {
            tokio::select! {
                biased;

                // Priority 1: Graceful shutdown
                _ = shutdown_token.cancelled() => {
                    info!("[VM:{}] Shutdown requested", self.id);
                    //TODO clean vm about this job
                    break;
                }

                // Priority 2: Results from remote VM (when in-flight)
                Some(result) = self.remote_rx.recv(), if self.in_flight.is_some() => {
                    self.on_result_received(result).await;

                    // Check shutdown before taking more work
                    if shutdown_token.is_cancelled() {
                        break;
                    }

                    // Immediately try to get more work
                    if let Some(run) = self.run_pool.take_run(&self.info.os, &self.info.capabilities).await {
                        self.dispatch(run).await;
                    }
                }

                // Priority 3: Wait for pool signal when idle
                _ = self.run_pool.wait_for_runs(), if self.in_flight.is_none() => {
                    // Check shutdown after wakeup
                    if shutdown_token.is_cancelled() {
                        break;
                    }

                    // Woken by signal - try to grab a run
                    if let Some(run) = self.run_pool.take_run(&self.info.os, &self.info.capabilities).await {
                        self.dispatch(run).await;
                    }
                }
            }
        }

        info!("[VM:{}] Executor stopped", self.id);
    }

    // ========================================================================
    // Dispatch
    // ========================================================================

    /// Dispatch a run to the remote VM.
    async fn dispatch(&mut self, envelope: RunEnvelope) {
        debug!(
            "[VM:{}] Dispatching run {} (type={}, job={})",
            self.id, envelope.run_id, envelope.run_type, envelope.job_id
        );

        // Reserve VM before dispatch
        if let Err(e) = self.targets.reserve(&self.id) {
            warn!("[VM:{}] Failed to reserve: {}", self.id, e);
            // Continue anyway - VM might already be busy from a previous run
        }

        // Get artifact ID (SHA256)
        let artifact_id = match &envelope.artifact.sha256 {
            Some(id) => id.clone(),
            None => {
                error!("[VM:{}] Artifact missing SHA256", self.id);
                self.route_error(&envelope, "Artifact missing SHA256".to_string())
                    .await;
                return;
            }
        };

        // Upload artifact to VM
        if let Err(e) = self
            .artifact_sender
            .send_artifact(&self.id, &artifact_id, &envelope.artifact.path)
            .await
        {
            error!("[VM:{}] Failed to upload artifact: {}", self.id, e);
            self.route_error(&envelope, format!("Artifact upload failed: {}", e))
                .await;
            return;
        }

        // Build command
        let command = ControllerMessage {
            payload: Some(controller_message::Payload::RunSample(RunSampleCommand {
                request_id: envelope.run_id.0.clone(),
                request: Some(SampleRequest {
                    job_id: envelope.job_id.0.clone(),
                    artifact_id: artifact_id.clone(),
                    trace_mode: envelope.run_type.trace_mode().to_string(),
                    timeout_seconds: envelope.timeout_seconds as i32,
                    is_dryrun: envelope.run_type.is_dryrun(),
                    ..Default::default()
                }),
            })),
        };

        // Track in-flight
        self.in_flight = Some(InFlightRun {
            envelope: envelope.clone(),
        });

        // Send to VM
        if self.remote_tx.send(command).await.is_err() {
            error!("[VM:{}] Remote channel closed", self.id);
            self.in_flight = None;
            self.route_error(&envelope, "VM disconnected".to_string())
                .await;
            return;
        }

        debug!("[VM:{}] Run {} dispatched to VM", self.id, envelope.run_id);
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
                    "[VM:{}] Received result but no run in-flight: {}",
                    self.id, result.run_id
                );
                return;
            }
        };
        let envelope = in_flight.envelope;

        // Verify run_id matches
        if envelope.run_id != result.run_id {
            warn!(
                "[VM:{}] Run ID mismatch: expected {}, got {}",
                self.id, envelope.run_id, result.run_id
            );
        }

        debug!(
            "[VM:{}] Run {} completed: detected={}, exit={}, success={}",
            self.id, result.run_id, result.detected, result.exit_code, result.success
        );

        // Release VM after run completes
        if let Err(e) = self.targets.release(&self.id) {
            warn!("[VM:{}] Failed to release: {}", self.id, e);
        }

        // Build result to route back
        let job_result = JobRunResult {
            run_id: envelope.run_id,
            job_id: envelope.job_id,
            round_id: envelope.round_id,
            outcome: result.into(),
            vm_id: self.id.clone(),
        };

        // Route back to JobWorker via RunPool
        self.run_pool.route_result(job_result).await;
    }

    /// Route an error result back to the JobWorker.
    async fn route_error(&self, envelope: &RunEnvelope, error: String) {
        // Release VM on error
        if let Err(e) = self.targets.release(&self.id) {
            warn!("[VM:{}] Failed to release on error: {}", self.id, e);
        }

        let job_result = JobRunResult {
            run_id: envelope.run_id.clone(),
            job_id: envelope.job_id.clone(),
            round_id: envelope.round_id.clone(),
            outcome: RunOutcome {
                detected: false,
                exit_code: -1,
                error: Some(error),
                success: false,
                elapsed_ms: 0.0,
                detection_verdict: "infra_error".to_string(),
                last_checkpoint: String::new(),
            },
            vm_id: self.id.clone(),
        };
        self.run_pool.route_result(job_result).await;
    }

    // ========================================================================
    // Accessors
    // ========================================================================

    /// Get VM ID.
    #[allow(dead_code)]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get VM info.
    #[allow(dead_code)]
    pub fn info(&self) -> &VMInfo {
        &self.info
    }

    /// Check if executor is idle (no in-flight run).
    #[allow(dead_code)]
    pub fn is_idle(&self) -> bool {
        self.in_flight.is_none()
    }
}

impl std::fmt::Debug for VMExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VMExecutor")
            .field("id", &self.id)
            .field("os", &self.info.os)
            .field("idle", &self.in_flight.is_none())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::TargetEvent;
    use std::path::Path;

    #[derive(Debug)]
    struct MockArtifactSender;

    impl ArtifactSender for MockArtifactSender {
        fn send_artifact(
            &self,
            _vm_id: &str,
            _artifact_id: &str,
            _path: &Path,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn test_vm_executor_creation() {
        let pool = Arc::new(RunPool::new());
        let (events_tx, _) = mpsc::channel::<TargetEvent>(10);
        let targets = Arc::new(TargetManager::new(30, events_tx, Arc::clone(&pool)));
        let (remote_tx, _) = mpsc::channel(100);
        let (_, remote_rx) = mpsc::channel(100);

        let executor = VMExecutor::new(
            "test-vm".to_string(),
            VMInfo {
                id: "test-vm".to_string(),
                os: "windows".to_string(),
                capabilities: vec!["defender".to_string()],
            },
            targets,
            pool,
            remote_tx,
            remote_rx,
            Arc::new(MockArtifactSender),
        );

        assert_eq!(executor.id(), "test-vm");
        assert!(executor.is_idle());
    }
}
