//! Unary RPC handler for artifact execution.
//!
//! Entry point for the `RunSample` RPC when invoked outside a bidirectional stream.
//! Acquires the execution lock, delegates to the execution engine, and maps the
//! outcome to a `SampleResponse`.

use crate::WorkerAgentService;
use crate::automutate::common::{SampleRequest, SampleResponse};
use crate::execution::engine;
use crate::execution::state::ExecutionLockGuard;
use crate::execution::types::{
    RunContext, RunRequest, format_output, resolve_run_id, sample_response_ok,
};
use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

/// Execute an artifact via the unary RunSample RPC path.
pub async fn run_sample(
    service: &WorkerAgentService,
    request: Request<SampleRequest>,
) -> Result<Response<SampleResponse>, Status> {
    let req = request.into_inner();

    // Resolve run_id: prefer controller-assigned, fallback to UUID
    let run_id = {
        let controller_run_id = if let Some(handler) = service.stream_handler.read().await.as_ref()
        {
            let state = handler.worker_state.read().await;
            state.current_run_id.clone()
        } else {
            None
        };
        resolve_run_id(controller_run_id.as_deref())
    };

    let job_id = req.job_id.clone();

    info!(
        "Received sample execution request: job_id={}, artifact_id={}, run_id={}",
        job_id, req.artifact_id, run_id
    );

    // Build typed request and context
    let run_request = RunRequest {
        job_id: job_id.clone(),
        artifact_id: req.artifact_id.clone(),
        timeout_seconds: req.timeout_seconds as u32,
        run_id: run_id.clone(),
    };

    let run_context = RunContext::new(
        &req.artifact_id,
        service.worker_id.clone(),
        service.config.clone(),
    );

    let artifact_name = run_context.artifact_name.clone();

    // Acquire single execution lock
    let _execution_lock = {
        let mut state = service.execution_lock.lock().await;

        if let Err(e) = state.acquire(job_id.clone(), artifact_name.clone(), run_id.clone()) {
            warn!("[ERROR] REJECTED: {}", e);
            return Err(Status::resource_exhausted(e.to_string()));
        }

        info!(
            "Execution lock ACQUIRED: job_id={}, artifact={}",
            job_id, artifact_name
        );

        ExecutionLockGuard::new(service.execution_lock.clone())
    };

    // Build sink from stream handler's tx channel (no Arc cycle)
    let sink = {
        let handler_lock = service.stream_handler.read().await;
        match handler_lock.as_ref() {
            Some(handler) => crate::execution::sink::build_sink(Some(handler.sender())),
            None => crate::execution::sink::build_sink(None),
        }
    };

    // Execute via engine — dryrun skips RedEDR/telemetry
    let outcome = if req.is_dryrun {
        engine::execute_dryrun(&run_request, &run_context)
            .await
            .map_err(|e| e.into_status())?
    } else {
        engine::execute_run(&run_request, &run_context, sink)
            .await
            .map_err(|e| e.into_status())?
    };

    // Map RunOutcome → SampleResponse
    let output = format_output(&outcome, req.timeout_seconds as u32);

    // Log final status
    if outcome.timed_out {
        warn!("TIMEOUT: {} - {}", artifact_name, output);
    } else if outcome.exit_code == 0 {
        info!("SUCCESS: {} - {}", artifact_name, output);
    } else {
        warn!("ERROR: {} - {}", artifact_name, output);
    }

    debug!(
        "[WORKER-RESPONSE] Creating SampleResponse with run_id='{}' (will be overwritten by stream_handler)",
        run_id
    );

    Ok(Response::new(sample_response_ok(
        &job_id, "", &outcome, output,
    )))
}
