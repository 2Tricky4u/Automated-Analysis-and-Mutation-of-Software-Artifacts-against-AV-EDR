//! Control plane sink trait for decoupling execution logic from transport.
//!
//! The execution engine sends status updates and telemetry through this trait
//! rather than directly holding an Arc<StreamHandler>.

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::Status;
use tracing::debug;

use crate::automutate::common::{
    ExecutionStatusReport, TelemetryBatch, WorkerMessage, worker_message,
};

/// Trait for sending execution updates to the control plane.
///
/// Implementations:
/// - `StreamSink`: delegates to the stream tx channel (bidirectional stream mode)
/// - `NullSink`: no-op (worker-only mode, no stream)
#[tonic::async_trait]
pub trait ControlPlaneSink: Send + Sync {
    async fn send_status(&self, status: ExecutionStatusReport) -> Result<()>;
    async fn send_telemetry(&self, batch: TelemetryBatch) -> Result<()>;
    async fn send_ack(&self, request_id: &str, success: bool, error: &str) -> Result<()>;
}

/// Sink that sends messages directly via the stream's tx channel.
/// Does NOT hold Arc<StreamHandler>, breaking the reference cycle.
pub struct StreamSink {
    tx: mpsc::Sender<Result<WorkerMessage, Status>>,
}

impl StreamSink {
    pub fn new(tx: mpsc::Sender<Result<WorkerMessage, Status>>) -> Self {
        Self { tx }
    }
}

#[tonic::async_trait]
impl ControlPlaneSink for StreamSink {
    async fn send_status(&self, status: ExecutionStatusReport) -> Result<()> {
        self.tx
            .send(Ok(WorkerMessage {
                payload: Some(worker_message::Payload::ExecutionStatus(status)),
            }))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send execution status: {}", e))
    }

    async fn send_telemetry(&self, batch: TelemetryBatch) -> Result<()> {
        debug!(
            "StreamSink: sending telemetry batch ({} events, job={})",
            batch.events.len(),
            batch.job_id
        );
        self.tx
            .send(Ok(WorkerMessage {
                payload: Some(worker_message::Payload::Telemetry(batch)),
            }))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send telemetry: {}", e))
    }

    async fn send_ack(&self, request_id: &str, success: bool, error: &str) -> Result<()> {
        debug!(
            "StreamSink: ack request_id={}, success={}, error='{}'",
            request_id, success, error
        );
        Ok(())
    }
}

/// No-op sink for when no bidirectional stream is available
pub struct NullSink;

#[tonic::async_trait]
impl ControlPlaneSink for NullSink {
    async fn send_status(&self, _status: ExecutionStatusReport) -> Result<()> {
        debug!("NullSink: status update dropped (no stream)");
        Ok(())
    }

    async fn send_telemetry(&self, _batch: TelemetryBatch) -> Result<()> {
        debug!("NullSink: telemetry batch dropped (no stream)");
        Ok(())
    }

    async fn send_ack(&self, _request_id: &str, _success: bool, _error: &str) -> Result<()> {
        debug!("NullSink: ack dropped (no stream)");
        Ok(())
    }
}

/// Build a ControlPlaneSink from a stream handler's tx channel.
///
/// If tx is available (stream mode), creates a StreamSink.
/// Otherwise creates a NullSink (worker-only mode).
pub fn build_sink(
    tx: Option<&mpsc::Sender<Result<WorkerMessage, Status>>>,
) -> Arc<dyn ControlPlaneSink> {
    match tx {
        Some(tx) => Arc::new(StreamSink::new(tx.clone())),
        None => Arc::new(NullSink),
    }
}
