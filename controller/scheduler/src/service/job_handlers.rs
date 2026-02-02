//! Job handlers - dispatch-based job management
//!
//! Uses JobSession -> Orchestrator -> Worker pattern

use crate::automutate::common::JobId;
use crate::automutate::controller::{
    BehaviorComparisonProto, CompareRunsRequest, CompareRunsResponse, GetRoundRequest,
    GetRoundResponse, JobProgressRequest, JobProgressResponse, JobRequest, JobResponse,
    JobStatusRequest, JobStatusResponse, RoundProto, RoundSummaryProto, StatusAck, StatusReport,
    StopJobRequest, StopJobResponse,
};
use crate::dispatch::JobSession;
use crate::service::SchedulerService;
use std::path::PathBuf;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

/// Schedule a new job via dispatch system
pub async fn schedule_job(
    service: &SchedulerService,
    request: Request<JobRequest>,
) -> Result<Response<JobResponse>, Status> {
    let req = request.into_inner();

    // Get max_rounds (default to 10)
    let max_rounds = if req.max_rounds == 0 { 10 } else { req.max_rounds };

    // Generate job ID
    let job_id = format!(
        "job-{}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().to_string()[..8]
    );

    info!(
        "Job submission: {} (type={}, source={}, max_rounds={})",
        job_id, req.artifact_type, req.source, max_rounds
    );

    // Create JobSession
    let mut job = JobSession::new(&job_id, max_rounds);

    // Set payload path from source
    if !req.source.is_empty() {
        job.payload_path = Some(PathBuf::from(&req.source));
    }

    // Set constraints
    job.required_capabilities = req.required_capabilities.clone();
    job.stop_on_evasion = req.stop_on_evasion;

    // Index to ES before submission
    if let Err(e) = service.index_job(&job).await {
        warn!("Failed to index job to ES: {}", e);
    }

    // Submit to Orchestrator via channel
    match service.job_tx.send(job).await {
        Ok(()) => {
            info!("Job {} submitted to Orchestrator", job_id);
            Ok(Response::new(JobResponse {
                job_id: Some(JobId { value: job_id }),
                accepted: true,
                message: "Job queued for execution".to_string(),
                estimated_duration_seconds: 1,
                max_rounds,
            }))
        }
        Err(e) => {
            error!("Failed to submit job: {}", e);
            Ok(Response::new(JobResponse {
                job_id: None,
                accepted: false,
                message: format!("Failed to queue job: {}", e),
                estimated_duration_seconds: 0,
                max_rounds: 0,
            }))
        }
    }
}

/// Get job status
/// Note: With dispatch architecture, job state is tracked by Worker/Orchestrator.
/// For now, query ES for stored job state.
pub async fn get_job_status(
    service: &SchedulerService,
    request: Request<JobStatusRequest>,
) -> Result<Response<JobStatusResponse>, Status> {
    let req = request.into_inner();

    debug!("[RPC] GetJobStatus: job_id={}", req.job_id);

    // Query ES for job status
    // TODO: Implement ES query for job status
    Ok(Response::new(JobStatusResponse {
        job_id: req.job_id,
        status: "unknown".to_string(),
        progress_percent: 0,
        current_phase: "Status tracking via ES pending".to_string(),
        logs: vec![],
    }))
}

/// Get detailed job progress
pub async fn get_job_progress(
    service: &SchedulerService,
    request: Request<JobProgressRequest>,
) -> Result<Response<JobProgressResponse>, Status> {
    let job_id = &request.get_ref().job_id;

    debug!("[RPC] GetJobProgress: job_id={}", job_id);

    // Query ES for job progress
    // TODO: Implement ES query for rounds
    Ok(Response::new(JobProgressResponse {
        job_id: job_id.clone(),
        status: "unknown".to_string(),
        current_round: 0,
        max_rounds: 0,
        progress_percent: 0,
        rounds: vec![],
    }))
}

/// Stop a running job
pub async fn stop_job(
    service: &SchedulerService,
    request: Request<StopJobRequest>,
) -> Result<Response<StopJobResponse>, Status> {
    let job_id = &request.get_ref().job_id;

    debug!("[RPC] StopJob: job_id={}", job_id);

    // TODO: Send stop signal to Orchestrator
    // For now, just acknowledge
    warn!("StopJob not fully implemented in dispatch architecture");

    Ok(Response::new(StopJobResponse {
        stopped: false,
        message: "Job stop signaling pending implementation".to_string(),
    }))
}

/// Get detailed round information
pub async fn get_round(
    service: &SchedulerService,
    request: Request<GetRoundRequest>,
) -> Result<Response<GetRoundResponse>, Status> {
    let req = request.get_ref();

    debug!(
        "[RPC] GetRound: job_id={}, round_id={}",
        req.job_id, req.round_id
    );

    // Query ES for round data
    // TODO: Implement ES query
    Ok(Response::new(GetRoundResponse { round: None }))
}

/// Compare baseline vs instrumented runs
pub async fn compare_runs(
    service: &SchedulerService,
    request: Request<CompareRunsRequest>,
) -> Result<Response<CompareRunsResponse>, Status> {
    let req = request.get_ref();

    debug!(
        "[RPC] CompareRuns: baseline={}, instrumented={}",
        req.baseline_run_id, req.instrumented_run_id
    );

    // Query ES for run comparison
    // TODO: Implement ES query
    Ok(Response::new(CompareRunsResponse { comparison: None }))
}

/// Handle status reports from workers
pub async fn report_status(
    service: &SchedulerService,
    request: Request<StatusReport>,
) -> Result<Response<StatusAck>, Status> {
    use tokio::time::Duration;

    let remote_addr = request
        .remote_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let report = request.into_inner();

    // Log status
    let status_line = format!(
        "[WORKER: {} ({})] [PID: {}] [JOB: {}] [RUN: {}] [STATUS: {}] [ARTIFACT: {}] {}",
        report.worker_ip,
        report.worker_id,
        report.pid,
        report.job_id,
        report.run_id,
        report.event_type.to_uppercase(),
        report.artifact_name,
        report.details
    );

    match report.event_type.as_str() {
        "error" | "timeout" | "stuck" | "crashed" => {
            warn!("[WARN] {}", status_line);
        }
        _ => {
            info!("[STATUS] {}", status_line);
        }
    }

    // Update target health
    let _ = service.targets.update_health(&report.worker_id);

    // Store final status to ES
    if matches!(
        report.event_type.as_str(),
        "success" | "error" | "timeout"
    ) {
        match tokio::time::timeout(Duration::from_secs(10), service.store_run_result(&report))
            .await
        {
            Ok(Ok(())) => {
                debug!("[OK] Run result stored: {}", report.run_id);
            }
            Ok(Err(e)) => {
                error!("[ERROR] Failed to store run result: {}", e);
            }
            Err(_) => {
                error!("[TIMEOUT] ES timeout storing run result");
            }
        }
    }

    Ok(Response::new(StatusAck { received: true }))
}