//! Job management REST endpoints
//!
//! Wraps Controller gRPC job endpoints.

use super::{ApiError, ApiResponse};
use crate::generated::controller::ModuleSelection;
use crate::grpc_client::{ControllerGrpcClient, ScheduleJobParams};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SubmitJobRequest {
    /// Path to payload .bin file
    pub source: String,

    /// Maximum rounds to run
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,

    /// Target OS (e.g., "win10", "win11") - optional
    pub target_os: Option<String>,

    /// Required capabilities (e.g., ["defender", "rededr"])
    #[serde(default)]
    pub required_capabilities: Vec<String>,

    /// Module selection
    pub modules: Option<ModuleSelectionRequest>,

    /// Payload encoding: "xor" or "english"
    pub encoding: Option<String>,

    /// Stop after first successful evasion
    #[serde(default)]
    pub stop_on_evasion: bool,

    /// Instrumented run trace mode (default: "lines")
    /// Valid: "off", "api", "bb", "api+bb", "lines", "all"
    pub trace_mode: Option<String>,

    /// Which module categories the selector may vary across rounds
    #[serde(default)]
    pub variable_categories: Vec<String>,
}

fn default_max_rounds() -> u32 {
    10
}

#[derive(Debug, Deserialize)]
pub struct ModuleSelectionRequest {
    pub carrier: Option<String>,
    pub decoder: Option<String>,
    pub antiemulation: Option<String>,
    pub deconditioner: Option<String>,
    pub guardrail: Option<String>,
    pub virtualprotect: Option<String>,
    pub decoy: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JobResponse {
    pub job_id: Option<String>,
    pub accepted: bool,
    pub message: String,
    pub max_rounds: u32,
}

#[derive(Debug, Serialize)]
pub struct JobStatusResponse {
    pub job_id: String,
    pub status: String,
    pub progress_percent: i32,
    pub current_phase: String,
}

#[derive(Debug, Serialize)]
pub struct JobProgressResponse {
    pub job_id: String,
    pub status: String,
    pub current_round: u32,
    pub max_rounds: u32,
    pub progress_percent: u32,
    pub rounds: Vec<RoundSummaryInfo>,
}

#[derive(Debug, Serialize)]
pub struct RoundSummaryInfo {
    pub round_id: String,
    pub round_number: u32,
    pub detected: bool,
    pub evasion_score: f64,
    pub differential_category: String,
    pub status: String,
    pub coverage_percent: f64,
    pub mutations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StopJobResponse {
    pub stopped: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct FunctionCoverageInfo {
    pub name: String,
    pub total_lines: u32,
    pub executed_lines: u32,
    pub percent: f64,
}

#[derive(Debug, Serialize)]
pub struct ModulesInfo {
    pub carrier: String,
    pub decoder: String,
    pub antiemulation: String,
    pub guardrail: String,
    pub virtualprotect: String,
    pub decoy: String,
    pub deconditioner: String,
}

#[derive(Debug, Serialize)]
pub struct RoundDetailResponse {
    pub round_id: String,
    pub job_id: String,
    pub round_number: u32,
    pub baseline_run: Option<RunResultInfo>,
    pub instrumented_run: Option<RunResultInfo>,
    pub status: String,
    pub assembled_source: Option<String>,
    pub coverage_percent: f64,
    pub cutoff_line: u32,
    pub cutoff_func: String,
    pub function_coverage: Vec<FunctionCoverageInfo>,
    pub modules: Option<ModulesInfo>,
    pub mutations: Vec<String>,
    pub coverage_total_lines: u32,
    pub coverage_executable_lines: u32,
    pub coverage_executed_lines: u32,
}

#[derive(Debug, Serialize)]
pub struct RunResultInfo {
    pub run_id: String,
    pub detected: bool,
    pub exit_code: i32,
    pub outcome: String,
}

#[derive(Debug, Serialize)]
pub struct CompareRunsResponse {
    pub baseline_detected: bool,
    pub instrumented_detected: bool,
    pub outcome_match: bool,
    pub differences: Vec<String>,
}

// ============================================================================
// Handlers
// ============================================================================

/// POST /api/jobs - Submit a new job
pub async fn submit_job(
    State(client): State<Arc<ControllerGrpcClient>>,
    Json(payload): Json<SubmitJobRequest>,
) -> Result<Json<ApiResponse<JobResponse>>, ApiError> {
    // Validate required fields
    if payload.source.is_empty() {
        return Err(ApiError::bad_request(
            "source field is required (path to .bin payload)",
        ));
    }

    if payload.max_rounds == 0 {
        return Err(ApiError::bad_request("max_rounds must be greater than 0"));
    }

    info!(
        "REST: Submit job (source={}, max_rounds={})",
        payload.source, payload.max_rounds
    );

    // Convert module selection
    let modules = payload.modules.map(|m| ModuleSelection {
        carrier: m.carrier.unwrap_or_default(),
        decoder: m.decoder.unwrap_or_default(),
        antiemulation: m.antiemulation.unwrap_or_default(),
        deconditioner: m.deconditioner.unwrap_or_default(),
        guardrail: m.guardrail.unwrap_or_default(),
        virtualprotect: m.virtualprotect.unwrap_or_default(),
        decoy: m.decoy.unwrap_or_default(),
    });

    match client
        .schedule_job(ScheduleJobParams {
            source: payload.source,
            max_rounds: payload.max_rounds,
            target_os: payload.target_os,
            required_capabilities: payload.required_capabilities,
            modules,
            encoding: payload.encoding,
            stop_on_evasion: payload.stop_on_evasion,
            trace_mode: payload.trace_mode,
            variable_categories: payload.variable_categories,
        })
        .await
    {
        Ok(resp) => {
            let job_id = resp.job_id.map(|j| j.value);
            info!("Job submitted: {:?}", job_id);

            Ok(Json(ApiResponse::new(JobResponse {
                job_id,
                accepted: resp.accepted,
                message: resp.message,
                max_rounds: resp.max_rounds,
            })))
        }
        Err(e) => {
            error!("Failed to submit job: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

/// GET /api/jobs/:id - Get job status
pub async fn get_job_status(
    State(client): State<Arc<ControllerGrpcClient>>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiResponse<JobStatusResponse>>, ApiError> {
    debug!("REST: Get job status (job_id={})", job_id);

    match client.get_job_status(&job_id).await {
        Ok(resp) => Ok(Json(ApiResponse::new(JobStatusResponse {
            job_id: resp.job_id,
            status: resp.status,
            progress_percent: resp.progress_percent,
            current_phase: resp.current_phase,
        }))),
        Err(e) => {
            error!("Failed to get job status: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

/// GET /api/jobs/:id/progress - Get detailed job progress
pub async fn get_job_progress(
    State(client): State<Arc<ControllerGrpcClient>>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiResponse<JobProgressResponse>>, ApiError> {
    debug!("REST: Get job progress (job_id={})", job_id);

    match client.get_job_progress(&job_id).await {
        Ok(resp) => {
            let rounds: Vec<RoundSummaryInfo> = resp
                .rounds
                .into_iter()
                .map(|r| RoundSummaryInfo {
                    round_id: r.round_id,
                    round_number: r.round_number,
                    detected: r.detected,
                    evasion_score: r.evasion_score,
                    differential_category: r.differential_category,
                    status: r.status,
                    coverage_percent: r.coverage_percent,
                    mutations: r.mutations,
                })
                .collect();

            Ok(Json(ApiResponse::new(JobProgressResponse {
                job_id: resp.job_id,
                status: resp.status,
                current_round: resp.current_round,
                max_rounds: resp.max_rounds,
                progress_percent: resp.progress_percent,
                rounds,
            })))
        }
        Err(e) => {
            error!("Failed to get job progress: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

/// POST /api/jobs/:id/stop - Stop a running job
pub async fn stop_job(
    State(client): State<Arc<ControllerGrpcClient>>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiResponse<StopJobResponse>>, ApiError> {
    info!("REST: Stop job (job_id={})", job_id);

    match client.stop_job(&job_id).await {
        Ok(resp) => Ok(Json(ApiResponse::new(StopJobResponse {
            stopped: resp.stopped,
            message: resp.message,
        }))),
        Err(e) => {
            error!("Failed to stop job: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

/// GET /api/jobs/:job_id/rounds/:round_id - Get round details
pub async fn get_round(
    State(client): State<Arc<ControllerGrpcClient>>,
    Path((job_id, round_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<RoundDetailResponse>>, ApiError> {
    debug!("REST: Get round (job_id={}, round_id={})", job_id, round_id);

    match client.get_round(&job_id, &round_id).await {
        Ok(resp) => {
            if let Some(round) = resp.round {
                let baseline_run = round.baseline_run.map(|r| RunResultInfo {
                    run_id: r.run_id,
                    detected: r.detected,
                    exit_code: r.exit_code,
                    outcome: r.outcome,
                });

                let instrumented_run = round.instrumented_run.map(|r| RunResultInfo {
                    run_id: r.run_id,
                    detected: r.detected,
                    exit_code: r.exit_code,
                    outcome: r.outcome,
                });

                let function_coverage: Vec<FunctionCoverageInfo> = round
                    .function_coverage
                    .into_iter()
                    .map(|fc| FunctionCoverageInfo {
                        name: fc.name,
                        total_lines: fc.total_lines,
                        executed_lines: fc.executed_lines,
                        percent: fc.percent,
                    })
                    .collect();

                let modules = round.modules.map(|m| ModulesInfo {
                    carrier: m.carrier,
                    decoder: m.decoder,
                    antiemulation: m.antiemulation,
                    guardrail: m.guardrail,
                    virtualprotect: m.virtualprotect,
                    decoy: m.decoy,
                    deconditioner: m.deconditioner,
                });

                let mutations: Vec<String> = round
                    .mutations
                    .into_iter()
                    .map(|m| m.id)
                    .collect();

                Ok(Json(ApiResponse::new(RoundDetailResponse {
                    round_id: round.round_id,
                    job_id: round.job_id,
                    round_number: round.round_number,
                    baseline_run,
                    instrumented_run,
                    status: round.status,
                    assembled_source: if round.assembled_source.is_empty() {
                        None
                    } else {
                        Some(round.assembled_source)
                    },
                    coverage_percent: round.coverage_percent,
                    cutoff_line: round.cutoff_line,
                    cutoff_func: round.cutoff_func,
                    function_coverage,
                    modules,
                    mutations,
                    coverage_total_lines: round.coverage_total_lines,
                    coverage_executable_lines: round.coverage_executable_lines,
                    coverage_executed_lines: round.coverage_executed_lines,
                })))
            } else {
                Err(ApiError::not_found(format!("Round {} not found", round_id)))
            }
        }
        Err(e) => {
            error!("Failed to get round: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

// ============================================================================
// Trace Lines Types & Handler
// ============================================================================

#[derive(Debug, Serialize)]
pub struct TraceLineInfo {
    pub seq: u64,
    pub file: String,
    pub line: u32,
    pub func: String,
    pub code: String,
    pub ts_us: u64,
}

#[derive(Debug, Serialize)]
pub struct TraceLinesResponse {
    pub run_id: String,
    pub lines: Vec<TraceLineInfo>,
    pub total_events: u32,
}

/// GET /api/runs/:run_id/trace?last=N - Get trace lines for a run
#[derive(Debug, Deserialize)]
pub struct TraceLinesQuery {
    pub last: Option<u32>,
}

pub async fn get_trace_lines(
    State(client): State<Arc<ControllerGrpcClient>>,
    Path(run_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<TraceLinesQuery>,
) -> Result<Json<ApiResponse<TraceLinesResponse>>, ApiError> {
    let last = query.last.unwrap_or(50);
    debug!("REST: Get trace lines (run_id={}, last={})", run_id, last);

    match client.get_trace_lines(&run_id, last).await {
        Ok(resp) => {
            let lines: Vec<TraceLineInfo> = resp
                .lines
                .into_iter()
                .map(|l| TraceLineInfo {
                    seq: l.seq,
                    file: l.file,
                    line: l.line,
                    func: l.func,
                    code: l.code,
                    ts_us: l.ts_us,
                })
                .collect();

            Ok(Json(ApiResponse::new(TraceLinesResponse {
                run_id: resp.run_id,
                lines,
                total_events: resp.total_events,
            })))
        }
        Err(e) => {
            error!("Failed to get trace lines: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}

/// GET /api/runs/compare?baseline=X&instrumented=Y - Compare runs
#[derive(Debug, Deserialize)]
pub struct CompareRunsQuery {
    pub baseline: String,
    pub instrumented: String,
}

pub async fn compare_runs(
    State(client): State<Arc<ControllerGrpcClient>>,
    axum::extract::Query(query): axum::extract::Query<CompareRunsQuery>,
) -> Result<Json<ApiResponse<CompareRunsResponse>>, ApiError> {
    // Validate query parameters
    if query.baseline.is_empty() {
        return Err(ApiError::bad_request("baseline run_id is required"));
    }
    if query.instrumented.is_empty() {
        return Err(ApiError::bad_request("instrumented run_id is required"));
    }

    debug!(
        "REST: Compare runs (baseline={}, instrumented={})",
        query.baseline, query.instrumented
    );

    match client
        .compare_runs(&query.baseline, &query.instrumented)
        .await
    {
        Ok(resp) => {
            if let Some(comparison) = resp.comparison {
                Ok(Json(ApiResponse::new(CompareRunsResponse {
                    baseline_detected: comparison.baseline_detected,
                    instrumented_detected: comparison.instrumented_detected,
                    outcome_match: comparison.outcome_match,
                    differences: comparison.differences,
                })))
            } else {
                Err(ApiError::not_found("Comparison data not available"))
            }
        }
        Err(e) => {
            error!("Failed to compare runs: {}", e);
            Err(ApiError::unavailable(format!(
                "Controller unavailable: {}",
                e
            )))
        }
    }
}
