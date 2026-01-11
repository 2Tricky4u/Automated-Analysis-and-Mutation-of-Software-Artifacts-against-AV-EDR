// Service modules - RPC handler implementations
pub mod artifact_handlers;
pub mod helpers;
pub mod info_handlers;
pub mod sample_handlers;
pub mod stream_handlers;

use crate::automutate::common::{
    ArtifactChunk, ControllerMessage, SampleRequest, SampleResponse, TelemetryData, WorkerMessage,
};
use crate::automutate::worker::{
    HealthRequest, HealthResponse, PingRequest, PingResponse, TelemetryRequest, TransferAck,
    WorkerInfoRequest, WorkerInfoResponse,
    worker_agent_server::WorkerAgent,
};
use crate::WorkerAgentService;
use tonic::{Request, Response, Status};

/// Implement WorkerAgent trait for WorkerAgentService by delegating to handler modules
#[tonic::async_trait]
impl WorkerAgent for WorkerAgentService {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        info_handlers::ping(self, request).await
    }

    async fn run_sample(
        &self,
        request: Request<SampleRequest>,
    ) -> Result<Response<SampleResponse>, Status> {
        sample_handlers::run_sample(self, request).await
    }

    async fn health_check(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        info_handlers::health_check(self, request).await
    }

    async fn send_artifact(
        &self,
        request: Request<tonic::Streaming<ArtifactChunk>>,
    ) -> Result<Response<TransferAck>, Status> {
        artifact_handlers::send_artifact(self, request).await
    }

    async fn get_worker_info(
        &self,
        request: Request<WorkerInfoRequest>,
    ) -> Result<Response<WorkerInfoResponse>, Status> {
        info_handlers::get_worker_info(self, request).await
    }

    type GetTelemetryStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<TelemetryData, Status>> + Send>,
    >;

    async fn get_telemetry(
        &self,
        request: Request<TelemetryRequest>,
    ) -> Result<Response<Self::GetTelemetryStream>, Status> {
        info_handlers::get_telemetry(self, request).await
    }

    type EstablishStreamStream =
        tokio_stream::wrappers::ReceiverStream<Result<WorkerMessage, Status>>;

    async fn establish_stream(
        &self,
        request: Request<tonic::Streaming<ControllerMessage>>,
    ) -> Result<Response<Self::EstablishStreamStream>, Status> {
        stream_handlers::establish_stream(self, request).await
    }
}
