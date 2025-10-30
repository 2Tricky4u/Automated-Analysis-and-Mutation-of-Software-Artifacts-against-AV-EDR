/// Triage Engine gRPC Service
///
/// Implements CLAUDE.md Section 2: Triage Engine
/// "Triage: Surrogate classifier + hypothesis generation"
///
/// gRPC service that:
/// - Analyzes runs to generate explainable hypotheses
/// - Maintains avoid-features list from surrogate classifier
/// - Provides feedback to Selector for adaptive mutation
use tonic::{Request, Response, Status, transport::Server};
use tracing::info;

pub mod edr {
    pub mod common {
        tonic::include_proto!("edr.common");
    }
    pub mod controller {
        tonic::include_proto!("edr.controller");
    }
    pub mod worker {
        tonic::include_proto!("edr.worker");
    }
}

use edr::controller::{
    AnalysisRequest, AnalysisResponse, AvoidListRequest, AvoidListResponse,
    triage_server::{Triage, TriageServer},
};

#[derive(Debug, Default)]
pub struct TriageService {}

impl TriageService {
    pub fn new() -> Self {
        Self {}
    }
}

#[tonic::async_trait]
impl Triage for TriageService {
    async fn analyze_run(
        &self,
        request: Request<AnalysisRequest>,
    ) -> Result<Response<AnalysisResponse>, Status> {
        let req = request.into_inner();
        info!("Analyzing run: {:?}", req.run_id);

        // TODO: Implement triage analysis
        // 1. Fetch RunResult from Elasticsearch
        // 2. Extract typed features from telemetry
        // 3. Run surrogate classifier (logistic regression)
        // 4. Generate hypotheses with confidence scores
        // 5. Update avoid-features list

        Ok(Response::new(AnalysisResponse {
            hypotheses: vec![],
            avoid_features: vec![],
        }))
    }

    async fn get_avoid_list(
        &self,
        request: Request<AvoidListRequest>,
    ) -> Result<Response<AvoidListResponse>, Status> {
        let req = request.into_inner();
        info!("Getting avoid list for job: {:?}", req.job_id);

        // TODO: Return current avoid-features for this job
        // These are features that caused detection in previous runs

        Ok(Response::new(AvoidListResponse {
            features: vec![
                // Example: "mem.rwx_short_window", "thread.start.anon"
            ],
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr = "0.0.0.0:50055".parse()?;
    let triage = TriageService::default();

    info!("Triage Engine gRPC service starting on {}", addr);

    Server::builder()
        .add_service(TriageServer::new(triage))
        .serve(addr)
        .await?;

    Ok(())
}
