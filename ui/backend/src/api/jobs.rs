//! Job management REST endpoints.
//!
//! Covers the full job lifecycle: submit → status → progress → stop, plus
//! per-round inspection, source-level trace retrieval, and the two-run
//! differential comparison.
//!
//! ## Round ID Format
//!
//! Round IDs follow the pattern `{job_id}-round-{N}` where `N` is the
//! 1-based round number (e.g. `abc123-round-3`).

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

/// Request body for `POST /api/jobs`.
///
/// Describes the shellcode source, round budget, worker filters, module
/// selection, and mutation strategy for a new job.
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

    /// Payload encoding: "xor", "english", "subbyte", or "none"
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

    /// Selector algorithm: "coverage" (default) or "fuzzer"
    #[serde(default)]
    pub selector_type: Option<String>,

    /// Variation strategy: "mutation" (default) or "full"
    #[serde(default)]
    pub variation_strategy: Option<String>,

    /// Mutation IDs to explore (empty = full catalog)
    #[serde(default)]
    pub mutation_pool: Vec<String>,

    /// Module names to apply mutations to (empty = all)
    #[serde(default)]
    pub mutation_targets: Vec<String>,

    /// Mutations always applied every round after baseline (empty = keep server default)
    #[serde(default)]
    pub fixed_mutations: Vec<String>,

    /// INT3 shellcode checkpoints (0 = disabled)
    #[serde(default)]
    pub sc_checkpoint_count: u32,

    /// Cache encoded payload across rounds (default: true)
    #[serde(default = "default_true")]
    pub cache_payload: bool,

    /// Use MSVC link.exe instead of lld-link (default: false)
    #[serde(default)]
    pub msvc_compat: bool,

    /// Keep dryrun in pool after grace period (default: false)
    #[serde(default)]
    pub keep_late_dryrun: bool,
}

/// Serde default that returns `true`. Used for [`SubmitJobRequest::cache_payload`].
fn default_true() -> bool {
    true
}

/// Serde default that returns `10`. Used for [`SubmitJobRequest::max_rounds`].
fn default_max_rounds() -> u32 {
    10
}

/// Explicit loader module selection for a job.
///
/// Each field names a module implementation within its category. Pass `None` to
/// accept the controller default.
///
/// ## Module Categories
///
/// - `carrier` — memory allocation and execution strategy
/// - `decoder` — payload decoding routine
/// - `antiemulation` — sandbox / emulator detection
/// - `deconditioner` — pre-execution environment conditioning
/// - `guardrail` — execution guardrails (keying, environment checks)
/// - `virtualprotect` — memory permission transition strategy
/// - `decoy` — benign decoy API calls
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

/// Controller acceptance reply after `POST /api/jobs`.
#[derive(Debug, Serialize)]
pub struct JobResponse {
    /// Assigned job ID, or `None` if the job was rejected.
    pub job_id: Option<String>,
    /// `true` if the Controller accepted the job.
    pub accepted: bool,
    /// Human-readable acceptance or rejection reason.
    pub message: String,
    /// Confirmed round budget (may differ from the requested value).
    pub max_rounds: u32,
}

/// Lightweight job status returned by `GET /api/jobs/:id`.
#[derive(Debug, Serialize)]
pub struct JobStatusResponse {
    /// Job identifier.
    pub job_id: String,
    /// Current status: `"pending"`, `"running"`, `"completed"`, `"failed"`,
    /// `"stopped"`.
    pub status: String,
    /// Completion percentage (0–100).
    pub progress_percent: i32,
    /// Human-readable phase label, e.g. `"building round 3"`.
    pub current_phase: String,
}

/// Detailed job progress with per-round summaries.
///
/// Returned by `GET /api/jobs/:id/progress`.
#[derive(Debug, Serialize)]
pub struct JobProgressResponse {
    /// Job identifier.
    pub job_id: String,
    /// Current status (same values as [`JobStatusResponse::status`]).
    pub status: String,
    /// Most recently completed or in-progress round number.
    pub current_round: u32,
    /// Total rounds budgeted for this job.
    pub max_rounds: u32,
    /// Completion percentage (0–100).
    pub progress_percent: u32,
    /// Per-round summary records, ordered by `round_number`.
    pub rounds: Vec<RoundSummaryInfo>,
}

/// One row in the rounds summary table within [`JobProgressResponse`].
#[derive(Debug, Serialize)]
pub struct RoundSummaryInfo {
    /// Round identifier (format: `{job_id}-round-{N}`).
    pub round_id: String,
    /// 1-based round number.
    pub round_number: u32,
    /// `true` if the artifact was detected (adjusted by differential protocol).
    pub detected: bool,
    /// Numeric evasion score (0.0 = fully detected, 1.0 = full evasion).
    pub evasion_score: f64,
    /// Differential category: `"real_detection"`, `"instrumentation_artifact"`,
    /// `"full_evasion"`, or `"pending"`.
    pub differential_category: String,
    /// Round status: `"completed"`, `"running"`, `"failed"`.
    pub status: String,
    /// Line coverage percentage from the instrumented run.
    pub coverage_percent: f64,
    /// List of mutation IDs applied in this round.
    pub mutations: Vec<String>,
    /// Final detection verdict string for display badges.
    pub detection_verdict: String,
}

/// Response for `POST /api/jobs/:id/stop`.
///
/// Idempotent — stopping an already-stopped job returns `stopped: true`
/// with a descriptive message.
#[derive(Debug, Serialize)]
pub struct StopJobResponse {
    /// `true` if the job is now stopped.
    pub stopped: bool,
    /// Human-readable result message.
    pub message: String,
}

/// Per-function line coverage entry within a round.
#[derive(Debug, Serialize)]
pub struct FunctionCoverageInfo {
    /// Function name as it appears in the source.
    pub name: String,
    /// Total instrumentable lines in the function.
    pub total_lines: u32,
    /// Lines actually executed during the instrumented run.
    pub executed_lines: u32,
    /// Coverage percentage (`executed_lines / total_lines × 100`).
    pub percent: f64,
}

/// Loader module selections used for a round.
///
/// Each field names the concrete module implementation selected from its
/// category (e.g. `carrier: "heap_alloc"`, `decoder: "xor_loop"`).
#[derive(Debug, Serialize)]
pub struct ModulesInfo {
    /// Memory allocation and execution carrier.
    pub carrier: String,
    /// Payload decoding routine.
    pub decoder: String,
    /// Anti-emulation / sandbox-detection module.
    pub antiemulation: String,
    /// Execution guardrail module.
    pub guardrail: String,
    /// Memory permission transition strategy.
    pub virtualprotect: String,
    /// Benign decoy API call module.
    pub decoy: String,
    /// Pre-execution environment conditioning module.
    pub deconditioner: String,
}

/// A single mutation applied during a round.
#[derive(Debug, Serialize)]
pub struct MutationInfo {
    /// Mutation identifier (e.g. `"antiemulation.loop_randomize"`).
    pub id: String,
    /// Mutation parameters as key-value pairs.
    pub params: std::collections::HashMap<String, String>,
}

/// Full round inspection payload returned by
/// `GET /api/jobs/:job_id/rounds/:round_id`.
///
/// Contains both run results (baseline + instrumented), the assembled source
/// snapshot, coverage data, module selections, and applied mutations.
#[derive(Debug, Serialize)]
pub struct RoundDetailResponse {
    /// Round identifier.
    pub round_id: String,
    /// Parent job identifier.
    pub job_id: String,
    /// 1-based round number.
    pub round_number: u32,
    /// Baseline (trace-off) run result, if completed.
    pub baseline_run: Option<RunResultInfo>,
    /// Instrumented (trace-on) run result, if completed.
    pub instrumented_run: Option<RunResultInfo>,
    /// Round status: `"completed"`, `"running"`, `"failed"`.
    pub status: String,
    /// Full assembled C source snapshot, if available.
    pub assembled_source: Option<String>,
    /// Overall line coverage percentage.
    pub coverage_percent: f64,
    /// Source line where execution was cut off (0 if no cutoff).
    pub cutoff_line: u32,
    /// Function in which execution was cut off.
    pub cutoff_func: String,
    /// Per-function coverage breakdown.
    pub function_coverage: Vec<FunctionCoverageInfo>,
    /// Module selections used for this round.
    pub modules: Option<ModulesInfo>,
    /// Mutations applied in this round.
    pub mutations: Vec<MutationInfo>,
    /// Total instrumentable source lines.
    pub coverage_total_lines: u32,
    /// Lines that are executable (excludes blanks, braces, etc.).
    pub coverage_executable_lines: u32,
    /// Lines actually executed.
    pub coverage_executed_lines: u32,
    /// Final detection verdict string.
    pub detection_verdict: String,
}

/// Result of a single run (baseline or instrumented) within a round.
#[derive(Debug, Serialize)]
pub struct RunResultInfo {
    /// Unique run identifier.
    pub run_id: String,
    /// Adjusted detection flag (after differential protocol).
    pub detected: bool,
    /// Raw detection flag before differential adjustment.
    pub raw_detected: bool,
    /// Process exit code.
    pub exit_code: i32,
    /// Detection outcome: `"MUTATION_FAILED"`, `"MUTATION_SUCCESS"`, or
    /// `"FULL_EVASION"`.
    pub outcome: String,
    /// Last INT3 checkpoint reached (e.g. `"sc_cp_3"`), or empty.
    pub last_checkpoint: String,
    /// Human-readable detection verdict for display.
    pub detection_verdict: String,
}

/// Result of comparing a baseline run against an instrumented run.
///
/// Implements the two-run differential protocol: both runs execute the same
/// artifact; differences indicate instrumentation artifacts rather than real
/// detections.
#[derive(Debug, Serialize)]
pub struct CompareRunsResponse {
    /// Whether the baseline (trace-off) run was detected.
    pub baseline_detected: bool,
    /// Whether the instrumented (trace-on) run was detected.
    pub instrumented_detected: bool,
    /// `true` if both runs agree on detection outcome.
    pub outcome_match: bool,
    /// List of observable differences between the two runs.
    pub differences: Vec<String>,
}

// ============================================================================
// Handlers
// ============================================================================

/// `POST /api/jobs` — Submit a new mutation job.
///
/// # Errors
///
/// - `BAD_REQUEST` if `source` is empty or `max_rounds` is zero.
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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
            selector_type: payload.selector_type,
            variable_categories: payload.variable_categories,
            variation_strategy: payload.variation_strategy,
            mutation_pool: payload.mutation_pool,
            mutation_targets: payload.mutation_targets,
            fixed_mutations: payload.fixed_mutations,
            sc_checkpoint_count: payload.sc_checkpoint_count,
            cache_payload: payload.cache_payload,
            msvc_compat: payload.msvc_compat,
            keep_late_dryrun: payload.keep_late_dryrun,
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

/// `GET /api/jobs/:id` — Get job status.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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

/// `GET /api/jobs/:id/progress` — Get detailed job progress with per-round summaries.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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
                    detection_verdict: r.detection_verdict,
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

/// `POST /api/jobs/:id/stop` — Stop a running job.
///
/// Idempotent — stopping an already-stopped job succeeds silently.
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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

/// `GET /api/jobs/:job_id/rounds/:round_id` — Get full round details.
///
/// # Errors
///
/// - `NOT_FOUND` if the round does not exist.
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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
                    raw_detected: r.raw_detected,
                    exit_code: r.exit_code,
                    outcome: r.outcome,
                    last_checkpoint: r.last_checkpoint,
                    detection_verdict: r.detection_verdict,
                });

                let instrumented_run = round.instrumented_run.map(|r| RunResultInfo {
                    run_id: r.run_id,
                    detected: r.detected,
                    raw_detected: r.raw_detected,
                    exit_code: r.exit_code,
                    outcome: r.outcome,
                    last_checkpoint: r.last_checkpoint,
                    detection_verdict: r.detection_verdict,
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

                let mutations: Vec<MutationInfo> = round
                    .mutations
                    .into_iter()
                    .map(|m| MutationInfo {
                        id: m.id,
                        params: m.params,
                    })
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
                    detection_verdict: round.detection_verdict,
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

/// A single source-level trace record from an instrumented run.
#[derive(Debug, Serialize)]
pub struct TraceLineInfo {
    /// Global sequence number across all trace events in the run.
    pub seq: u64,
    /// Source file path (relative to the build root).
    pub file: String,
    /// 1-based line number in the source file.
    pub line: u32,
    /// Name of the enclosing function.
    pub func: String,
    /// Source code text of the executed line.
    pub code: String,
    /// Timestamp in microseconds since run start.
    pub ts_us: u64,
}

/// Paginated trace response for `GET /api/runs/:run_id/trace`.
#[derive(Debug, Serialize)]
pub struct TraceLinesResponse {
    /// Run identifier.
    pub run_id: String,
    /// Trace records (most recent `last` entries).
    pub lines: Vec<TraceLineInfo>,
    /// Total number of trace events in the run (may exceed `lines.len()`).
    pub total_events: u32,
}

/// Query parameters for `GET /api/runs/:run_id/trace`.
#[derive(Debug, Deserialize)]
pub struct TraceLinesQuery {
    /// Number of most-recent trace lines to return. Defaults to `50` when
    /// omitted.
    pub last: Option<u32>,
}

/// `GET /api/runs/:run_id/trace?last=N` — Get trace lines for a run.
///
/// Returns the most recent `last` trace lines (default 50).
///
/// # Errors
///
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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

/// Query parameters for `GET /api/runs/compare`.
#[derive(Debug, Deserialize)]
pub struct CompareRunsQuery {
    /// Baseline (trace-off) run ID.
    pub baseline: String,
    /// Instrumented (trace-on) run ID.
    pub instrumented: String,
}

/// `GET /api/runs/compare?baseline=X&instrumented=Y` — Compare two runs.
///
/// Implements the two-run differential protocol to distinguish real detections
/// from instrumentation artifacts.
///
/// # Errors
///
/// - `BAD_REQUEST` if either `baseline` or `instrumented` is empty.
/// - `NOT_FOUND` if comparison data is not available.
/// - `SERVICE_UNAVAILABLE` if the Controller is unreachable.
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
