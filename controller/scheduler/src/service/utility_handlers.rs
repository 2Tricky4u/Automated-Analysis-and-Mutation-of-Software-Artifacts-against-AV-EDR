use crate::automutate::controller::{
    PingRequest, PingResponse, QueryRequest, QueryResponse, TriageRequest, TriageResponse,
};
use crate::SchedulerService;
use tonic::{Request, Response, Status};
use tracing::info;

pub async fn ping(
    _service: &SchedulerService,
    request: Request<PingRequest>,
) -> Result<Response<PingResponse>, Status> {
    let req = request.into_inner();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    info!("Ping received: {}", req.message);

    Ok(Response::new(PingResponse {
        message: format!("pong: {}", req.message),
        timestamp,
        server: "controller/scheduler".to_string(),
    }))
}

pub async fn submit_triage(
    _service: &SchedulerService,
    request: Request<TriageRequest>,
) -> Result<Response<TriageResponse>, Status> {
    let req = request.into_inner();
    let triage_id = format!(
        "triage-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    info!(
        "Triage submitted for job {}: detected={}",
        req.job_id, req.detected
    );

    Ok(Response::new(TriageResponse {
        job_id: req.job_id,
        stored: true,
        triage_id,
    }))
}

pub async fn query_results(
    _service: &SchedulerService,
    request: Request<QueryRequest>,
) -> Result<Response<QueryResponse>, Status> {
    let _req = request.into_inner();

    Ok(Response::new(QueryResponse {
        results: vec![],
        total_count: 0,
    }))
}
