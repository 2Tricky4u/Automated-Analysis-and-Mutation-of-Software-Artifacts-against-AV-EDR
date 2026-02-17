//! Job handlers - dispatch-based job management
//!
//! Thin gRPC handlers: validate request, call storage, map to proto response.
//! All ES logic lives in storage/ — handlers never touch SearchParts or json!({}).

use super::extract::{
    bool_field, f64_field, i32_field, parse_date_to_unix_secs, str_field, string_array_field,
    u32_field, u64_field,
};
use crate::api::SchedulerService;
use crate::automutate::common::JobId;
use crate::automutate::controller::{
    BehaviorComparisonProto, CompareRunsRequest, CompareRunsResponse, GetRoundRequest,
    GetRoundResponse, JobProgressRequest, JobProgressResponse, JobRequest, JobResponse,
    JobStatusRequest, JobStatusResponse, RoundSummaryProto, RunResultProto, StatusAck,
    StatusReport, StopJobRequest, StopJobResponse,
};
use crate::dispatch::{
    JobControlCommand, JobId as DispatchJobId, JobSession, ModularBuildSpec, ModuleSelectionSpec,
};
use serde_json::Value;
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
    let max_rounds = if req.max_rounds == 0 {
        10
    } else {
        req.max_rounds
    };

    // Validate source (payload path to .bin file)
    if req.source.is_empty() {
        return Ok(Response::new(JobResponse {
            job_id: None,
            accepted: false,
            message: "source field required (path to .bin payload)".to_string(),
            estimated_duration_seconds: 0,
            max_rounds: 0,
        }));
    }

    // Generate job ID
    let job_id = format!(
        "job-{}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().to_string()[..8]
    );

    // Build module selection from proto (use defaults if empty)
    let modules = match &req.modules {
        Some(m) => ModuleSelectionSpec::from_proto_or_default(m),
        None => ModuleSelectionSpec::default(),
    };

    // Get encoding (default to xor)
    let encoding = if req.encoding.is_empty() {
        "xor".to_string()
    } else {
        req.encoding.clone()
    };

    // Create build spec from request
    let build_spec = ModularBuildSpec {
        modules,
        payload_path: PathBuf::from(&req.source),
        encoding,
    };

    info!(
        "Job submission: {} (payload={}, target_os={}, caps={:?}, carrier={}, decoder={}, encoding={}, max_rounds={})",
        job_id,
        req.source,
        if req.target_os.is_empty() {
            "any"
        } else {
            &req.target_os
        },
        req.required_capabilities,
        build_spec.modules.carrier,
        build_spec.modules.decoder,
        build_spec.encoding,
        max_rounds
    );

    // Create JobSession with build spec
    let mut job = JobSession::new(&job_id, max_rounds, build_spec);

    // Set constraints
    job.target_os = if req.target_os.is_empty() {
        None
    } else {
        Some(req.target_os.clone())
    };
    job.required_capabilities = req.required_capabilities.clone();
    job.stop_on_evasion = req.stop_on_evasion;

    // Index to ES before submission
    if let Err(e) = service.storage.index_job(&job).await {
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
pub async fn get_job_status(
    service: &SchedulerService,
    request: Request<JobStatusRequest>,
) -> Result<Response<JobStatusResponse>, Status> {
    let req = request.into_inner();
    debug!("[RPC] GetJobStatus: job_id={}", req.job_id);

    match service.storage.query_job(&req.job_id).await {
        Some(source) => {
            let status = str_field(&source, "status");
            let current_round = u32_field(&source, "current_round");
            let max_rounds = u32_field(&source, "max_rounds");
            let progress = if max_rounds > 0 {
                ((current_round as f64 / max_rounds as f64) * 100.0) as i32
            } else {
                0
            };

            Ok(Response::new(JobStatusResponse {
                job_id: req.job_id,
                status,
                progress_percent: progress,
                current_phase: format!("Round {}/{}", current_round, max_rounds),
                logs: vec![],
            }))
        }
        None => Ok(Response::new(JobStatusResponse {
            job_id: req.job_id,
            status: "not_found".to_string(),
            progress_percent: 0,
            current_phase: "Job not found in Elasticsearch".to_string(),
            logs: vec![],
        })),
    }
}

/// Get detailed job progress
pub async fn get_job_progress(
    service: &SchedulerService,
    request: Request<JobProgressRequest>,
) -> Result<Response<JobProgressResponse>, Status> {
    let job_id = &request.get_ref().job_id;
    debug!("[RPC] GetJobProgress: job_id={}", job_id);

    // Query job + rounds in parallel
    let (job_doc, round_docs) = tokio::join!(
        service.storage.query_job(job_id),
        service.storage.query_rounds(job_id),
    );

    let (status, current_round, max_rounds) = match job_doc {
        Some(source) => (
            str_field(&source, "status"),
            u32_field(&source, "current_round"),
            u32_field(&source, "max_rounds"),
        ),
        None => ("not_found".to_string(), 0, 0),
    };

    let rounds: Vec<RoundSummaryProto> = round_docs.iter().map(round_doc_to_proto).collect();

    let progress = if max_rounds > 0 {
        ((current_round as f64 / max_rounds as f64) * 100.0) as u32
    } else {
        0
    };

    Ok(Response::new(JobProgressResponse {
        job_id: job_id.clone(),
        status,
        current_round,
        max_rounds,
        progress_percent: progress,
        rounds,
    }))
}

/// Stop a running job
pub async fn stop_job(
    service: &SchedulerService,
    request: Request<StopJobRequest>,
) -> Result<Response<StopJobResponse>, Status> {
    let job_id = &request.get_ref().job_id;
    info!("[RPC] StopJob: job_id={}", job_id);

    let cmd = JobControlCommand::Stop {
        job_id: DispatchJobId(job_id.clone()),
    };

    match service.job_control_tx.send(cmd).await {
        Ok(_) => {
            info!("[RPC] StopJob: Stop command sent for job {}", job_id);
            let _ = service
                .storage
                .update_job_field(job_id, "status", "stopping")
                .await;

            Ok(Response::new(StopJobResponse {
                stopped: true,
                message: format!("Job {} stop command sent", job_id),
            }))
        }
        Err(e) => {
            error!(
                "[RPC] StopJob: Failed to send stop command for job {}: {}",
                job_id, e
            );
            Ok(Response::new(StopJobResponse {
                stopped: false,
                message: format!("Failed to stop job {}: channel closed", job_id),
            }))
        }
    }
}

/// Get detailed round information
pub async fn get_round(
    service: &SchedulerService,
    request: Request<GetRoundRequest>,
) -> Result<Response<GetRoundResponse>, Status> {
    use crate::automutate::controller::RoundProto;

    let req = request.get_ref();
    debug!(
        "[RPC] GetRound: job_id={}, round_id={}",
        req.job_id, req.round_id
    );

    let source = match service.storage.query_round(&req.job_id, &req.round_id).await {
        Some(s) => s,
        None => return Ok(Response::new(GetRoundResponse { round: None })),
    };

    // Parse mutations
    let mutations = string_array_field(&source, "mutations")
        .into_iter()
        .map(|id| crate::automutate::common::Mutation {
            id,
            params: Default::default(),
        })
        .collect();

    // Query both runs
    let baseline_run_id = str_field(&source, "baseline_run_id");
    let instrumented_run_id = str_field(&source, "instrumented_run_id");

    let runs = service
        .storage
        .query_runs_by_ids(&[&baseline_run_id, &instrumented_run_id])
        .await;

    let mut baseline_run = None;
    let mut instrumented_run = None;
    for run in &runs {
        let rid = str_field(run, "run_id");
        let proto = run_doc_to_proto(run);
        if rid == baseline_run_id {
            baseline_run = Some(proto);
        } else if rid == instrumented_run_id {
            instrumented_run = Some(proto);
        }
    }

    // Build behavior comparison
    let behavior_match = if source["behavior_match"].is_boolean() {
        Some(build_behavior_comparison(
            &source,
            &baseline_run,
            &instrumented_run,
        ))
    } else {
        None
    };

    let round = RoundProto {
        round_id: str_field(&source, "round_id"),
        job_id: str_field(&source, "job_id"),
        round_number: u32_field(&source, "round_number"),
        mutations,
        baseline_run,
        instrumented_run,
        status: str_field(&source, "status"),
        behavior_match,
    };

    Ok(Response::new(GetRoundResponse { round: Some(round) }))
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

    let runs = service
        .storage
        .query_runs_by_ids(&[&req.baseline_run_id, &req.instrumented_run_id])
        .await;

    if runs.is_empty() {
        return Ok(Response::new(CompareRunsResponse { comparison: None }));
    }

    // Reuse run_doc_to_proto to extract fields consistently
    let mut baseline: Option<RunResultProto> = None;
    let mut instrumented: Option<RunResultProto> = None;
    for run in &runs {
        let proto = run_doc_to_proto(run);
        if proto.run_id == req.baseline_run_id {
            baseline = Some(proto);
        } else if proto.run_id == req.instrumented_run_id {
            instrumented = Some(proto);
        }
    }

    let b = baseline.as_ref();
    let i = instrumented.as_ref();
    let b_detected = b.map(|r| r.detected).unwrap_or(false);
    let i_detected = i.map(|r| r.detected).unwrap_or(false);
    let b_exit = b.map(|r| r.exit_code).unwrap_or(0);
    let i_exit = i.map(|r| r.exit_code).unwrap_or(0);

    let mut differences = Vec::new();
    if b_detected != i_detected {
        differences.push(format!(
            "Detection mismatch: baseline={}, instrumented={}",
            b_detected, i_detected
        ));
    }
    if b_exit != i_exit {
        differences.push(format!(
            "Exit code mismatch: baseline={}, instrumented={}",
            b_exit, i_exit
        ));
    }

    let outcome_match = b_detected == i_detected && b_exit == i_exit;
    let confidence = if outcome_match { 1.0 } else { 0.5 };

    Ok(Response::new(CompareRunsResponse {
        comparison: Some(BehaviorComparisonProto {
            outcome_match,
            baseline_detected: b_detected,
            baseline_exit_code: b_exit,
            instrumented_detected: i_detected,
            instrumented_exit_code: i_exit,
            differences,
            confidence,
        }),
    }))
}

/// Handle status reports from workers
pub async fn report_status(
    service: &SchedulerService,
    request: Request<StatusReport>,
) -> Result<Response<StatusAck>, Status> {
    use tokio::time::Duration;

    let report = request.into_inner();

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
        "error" | "timeout" | "stuck" | "crashed" => warn!("[WARN] {}", status_line),
        _ => info!("[STATUS] {}", status_line),
    }

    let _ = service.targets.update_health(&report.worker_id);

    if matches!(report.event_type.as_str(), "success" | "error" | "timeout") {
        match tokio::time::timeout(
            Duration::from_secs(10),
            service.storage.index_run_status(&report),
        )
        .await
        {
            Ok(Ok(())) => debug!("[OK] Run result stored: {}", report.run_id),
            Ok(Err(e)) => error!("[ERROR] Failed to store run result: {}", e),
            Err(_) => error!("[TIMEOUT] ES timeout storing run result"),
        }
    }

    Ok(Response::new(StatusAck { received: true }))
}

// ===========================================================================
// Proto mapping helpers
// ===========================================================================

fn round_doc_to_proto(source: &Value) -> RoundSummaryProto {
    RoundSummaryProto {
        round_id: str_field(source, "round_id"),
        round_number: u32_field(source, "round_number"),
        mutations: string_array_field(source, "mutations"),
        detected: bool_field(source, "detected"),
        behavior_match: bool_field(source, "behavior_match"),
        evasion_score: f64_field(source, "evasion_score"),
        status: str_field(source, "status"),
        completed_at: parse_date_to_unix_secs(&source["completed_at"]),
    }
}

fn run_doc_to_proto(source: &Value) -> RunResultProto {
    RunResultProto {
        run_id: str_field(source, "run_id"),
        job_id: str_field(source, "job_id"),
        round_id: str_field(source, "round_id"),
        run_type: match source["run_type"].as_str().unwrap_or("") {
            "baseline" => 1,
            "instrumented" => 2,
            _ => 0,
        },
        artifact_id: String::new(),
        mutations: string_array_field(source, "mutations"),
        outcome: str_field(source, "detection_outcome"),
        detected: bool_field(source, "detected"),
        exit_code: i32_field(source, "exit_code"),
        telemetry_events_count: u64_field(source, "telemetry_events_count"),
        elapsed_seconds: u64_field(source, "elapsed_seconds"),
        started_at: 0,
        completed_at: 0,
    }
}

fn build_behavior_comparison(
    source: &Value,
    baseline: &Option<RunResultProto>,
    instrumented: &Option<RunResultProto>,
) -> BehaviorComparisonProto {
    let b_detected = baseline.as_ref().map(|r| r.detected).unwrap_or(false);
    let i_detected = instrumented.as_ref().map(|r| r.detected).unwrap_or(false);
    let b_exit = baseline.as_ref().map(|r| r.exit_code).unwrap_or(0);
    let i_exit = instrumented.as_ref().map(|r| r.exit_code).unwrap_or(0);
    let outcome_match = bool_field(source, "behavior_match");

    BehaviorComparisonProto {
        outcome_match,
        baseline_detected: b_detected,
        baseline_exit_code: b_exit,
        instrumented_detected: i_detected,
        instrumented_exit_code: i_exit,
        differences: vec![],
        confidence: if outcome_match { 1.0 } else { 0.5 },
    }
}
