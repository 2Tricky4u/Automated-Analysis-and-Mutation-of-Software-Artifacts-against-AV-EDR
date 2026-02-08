//! Job handlers - dispatch-based job management
//!
//! Uses JobSession -> Orchestrator -> Worker pattern

use crate::automutate::common::JobId;
use crate::automutate::controller::{
    BehaviorComparisonProto, CompareRunsRequest, CompareRunsResponse, GetRoundRequest,
    GetRoundResponse, JobProgressRequest, JobProgressResponse, JobRequest, JobResponse,
    JobStatusRequest, JobStatusResponse, RoundSummaryProto, StatusAck, StatusReport,
    StopJobRequest, StopJobResponse,
};
use crate::dispatch::{JobControlCommand, JobSession, ModularBuildSpec, ModuleSelectionSpec, JobId as DispatchJobId};
use crate::api::SchedulerService;
use elasticsearch::SearchParts;
use serde_json::{json, Value};
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
    let default_modules = ModuleSelectionSpec::default();
    let modules = if let Some(m) = &req.modules {
        ModuleSelectionSpec {
            carrier: if m.carrier.is_empty() { default_modules.carrier } else { m.carrier.clone() },
            decoder: if m.decoder.is_empty() { default_modules.decoder } else { m.decoder.clone() },
            antiemulation: if m.antiemulation.is_empty() { default_modules.antiemulation } else { m.antiemulation.clone() },
            guardrail: if m.guardrail.is_empty() { default_modules.guardrail } else { m.guardrail.clone() },
            virtualprotect: if m.virtualprotect.is_empty() { default_modules.virtualprotect } else { m.virtualprotect.clone() },
            decoy: if m.decoy.is_empty() { default_modules.decoy } else { m.decoy.clone() },
        }
    } else {
        default_modules
    };

    // Get encoding (default to xor)
    let encoding = if req.encoding.is_empty() { "xor".to_string() } else { req.encoding.clone() };

    // Create build spec from request
    let build_spec = ModularBuildSpec {
        modules,
        payload_path: PathBuf::from(&req.source),
        encoding,
    };

    info!(
        "Job submission: {} (payload={}, target_os={}, caps={:?}, carrier={}, decoder={}, encoding={}, max_rounds={})",
        job_id, req.source,
        if req.target_os.is_empty() { "any" } else { &req.target_os },
        req.required_capabilities,
        build_spec.modules.carrier, build_spec.modules.decoder,
        build_spec.encoding, max_rounds
    );

    // Create JobSession with build spec
    let mut job = JobSession::new(&job_id, max_rounds, build_spec);

    // Set constraints
    job.target_os = if req.target_os.is_empty() { None } else { Some(req.target_os.clone()) };
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
/// Query ES for stored job state.
pub async fn get_job_status(
    service: &SchedulerService,
    request: Request<JobStatusRequest>,
) -> Result<Response<JobStatusResponse>, Status> {
    let req = request.into_inner();
    debug!("[RPC] GetJobStatus: job_id={}", req.job_id);

    // Query ES for job document across all jobs-* indices
    let search_result = service
        .es_client
        .search(SearchParts::Index(&["jobs-*"]))
        .body(json!({
            "query": {
                "term": { "job_id": req.job_id }
            },
            "size": 1
        }))
        .send()
        .await;

    match search_result {
        Ok(response) => {
            if let Ok(body) = response.json::<Value>().await {
                if let Some(hits) = body["hits"]["hits"].as_array() {
                    if let Some(hit) = hits.first() {
                        let source = &hit["_source"];
                        let status = source["status"].as_str().unwrap_or("unknown").to_string();
                        let current_round = source["current_round"].as_u64().unwrap_or(0) as u32;
                        let max_rounds = source["max_rounds"].as_u64().unwrap_or(0) as u32;
                        let progress = if max_rounds > 0 {
                            ((current_round as f64 / max_rounds as f64) * 100.0) as i32
                        } else {
                            0
                        };

                        return Ok(Response::new(JobStatusResponse {
                            job_id: req.job_id,
                            status,
                            progress_percent: progress,
                            current_phase: format!("Round {}/{}", current_round, max_rounds),
                            logs: vec![],
                        }));
                    }
                }
            }
            // Job not found in ES
            Ok(Response::new(JobStatusResponse {
                job_id: req.job_id,
                status: "not_found".to_string(),
                progress_percent: 0,
                current_phase: "Job not found in Elasticsearch".to_string(),
                logs: vec![],
            }))
        }
        Err(e) => {
            error!("ES query failed: {}", e);
            Ok(Response::new(JobStatusResponse {
                job_id: req.job_id,
                status: "error".to_string(),
                progress_percent: 0,
                current_phase: format!("ES query failed: {}", e),
                logs: vec![],
            }))
        }
    }
}

/// Get detailed job progress
pub async fn get_job_progress(
    service: &SchedulerService,
    request: Request<JobProgressRequest>,
) -> Result<Response<JobProgressResponse>, Status> {
    let job_id = &request.get_ref().job_id;
    debug!("[RPC] GetJobProgress: job_id={}", job_id);

    // Query ES for job document
    let job_result = service
        .es_client
        .search(SearchParts::Index(&["jobs-*"]))
        .body(json!({
            "query": { "term": { "job_id": job_id } },
            "size": 1
        }))
        .send()
        .await;

    let (status, current_round, max_rounds) = match job_result {
        Ok(response) => {
            if let Ok(body) = response.json::<Value>().await {
                if let Some(hit) = body["hits"]["hits"].as_array().and_then(|h| h.first()) {
                    let source = &hit["_source"];
                    (
                        source["status"].as_str().unwrap_or("unknown").to_string(),
                        source["current_round"].as_u64().unwrap_or(0) as u32,
                        source["max_rounds"].as_u64().unwrap_or(0) as u32,
                    )
                } else {
                    ("not_found".to_string(), 0, 0)
                }
            } else {
                ("error".to_string(), 0, 0)
            }
        }
        Err(_) => ("error".to_string(), 0, 0),
    };

    // Query ES for rounds associated with this job
    let rounds_result = service
        .es_client
        .search(SearchParts::Index(&["rounds-*"]))
        .body(json!({
            "query": { "term": { "job_id": job_id } },
            "sort": [{ "round_number": "asc" }],
            "size": 100
        }))
        .send()
        .await;

    let mut rounds = Vec::new();
    if let Ok(response) = rounds_result {
        if let Ok(body) = response.json::<Value>().await {
            if let Some(hits) = body["hits"]["hits"].as_array() {
                for hit in hits {
                    let source = &hit["_source"];
                    rounds.push(RoundSummaryProto {
                        round_id: source["round_id"].as_str().unwrap_or("").to_string(),
                        round_number: source["round_number"].as_u64().unwrap_or(0) as u32,
                        mutations: source["mutations"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default(),
                        detected: source["detected"].as_bool().unwrap_or(false),
                        behavior_match: source["behavior_match"].as_bool().unwrap_or(false),
                        evasion_score: source["evasion_score"].as_f64().unwrap_or(0.0),
                        status: source["status"].as_str().unwrap_or("unknown").to_string(),
                        completed_at: source["completed_at"].as_i64().unwrap_or(0),
                    });
                }
            }
        }
    }

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

    // Send stop command to Orchestrator
    let cmd = JobControlCommand::Stop {
        job_id: DispatchJobId(job_id.clone()),
    };

    match service.job_control_tx.send(cmd).await {
        Ok(_) => {
            info!("[RPC] StopJob: Stop command sent for job {}", job_id);

            // Update job status in ES
            let _ = service
                .es_client
                .update(elasticsearch::UpdateParts::IndexId("jobs-*", job_id))
                .body(json!({
                    "doc": { "status": "stopping" }
                }))
                .send()
                .await;

            Ok(Response::new(StopJobResponse {
                stopped: true,
                message: format!("Job {} stop command sent", job_id),
            }))
        }
        Err(e) => {
            error!("[RPC] StopJob: Failed to send stop command for job {}: {}", job_id, e);
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
    debug!("[RPC] GetRound: job_id={}, round_id={}", req.job_id, req.round_id);

    // Query ES for round document
    let round_result = service
        .es_client
        .search(SearchParts::Index(&["rounds-*"]))
        .body(json!({
            "query": {
                "bool": {
                    "must": [
                        { "term": { "job_id": req.job_id } },
                        { "term": { "round_id": req.round_id } }
                    ]
                }
            },
            "size": 1
        }))
        .send()
        .await;

    match round_result {
        Ok(response) => {
            if let Ok(body) = response.json::<Value>().await {
                if let Some(hit) = body["hits"]["hits"].as_array().and_then(|h| h.first()) {
                    let source = &hit["_source"];

                    // TODO: GetRound implementation is incomplete - needs additional ES queries
                    let round = RoundProto {
                        round_id: source["round_id"].as_str().unwrap_or("").to_string(),
                        job_id: source["job_id"].as_str().unwrap_or("").to_string(),
                        round_number: source["round_number"].as_u64().unwrap_or(0) as u32,
                        mutations: vec![], // TODO: Parse mutations from ES
                        baseline_run: None,     // TODO: Query run details from runs-* index
                        instrumented_run: None, // TODO: Query run details from runs-* index
                        status: source["status"].as_str().unwrap_or("unknown").to_string(),
                        behavior_match: None,   // TODO: Parse comparison from round document
                    };

                    return Ok(Response::new(GetRoundResponse { round: Some(round) }));
                }
            }
            Ok(Response::new(GetRoundResponse { round: None }))
        }
        Err(e) => {
            error!("ES query failed for round: {}", e);
            Ok(Response::new(GetRoundResponse { round: None }))
        }
    }
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

    // Query ES for both runs
    let runs_result = service
        .es_client
        .search(SearchParts::Index(&["runs-*"]))
        .body(json!({
            "query": {
                "terms": {
                    "run_id": [req.baseline_run_id, req.instrumented_run_id]
                }
            },
            "size": 2
        }))
        .send()
        .await;

    match runs_result {
        Ok(response) => {
            if let Ok(body) = response.json::<Value>().await {
                if let Some(hits) = body["hits"]["hits"].as_array() {
                    let mut baseline_detected = false;
                    let mut baseline_exit_code = 0i32;
                    let mut instrumented_detected = false;
                    let mut instrumented_exit_code = 0i32;
                    let mut differences = Vec::new();

                    for hit in hits {
                        let source = &hit["_source"];
                        let run_id = source["run_id"].as_str().unwrap_or("");
                        let detected = source["detected"].as_bool().unwrap_or(false);
                        let exit_code = source["exit_code"].as_i64().unwrap_or(0) as i32;

                        if run_id == req.baseline_run_id {
                            baseline_detected = detected;
                            baseline_exit_code = exit_code;
                        } else if run_id == req.instrumented_run_id {
                            instrumented_detected = detected;
                            instrumented_exit_code = exit_code;
                        }
                    }

                    // Calculate differences
                    if baseline_detected != instrumented_detected {
                        differences.push(format!(
                            "Detection mismatch: baseline={}, instrumented={}",
                            baseline_detected, instrumented_detected
                        ));
                    }
                    if baseline_exit_code != instrumented_exit_code {
                        differences.push(format!(
                            "Exit code mismatch: baseline={}, instrumented={}",
                            baseline_exit_code, instrumented_exit_code
                        ));
                    }

                    let outcome_match = baseline_detected == instrumented_detected
                        && baseline_exit_code == instrumented_exit_code;

                    let confidence = if outcome_match { 1.0 } else { 0.5 };

                    return Ok(Response::new(CompareRunsResponse {
                        comparison: Some(BehaviorComparisonProto {
                            outcome_match,
                            baseline_detected,
                            baseline_exit_code,
                            instrumented_detected,
                            instrumented_exit_code,
                            differences,
                            confidence,
                        }),
                    }));
                }
            }
            Ok(Response::new(CompareRunsResponse { comparison: None }))
        }
        Err(e) => {
            error!("ES query failed for run comparison: {}", e);
            Ok(Response::new(CompareRunsResponse { comparison: None }))
        }
    }
}

/// Handle status reports from workers
pub async fn report_status(
    service: &SchedulerService,
    request: Request<StatusReport>,
) -> Result<Response<StatusAck>, Status> {
    use tokio::time::Duration;
    
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