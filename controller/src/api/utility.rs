//! Utility gRPC handlers: ping, triage submission, and result queries.
//!
//! Lightweight RPCs that don't involve the dispatch engine or VM lifecycle.

use crate::api::SchedulerService;
use crate::automutate::controller::{
    AnalysisResult, PingRequest, PingResponse, QueryRequest, QueryResponse, TriageRequest,
    TriageResponse,
};
use tonic::{Request, Response, Status};
use tracing::{debug, info};

/// Respond to a ping request with a pong and the current server timestamp.
///
/// # Errors
///
/// Always returns `Ok`.
pub async fn ping(
    _service: &SchedulerService,
    request: Request<PingRequest>,
) -> Result<Response<PingResponse>, Status> {
    let req = request.into_inner();
    let timestamp = crate::storage::helpers::now_unix_secs();

    debug!("Ping received: {}", req.message);

    Ok(Response::new(PingResponse {
        message: format!("pong: {}", req.message),
        timestamp,
        server: "controller".to_string(),
    }))
}

/// Submit triage results (backwards-compatibility endpoint).
///
/// The internal triage pipeline ([`extract_and_score`](crate::triage::extractor::extract_and_score),
/// called from [`JobWorker::finalize_round`](crate::dispatch::job_worker::JobWorker))
/// already handles token extraction, lift/confidence scoring, and guidance generation.
/// This endpoint is retained for external callers but does not duplicate that pipeline.
///
/// # Errors
///
/// Always returns `Ok`.
pub async fn submit_triage(
    _service: &SchedulerService,
    request: Request<TriageRequest>,
) -> Result<Response<TriageResponse>, Status> {
    let req = request.into_inner();
    let triage_id = format!("triage-{}", crate::storage::helpers::now_unix_secs());

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

/// Query analysis results from Elasticsearch.
///
/// Searches `runs-*` index filtered by job_ids and date range,
/// returning structured results for the UI.
///
/// # Errors
///
/// Always returns `Ok` — ES query failures produce an empty result set.
pub async fn query_results(
    service: &SchedulerService,
    request: Request<QueryRequest>,
) -> Result<Response<QueryResponse>, Status> {
    let req = request.into_inner();

    debug!(
        "[RPC] QueryResults: job_ids={:?}, date_from={}, date_to={}",
        req.job_ids, req.date_from, req.date_to
    );

    let docs = service
        .storage
        .query_analysis_results(&req.job_ids, &req.date_from, &req.date_to)
        .await;

    let results: Vec<AnalysisResult> = docs
        .iter()
        .map(|doc| {
            let mut summary = std::collections::HashMap::new();
            let fields = [
                ("run_id", "run_id"),
                ("round_id", "round_id"),
                ("elapsed_ms", "elapsed_ms"),
                ("vm_id", "vm_id"),
                ("exit_code", "exit_code"),
                ("run_type", "run_type"),
                ("timestamp", "timestamp"),
            ];
            for (key, doc_field) in &fields {
                let val = if *doc_field == "elapsed_ms" || *doc_field == "exit_code" {
                    // Numeric fields: try as number first, then as string
                    doc[doc_field]
                        .as_i64()
                        .map(|n| n.to_string())
                        .or_else(|| doc[doc_field].as_f64().map(|n| format!("{:.1}", n)))
                        .or_else(|| doc[doc_field].as_str().map(|s| s.to_string()))
                } else {
                    doc[doc_field].as_str().map(|s| s.to_string())
                };
                if let Some(v) = val {
                    summary.insert(key.to_string(), v);
                }
            }
            AnalysisResult {
                job_id: doc["job_id"].as_str().unwrap_or("").to_string(),
                artifact_hash: doc["artifact_id"].as_str().unwrap_or("").to_string(),
                detected: doc["detected"].as_bool().unwrap_or(false),
                detection_rate: doc["detection_verdict"].as_str().unwrap_or("").to_string(),
                evasion_techniques: vec![],
                telemetry_summary: summary,
            }
        })
        .collect();

    let total_count = results.len() as i32;

    if total_count > 0 {
        debug!("[RPC] QueryResults: returning {} results", total_count);
    }

    Ok(Response::new(QueryResponse {
        results,
        total_count,
    }))
}
