// Service modules - RPC handler implementations
pub mod artifact_handlers;
pub mod job_handlers;
pub mod utility_handlers;
pub mod worker_handlers;

use crate::automutate::common::TelemetryData;
use crate::automutate::controller::{
    BuildRequest, BuildResponse, CompareRunsRequest, CompareRunsResponse, DeployRequest,
    DeployResponse, GetRoundRequest, GetRoundResponse, JobProgressRequest, JobProgressResponse,
    JobRequest, JobResponse, JobStatusRequest, JobStatusResponse, ListWorkersRequest,
    ListWorkersResponse, PingRequest, PingResponse, QueryRequest, QueryResponse, StatusAck,
    StatusReport, StopJobRequest, StopJobResponse, TriageRequest, TriageResponse,
    controller_server::Controller,
};
use crate::SchedulerService;
use tonic::{Request, Response, Status};

/// Implement Controller trait for SchedulerService by delegating to handler modules
#[tonic::async_trait]
impl Controller for SchedulerService {
    // Utility handlers
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        utility_handlers::ping(self, request).await
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

    async fn stream_telemetry(
        &self,
        request: Request<tonic::Streaming<TelemetryData>>,
    ) -> Result<Response<crate::automutate::common::TelemetryAck>, Status> {
        worker_handlers::stream_telemetry(self, request).await
    }
}
