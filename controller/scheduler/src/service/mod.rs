//! Service module - gRPC handlers for Controller service
//!
//! Implements the Controller trait by delegating to handler modules.
//! Uses dispatch-based architecture (JobSession -> Orchestrator -> Worker)

pub mod artifact_handlers;
pub mod job_handlers;
pub mod utility_handlers;
pub mod worker_handlers;

use crate::automutate::common::TelemetryData;
use crate::automutate::controller::{
    controller_server::Controller, BuildRequest, BuildResponse, CompareRunsRequest,
    CompareRunsResponse, DeployRequest, DeployResponse, GetAvailableWorkersRequest,
    GetAvailableWorkersResponse, GetOrchestratorStatusRequest, GetOrchestratorStatusResponse,
    GetPoolMetricsRequest, GetPoolMetricsResponse, GetRoundRequest, GetRoundResponse,
    GetWorkerMetadataRequest, GetWorkerMetadataResponse, GetWorkerRequest, GetWorkerResponse,
    JobProgressRequest, JobProgressResponse, JobRequest, JobResponse, JobStatusRequest,
    JobStatusResponse, ListWorkersRequest, ListWorkersResponse, PingRequest, PingResponse,
    QueryRequest, QueryResponse, StatusAck, StatusReport, StopJobRequest, StopJobResponse,
    TriageRequest, TriageResponse,
};
use crate::dispatch::JobSession;
use crate::target_manager::TargetManager;
use elasticsearch::Elasticsearch;
use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::{Request, Response, Status};

// ============================================================================
// SchedulerService
// ============================================================================

/// Main service struct holding shared state for gRPC handlers
#[derive(Clone)]
pub struct SchedulerService {
    pub es_client: Elasticsearch,
    pub job_tx: mpsc::Sender<JobSession>,
    pub targets: Arc<TargetManager>,
}

impl SchedulerService {
    pub fn new(
        es_client: Elasticsearch,
        job_tx: mpsc::Sender<JobSession>,
        targets: Arc<TargetManager>,
    ) -> Self {
        Self {
            es_client,
            job_tx,
            targets,
        }
    }

    /// Index telemetry events to Elasticsearch
    pub async fn index_telemetry_batch(
        &self,
        events: &[TelemetryData],
    ) -> anyhow::Result<()> {
        use elasticsearch::IndexParts;
        use serde_json::json;

        if events.is_empty() {
            return Ok(());
        }

        let index_name = format!("telemetry-{}", chrono::Utc::now().format("%Y.%m"));

        // Index each event individually (simpler than bulk for now)
        for event in events {
            let doc = json!({
                "event_type": event.event_type,
                "timestamp": event.timestamp,
                "job_id": event.job_id,
                "metadata": event.metadata,
            });

            let response = self
                .es_client
                .index(IndexParts::Index(&index_name))
                .body(doc)
                .send()
                .await?;

            if !response.status_code().is_success() {
                return Err(anyhow::anyhow!(
                    "Index failed: {}",
                    response.status_code()
                ));
            }
        }

        Ok(())
    }

    /// Index job metadata to Elasticsearch
    pub async fn index_job(&self, job: &JobSession) -> anyhow::Result<()> {
        use elasticsearch::IndexParts;
        use serde_json::json;

        let index_name = format!("jobs-{}", chrono::Utc::now().format("%Y.%m"));

        let doc = json!({
            "job_id": job.id.0,
            "target_os": job.target_os,
            "required_capabilities": job.required_capabilities,
            "max_rounds": job.max_rounds,
            "current_round": job.current_round,
            "created_at": job.created_at.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default().as_secs(),
            "status": "queued",
            "build_spec": {
                "payload_path": job.build_spec.payload_path.display().to_string(),
                "encoding": job.build_spec.encoding,
                "carrier": job.build_spec.modules.carrier,
                "decoder": job.build_spec.modules.decoder,
            },
        });

        let response = self
            .es_client
            .index(IndexParts::IndexId(&index_name, &job.id.0))
            .body(doc)
            .send()
            .await?;

        if !response.status_code().is_success() {
            return Err(anyhow::anyhow!(
                "Index failed: {}",
                response.status_code()
            ));
        }

        Ok(())
    }

    /// Store run result to Elasticsearch
    pub async fn store_run_result(&self, report: &StatusReport) -> anyhow::Result<()> {
        use elasticsearch::IndexParts;
        use serde_json::json;

        let index_name = format!("runs-{}", chrono::Utc::now().format("%Y.%m"));

        let doc = json!({
            "run_id": report.run_id,
            "job_id": report.job_id,
            "worker_id": report.worker_id,
            "worker_ip": report.worker_ip,
            "artifact_name": report.artifact_name,
            "event_type": report.event_type,
            "details": report.details,
            "pid": report.pid,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        let response = self
            .es_client
            .index(IndexParts::Index(&index_name))
            .body(doc)
            .send()
            .await?;

        if !response.status_code().is_success() {
            return Err(anyhow::anyhow!(
                "Index failed: {}",
                response.status_code()
            ));
        }

        Ok(())
    }
}

// ============================================================================
// Controller trait implementation
// ============================================================================

#[tonic::async_trait]
impl Controller for SchedulerService {
    // Utility handlers
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        utility_handlers::ping(self, request).await
    }

    // Job handlers
    async fn schedule_job(
        &self,
        request: Request<JobRequest>,
    ) -> Result<Response<JobResponse>, Status> {
        job_handlers::schedule_job(self, request).await
    }

    async fn get_job_status(
        &self,
        request: Request<JobStatusRequest>,
    ) -> Result<Response<JobStatusResponse>, Status> {
        job_handlers::get_job_status(self, request).await
    }

    async fn submit_triage(
        &self,
        request: Request<TriageRequest>,
    ) -> Result<Response<TriageResponse>, Status> {
        utility_handlers::submit_triage(self, request).await
    }

    async fn query_results(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        utility_handlers::query_results(self, request).await
    }

    async fn stream_telemetry(
        &self,
        request: Request<tonic::Streaming<TelemetryData>>,
    ) -> Result<Response<crate::automutate::common::TelemetryAck>, Status> {
        worker_handlers::stream_telemetry(self, request).await
    }

    async fn report_status(
        &self,
        request: Request<StatusReport>,
    ) -> Result<Response<StatusAck>, Status> {
        job_handlers::report_status(self, request).await
    }

    // Artifact handlers
    async fn build_artifact(
        &self,
        request: Request<BuildRequest>,
    ) -> Result<Response<BuildResponse>, Status> {
        artifact_handlers::build_artifact(self, request).await
    }

    async fn deploy_artifact(
        &self,
        request: Request<DeployRequest>,
    ) -> Result<Response<DeployResponse>, Status> {
        artifact_handlers::deploy_artifact(self, request).await
    }

    // Worker handlers
    async fn list_workers(
        &self,
        request: Request<ListWorkersRequest>,
    ) -> Result<Response<ListWorkersResponse>, Status> {
        worker_handlers::list_workers(self, request).await
    }

    async fn get_job_progress(
        &self,
        request: Request<JobProgressRequest>,
    ) -> Result<Response<JobProgressResponse>, Status> {
        job_handlers::get_job_progress(self, request).await
    }

    async fn stop_job(
        &self,
        request: Request<StopJobRequest>,
    ) -> Result<Response<StopJobResponse>, Status> {
        job_handlers::stop_job(self, request).await
    }

    async fn get_round(
        &self,
        request: Request<GetRoundRequest>,
    ) -> Result<Response<GetRoundResponse>, Status> {
        job_handlers::get_round(self, request).await
    }

    async fn compare_runs(
        &self,
        request: Request<CompareRunsRequest>,
    ) -> Result<Response<CompareRunsResponse>, Status> {
        job_handlers::compare_runs(self, request).await
    }

    // Monitoring handlers
    async fn get_worker(
        &self,
        request: Request<GetWorkerRequest>,
    ) -> Result<Response<GetWorkerResponse>, Status> {
        worker_handlers::get_worker(self, request).await
    }

    async fn get_available_workers(
        &self,
        request: Request<GetAvailableWorkersRequest>,
    ) -> Result<Response<GetAvailableWorkersResponse>, Status> {
        worker_handlers::get_available_workers(self, request).await
    }

    async fn get_worker_metadata(
        &self,
        request: Request<GetWorkerMetadataRequest>,
    ) -> Result<Response<GetWorkerMetadataResponse>, Status> {
        worker_handlers::get_worker_metadata(self, request).await
    }

    async fn get_pool_metrics(
        &self,
        request: Request<GetPoolMetricsRequest>,
    ) -> Result<Response<GetPoolMetricsResponse>, Status> {
        worker_handlers::get_pool_metrics(self, request).await
    }

    async fn get_orchestrator_status(
        &self,
        request: Request<GetOrchestratorStatusRequest>,
    ) -> Result<Response<GetOrchestratorStatusResponse>, Status> {
        worker_handlers::get_orchestrator_status(self, request).await
    }
}