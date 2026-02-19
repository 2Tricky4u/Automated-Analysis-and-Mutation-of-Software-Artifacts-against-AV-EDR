//! API module - gRPC handlers for Controller service
//!
//! Implements the Controller trait by delegating to handler modules.
//! Uses JobWorker architecture (JobSession -> Orchestrator -> JobWorker -> RunPool -> VMExecutor)

pub mod artifact;
pub mod extract;
pub mod job;
pub mod utility;
pub mod worker;

use crate::automutate::common::TelemetryData;
use crate::automutate::controller::{
    BuildRequest, BuildResponse, CompareRunsRequest, CompareRunsResponse, DeployRequest,
    DeployResponse, GetAvailableWorkersRequest, GetAvailableWorkersResponse,
    GetOrchestratorStatusRequest, GetOrchestratorStatusResponse, GetPoolMetricsRequest,
    GetPoolMetricsResponse, GetRoundRequest, GetRoundResponse, GetWorkerMetadataRequest,
    GetWorkerMetadataResponse, GetWorkerRequest, GetWorkerResponse, JobProgressRequest,
    JobProgressResponse, JobRequest, JobResponse, JobStatusRequest, JobStatusResponse,
    ListWorkersRequest, ListWorkersResponse, PingRequest, PingResponse, QueryRequest,
    QueryResponse, StatusAck, StatusReport, StopJobRequest, StopJobResponse, TriageRequest,
    TriageResponse, controller_server::Controller,
};
use crate::dispatch::{JobControlCommand, JobSession, RunPool};
use crate::storage::EsStorage;
use crate::vm::TargetManager;
use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::{Request, Response, Status};

// ============================================================================
// SchedulerService
// ============================================================================

/// Main service struct holding shared state for gRPC handlers
#[derive(Clone)]
pub struct SchedulerService {
    pub storage: Arc<EsStorage>,
    pub job_tx: mpsc::Sender<JobSession>,
    pub job_control_tx: mpsc::Sender<JobControlCommand>,
    pub targets: Arc<TargetManager>,
    pub run_pool: Arc<RunPool>,
}

impl SchedulerService {
    pub fn new(
        storage: Arc<EsStorage>,
        job_tx: mpsc::Sender<JobSession>,
        job_control_tx: mpsc::Sender<JobControlCommand>,
        targets: Arc<TargetManager>,
        run_pool: Arc<RunPool>,
    ) -> Self {
        Self {
            storage,
            job_tx,
            job_control_tx,
            targets,
            run_pool,
        }
    }
}

// ============================================================================
// Controller trait implementation
// ============================================================================

#[tonic::async_trait]
impl Controller for SchedulerService {
    // Utility handlers
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        utility::ping(self, request).await
    }

    // Job handlers
    async fn schedule_job(
        &self,
        request: Request<JobRequest>,
    ) -> Result<Response<JobResponse>, Status> {
        job::schedule_job(self, request).await
    }

    async fn get_job_status(
        &self,
        request: Request<JobStatusRequest>,
    ) -> Result<Response<JobStatusResponse>, Status> {
        job::get_job_status(self, request).await
    }

    async fn submit_triage(
        &self,
        request: Request<TriageRequest>,
    ) -> Result<Response<TriageResponse>, Status> {
        utility::submit_triage(self, request).await
    }

    async fn query_results(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        utility::query_results(self, request).await
    }

    async fn stream_telemetry(
        &self,
        request: Request<tonic::Streaming<TelemetryData>>,
    ) -> Result<Response<crate::automutate::common::TelemetryAck>, Status> {
        worker::stream_telemetry(self, request).await
    }

    async fn report_status(
        &self,
        request: Request<StatusReport>,
    ) -> Result<Response<StatusAck>, Status> {
        job::report_status(self, request).await
    }

    // Artifact handlers
    async fn build_artifact(
        &self,
        request: Request<BuildRequest>,
    ) -> Result<Response<BuildResponse>, Status> {
        artifact::build_artifact(self, request).await
    }

    async fn deploy_artifact(
        &self,
        request: Request<DeployRequest>,
    ) -> Result<Response<DeployResponse>, Status> {
        artifact::deploy_artifact(self, request).await
    }

    // Worker handlers
    async fn list_workers(
        &self,
        request: Request<ListWorkersRequest>,
    ) -> Result<Response<ListWorkersResponse>, Status> {
        worker::list_workers(self, request).await
    }

    async fn get_job_progress(
        &self,
        request: Request<JobProgressRequest>,
    ) -> Result<Response<JobProgressResponse>, Status> {
        job::get_job_progress(self, request).await
    }

    async fn stop_job(
        &self,
        request: Request<StopJobRequest>,
    ) -> Result<Response<StopJobResponse>, Status> {
        job::stop_job(self, request).await
    }

    async fn get_round(
        &self,
        request: Request<GetRoundRequest>,
    ) -> Result<Response<GetRoundResponse>, Status> {
        job::get_round(self, request).await
    }

    async fn compare_runs(
        &self,
        request: Request<CompareRunsRequest>,
    ) -> Result<Response<CompareRunsResponse>, Status> {
        job::compare_runs(self, request).await
    }

    // Monitoring handlers
    async fn get_worker(
        &self,
        request: Request<GetWorkerRequest>,
    ) -> Result<Response<GetWorkerResponse>, Status> {
        worker::get_worker(self, request).await
    }

    async fn get_available_workers(
        &self,
        request: Request<GetAvailableWorkersRequest>,
    ) -> Result<Response<GetAvailableWorkersResponse>, Status> {
        worker::get_available_workers(self, request).await
    }

    async fn get_worker_metadata(
        &self,
        request: Request<GetWorkerMetadataRequest>,
    ) -> Result<Response<GetWorkerMetadataResponse>, Status> {
        worker::get_worker_metadata(self, request).await
    }

    async fn get_pool_metrics(
        &self,
        request: Request<GetPoolMetricsRequest>,
    ) -> Result<Response<GetPoolMetricsResponse>, Status> {
        worker::get_pool_metrics(self, request).await
    }

    async fn get_orchestrator_status(
        &self,
        request: Request<GetOrchestratorStatusRequest>,
    ) -> Result<Response<GetOrchestratorStatusResponse>, Status> {
        worker::get_orchestrator_status(self, request).await
    }
}
