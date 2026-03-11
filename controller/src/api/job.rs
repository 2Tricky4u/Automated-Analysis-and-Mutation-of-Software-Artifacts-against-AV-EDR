//! gRPC handlers for job scheduling, status queries, round inspection,
//! run comparison, and token comparison.
//!
//! Handles 8+ RPCs defined in `controller.proto`. Each handler receives a
//! [`SchedulerService`] reference, extracts proto
//! fields, queries or mutates Elasticsearch, and returns a typed proto response.

use super::extract::{
    bool_field, f64_field, i32_field, parse_date_to_unix_secs, str_field, string_array_field,
    u32_field, u64_field,
};
use crate::api::SchedulerService;
use crate::automutate::common::JobId;
use crate::automutate::controller::{
    BehaviorComparisonProto, CompareRunsRequest, CompareRunsResponse, CompareTokensRequest,
    CompareTokensResponse, FunctionCoverageProto, GetRoundCoverageRequest,
    GetRoundCoverageResponse, GetRoundRequest, GetRoundResponse, GetRoundSourceRequest,
    GetRoundSourceResponse, GetTraceLinesRequest, GetTraceLinesResponse, JobProgressRequest,
    JobProgressResponse, JobRequest, JobResponse, JobStatusRequest, JobStatusResponse,
    ModuleSelection, MutationComparisonProto, ParamComparisonProto, RoundSummaryProto,
    RunResultProto, StatusAck, StatusReport, StopJobRequest, StopJobResponse,
    TokenSetComparisonProto, TraceLine,
};
use crate::dispatch::{
    JobControlCommand, JobId as DispatchJobId, JobSession, ModularBuildSpec, ModuleSelectionSpec,
};
use serde_json::Value;
use std::path::PathBuf;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

/// Schedule a new mutation-exploration job via the dispatch system.
///
/// # Errors
///
/// Returns `Ok` with `accepted=false` if the `source` field is empty or the
/// job channel is closed. Never returns a gRPC error status — failures are
/// encoded in the response body.
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
    if !req.trace_mode.is_empty() {
        job.trace_mode = req.trace_mode.clone();
    }
    if !req.variable_categories.is_empty() {
        job.search_space.variable_categories = req.variable_categories.clone();
    }
    job.search_space.selector =
        crate::triage::SelectorType::from_str_or_default(&req.selector_type);
    job.search_space.strategy =
        crate::triage::VariationStrategy::from_str_or_default(&req.variation_strategy);
    if !req.mutation_pool.is_empty() {
        job.search_space.mutation_pool = req.mutation_pool.clone();
    }
    if !req.mutation_targets.is_empty() {
        job.search_space.mutation_targets = req.mutation_targets.clone();
    }
    if !req.fixed_mutations.is_empty() {
        job.search_space.fixed_mutations = req.fixed_mutations.clone();
    }
    if req.sc_checkpoint_count > 0 {
        job.sc_checkpoint_count = Some(req.sc_checkpoint_count);
    }
    job.cache_payload = req.cache_payload;
    job.msvc_compat = req.msvc_compat;
    job.msvc_vcvarsall = req.msvc_vcvarsall.clone();
    job.keep_late_dryrun = req.keep_late_dryrun;

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

/// Get job status from Elasticsearch.
///
/// # Errors
///
/// Returns `Ok` with `status="not_found"` if the job does not exist in ES.
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

/// Get detailed job progress including per-round summaries.
///
/// # Errors
///
/// Returns `Ok` with `status="not_found"` if the job does not exist.
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

    let (status, current_round, max_rounds, source_file) = match job_doc {
        Some(source) => (
            str_field(&source, "status"),
            u32_field(&source, "current_round"),
            u32_field(&source, "max_rounds"),
            str_field(&source, "source_file"),
        ),
        None => ("not_found".to_string(), 0, 0, String::new()),
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
        source_file,
    }))
}

/// Stop a running job by sending a control command to the orchestrator.
///
/// # Errors
///
/// Returns `Ok` with `stopped=false` if the control channel is closed.
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

/// Get detailed round information including both run results and behavior comparison.
///
/// # Errors
///
/// Returns `Ok` with `round=None` if the round is not found in ES.
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

    let source = match service
        .storage
        .query_round_light(&req.job_id, &req.round_id)
        .await
    {
        Some(s) => s,
        None => return Ok(Response::new(GetRoundResponse { round: None })),
    };

    // Parse mutations (prefer mutation_recipe with params, fall back to mutations string array)
    let mutations = parse_mutation_recipe(&source);

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

    // Apply dryrun correction: override baseline detected using round doc's stored category
    apply_round_correction(&source, &mut baseline_run, &mut instrumented_run);

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

    // Lazy-loaded fields: function_coverage is excluded from light query.
    // We infer has_function_coverage from coverage_executed_lines > 0.
    let has_assembled_source = source
        .get("has_assembled_source")
        .and_then(|v| v.as_bool())
        .unwrap_or(true); // backward compat: old rounds without this field have source
    let has_function_coverage = u32_field(&source, "coverage_executed_lines") > 0;

    // Parse modules object from ES
    let modules = if source["modules"].is_object() {
        Some(ModuleSelection {
            carrier: str_field(&source["modules"], "carrier"),
            decoder: str_field(&source["modules"], "decoder"),
            antiemulation: str_field(&source["modules"], "antiemulation"),
            guardrail: str_field(&source["modules"], "guardrail"),
            virtualprotect: str_field(&source["modules"], "virtualprotect"),
            decoy: str_field(&source["modules"], "decoy"),
            deconditioner: str_field(&source["modules"], "deconditioner"),
        })
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
        assembled_source: String::new(), // lazy-loaded via GetRoundSource
        coverage_percent: f64_field(&source, "coverage_percent"),
        cutoff_line: u32_field(&source, "cutoff_line"),
        cutoff_func: str_field(&source, "cutoff_func"),
        function_coverage: Vec::new(), // lazy-loaded via GetRoundCoverage
        modules,
        coverage_total_lines: u32_field(&source, "coverage_total_lines"),
        coverage_executable_lines: u32_field(&source, "coverage_executable_lines"),
        coverage_executed_lines: u32_field(&source, "coverage_executed_lines"),
        dryrun_run: None, // Populated if dryrun data exists in ES
        dry_run_exit_code: source["dry_run_exit_code"].as_i64().unwrap_or(0) as i32,
        detection_verdict: str_field(&source, "detection_verdict"),
        has_assembled_source,
        has_function_coverage,
    };

    Ok(Response::new(GetRoundResponse { round: Some(round) }))
}

/// Get assembled source for a round (lazy-loaded by source viewer).
pub async fn get_round_source(
    service: &SchedulerService,
    request: Request<GetRoundSourceRequest>,
) -> Result<Response<GetRoundSourceResponse>, Status> {
    let req = request.get_ref();
    debug!(
        "[RPC] GetRoundSource: job_id={}, round_id={}",
        req.job_id, req.round_id
    );

    let source = match service
        .storage
        .query_round_source_only(&req.job_id, &req.round_id)
        .await
    {
        Some(s) => s,
        None => {
            return Ok(Response::new(GetRoundSourceResponse {
                assembled_source: String::new(),
                last_checkpoint: String::new(),
            }));
        }
    };

    let assembled_source = str_field(&source, "assembled_source");
    let instrumented_run_id = str_field(&source, "instrumented_run_id");

    // Look up last_checkpoint from the instrumented run doc
    let last_checkpoint = if !instrumented_run_id.is_empty() {
        let runs = service
            .storage
            .query_runs_by_ids(&[&instrumented_run_id])
            .await;
        runs.first()
            .map(|r| str_field(r, "last_checkpoint"))
            .unwrap_or_default()
    } else {
        String::new()
    };

    Ok(Response::new(GetRoundSourceResponse {
        assembled_source,
        last_checkpoint,
    }))
}

/// Get function coverage for a round (lazy-loaded on section expand).
pub async fn get_round_coverage(
    service: &SchedulerService,
    request: Request<GetRoundCoverageRequest>,
) -> Result<Response<GetRoundCoverageResponse>, Status> {
    let req = request.get_ref();
    debug!(
        "[RPC] GetRoundCoverage: job_id={}, round_id={}",
        req.job_id, req.round_id
    );

    let source = match service
        .storage
        .query_round_coverage_only(&req.job_id, &req.round_id)
        .await
    {
        Some(s) => s,
        None => {
            return Ok(Response::new(GetRoundCoverageResponse {
                function_coverage: Vec::new(),
                coverage_total_lines: 0,
                coverage_executable_lines: 0,
                coverage_executed_lines: 0,
            }));
        }
    };

    let function_coverage: Vec<FunctionCoverageProto> = source["function_coverage"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|fc| FunctionCoverageProto {
                    name: str_field(fc, "name"),
                    total_lines: u32_field(fc, "total"),
                    executed_lines: u32_field(fc, "executed"),
                    percent: f64_field(fc, "percent"),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Response::new(GetRoundCoverageResponse {
        function_coverage,
        coverage_total_lines: u32_field(&source, "coverage_total_lines"),
        coverage_executable_lines: u32_field(&source, "coverage_executable_lines"),
        coverage_executed_lines: u32_field(&source, "coverage_executed_lines"),
    }))
}

/// Compare baseline vs instrumented runs for differential analysis.
///
/// # Errors
///
/// Returns `Ok` with `comparison=None` if neither run is found in ES.
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

    // Look up the round doc to get corrected values (one extra ES query, but user-triggered)
    let round_source = 'round: {
        // Extract job_id + round_id from whichever run doc we have
        let (job_id, round_id) = baseline
            .as_ref()
            .or(instrumented.as_ref())
            .map(|r| (r.job_id.clone(), r.round_id.clone()))
            .unwrap_or_default();

        if !job_id.is_empty()
            && !round_id.is_empty()
            && let Some(doc) = service.storage.query_round_light(&job_id, &round_id).await
        {
            break 'round Some(doc);
        }
        None
    };

    // Apply dryrun correction if round doc is available
    if let Some(ref round_doc) = round_source {
        apply_round_correction(round_doc, &mut baseline, &mut instrumented);
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

    // Read canonical category from round doc if available, otherwise recompute
    let differential_category = round_source
        .as_ref()
        .map(|doc| str_field(doc, "differential_category"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            crate::dispatch::types::DifferentialCategory::from_runs(b_detected, i_detected)
                .as_str()
                .to_string()
        });

    Ok(Response::new(CompareRunsResponse {
        comparison: Some(BehaviorComparisonProto {
            outcome_match,
            baseline_detected: b_detected,
            baseline_exit_code: b_exit,
            instrumented_detected: i_detected,
            instrumented_exit_code: i_exit,
            differences,
            confidence,
            differential_category,
        }),
    }))
}

/// Compare triage token sets between two runs.
///
/// # Errors
///
/// Returns `Ok` with `comparison=None` and a non-empty `error` string if
/// either run or its token set cannot be found.
pub async fn compare_tokens(
    service: &SchedulerService,
    request: Request<CompareTokensRequest>,
) -> Result<Response<CompareTokensResponse>, Status> {
    use crate::triage::param_space::default_registry;
    use crate::triage::token_diff;

    let req = request.get_ref();
    debug!(
        "[RPC] CompareTokens: run_a={}, run_b={}",
        req.run_id_a, req.run_id_b
    );

    // Look up both run docs to get job_id + round_id
    let runs = service
        .storage
        .query_runs_by_ids(&[&req.run_id_a, &req.run_id_b])
        .await;

    let mut run_a_info: Option<(String, String)> = None;
    let mut run_b_info: Option<(String, String)> = None;
    for run in &runs {
        let rid = str_field(run, "run_id");
        let job_id = str_field(run, "job_id");
        let round_id = str_field(run, "round_id");
        if rid == req.run_id_a {
            run_a_info = Some((job_id, round_id));
        } else if rid == req.run_id_b {
            run_b_info = Some((job_id, round_id));
        }
    }

    let (job_a, round_a) = match run_a_info {
        Some(info) => info,
        None => {
            return Ok(Response::new(CompareTokensResponse {
                comparison: None,
                error: format!("Run {} not found", req.run_id_a),
            }));
        }
    };
    let (job_b, round_b) = match run_b_info {
        Some(info) => info,
        None => {
            return Ok(Response::new(CompareTokensResponse {
                comparison: None,
                error: format!("Run {} not found", req.run_id_b),
            }));
        }
    };

    // Query token docs for both rounds in parallel
    let (token_doc_a, token_doc_b) = tokio::join!(
        service
            .storage
            .query_token_set_by_round_id(&job_a, &round_a),
        service
            .storage
            .query_token_set_by_round_id(&job_b, &round_b),
    );

    let extract_tokens = |doc: Option<Value>| -> Vec<String> {
        doc.and_then(|d| d["tokens"].as_array().cloned())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };

    let tokens_a = extract_tokens(token_doc_a);
    let tokens_b = extract_tokens(token_doc_b);

    if tokens_a.is_empty() && tokens_b.is_empty() {
        return Ok(Response::new(CompareTokensResponse {
            comparison: None,
            error: "No token data found for either run".to_string(),
        }));
    }

    // Compare
    let registry = default_registry();
    let cmp = token_diff::compare_token_sets(&tokens_a, &tokens_b, &registry);

    // Convert to proto
    let mutation_comparisons: Vec<MutationComparisonProto> = cmp
        .mutation_comparisons
        .iter()
        .map(|mc| {
            let params: Vec<ParamComparisonProto> = mc
                .param_comparison
                .as_ref()
                .map(|pc| {
                    pc.param_distances
                        .iter()
                        .map(|pd| ParamComparisonProto {
                            name: pd.name.clone(),
                            value_a: pd.value_a.clone(),
                            value_b: pd.value_b.clone(),
                            param_type: pd.distance.param_type.clone(),
                            normalized_distance: pd.distance.normalized,
                            range_info: pd.distance.range_info.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            MutationComparisonProto {
                mutation_id: mc.mutation_id.clone(),
                presence: mc.presence.clone(),
                token_a: mc.token_a.clone(),
                token_b: mc.token_b.clone(),
                params,
                overall_distance: mc.overall_distance,
            }
        })
        .collect();

    Ok(Response::new(CompareTokensResponse {
        comparison: Some(TokenSetComparisonProto {
            only_in_a: cmp.only_in_a,
            only_in_b: cmp.only_in_b,
            common: cmp.common,
            mutation_comparisons,
            jaccard_distance: cmp.jaccard_distance,
            count_a: cmp.count_a as u32,
            count_b: cmp.count_b as u32,
        }),
        error: String::new(),
    }))
}

/// Handle status reports from workers (execution events, errors, timeouts).
///
/// # Errors
///
/// Always returns `Ok` — ES indexing failures are logged but not propagated.
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

/// Get trace lines for an instrumented run's execution path.
///
/// Resolves source code and function names via [`SourceMap`](crate::triage::source_resolver::SourceMap)
/// when assembled source is available in the round document.
///
/// # Errors
///
/// Always returns `Ok` — missing data produces empty results.
pub async fn get_trace_lines(
    service: &SchedulerService,
    request: Request<GetTraceLinesRequest>,
) -> Result<Response<GetTraceLinesResponse>, Status> {
    use crate::triage::source_resolver::SourceMap;

    let req = request.get_ref();
    debug!(
        "[RPC] GetTraceLines: run_id={}, last={}",
        req.run_id, req.last
    );

    let (docs, total) = service
        .storage
        .query_trace_lines(&req.run_id, req.last)
        .await;

    // Try to build a SourceMap for code resolution:
    // run doc → job_id + round_id → round doc → assembled_source
    let source_map = 'resolve: {
        let run_docs = service.storage.query_runs_by_ids(&[&req.run_id]).await;
        let run_doc = match run_docs.first() {
            Some(d) => d,
            None => {
                debug!("Source resolution: run doc not found for {}", req.run_id);
                break 'resolve None;
            }
        };
        let job_id = str_field(run_doc, "job_id");
        let round_id = str_field(run_doc, "round_id");
        if job_id.is_empty() || round_id.is_empty() {
            debug!(
                "Source resolution: missing job_id/round_id on run {}",
                req.run_id
            );
            break 'resolve None;
        }
        let round_doc = match service.storage.query_round(&job_id, &round_id).await {
            Some(d) => d,
            None => {
                debug!(
                    "Source resolution: round doc not found for {}/{}",
                    job_id, round_id
                );
                break 'resolve None;
            }
        };
        let assembled = str_field(&round_doc, "assembled_source");
        if assembled.is_empty() {
            debug!(
                "Source resolution: no assembled_source in round {}",
                round_id
            );
            break 'resolve None;
        }
        Some(SourceMap::new(&assembled))
    };

    let lines: Vec<TraceLine> = docs
        .iter()
        .map(|doc| {
            let line_num = u32_field(doc, "payload_line");
            let mut func = str_field(doc, "payload_func");
            let mut code = String::new();

            // Resolve code + fallback func from SourceMap
            if let Some(ref sm) = source_map
                && let Some(resolved) = sm.resolve(line_num as usize)
            {
                code = resolved.code;
                if func.is_empty()
                    && let Some(f) = resolved.func
                {
                    func = f;
                }
            }

            TraceLine {
                seq: u64_field(doc, "payload_seq"),
                file: str_field(doc, "payload_file"),
                line: line_num,
                func,
                code,
                ts_us: u64_field(doc, "payload_ts_us"),
            }
        })
        .collect();

    Ok(Response::new(GetTraceLinesResponse {
        run_id: req.run_id.clone(),
        lines,
        total_events: total as u32,
    }))
}

// ===========================================================================
// Proto mapping helpers
// ===========================================================================

/// Convert an ES round `_source` document into a [`RoundSummaryProto`].
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
        differential_category: str_field(source, "differential_category"),
        coverage_percent: f64_field(source, "coverage_percent"),
        dry_run_exit_code: source["dry_run_exit_code"].as_i64().unwrap_or(0) as i32,
        has_dryrun: bool_field(source, "has_dryrun"),
        detection_verdict: str_field(source, "detection_verdict"),
    }
}

/// Convert an ES run `_source` document into a [`RunResultProto`].
fn run_doc_to_proto(source: &Value) -> RunResultProto {
    let detected = bool_field(source, "detected");
    let verdict = str_field(source, "detection_verdict");
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
        // Backward compat: populate outcome from detection_verdict, fall back to old detection_outcome
        outcome: {
            let v = str_field(source, "detection_verdict");
            if v.is_empty() {
                str_field(source, "detection_outcome")
            } else {
                v
            }
        },
        detected,
        exit_code: i32_field(source, "exit_code"),
        telemetry_events_count: u64_field(source, "telemetry_events_count"),
        elapsed_seconds: u64_field(source, "elapsed_seconds"),
        started_at: 0,
        completed_at: 0,
        last_checkpoint: str_field(source, "last_checkpoint"),
        raw_detected: detected, // same initially; apply_round_correction may diverge
        detection_verdict: verdict,
    }
}

/// Apply dryrun correction to run protos using the already-stored round doc values.
///
/// The round doc's `differential_category` was computed by `RoundAgg::to_summary()` which
/// accounts for dryrun overrides.  We recover the corrected baseline `detected` from it:
///   - `real_detection` / `flaky` → baseline detected = true
///   - `evasion` / `instrumentation_artifact` → baseline detected = false
///
/// Instrumented `detected` is never overridden by dryrun — stays as-is.
/// `raw_detected` preserves the original per-run value for debugging.
fn apply_round_correction(
    round_source: &Value,
    baseline: &mut Option<RunResultProto>,
    instrumented: &mut Option<RunResultProto>,
) {
    // Save raw values (already set by run_doc_to_proto, but be explicit)
    if let Some(b) = baseline {
        b.raw_detected = b.detected;
    }
    if let Some(i) = instrumented {
        i.raw_detected = i.detected;
    }

    if bool_field(round_source, "has_dryrun") {
        let category = str_field(round_source, "differential_category");

        if matches!(category.as_str(), "mutation_failed" | "payload_failed") {
            // Broken artifact: both runs should show not-detected
            if let Some(b) = baseline {
                b.detected = false;
            }
            if let Some(i) = instrumented {
                i.detected = false;
            }
        } else if let Some(b) = baseline {
            b.detected = matches!(category.as_str(), "real_detection" | "flaky");
        }
    }
}

/// Parse mutations from ES round doc.
/// Prefers `mutation_recipe` (array of {id, params}) over `mutations` (array of strings).
fn parse_mutation_recipe(source: &Value) -> Vec<crate::automutate::common::Mutation> {
    // Try mutation_recipe first (has full params)
    if let Some(arr) = source["mutation_recipe"].as_array() {
        let recipes: Vec<crate::automutate::common::Mutation> = arr
            .iter()
            .filter_map(|entry| {
                let id = entry["id"].as_str()?.to_string();
                let params = entry["params"]
                    .as_object()
                    .map(|obj| {
                        obj.iter()
                            .map(|(k, v)| {
                                let val = v
                                    .as_str()
                                    .map(String::from)
                                    .unwrap_or_else(|| v.to_string());
                                (k.clone(), val)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(crate::automutate::common::Mutation { id, params })
            })
            .collect();
        if !recipes.is_empty() {
            debug!(
                "parse_mutation_recipe: parsed {} recipes from mutation_recipe",
                recipes.len()
            );
            for r in &recipes {
                debug!("  mutation: id={}, params={:?}", r.id, r.params);
            }
            return recipes;
        }
        debug!("parse_mutation_recipe: mutation_recipe was empty, falling back to mutations[]");
    }

    // Fall back to mutations string array (no params)
    debug!("parse_mutation_recipe: using fallback mutations[] field");
    string_array_field(source, "mutations")
        .into_iter()
        .map(|id| crate::automutate::common::Mutation {
            id,
            params: Default::default(),
        })
        .collect()
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

    // Read canonical corrected category from round doc (set by RoundAgg::to_summary)
    let differential_category = str_field(source, "differential_category");

    // Fallback: if round doc doesn't have it, recompute from (now-corrected) per-run data
    let differential_category = if differential_category.is_empty() {
        crate::dispatch::types::DifferentialCategory::from_runs(b_detected, i_detected)
            .as_str()
            .to_string()
    } else {
        differential_category
    };

    BehaviorComparisonProto {
        outcome_match,
        baseline_detected: b_detected,
        baseline_exit_code: b_exit,
        instrumented_detected: i_detected,
        instrumented_exit_code: i_exit,
        differences: vec![],
        confidence: if outcome_match { 1.0 } else { 0.5 },
        differential_category,
    }
}
