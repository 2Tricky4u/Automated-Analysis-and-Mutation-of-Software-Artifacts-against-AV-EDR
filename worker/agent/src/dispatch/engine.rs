//! Execution engine - orchestrates artifact execution lifecycle.
//!
//! Contains the core execution logic, transport-agnostic.
//! Uses ControlPlaneSink for status/telemetry delivery.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::dispatch::classifier;
use crate::dispatch::guards::{DELAY, MonitorGuard, ProcessGuard, RedEdrGuard};
use crate::dispatch::sink::ControlPlaneSink;
use crate::dispatch::types::{RunContext, RunOutcome, RunPhaseTimings, RunRequest};
use crate::infra::helpers;
use crate::telemetry;

/// Errors during execution setup (before process spawns)
#[derive(Debug)]
pub enum RunError {
    ArtifactNotFound(String),
    RedEdrSetupFailed(String),
    EnvironmentSetupFailed(String),
    ProcessSpawnFailed(String),
    /// RedEDR had leftover events from a previous run (strict mode)
    FailedPrecondition(String),
}

impl RunError {
    pub fn into_status(self) -> tonic::Status {
        match self {
            RunError::ArtifactNotFound(msg) => tonic::Status::not_found(msg),
            RunError::RedEdrSetupFailed(msg) => tonic::Status::internal(msg),
            RunError::EnvironmentSetupFailed(msg) => tonic::Status::internal(msg),
            RunError::ProcessSpawnFailed(msg) => tonic::Status::internal(msg),
            RunError::FailedPrecondition(msg) => tonic::Status::failed_precondition(msg),
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::ArtifactNotFound(msg) => write!(f, "Artifact not found: {}", msg),
            RunError::RedEdrSetupFailed(msg) => write!(f, "RedEDR setup failed: {}", msg),
            RunError::EnvironmentSetupFailed(msg) => write!(f, "Environment setup failed: {}", msg),
            RunError::ProcessSpawnFailed(msg) => write!(f, "Process spawn failed: {}", msg),
            RunError::FailedPrecondition(msg) => write!(f, "Failed precondition: {}", msg),
        }
    }
}

impl std::error::Error for RunError {}

/// Execute an artifact run.
///
/// Core execution pipeline:
/// 1. Validate artifact exists
/// 2. Setup RedEDR (sanity check, start tracing)
/// 3. Prepare environment (telemetry dir, trace collectors)
/// 4. Spawn artifact process
/// 5. Monitor execution with timeout
/// 6. Collect all telemetry (RedEDR, trace, coverage, checkpoints)
/// 7. Stream telemetry to controller via sink
/// 8. Cleanup (reset RedEDR)
///
/// Assumes execution lock is already held by caller.
pub async fn execute_run(
    request: &RunRequest,
    context: &RunContext,
    sink: Arc<dyn ControlPlaneSink>,
) -> Result<RunOutcome, RunError> {
    let mut phase_timings = RunPhaseTimings::default();

    info!(
        "[OK] Starting sample execution: job_id={}, artifact_id={}",
        request.job_id, request.artifact_id
    );

    // ====================================================================
    // Phase 1: Validate artifact
    // ====================================================================

    if !context.artifact_path.exists() {
        return Err(RunError::ArtifactNotFound(format!(
            "Artifact {} not found on worker. Transfer it first using SendArtifact RPC.",
            request.artifact_id
        )));
    }

    info!("Resolved artifact to path: {:?}", context.artifact_path);

    // ====================================================================
    // Phase 2: Setup RedEDR
    // ====================================================================

    let rededr_start = Instant::now();

    let collector = telemetry::collectors::rededr::RedEdrCollector::new(
        telemetry::collectors::rededr::RedEdrCollectorConfig {
            base_url: context.config.telemetry.rededr.base_url.clone(),
            flush_interval_ms: 1000,
            job_id: request.job_id.clone(),
            run_id: request.run_id.clone(),
        },
    );
    let rededr_guard = RedEdrGuard::new(collector);

    // Sanity check: RedEDR should be clean
    info!("Performing pre-run sanity check: RedEDR should be empty");
    let pre_run_events = rededr_guard
        .collector()
        .collect_all("sanity-check")
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to collect pre-run events during sanity check: {}", e);
            warn!("This might be due to malformed initialization event - treating as empty and continuing");
            Vec::new()
        });

    let leftover_count = pre_run_events.len();
    let has_real_contamination = leftover_count > 1;
    let strict_mode = context.config.telemetry.rededr.strict_contamination_check;

    if leftover_count == 1 {
        info!(
            "Sanity check: Found 1 event (likely initialization noise), silently discarding and continuing"
        );
    } else if has_real_contamination {
        warn!(
            "SANITY CHECK FAILED: Found {} leftover events in RedEDR before starting new run!",
            leftover_count
        );
        warn!("This indicates the previous run did not reset properly.");

        // Strict mode: fail immediately without attempting recovery
        if strict_mode {
            error!(
                "strict_contamination_check is enabled: refusing to run with {} contaminated events",
                leftover_count
            );
            return Err(RunError::FailedPrecondition(format!(
                "RedEDR has {} leftover events from previous run (strict_contamination_check=true)",
                leftover_count
            )));
        }

        warn!(
            "Sending contaminated events to controller with metadata: job_id=contaminated, artifact_id=unknown"
        );

        let _contaminated_events: Vec<crate::automutate::common::TelemetryData> = pre_run_events
            .into_iter()
            .map(|mut event| {
                event.job_id = "contaminated".to_string();
                event
                    .metadata
                    .insert("artifact_id".to_string(), "unknown".to_string());
                event.metadata.insert(
                    "run_id".to_string(),
                    format!("contaminated-{}", chrono::Utc::now().timestamp()),
                );
                event
                    .metadata
                    .insert("contamination_detected".to_string(), "true".to_string());
                event
            })
            .collect();
        warn!(
            "{} contaminated events detected but not sent",
            leftover_count
        );

        info!(
            "Setting RedEDR to trace new artifact: {} (before reset)",
            context.artifact_name
        );
        if let Err(e) = rededr_guard
            .collector()
            .start_trace(vec![context.artifact_name.clone()])
            .await
        {
            error!("Failed to set new trace target before reset: {}", e);
            return Err(RunError::RedEdrSetupFailed(format!(
                "Failed to configure RedEDR trace target: {}",
                e
            )));
        }

        warn!("Force-resetting RedEDR to clear contaminated state (trace target already set)...");
        if let Err(e) = rededr_guard.collector().reset().await {
            error!("Failed to force-reset RedEDR: {}", e);
            return Err(RunError::FailedPrecondition(format!(
                "RedEDR is contaminated ({} leftover events) and reset failed: {}",
                leftover_count, e
            )));
        }
        info!(
            "RedEDR force-reset completed. Now watching: {}",
            context.artifact_name
        );
    } else {
        info!("[+] Pre-run sanity check passed: RedEDR is clean");
    }

    if !has_real_contamination {
        rededr_guard
            .collector()
            .start_trace(vec![context.artifact_name.clone()])
            .await
            .map_err(|e| {
                error!("Failed to start RedEDR tracing: {}", e);
                RunError::RedEdrSetupFailed(format!("Failed to start RedEDR tracing: {}", e))
            })?;

        info!(
            "RedEDR tracing started for artifact: {}",
            context.artifact_name
        );
    }

    phase_timings.rededr_setup_ms = rededr_start.elapsed().as_millis() as u64;

    // ====================================================================
    // Phase 3: Prepare environment
    // ====================================================================

    // Create telemetry directory (clean if exists to avoid stale files)
    crate::infra::system::prepare_telemetry_dir(&context.telemetry_dir).map_err(|e| {
        RunError::EnvironmentSetupFailed(format!("Failed to create telemetry directory: {}", e))
    })?;

    // Start line-level trace collector with streaming to file
    let trace_events_file = context.telemetry_dir.join("trace_events.jsonl");
    let trace_events_file_clone = trace_events_file.clone();

    let (trace_tx, mut trace_rx) = tokio::sync::mpsc::channel(100_000);
    let trace_collector = telemetry::collectors::trace::TraceCollector::new(trace_tx.clone());

    // Spawn streaming writer (optimized: only include thread_id when it changes)
    let streaming_handle = tokio::spawn(async move {
        use tokio::io::{AsyncWriteExt, BufWriter};

        match tokio::fs::File::create(&trace_events_file_clone).await {
            Ok(file) => {
                let mut writer = BufWriter::with_capacity(256 * 1024, file);
                let mut event_count = 0u64;
                let mut json_buffer = String::with_capacity(512);
                let mut last_thread_id: Option<u32> = None;

                while let Some(mut event) = trace_rx.recv().await {
                    let _include_thread_id = match last_thread_id {
                        None => {
                            last_thread_id = Some(event.thread_id);
                            true
                        }
                        Some(prev_tid) if prev_tid != event.thread_id => {
                            last_thread_id = Some(event.thread_id);
                            true
                        }
                        Some(_) => {
                            event.thread_id = 0;
                            false
                        }
                    };

                    json_buffer.clear();
                    match serde_json::to_writer(unsafe { json_buffer.as_mut_vec() }, &event) {
                        Ok(_) => {
                            json_buffer.push('\n');
                            if let Err(e) = writer.write_all(json_buffer.as_bytes()).await {
                                error!("Failed to write trace event to file: {}", e);
                                break;
                            }
                            event_count += 1;

                            if event_count.is_multiple_of(10_000)
                                && let Err(e) = writer.flush().await
                            {
                                error!("Failed to flush trace file: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Failed to serialize trace event: {}", e);
                        }
                    }
                }

                let _ = writer.flush().await;
                info!(
                    "[OK] Streaming writer closed, wrote {} trace events to file",
                    event_count
                );
            }
            Err(e) => {
                error!("Failed to create trace events file: {}", e);
            }
        }
    });

    // Spawn async trace collector (reads from named pipe, sends to channel)
    let trace_handle = tokio::spawn(async move {
        if let Err(e) = trace_collector.start_server().await {
            error!("Trace collector failed: {}", e);
        }
    });

    info!(
        "Async trace collector started on named pipe: \\\\.\\pipe\\rededr_trace (streaming to file)"
    );

    // ====================================================================
    // Phase 4: Spawn process
    // ====================================================================

    let spawn_start = Instant::now();

    let child =
        crate::infra::process::spawn_artifact(&context.artifact_path, &context.telemetry_dir)
            .map_err(|e| {
                error!("Failed to spawn process: {}", e);
                RunError::ProcessSpawnFailed(format!("Failed to spawn process: {}", e))
            })?;

    let mut process_guard = ProcessGuard::new(child);

    let pid = process_guard.child_mut().id().ok_or_else(|| {
        error!("Failed to get PID from spawned process");
        RunError::ProcessSpawnFailed("Failed to get PID".to_string())
    })?;

    info!("Artifact process spawned: pid={}", pid);

    phase_timings.process_spawn_ms = spawn_start.elapsed().as_millis() as u64;

    // ====================================================================
    // Capture output streams
    // ====================================================================

    let stdout = process_guard.child_mut().stdout.take();
    let stderr = process_guard.child_mut().stderr.take();

    let stdout_handle = crate::infra::process::capture_stream(stdout);
    let stderr_handle = crate::infra::process::capture_stream(stderr);

    // ====================================================================
    // Phase 5: Start monitoring
    // ====================================================================

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

    let monitor = crate::dispatch::monitor::ExecutionMonitor::new(
        crate::dispatch::monitor::MonitorConfig {
            run_id: request.run_id.clone(),
            job_id: request.job_id.clone(),
            worker_id: context.worker_id.clone(),
            worker_ip: context.config.worker.ip_address.clone(),
            artifact_name: context.artifact_name.clone(),
            pid,
            rededr_base_url: context.config.telemetry.rededr.base_url.clone(),
            timeout_seconds: request.timeout_seconds as i32,
        },
        sink.clone(),
    );

    let monitor_handle = tokio::spawn(async move {
        monitor.start(stop_rx, event_tx).await;
    });

    let event_consumer = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event.event_type.as_str() {
                "started" => info!("Monitor: {}", event.details),
                "heartbeat" => info!("Monitor: {}", event.details),
                "telemetry_idle" => warn!("Monitor: {}", event.details),
                "terminated" => info!("Monitor: {}", event.details),
                "completed" => info!("Monitor: {}", event.details),
                _ => info!("Monitor: {} - {}", event.event_type, event.details),
            }
        }
    });

    let mut monitor_guard = Some(MonitorGuard::new(stop_tx, monitor_handle, event_consumer));

    // ====================================================================
    // Phase 6: Wait for process completion or timeout
    // ====================================================================

    let wait_start = Instant::now();
    let timeout_duration = Duration::from_secs(request.timeout_seconds as u64);

    let exit_result =
        tokio::time::timeout(timeout_duration, process_guard.child_mut().wait()).await;

    let (exit_code, timed_out) = match exit_result {
        Ok(Ok(status)) => match status.code() {
            Some(code) => {
                info!("Process exited with code: {}", code);
                (code, false)
            }
            None => {
                warn!(
                    "Process was terminated externally (likely by AV/EDR) - no exit code available"
                );
                (-2, false)
            }
        },
        Ok(Err(e)) => {
            error!("Failed to wait for process: {}", e);
            if let Some(guard) = monitor_guard.take() {
                guard.stop().await;
            }
            (-1, false)
        }
        Err(_) => {
            // Timeout triggered, but check if process already exited naturally
            let pid = process_guard.child_mut().id();

            tokio::time::sleep(Duration::from_millis(100)).await;

            match process_guard.child_mut().try_wait() {
                Ok(Some(status)) => {
                    info!("Process exited naturally just as timeout expired (race condition)");
                    match status.code() {
                        Some(code) => {
                            info!("Process exited with code: {}", code);
                            if let Some(guard) = monitor_guard.take() {
                                guard.stop().await;
                            }
                            (code, false)
                        }
                        None => {
                            warn!("Process was terminated externally (likely by AV/EDR)");
                            if let Some(guard) = monitor_guard.take() {
                                guard.stop().await;
                            }
                            (-2, false)
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    info!(
                        "Timeout: Process still running after {}s, forcefully killing",
                        request.timeout_seconds
                    );

                    crate::infra::process::kill_process_tree(process_guard.child_mut(), pid).await;

                    if let Some(pid) = pid
                        && crate::infra::process::is_process_alive(pid)
                    {
                        warn!("Process {} still alive after kill attempt!", pid);
                    }

                    if let Some(guard) = monitor_guard.take() {
                        guard.stop().await;
                    }

                    (-1, true)
                }
            }
        }
    };

    let _ = process_guard.disarm();

    phase_timings.process_wait_ms = wait_start.elapsed().as_millis() as u64;
    // Elapsed time available via phase_timings

    // ====================================================================
    // Collect output and cleanup monitoring
    // ====================================================================

    let stdout_output = stdout_handle.await.unwrap_or_default();
    let stderr_output = stderr_handle.await.unwrap_or_default();

    if !stdout_output.is_empty() {
        let formatted = crate::WorkerAgentService::truncate_middle_output(&stdout_output);
        info!("Process stdout:\n{}", formatted);
    }

    if !stderr_output.is_empty() {
        let formatted = crate::WorkerAgentService::truncate_middle_output(&stderr_output);
        info!("Process stderr:\n{}", formatted);
    } else {
        info!("Process stderr: (empty)");
    }

    // Stop monitor BEFORE telemetry window
    if let Some(guard) = monitor_guard.take() {
        guard.stop().await;
    }

    // Give trace collector a moment to read final events from pipe
    info!("Waiting for trace collector to finish reading pipe...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    trace_handle.abort();
    drop(trace_tx);

    match tokio::time::timeout(Duration::from_secs(DELAY), streaming_handle).await {
        Ok(Ok(())) => info!("Streaming writer completed successfully"),
        Ok(Err(e)) => error!("Streaming writer panicked: {:?}", e),
        Err(_) => warn!("Streaming writer timeout after {} seconds", DELAY),
    }

    // ====================================================================
    // Phase 7: Collect telemetry
    // ====================================================================

    let telemetry_start = Instant::now();

    info!("Collecting telemetry events from RedEDR...");
    let mut telemetry_events = rededr_guard
        .collector()
        .collect_all(&request.job_id)
        .await
        .unwrap_or_else(|e| {
            error!("Failed to collect telemetry: {}", e);
            error!("Continuing with empty telemetry! Execution status will still be reported");
            Vec::new()
        });

    info!("Collected {} RedEDR events", telemetry_events.len());

    // Collect trace log (JSONL file from named pipe)
    crate::telemetry::pipeline::package_trace_log(
        &trace_events_file,
        &request.job_id,
        &mut telemetry_events,
    );

    // Collect trace.log (binary protocol fallback)
    let trace_log_path = context.telemetry_dir.join("trace.log");
    crate::telemetry::pipeline::collect_trace_log_binary(
        &trace_log_path,
        &request.job_id,
        &mut telemetry_events,
    );

    // Collect BB coverage
    let coverage_bin_path = context.telemetry_dir.join("coverage.bin");
    let coverage_bbs_path = context.telemetry_dir.join("coverage_bbs.txt");

    if coverage_bin_path.exists() && coverage_bbs_path.exists() {
        info!(
            "Found BB coverage files: {:?}, {:?}",
            coverage_bin_path, coverage_bbs_path
        );

        match helpers::collect_bb_coverage(&coverage_bin_path, &coverage_bbs_path, &request.job_id)
            .await
        {
            Ok(coverage_event) => {
                info!(
                    "Collected BB coverage: {} basic blocks",
                    coverage_event
                        .typed_event
                        .as_ref()
                        .and_then(|te| match te {
                            crate::automutate::common::telemetry_data::TypedEvent::Coverage(c) =>
                                Some(c.total_bbs),
                            _ => None,
                        })
                        .unwrap_or(0)
                );
                telemetry_events.push(coverage_event);
            }
            Err(e) => {
                warn!("Failed to collect BB coverage: {}", e);
            }
        }
    } else {
        warn!("BB coverage files NOT found in telemetry directory:");
        if !coverage_bin_path.exists() {
            warn!("  Missing: {:?}", coverage_bin_path);
        }
        if !coverage_bbs_path.exists() {
            warn!("  Missing: {:?}", coverage_bbs_path);
        }
        warn!("  Artifact may not be instrumented for BB coverage, or runtime did not flush files");
    }

    // Collect API checkpoints
    let checkpoints_path = context.telemetry_dir.join("checkpoints.log");
    if checkpoints_path.exists() {
        info!("Found API checkpoints file: {:?}", checkpoints_path);

        match helpers::collect_api_checkpoints(&checkpoints_path, &request.job_id).await {
            Ok(checkpoint_events) => {
                let checkpoint_count = checkpoint_events.len();
                info!("Collected {} API checkpoint events", checkpoint_count);
                telemetry_events.extend(checkpoint_events);
            }
            Err(e) => {
                warn!("Failed to collect API checkpoints: {}", e);
            }
        }
    } else {
        warn!("API checkpoints file NOT found: {:?}", checkpoints_path);
        warn!("  Artifact may not be instrumented for API tracing, or runtime did not flush file");
    }

    let telemetry_count = telemetry_events.len() as i32;
    info!("Total telemetry events collected: {}", telemetry_count);

    phase_timings.telemetry_collect_ms = telemetry_start.elapsed().as_millis() as u64;

    // ====================================================================
    // Phase 7b: Classify detection outcome
    // ====================================================================

    let actual_elapsed =
        Duration::from_millis(phase_timings.process_spawn_ms + phase_timings.process_wait_ms);

    let (verdict, last_checkpoint) = classifier::classify_run(
        exit_code,
        timed_out,
        actual_elapsed.as_secs_f64() * 1000.0,
        &telemetry_events,
        None, // dry_run_exit_code: future dry-run integration
    );

    info!(
        "Detection verdict: {:?} (detected={}), last_checkpoint={:?}",
        verdict,
        verdict.is_detected(),
        last_checkpoint
    );

    let detection_verdict = verdict.as_str().to_string();
    let last_checkpoint_str = last_checkpoint.unwrap_or_default();

    // Add phase timings as a telemetry event for observability
    telemetry_events.push(crate::automutate::common::TelemetryData {
        job_id: request.job_id.clone(),
        event_type: "phase_timings".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        payload: vec![],
        metadata: phase_timings.to_metadata(),
        typed_event: None,
    });

    let telemetry_count = telemetry_events.len() as i32;

    // ====================================================================
    // Phase 8: Stream telemetry to controller
    // ====================================================================

    if !telemetry_events.is_empty() {
        info!(
            "Streaming {} telemetry events to controller via sink",
            telemetry_count
        );

        debug!(
            "[WORKER-TELEMETRY-SEND] run_id='{}' job_id='{}' events={}",
            request.run_id, request.job_id, telemetry_count
        );

        let telemetry_batch = crate::automutate::common::TelemetryBatch {
            job_id: request.job_id.clone(),
            run_id: request.run_id.clone(),
            events: telemetry_events.clone(),
            is_final: true,
        };

        match sink.send_telemetry(telemetry_batch).await {
            Ok(_) => {
                info!(
                    "Successfully streamed {} telemetry events to controller",
                    telemetry_count
                );
            }
            Err(e) => {
                error!("Failed to stream telemetry to controller: {}", e);
                warn!(
                    "Telemetry will be lost - controller should handle stream failures gracefully"
                );
            }
        }
    } else {
        warn!(
            "No telemetry events collected [job: {}, run: {}]",
            request.job_id, request.run_id
        );
    }

    // ====================================================================
    // Phase 9: Reset RedEDR
    // ====================================================================

    let reset_start = Instant::now();

    if let Err(e) = rededr_guard.reset_now().await {
        error!(
            "Failed to reset RedEDR: {} - Next execution may have contaminated events!",
            e
        );
    }

    phase_timings.rededr_reset_ms = reset_start.elapsed().as_millis() as u64;

    // ====================================================================
    // Build outcome
    // ====================================================================

    Ok(RunOutcome {
        exit_code,
        timed_out,
        stdout: stdout_output,
        stderr: stderr_output,
        telemetry_events,
        elapsed: actual_elapsed,
        phase_timings,
        detection_verdict,
        last_checkpoint: last_checkpoint_str,
    })
}
