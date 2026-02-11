// API modules - gRPC handler implementations (thin adapters)
pub mod artifacts;
pub mod info;
pub mod run;
pub mod stream;

use crate::automutate::common::{
    ArtifactChunk, ControllerMessage, SampleRequest, SampleResponse, TelemetryData, WorkerMessage,
};
use crate::automutate::worker::{
    HealthRequest, HealthResponse, PingRequest, PingResponse, TelemetryRequest, TransferAck,
    WorkerInfoRequest, WorkerInfoResponse,
    worker_agent_server::WorkerAgent,
};
use crate::WorkerAgentService;  // Now available from lib.rs
use tonic::{Request, Response, Status};

/// Implement WorkerAgent trait for WorkerAgentService by delegating to handler modules
#[tonic::async_trait]
impl WorkerAgent for WorkerAgentService {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        info::ping(self, request).await
    }

    async fn run_sample(
        &self,
        request: Request<SampleRequest>,
    ) -> Result<Response<SampleResponse>, Status> {
        run::run_sample(self, request).await
    }

    async fn health_check(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        info::health_check(self, request).await
    }

    async fn send_artifact(
        &self,
        request: Request<tonic::Streaming<ArtifactChunk>>,
    ) -> Result<Response<TransferAck>, Status> {
        artifacts::send_artifact(self, request).await
    }

    async fn get_worker_info(
        &self,
        request: Request<WorkerInfoRequest>,
    ) -> Result<Response<WorkerInfoResponse>, Status> {
        info::get_worker_info(self, request).await
    }

    type GetTelemetryStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<TelemetryData, Status>> + Send>,
    >;

    async fn get_telemetry(
        &self,
        request: Request<TelemetryRequest>,
    ) -> Result<Response<Self::GetTelemetryStream>, Status> {
        info::get_telemetry(self, request).await
    }

    type EstablishStreamStream =
        tokio_stream::wrappers::ReceiverStream<Result<WorkerMessage, Status>>;

    async fn establish_stream(
        &self,
        request: Request<tonic::Streaming<ControllerMessage>>,
    ) -> Result<Response<Self::EstablishStreamStream>, Status> {
        stream::establish_stream(self, request).await
    }
}
