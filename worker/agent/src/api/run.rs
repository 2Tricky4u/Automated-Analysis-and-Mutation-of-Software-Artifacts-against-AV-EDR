use crate::WorkerAgentService;
use crate::automutate::common::{SampleRequest, SampleResponse};
use crate::dispatch::engine;
use crate::dispatch::state::ExecutionLockGuard;
use crate::dispatch::types::{
    RunContext, RunRequest, resolve_run_id, sample_response_ok, EXIT_INFRA, EXIT_NO_CODE,
    EXIT_TIMEOUT, EXIT_WAIT_FAILED,
};
use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

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
    let artifact_name = format!("{}.exe", req.artifact_id);

    info!(
        "Received sample execution request: job_id={}, artifact_id={}, run_id={}",
        job_id, req.artifact_id, run_id
    );

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

    // Build sink from stream handler's tx channel (no Arc cycle)
    let sink = {
        let handler_lock = service.stream_handler.read().await;
        match handler_lock.as_ref() {
            Some(handler) => crate::dispatch::sink::build_sink(Some(handler.sender())),
            None => crate::dispatch::sink::build_sink(None),
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

/// Format human-readable output string from RunOutcome.
/// Public so the stream handler can reuse this for consistent output formatting.
pub fn format_output(outcome: &crate::dispatch::types::RunOutcome, timeout_seconds: u32) -> String {
    if outcome.timed_out {
        format!("Execution timed out after {}s", timeout_seconds)
    } else if outcome.exit_code == 0 {
        if !outcome.stdout.is_empty() {
            format!(
                "Execution completed in {:.2}s\nOutput:\n{}",
                outcome.elapsed.as_secs_f64(),
                outcome.stdout
            )
        } else {
            format!(
                "Execution completed in {:.2}s",
                outcome.elapsed.as_secs_f64()
            )
        }
    } else {
        let error_type = describe_exit(outcome.exit_code);
        let mut output = format!(
            "Execution failed with exit code {} ({}), elapsed: {:.2}s",
            outcome.exit_code,
            error_type,
            outcome.elapsed.as_secs_f64()
        );

        if !outcome.stderr.is_empty() {
            output.push_str(&format!("\nStderr:\n{}", outcome.stderr));
        }

        if !outcome.stdout.is_empty() {
            output.push_str(&format!("\nStdout:\n{}", outcome.stdout));
        }

        output
    }
}

// ============================================================================
// Exit code interpretation helpers
// ============================================================================

fn describe_exit(exit_code: i32) -> String {
    // Synthetic engine exit codes (negative)
    match exit_code {
        EXIT_NO_CODE => return "Externally terminated (no exit code)".to_string(),
        EXIT_WAIT_FAILED => return "wait() failed".to_string(),
        EXIT_TIMEOUT => return "Timeout (process killed)".to_string(),
        EXIT_INFRA => return "Infrastructure error (never executed)".to_string(),
        _ => {}
    }

    let code_u32 = exit_code as u32;

    if code_u32 == 0 {
        return "Success".to_string();
    }

    // Loader namespaced ranges
    match exit_code {
        10..=19 => return "Guardrail failed".to_string(),
        30 => return "Carrier: VirtualAlloc failed".to_string(),
        31 => return "Carrier: VirtualProtect failed".to_string(),
        32 => return "Carrier: PEB module resolution failed".to_string(),
        33 => return "Carrier: PEB export resolution failed".to_string(),
        34..=39 => return "Carrier: unknown error".to_string(),
        _ => {}
    }

    if looks_like_ntstatus(code_u32) {
        if let Some(msg) = ntstatus_to_message(code_u32) {
            return format!("NTSTATUS 0x{code_u32:08X}: {msg}");
        }
        return format!("NTSTATUS 0x{code_u32:08X}");
    }

    format!("Exit code {code_u32} (0x{code_u32:08X})")
}

#[cfg(target_os = "windows")]
fn ntstatus_to_message(status: u32) -> Option<String> {
    use windows::Win32::Foundation::{
        GetLastError, HLOCAL, LocalFree, NTSTATUS, RtlNtStatusToDosError,
    };
    use windows::Win32::System::Diagnostics::Debug::{
        FORMAT_MESSAGE_ALLOCATE_BUFFER, FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS,
        FormatMessageW,
    };
    use windows::core::PWSTR;

    let dos: u32 = unsafe { RtlNtStatusToDosError(NTSTATUS(status as i32)) };
    if dos == 0 {
        return None;
    }

    let flags =
        FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS | FORMAT_MESSAGE_ALLOCATE_BUFFER;

    let buf: PWSTR = PWSTR::null();

    let len = unsafe { FormatMessageW(flags, None, dos, 0, PWSTR(buf.0), 0, None) };

    if len == 0 || buf.is_null() {
        let _ = unsafe { GetLastError() };
        return None;
    }

    let slice = unsafe { std::slice::from_raw_parts(buf.0, len as usize) };
    let s = String::from_utf16_lossy(slice).trim().to_string();

    unsafe {
        let _ = LocalFree(Option::from(HLOCAL(buf.0 as _)));
    }

    Some(s)
}

#[cfg(not(target_os = "windows"))]
fn ntstatus_to_message(_status: u32) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn looks_like_ntstatus(code: u32) -> bool {
    (code & 0x8000_0000) != 0
}

#[cfg(not(target_os = "windows"))]
fn looks_like_ntstatus(_code: u32) -> bool {
    false
}
