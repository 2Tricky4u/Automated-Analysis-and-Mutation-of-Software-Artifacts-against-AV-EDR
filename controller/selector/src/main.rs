/// Selector gRPC Service
///
/// Implements CLAUDE.md Section 2: Selector component
/// "Selector: feedback-driven mutation selection with exploration vs exploitation"
///
/// gRPC service that:
/// - Selects mutations based on triage feedback
/// - Implements exploration/exploitation tradeoff
/// - Tracks outcome history for adaptive selection
use tonic::{transport::Server, Request, Response, Status};
use tracing::info;

pub mod edr {
    pub mod controller {
        tonic::include_proto!("edr.controller");
    }
    pub mod common {
        tonic::include_proto!("edr.common");
    }
}

use edr::controller::{
    selector_server::{Selector, SelectorServer},
    OutcomeAck, OutcomeReport, SelectionRequest, SelectionResponse,
};

#[derive(Debug, Default)]
pub struct SelectorService {}

impl SelectorService {
    pub fn new() -> Self {
        Self {}
    }
}

#[tonic::async_trait]
impl Selector for SelectorService {
    async fn select_mutation(
        &self,
        request: Request<SelectionRequest>,
    ) -> Result<Response<SelectionResponse>, Status> {
        let req = request.into_inner();
        info!("Selecting mutations for job: {:?}", req.job_id);

        // TODO: Implement selection logic
        // 1. Query triage engine for avoid-features
        // 2. Filter mutation pool to exclude avoid-features
        // 3. Apply exploration vs exploitation (epsilon-greedy)
        // 4. Return selected mutations with rationale

        Ok(Response::new(SelectionResponse {
            mutations: vec![],
            exploration_probability: 0.3,
            rationale: "Placeholder: random exploration".to_string(),
        }))
    }

    async fn report_outcome(
        &self,
        request: Request<OutcomeReport>,
    ) -> Result<Response<OutcomeAck>, Status> {
        let req = request.into_inner();
        info!("Outcome reported for run: {:?}", req.run_id);

        // TODO: Store outcome for feedback loop
        // 1. Update mutation success rates
        // 2. Adjust exploration probability
        // 3. Trigger triage analysis if detected

        Ok(Response::new(OutcomeAck { received: true }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr = "0.0.0.0:50054".parse()?;
    let selector = SelectorService::default();

    info!("Selector gRPC service starting on {}", addr);

    Server::builder()
        .add_service(SelectorServer::new(selector))
        .serve(addr)
        .await?;

    Ok(())
}
