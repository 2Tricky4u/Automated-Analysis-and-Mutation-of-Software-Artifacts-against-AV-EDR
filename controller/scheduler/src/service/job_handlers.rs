use crate::automutate::common::JobId;
use crate::automutate::controller::{
    BehaviorComparisonProto, CompareRunsRequest, CompareRunsResponse, GetRoundRequest,
    GetRoundResponse, JobProgressRequest, JobProgressResponse, JobRequest, JobResponse,
    JobStatusRequest, JobStatusResponse, RoundProto, RoundSummaryProto, StatusAck, StatusReport,
    StopJobRequest, StopJobResponse,
};
use crate::job;
use crate::SchedulerService;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn, debug};

const DELAY: u64 = 20;

pub async fn schedule_job(
    service: &SchedulerService,
    request: Request<JobRequest>,
) -> Result<Response<JobResponse>, Status> {
    let req = request.into_inner();

    // Check if scheduler core is available
    let scheduler_core = match &service.scheduler_core {
        Some(core) => core,
        None => {
            warn!("Scheduler core not available, job submission rejected");
            return Ok(Response::new(JobResponse {
                job_id: None,
                accepted: false,
                message: "Scheduler core not initialized. Check that worker configs exist in automation/generated/".to_string(),
                estimated_duration_seconds: 0,
                max_rounds: 0,
            }));
        }
    };

    // Mutations are now selected per-round by Selector service
    let mutations: Vec<job::MutationSpec> = vec![]; // Empty for now

    // Get max_rounds from request (default to 10 if not specified)
    let max_rounds = if req.max_rounds == 0 {
        10
    } else {
        req.max_rounds
    };

    // Get stopping conditions from request (default to false)
    let stop_on_evasion = req.stop_on_evasion;
    let stop_on_detection = req.stop_on_detection;

    // Submit job to scheduler queue
    match scheduler_core.queue().submit_job(
        req.artifact_type.clone(), // template_name
        req.source.clone(),        // source_file
        mutations,
        "lines".to_string(), // Default trace mode //TODO make modular
        req.priority,
        max_rounds,
        stop_on_evasion,
        stop_on_detection,
    ) {
        Ok(job_id) => {
            info!(
                "Job {} submitted to scheduler queue: {} ({})",
                job_id, req.name, req.artifact_type
            );

            // Index job to Elasticsearch
            if let Some(job) = scheduler_core.queue().get_job(&job_id) {
                if let Err(e) = service.index_job(&job).await {
                    warn!("Failed to index job {} to Elasticsearch: {}", job_id, e);
                }
            }

            Ok(Response::new(JobResponse {
                job_id: Some(JobId {
                    value: job_id.clone(),
                }),
                accepted: true,
                message: format!("Job {} queued for execution", job_id),
                estimated_duration_seconds: 1, // Estimated from config timeout
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

pub async fn get_job_status(
    service: &SchedulerService,
    request: Request<JobStatusRequest>,
) -> Result<Response<JobStatusResponse>, Status> {
    let req = request.into_inner();

    // Check if scheduler core is available
    let scheduler_core = match &service.scheduler_core {
        Some(core) => core,
        None => {
            return Err(Status::unavailable("Scheduler core not initialized"));
        }
    };

    // Query job from scheduler queue
    match scheduler_core.queue().get_job(&req.job_id) {
        Some(job) => {
            let progress_percent = if job.is_terminal() {
                100
            } else {
                job.progress_percent()
            };

            Ok(Response::new(JobStatusResponse {
                job_id: job.id.clone(),
                status: job.status.to_string(),
                progress_percent: progress_percent as i32,
                current_phase: format!("{:?}", job.status),
                logs: vec![format!("Job {} is {}", job.id, job.status)],
            }))
        }
        None => Err(Status::not_found(format!("Job {} not found", req.job_id))),
    }
}

pub async fn get_job_progress(
    service: &SchedulerService,
    request: Request<JobProgressRequest>,
) -> Result<Response<JobProgressResponse>, Status> {
    let job_id = &request.get_ref().job_id;

    debug!("[RPC] GetJobProgress: job_id={}", job_id);

    // Get job from queue
    let scheduler = service
        .scheduler_core
        .as_ref()
        .ok_or_else(|| Status::internal("Scheduler not initialized"))?;
    let job = scheduler
        .queue()
        .get_job(job_id)
        .ok_or_else(|| Status::not_found(format!("Job not found: {}", job_id)))?;

    // Calculate progress percentage
    let progress_percent = job.progress_percent();

    // Convert rounds to protobuf format
    let rounds: Vec<RoundSummaryProto> = job
        .rounds
        .iter()
        .map(|r| RoundSummaryProto {
            round_id: r.round_id.clone(),
            round_number: r.round_number,
            mutations: r.mutations.clone(),
            detected: r.detected,
            behavior_match: r.behavior_match,
            evasion_score: r.evasion_score,
            status: if r.behavior_match {
                "completed".to_string()
            } else {
                "behavior_mismatch".to_string()
            },
            completed_at: r
                .completed_at
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        })
        .collect();

    let response = JobProgressResponse {
        job_id: job.id.clone(),
        status: job.status.to_string(),
        current_round: job.current_round,
        max_rounds: job.max_rounds,
        progress_percent,
        rounds,
    };

    Ok(Response::new(response))
}

pub async fn stop_job(
    service: &SchedulerService,
    request: Request<StopJobRequest>,
) -> Result<Response<StopJobResponse>, Status> {
    let job_id = &request.get_ref().job_id;

    debug!("[RPC] StopJob: job_id={}", job_id);

    // Get scheduler
    let scheduler = service
        .scheduler_core
        .as_ref()
        .ok_or_else(|| Status::internal("Scheduler not initialized"))?;

    // Get job from queue
    let mut job = scheduler
        .queue()
        .get_job(job_id)
        .ok_or_else(|| Status::not_found(format!("Job not found: {}", job_id)))?;

    // Check if job can be stopped (only stop running jobs)
    if !matches!(job.status, job::JobStatus::Running) {
        let message = format!(
            "Job {} cannot be stopped (current status: {})",
            job_id,
            job.status.to_string()
        );
        warn!("[RPC] {}", message);
        return Ok(Response::new(StopJobResponse {
            stopped: false,
            message,
        }));
    }

    // Mark job as stopped
    job.mark_stopped();

    // Update job in queue
    scheduler
        .queue()
        .update_job(&job)
        .map_err(|e| Status::internal(format!("Failed to update job: {}", e)))?;

    debug!("[RPC] Job {} stopped successfully", job_id);

    let response = StopJobResponse {
        stopped: true,
        message: format!("Job {} stopped after {} rounds", job_id, job.current_round),
    };

    Ok(Response::new(response))
}

pub async fn get_round(
    service: &SchedulerService,
    request: Request<GetRoundRequest>,
) -> Result<Response<GetRoundResponse>, Status> {
    let req = request.get_ref();
    let job_id = &req.job_id;
    let round_id = &req.round_id;

    debug!("[RPC] GetRound: job_id={}, round_id={}", job_id, round_id);

    // Get job from queue
    let scheduler = service
        .scheduler_core
        .as_ref()
        .ok_or_else(|| Status::internal("Scheduler not initialized"))?;
    let job = scheduler
        .queue()
        .get_job(job_id)
        .ok_or_else(|| Status::not_found(format!("Job not found: {}", job_id)))?;

    // Find the round
    let round_summary = job
        .rounds
        .iter()
        .find(|r| r.round_id == *round_id)
        .ok_or_else(|| Status::not_found(format!("Round not found: {}", round_id)))?;

    // Convert to RoundProto (detailed format)
    // we don't have full RunResult details yet

    // Infer status from RoundSummary fields
    let status = if round_summary.behavior_match {
        "completed"
    } else {
        "behavior_mismatch"
    };

    let round_proto = RoundProto {
        round_id: round_summary.round_id.clone(),
        job_id: job.id.clone(),
        round_number: round_summary.round_number,
        mutations: vec![],      // TODO Convert mutations to protobuf format
        baseline_run: None,     // TODO Add RunResult data
        instrumented_run: None, // TODO Add RunResult data
        status: status.to_string(),
        behavior_match: Some(BehaviorComparisonProto {
            outcome_match: round_summary.behavior_match,
            baseline_detected: round_summary.detected,
            baseline_exit_code: 0, // TODO Add from RunResult
            instrumented_detected: round_summary.detected,
            instrumented_exit_code: 0, // TODO Add from RunResult
            differences: vec![],       // TODO Add from BehaviorComparison
            confidence: 1.0,           // TODO  Add from BehaviorComparison
        }),
    };

    let response = GetRoundResponse {
        round: Some(round_proto),
    };

    Ok(Response::new(response))
}

pub async fn compare_runs(
    service: &SchedulerService,
    request: Request<CompareRunsRequest>,
) -> Result<Response<CompareRunsResponse>, Status> {
    let req = request.get_ref();
    let baseline_run_id = &req.baseline_run_id;
    let instrumented_run_id = &req.instrumented_run_id;

    debug!(
        "[RPC] CompareRuns: baseline={}, instrumented={}",
        baseline_run_id, instrumented_run_id
    );

    // Parse run IDs (format: job_id/round_id/run_type)
    let baseline_parts: Vec<&str> = baseline_run_id.split('/').collect();
    let instrumented_parts: Vec<&str> = instrumented_run_id.split('/').collect();

    if baseline_parts.len() != 3 || instrumented_parts.len() != 3 {
        return Err(Status::invalid_argument(
            "Invalid run ID format. Expected: job_id/round_id/run_type",
        ));
    }

    let job_id = baseline_parts[0];
    let round_id = baseline_parts[1];

    // Verify both runs are from same job and round
    if baseline_parts[0] != instrumented_parts[0] || baseline_parts[1] != instrumented_parts[1]
    {
        return Err(Status::invalid_argument(
            "Baseline and instrumented runs must be from same job and round",
        ));
    }

    // Get scheduler
    let scheduler = service
        .scheduler_core
        .as_ref()
        .ok_or_else(|| Status::internal("Scheduler not initialized"))?;

    // Get job
    let job = scheduler
        .queue()
        .get_job(job_id)
        .ok_or_else(|| Status::not_found(format!("Job not found: {}", job_id)))?;

    // Find the round
    let round_summary = job
        .rounds
        .iter()
        .find(|r| r.round_id == round_id)
        .ok_or_else(|| Status::not_found(format!("Round not found: {}", round_id)))?;

    // Create comparison response from round summary
    // should add full RunResult details
    let comparison = BehaviorComparisonProto {
        outcome_match: round_summary.behavior_match,
        baseline_detected: round_summary.detected,
        baseline_exit_code: 0, // TODO Get from stored RunResult
        instrumented_detected: round_summary.detected,
        instrumented_exit_code: 0, // TODO Get from stored RunResult
        differences: if round_summary.behavior_match {
            vec![]
        } else {
            vec!["Behavior mismatch detected".to_string()]
        },
        confidence: if round_summary.behavior_match {
            1.0
        } else {
            0.5
        },
    };

    let response = CompareRunsResponse {
        comparison: Some(comparison),
    };

    Ok(Response::new(response))
}

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

    // Update worker health and job status in scheduler core (if available)
    if let Some(ref scheduler_core) = service.scheduler_core {
        // Update worker health timestamp
        if let Err(e) = scheduler_core.pool().update_health(&report.worker_id).await {
            warn!(
                "Failed to update worker health for {}: {}",
                report.worker_id, e
            );
        }

        // Update job status based on status report
        if let Some(mut job) = scheduler_core.queue().get_job(&report.job_id) {
            match report.event_type.as_str() {
                "success" => {
                    job.mark_completed();
                    let _ = scheduler_core.queue().update_job(&job);
                    // Release worker
                    let _ = scheduler_core.pool().release_worker(&report.worker_id);
                }
                "error" => {
                    job.mark_failed(report.details.clone());
                    let _ = scheduler_core.queue().update_job(&job);
                    // Release worker
                    let _ = scheduler_core.pool().release_worker(&report.worker_id);
                }
                "timeout" => {
                    job.mark_failed("Execution timeout".to_string());
                    let _ = scheduler_core.queue().update_job(&job);
                    // Release worker
                    let _ = scheduler_core.pool().release_worker(&report.worker_id);
                }
                _ => {
                    // Heartbeat or other status - job is still running, just update
                    let _ = scheduler_core.queue().update_job(&job);
                }
            }
        }
    }

    // Format status display: [WORKER: ip] [PID: pid] [JOB: job_id] [STATUS: event_type] details
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

    // Use appropriate log level based on status type
    match report.event_type.as_str() {
        "error" | "timeout" | "stuck" | "crashed" => {
            tracing::warn!("[WARN]  {}", status_line);
        }
        _ => {
            info!("[STATUS] {}", status_line);
        }
    }

    // Store RunResult to Elasticsearch when final status received
    // Use timeout to prevent blocking RPC if Elasticsearch is slow/unreachable
    if matches!(report.event_type.as_str(), "success" | "error" | "timeout") {
        debug!(
            "[SAVE]Storing final run result for job: {} [worker: {}]",
            report.job_id, remote_addr
        );
        match tokio::time::timeout(Duration::from_secs(DELAY), service.store_run_result(&report))
            .await
        {
            Ok(Ok(())) => {
                debug!(
                    "[OK] Run result stored successfully [job: {}, run: {}]",
                    report.job_id, report.run_id
                );
            }
            Ok(Err(e)) => {
                error!("[ERROR] ELASTICSEARCH ERROR: Failed to store run result");
                error!(
                    "   Job: {}, Run: {}, Worker: {}",
                    report.job_id, report.run_id, remote_addr
                );
                error!("   Error details: {}", e);
                error!(
                    "   [WARN]  Status report received but NOT INDEXED (Elasticsearch may be down/unreachable)"
                );
                warn!(
                    "   Possible causes: Elasticsearch down, network issue, mapping conflict, disk full"
                );
                // Don't fail the RPC - status was received, just not indexed
            }
            Err(_) => {
                error!(
                    "[TIMEOUT]  ELASTICSEARCH TIMEOUT: Storing run result exceeded {}s limit",
                    DELAY
                );
                error!(
                    "   Job: {}, Run: {}, Worker: {}",
                    report.job_id, report.run_id, remote_addr
                );
                error!(
                    "   [WARN]  Status report received but NOT INDEXED (Elasticsearch is slow/unavailable)"
                );
                warn!(
                    "   Possible causes: Elasticsearch overloaded, slow disk I/O, query queue backlog"
                );
                // Don't fail the RPC - status was received, just not indexed
            }
        }
    }

    Ok(Response::new(StatusAck { received: true }))
}
