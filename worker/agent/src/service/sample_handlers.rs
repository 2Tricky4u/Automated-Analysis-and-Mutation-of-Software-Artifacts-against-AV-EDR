use crate::automutate::common::{SampleRequest, SampleResponse, TelemetryData};
use crate::automutate::worker::MonitorEvent;
use crate::execution::guards::{MonitorGuard, ProcessGuard, RedEdrGuard, DELAY};
use crate::execution::monitor::ExecutionMonitor;
use crate::service::helpers;
use crate::{stream_handler, telemetry, WorkerAgentService};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::watch;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

const ARTIFACTS_PATH: &str = "C:\\temp\\artifacts"; //TODO read from config.storage.artifact_path

pub async fn run_sample(
    service: &WorkerAgentService,
    request: Request<SampleRequest>,
) -> Result<Response<SampleResponse>, Status> {
        let req = request.into_inner();

        // Use run_id from worker state (set by controller via stream_handler)
        // This ensures telemetry batches and responses use the same run_id for correlation
        let run_id = {
            if let Some(handler) = service.stream_handler.read().await.as_ref() {
                let state = handler.worker_state.read().await;
                state.current_run_id.clone().unwrap_or_else(|| {
                    warn!("current_run_id not set in worker state, generating new UUID");
                    uuid::Uuid::new_v4().to_string()
                })
            } else {
                // No stream handler (Phase 1 mode), generate UUID
                uuid::Uuid::new_v4().to_string()
            }
        };

        let job_id = req.job_id.clone();
        let artifact_name = format!("{}.exe", req.artifact_id);

        info!(
            "Received sample execution request: job_id={}, artifact_id={}, run_id={}",
            job_id, req.artifact_id, run_id
        );
        // ====================================================================
        // Acquire single execution lock
        // ====================================================================

        let _execution_lock = {
            let mut state = service.execution_lock.lock().await;

            if state.busy {
                let current_job = state.current_job_id.as_deref().unwrap_or("unknown");
                let current_artifact = state.current_artifact.as_deref().unwrap_or("unknown");
                let msg = format!(
                    "Worker is busy executing job_id={} artifact={}. This worker supports only ONE concurrent execution.",
                    current_job, current_artifact
                );
                warn!("[ERROR] REJECTED: {}", msg);
                return Err(Status::resource_exhausted(msg));
            }

            // Acquire lock
            state.busy = true;
            state.current_job_id = Some(job_id.clone());
            state.current_artifact = Some(artifact_name.clone());

            info!(
                "Execution lock ACQUIRED: job_id={}, artifact={}",
                job_id, artifact_name
            );

            crate::execution::guards::ExecutionLockGuard::new(service.execution_lock.clone())
        };


        info!(
            "[OK] Starting sample execution: job_id={}, artifact_id={}",
            job_id, req.artifact_id
        );

        // ====================================================================
        // Setup with RAII guards for automatic cleanup
        // ====================================================================

        // 1. Resolve artifact_id to local path
        let artifact_path =
            std::path::Path::new(ARTIFACTS_PATH).join(format!("{}.exe", req.artifact_id));

        if !artifact_path.exists() {
            return Err(Status::not_found(format!(
                "Artifact {} not found on worker. Transfer it first using SendArtifact RPC.",
                req.artifact_id
            )));
        }

        info!("Resolved artifact to path: {:?}", artifact_path);

        // 2. Create RedEDR collector with guard (cleanup on any error/panic)
        let collector = telemetry::collectors::rededr::RedEdrCollector::new(
            telemetry::collectors::rededr::RedEdrCollectorConfig {
                base_url: service.config.telemetry.rededr.base_url.clone(),
                flush_interval_ms: 1000,
                job_id: job_id.clone(),
                run_id: run_id.clone(),
            },
        );
        //TODO activate lock on rededr?
        //collector.acquire_lock().await.expect("TODO: panic message");
        let rededr_guard = RedEdrGuard::new(collector);

        // 4. Sanity check RedEDR should be clean (no leftover events from previous run)
        info!("Performing pre-run sanity check: RedEDR should be empty");
        let pre_run_events = match rededr_guard.collector().collect_all("sanity-check").await {
            Ok(events) => events,
            Err(e) => {
                warn!(
                    "Failed to collect pre-run events during sanity check: {}",
                    e
                );
                warn!(
                    "This might be due to malformed initialization event - treating as empty and continuing"
                );
                Vec::new() // Treat collection failure as empty (no contamination)
            }
        };

        let leftover_count = pre_run_events.len();

        // Tolerate single initialization event (RedEdr startup noise)
        // More than 1 event = real contamination from previous run
        let has_real_contamination = leftover_count > 1;

        if leftover_count == 1 {
            info!(
                "Sanity check: Found 1 event (likely initialization noise), silently discarding and continuing"
            );
            // Discard the single event might be malformed
            // Just drop pre_run_events and proceed with execution
        } else if has_real_contamination {
            warn!(
                "SANITY CHECK FAILED: Found {} leftover events in RedEDR before starting new run!",
                leftover_count
            );
            warn!("This indicates the previous run did not reset properly.");
            warn!(
                "Sending contaminated events to controller with metadata: job_id=contaminated, artifact_id=unknown"
            );

            // Send contaminated events with special metadata
            let contaminated_events: Vec<crate::automutate::common::TelemetryData> = pre_run_events
                .into_iter()
                .map(|mut event| {
                    // Override job_id and add metadata to mark as contaminated
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
                "{} contaminated events detected but not sent (controller will pull via GetTelemetry)",
                leftover_count
            );

            // CRITICAL: Start tracing the NEW artifact BEFORE resetting
            // This ensures RedEDR switches to watching the new process instead of the old one
            info!(
                "Setting RedEDR to trace new artifact: {} (before reset)",
                artifact_name
            );
            if let Err(e) = rededr_guard
                .collector()
                .start_trace(vec![artifact_name.clone()])
                .await
            {
                error!("Failed to set new trace target before reset: {}", e);
                return Err(Status::internal(format!(
                    "Failed to configure RedEDR trace target: {}",
                    e
                )));
            }

            // Force reset RedEDR to clear contamination (trace target already set above)
            warn!(
                "Force-resetting RedEDR to clear contaminated state (trace target already set)..."
            );
            if let Err(e) = rededr_guard.collector().reset().await {
                error!("Failed to force-reset RedEDR: {}", e);
                return Err(Status::internal(format!(
                    "RedEDR is contaminated and reset failed: {}",
                    e
                )));
            }
            info!(
                "RedEDR force-reset completed. Now watching: {}",
                artifact_name
            );
        } else {
            info!("[+] Pre-run sanity check passed: RedEDR is clean");
        }

        // 5. Start RedEDR tracing if not already started (normal path when no real contamination)
        if !has_real_contamination {
            rededr_guard
                .collector()
                .start_trace(vec![artifact_name.clone()])
                .await
                .map_err(|e| {
                    error!("Failed to start RedEDR tracing: {}", e);
                    Status::internal(format!("Failed to start RedEDR tracing: {}", e))
                })?;

            info!("RedEDR tracing started for artifact: {}", artifact_name);
        }

        // 4. Spawn process with guard ensures kill if error occurs
        // Create artifact-specific telemetry directory to avoid cross-contamination
        let artifacts_base = std::path::Path::new(ARTIFACTS_PATH);
        let telemetry_dir = artifacts_base.join(format!("telemetry_{}", req.artifact_id));

        // Create telemetry directory (clean it if it already exists to avoid stale files)
        if telemetry_dir.exists() {
            let _ = std::fs::remove_dir_all(&telemetry_dir);
        }
        std::fs::create_dir_all(&telemetry_dir).map_err(|e| {
            error!("Failed to create telemetry directory: {}", e);
            Status::internal(format!("Failed to create telemetry directory: {}", e))
        })?;

        info!(
            "Created artifact-specific telemetry directory: {:?}",
            telemetry_dir
        );

        // 5. Start line-level trace collector with streaming to file
        // Stream events to file during execution
        let trace_events_file = telemetry_dir.join("trace_events.jsonl");
        let trace_events_file_clone = trace_events_file.clone();

        let (trace_tx, mut trace_rx) = tokio::sync::mpsc::channel(100_000);
        let trace_collector = telemetry::collectors::trace::TraceCollector::new(trace_tx.clone());

        // Spawn streaming writer
        // Optimized: only include thread_id when it changes
        let streaming_handle = tokio::spawn(async move {
            use tokio::io::{AsyncWriteExt, BufWriter};

            match tokio::fs::File::create(&trace_events_file_clone).await {
                Ok(file) => {
                    // Use buffered writer for better performance
                    let mut writer = BufWriter::with_capacity(256 * 1024, file);
                    let mut event_count = 0u64;
                    let mut json_buffer = String::with_capacity(512);
                    let mut last_thread_id: Option<u32> = None;

                    while let Some(mut event) = trace_rx.recv().await {
                        // TODO: add thead id to feedback?
                        let include_thread_id = match last_thread_id {
                            None => {
                                // First event, always include thread_id
                                last_thread_id = Some(event.thread_id);
                                true
                            }
                            Some(prev_tid) if prev_tid != event.thread_id => {
                                // Thread changed include it
                                last_thread_id = Some(event.thread_id);
                                true
                            }
                            Some(_) => {
                                // Same thread - omit thread_id (set to 0 as marker)
                                event.thread_id = 0;
                                false
                            }
                        };

                        // Serialize event to JSON
                        json_buffer.clear();
                        match serde_json::to_writer(unsafe { json_buffer.as_mut_vec() }, &event) {
                            Ok(_) => {
                                // Write JSON line with newline
                                json_buffer.push('\n');
                                if let Err(e) = writer.write_all(json_buffer.as_bytes()).await {
                                    error!("Failed to write trace event to file: {}", e);
                                    break;
                                }
                                event_count += 1;

                                // Flush every 10000 events for safety (less frequent = faster)
                                if event_count % 10_000 == 0 {
                                    if let Err(e) = writer.flush().await {
                                        error!("Failed to flush trace file: {}", e);
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Failed to serialize trace event: {}", e);
                            }
                        }
                    }

                    // Final flush
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

        let child = tokio::process::Command::new(&artifact_path)
            .current_dir(&telemetry_dir) // Runtime will write coverage.bin, checkpoints.log here
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                error!("Failed to spawn process: {}", e);
                Status::internal(format!("Failed to spawn process: {}", e))
            })?;

        let mut process_guard = ProcessGuard::new(child);

        let pid = process_guard.child_mut().id().ok_or_else(|| {
            error!("Failed to get PID from spawned process");
            Status::internal("Failed to get PID")
        })?;

        info!("Artifact process spawned: pid={}", pid);

        // ====================================================================
        // Capture output streams
        // ====================================================================

        // Capture stdout and stderr for error reporting
        let stdout = process_guard.child_mut().stdout.take();
        let stderr = process_guard.child_mut().stderr.take();

        // Spawn tasks to capture output streams
        let stdout_handle = tokio::spawn(async move {
            if let Some(stdout) = stdout {
                use tokio::io::AsyncReadExt;
                let mut reader = tokio::io::BufReader::new(stdout);
                let mut output = String::new();
                let _ = reader.read_to_string(&mut output).await;
                output
            } else {
                String::new()
            }
        });

        let stderr_handle = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                use tokio::io::AsyncReadExt;
                let mut reader = tokio::io::BufReader::new(stderr);
                let mut output = String::new();
                let _ = reader.read_to_string(&mut output).await;
                output
            } else {
                String::new()
            }
        });

        // ====================================================================
        // Start monitoring with guard
        // ====================================================================

        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

        // Retrieve StreamHandler if available (set by establish_stream)
        let stream_handler = {
            let handler_lock = service.stream_handler.read().await;
            handler_lock.clone()
        };

        if stream_handler.is_some() {
            info!("ExecutionMonitor will send status via StreamHandler");
        } else {
            info!("ExecutionMonitor running without StreamHandler (worker-only mode)");
        }

        // Clone for ExecutionMonitor (we need to keep a copy for telemetry streaming later)
        let stream_handler_for_monitor = stream_handler.clone();

        let monitor = crate::execution::monitor::ExecutionMonitor::new(
            run_id.clone(),
            job_id.clone(),
            service.worker_id.clone(),
            service.config.worker.ip_address.clone(),
            artifact_name.clone(),
            pid,
            service.config.telemetry.rededr.base_url.clone(),
            stream_handler_for_monitor, // Pass cloned StreamHandler to monitor
            req.timeout_seconds,
        );

        let monitor_handle = tokio::spawn(async move {
            monitor.start(stop_rx, event_tx).await;
        });

        // Spawn task to consume monitor events (log them)
        let event_consumer = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event.event_type.as_str() {
                    "started" => info!("Monitor: {}", event.details),
                    "heartbeat" => info!("Monitor: {}", event.details),
                    "stuck" => warn!("Monitor: {}", event.details),
                    "terminated" => info!("Monitor: {}", event.details),
                    "completed" => info!("Monitor: {}", event.details),
                    _ => info!("Monitor: {} - {}", event.event_type, event.details),
                }
            }
        });

        // Create monitor guard (cleanup on any error)
        let mut monitor_guard = Some(MonitorGuard::new(stop_tx, monitor_handle, event_consumer));

        // ====================================================================
        // Wait for process completion or timeout
        // ====================================================================

        let timeout_duration = Duration::from_secs(req.timeout_seconds as u64);
        let start_time = Instant::now();

        let exit_result =
            tokio::time::timeout(timeout_duration, process_guard.child_mut().wait()).await;

        let (exit_code, timed_out) = match exit_result {
            Ok(Ok(status)) => {
                // Check if process exited normally or was killed
                match status.code() {
                    Some(code) => {
                        info!("Process exited with code: {}", code);
                        (code, false)
                    }
                    None => {
                        // Process was terminated by signal/external kill (e.g., Windows Defender)
                        // On Windows, this typically means AV/EDR killed it
                        warn!(
                            "Process was terminated externally (likely by AV/EDR) - no exit code available"
                        );
                        // Use distinctive exit code to indicate external termination
                        (-2, false) // -2 = killed by external signal/AV
                    }
                }
            }
            Ok(Err(e)) => {
                error!("Failed to wait for process: {}", e);

                // Stop monitor immediately, process likely crashed/killed
                if let Some(guard) = monitor_guard.take() {
                    guard.stop().await;
                }

                (-1, false) // -1 = wait() failed
            }
            Err(_) => {
                // Timeout triggered, but check if process already exited naturally
                let pid = process_guard.child_mut().id();

                // Give the process a brief moment to report its exit status
                // (race condition: process might have exited just as timeout fired)
                tokio::time::sleep(Duration::from_millis(100)).await;

                // Try to collect exit status without blocking
                match process_guard.child_mut().try_wait() {
                    Ok(Some(status)) => {
                        // Process already exited! This was a race condition, not a real timeout
                        info!("Process exited naturally just as timeout expired (race condition)");
                        match status.code() {
                            Some(code) => {
                                info!("Process exited with code: {}", code);
                                // Stop monitor since process completed
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
                                (-2, false) // External termination, not timeout
                            }
                        }
                    }
                    Ok(None) | Err(_) => {
                        // Process is still running or status unavailable, this is a real timeout
                        info!(
                            "Timeout: Process still running after {}s, forcefully killing",
                            req.timeout_seconds
                        );

                        // taskkill /F /T to kill process tree forcefully
                        #[cfg(target_os = "windows")]
                        if let Some(pid) = pid {
                            info!("Timeout: Forcefully killing process tree for PID {}", pid);
                            let kill_result = std::process::Command::new("taskkill")
                                .args(&["/F", "/T", "/PID", &pid.to_string()])
                                .output();

                            if let Err(e) = kill_result {
                                error!("Failed to run taskkill: {}", e);
                            }
                        }

                        // Tokio's kill as backup
                        let _ = process_guard.child_mut().kill().await;

                        // Wait a moment for process to die
                        tokio::time::sleep(Duration::from_millis(500)).await;

                        // Verify process is dead
                        #[cfg(target_os = "windows")]
                        if let Some(pid) = pid {
                            use windows::Win32::Foundation::CloseHandle;
                            use windows::Win32::System::Threading::OpenProcess;
                            unsafe {
                                if let Ok(handle) = OpenProcess(
                                    windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
                                    false,
                                    pid,
                                ) {
                                    if !handle.is_invalid() {
                                        let _ = CloseHandle(handle);
                                        warn!("Process {} still alive after kill attempt!", pid);
                                    }
                                }
                            }
                        }

                        // Stop monitor immediately
                        if let Some(guard) = monitor_guard.take() {
                            guard.stop().await;
                        }

                        (-1, true) // Real timeout
                    }
                }
            }
        };

        // Disarm process guard since we've handled completion/timeout
        let _ = process_guard.disarm();

        let elapsed = start_time.elapsed();

        // ====================================================================
        // Collect output and cleanup monitoring
        // ====================================================================

        // Collect stdout and stderr
        let stdout_output = stdout_handle.await.unwrap_or_default();
        let stderr_output = stderr_handle.await.unwrap_or_default();

        // Log captured output
        if !stdout_output.is_empty() {
            let formatted = WorkerAgentService::truncate_middle_output(&stdout_output);
            info!("Process stdout:\n{}", formatted);
        }

        if !stderr_output.is_empty() {
            let formatted = WorkerAgentService::truncate_middle_output(&stderr_output);
            info!("Process stderr:\n{}", formatted); // Changed to INFO so we always see it
        } else {
            info!("Process stderr: (empty)");
        }

        // ====================================================================
        // Stop monitor and post-exit telemetry window (10 seconds)
        // ====================================================================

        // Stop monitor BEFORE telemetry window to prevent duplicate status reports
        if let Some(guard) = monitor_guard.take() {
            guard.stop().await;
        }

        // Give trace collector a moment to read any final events from the pipe
        // (Dont abort immediately SUCCESS/CHECKPOINT events may still be in pipe buffer)
        info!("Waiting for trace collector to finish reading pipe...");
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Now stop trace collector and streaming writer
        trace_handle.abort(); // Stop named pipe collector
        drop(trace_tx); // Close channel sender, which will cause streaming_handle to finish

        // Wait for streaming writer to flush all events to disk
        match tokio::time::timeout(Duration::from_secs(DELAY), streaming_handle).await {
            Ok(Ok(())) => {
                info!("Streaming writer completed successfully");
            }
            Ok(Err(e)) => {
                error!("Streaming writer panicked: {:?}", e);
            }
            Err(_) => {
                warn!("Streaming writer timeout after {} seconds", DELAY);
            }
        }

        // ====================================================================
        // Collect telemetry and reset RedEDR (BEFORE final status)
        // ====================================================================

        // Collect full telemetry batch
        info!("Collecting telemetry events from RedEDR...");
        let mut telemetry_events = rededr_guard
            .collector()
            .collect_all(&job_id)
            .await
            .unwrap_or_else(|e| {
                error!("Failed to collect telemetry: {}", e);
                error!("Continuing with empty telemetry! Execution status will still be reported");
                Vec::new() // Continue with empty telemetry instead of failing entire job
            });

        info!("Collected {} RedEDR events", telemetry_events.len());

        // Send line-level trace log with TWO-PHASE approach:
        // 1. If trace ≤ 2MB: send entire trace immediately with main telemetry batch (RedEDR, BB, API)
        // 2. If trace > 2MB: send last 2MB immediately, then spawn async task to compress & send full trace
        if trace_events_file.exists() {
            info!("Reading trace events from file: {:?}", trace_events_file);

            match std::fs::read_to_string(&trace_events_file) {
                Ok(contents) => {
                    let original_size = contents.len();
                    let trace_events_count = contents.lines().count();

                    const MAX_IMMEDIATE_SIZE: usize = 2_097_152; // 2MB threshold
                    const MAX_PAYLOAD_SIZE: usize = 4_000_000; // 4MB gRPC limit (slightly under)

                    // ========== CASE 1: SMALL TRACE (≤ 2MB) ==========
                    // Send entire trace immediately, no async task
                    if original_size <= MAX_IMMEDIATE_SIZE {
                        let mut metadata = std::collections::HashMap::new();
                        metadata.insert(
                            "trace_file".to_string(),
                            trace_events_file.to_string_lossy().to_string(),
                        );
                        metadata.insert("event_count".to_string(), trace_events_count.to_string());
                        metadata
                            .insert("original_size_bytes".to_string(), original_size.to_string());

                        // Wrap payload in JSON so controller can parse it
                        let payload = if original_size <= MAX_PAYLOAD_SIZE {
                            metadata.insert("compression".to_string(), "none".to_string());
                            // Wrap in JSON: {"content": "trace content here"}
                            serde_json::json!({"content": contents})
                                .to_string()
                                .into_bytes()
                        } else {
                            metadata
                                .insert("compression".to_string(), "truncated_tail".to_string());

                            let max_tail_bytes = MAX_PAYLOAD_SIZE.saturating_sub(1);

                            // Keep only the tail, but ensure we cut at a UTF-8 boundary.
                            let tail = if contents.len() <= max_tail_bytes {
                                contents.clone()
                            } else {
                                let start = contents.len() - max_tail_bytes;

                                // Move start forward until it's a valid char boundary
                                let mut s = start;
                                while s < contents.len() && !contents.is_char_boundary(s) {
                                    s += 1;
                                }
                                contents[s..].to_string()
                            };

                            serde_json::json!({ "content": tail })
                                .to_string()
                                .into_bytes()
                        };

                        let final_size = payload.len();
                        metadata.insert("final_size_bytes".to_string(), final_size.to_string());

                        let telemetry_data = crate::automutate::common::TelemetryData {
                            job_id: job_id.clone(),
                            event_type: "trace_log".to_string(),
                            timestamp: chrono::Utc::now().timestamp(),
                            payload,
                            metadata,
                            typed_event: None,
                        };
                        telemetry_events.push(telemetry_data);

                        info!(
                            "[OK] Collected trace log as single event ({} line traces, {} -> {} bytes)",
                            trace_events_count, original_size, final_size
                        );

                    // ========== CASE 2: LARGE TRACE (> 2MB) ==========
                    } else {
                        // send last 2MB immediately in main batch
                        // Extract complete JSON lines only (JSONL format - one JSON object per line)
                        let byte_offset = original_size.saturating_sub(MAX_IMMEDIATE_SIZE);
                        let mut last_2mb = contents[byte_offset..].to_string();

                        // Remove incomplete first line (likely cut in the middle)
                        if let Some(first_newline) = last_2mb.find('\n') {
                            last_2mb = last_2mb[first_newline + 1..].to_string();
                        }

                        let immediate_line_count = last_2mb.lines().count();

                        let mut immediate_metadata = std::collections::HashMap::new();
                        immediate_metadata.insert(
                            "trace_file".to_string(),
                            trace_events_file.to_string_lossy().to_string(),
                        );
                        immediate_metadata
                            .insert("event_count".to_string(), trace_events_count.to_string());
                        immediate_metadata
                            .insert("original_size_bytes".to_string(), original_size.to_string());
                        immediate_metadata.insert("compression".to_string(), "none".to_string());
                        immediate_metadata.insert(
                            "payload_type".to_string(),
                            "last_complete_jsonl".to_string(),
                        );
                        immediate_metadata.insert(
                            "immediate_line_count".to_string(),
                            immediate_line_count.to_string(),
                        );
                        immediate_metadata.insert(
                            "note".to_string(),
                            format!("Last {} complete JSON lines (~2MB) sent immediately; full compressed trace will follow", immediate_line_count),
                        );

                        // Wrap last 2MB in JSON
                        let immediate_payload = serde_json::json!({"content": last_2mb})
                            .to_string()
                            .into_bytes();
                        immediate_metadata.insert(
                            "final_size_bytes".to_string(),
                            immediate_payload.len().to_string(),
                        );

                        let immediate_telemetry = crate::automutate::common::TelemetryData {
                            job_id: job_id.clone(),
                            event_type: "trace_log".to_string(),
                            timestamp: chrono::Utc::now().timestamp(),
                            payload: immediate_payload,
                            metadata: immediate_metadata,
                            typed_event: None,
                        };
                        telemetry_events.push(immediate_telemetry);

                        info!(
                            "[OK] Collected last {} complete JSON lines (~2MB) of trace log ({} total line traces, {} bytes total)",
                            immediate_line_count, trace_events_count, original_size
                        );

                        //async compression of full trace
                        let job_id_clone = job_id.clone();
                        let trace_file_clone = trace_events_file.clone();
                        let contents_clone = contents.clone();

                        info!(
                            "Spawning async compression task for full trace ({} bytes)...",
                            original_size
                        );

                        tokio::spawn(async move {
                            info!(
                                "Async compression task started for trace: {:?}",
                                trace_file_clone
                            );

                            let mut metadata = std::collections::HashMap::new();
                            metadata.insert(
                                "trace_file".to_string(),
                                trace_file_clone.to_string_lossy().to_string(),
                            );
                            metadata
                                .insert("event_count".to_string(), trace_events_count.to_string());
                            metadata.insert(
                                "original_size_bytes".to_string(),
                                original_size.to_string(),
                            );

                            let payload = if original_size <= MAX_PAYLOAD_SIZE {
                                metadata.insert("compression".to_string(), "none".to_string());
                                // Wrap full content in JSON
                                serde_json::json!({"content": contents_clone.clone()})
                                    .to_string()
                                    .into_bytes()
                            } else {
                                info!(
                                    "Trace log too large ({} bytes), applying CLP+MatrixProfile+Sequitur compression...",
                                    original_size
                                );
                                let compressed = telemetry::trace_compressor::compress_trace_log(
                                    &contents_clone,
                                    3,
                                );

                                metadata.insert(
                                    "compression_ratio".to_string(),
                                    format!("{:.2}x", compressed.compression_ratio),
                                );
                                metadata.insert(
                                    "compression_stats".to_string(),
                                    serde_json::to_string(&compressed.statistics)
                                        .unwrap_or_default(),
                                );

                                if compressed.compressed_size <= MAX_PAYLOAD_SIZE {
                                    info!(
                                        "Loop compression successful: {} -> {} bytes ({:.1}x)",
                                        original_size,
                                        compressed.compressed_size,
                                        compressed.compression_ratio
                                    );
                                    metadata.insert(
                                        "compression".to_string(),
                                        "loop_detection".to_string(),
                                    );
                                    // Wrap compressed content in JSON
                                    serde_json::json!({"content": compressed.content})
                                        .to_string()
                                        .into_bytes()
                                } else {
                                    info!(
                                        "Still too large after loop compression, applying gzip..."
                                    );
                                    match telemetry::trace_compressor::gzip_compress(
                                        compressed.content.as_bytes(),
                                    ) {
                                        Ok(gzipped) if gzipped.len() <= MAX_PAYLOAD_SIZE => {
                                            info!(
                                                "Gzip compression successful: {} -> {} bytes",
                                                compressed.compressed_size,
                                                gzipped.len()
                                            );
                                            metadata.insert(
                                                "compression".to_string(),
                                                "loop+gzip".to_string(),
                                            );
                                            // Store gzipped as base64 in JSON
                                            use base64::{Engine as _, engine::general_purpose};
                                            let gzipped_b64 =
                                                general_purpose::STANDARD.encode(&gzipped);
                                            serde_json::json!({"content_b64": gzipped_b64, "encoding": "gzip+base64"}).to_string().into_bytes()
                                        }
                                        Ok(gzipped) => {
                                            warn!(
                                                "Trace log too large even after compression ({} bytes), sending metadata only",
                                                gzipped.len()
                                            );
                                            metadata.insert(
                                                "compression".to_string(),
                                                "truncated".to_string(),
                                            );
                                            metadata.insert(
                                                "note".to_string(),
                                                "Log too large for gRPC transmission, file stored on worker"
                                                    .to_string(),
                                            );

                                            let lines: Vec<&str> = contents_clone.lines().collect();
                                            let sample = if lines.len() > 200 {
                                                format!(
                                                    "=== First 100 lines ===\n{}\n=== Last 100 lines ===\n{}",
                                                    lines[..100].join("\n"),
                                                    lines[lines.len() - 100..].join("\n")
                                                )
                                            } else {
                                                contents_clone.clone()
                                            };
                                            // Wrap truncated sample in JSON
                                            serde_json::json!({"content": sample})
                                                .to_string()
                                                .into_bytes()
                                        }
                                        Err(e) => {
                                            error!("Gzip compression failed: {}", e);
                                            metadata.insert(
                                                "compression".to_string(),
                                                "error".to_string(),
                                            );
                                            serde_json::json!({"error": "compression_failed"})
                                                .to_string()
                                                .into_bytes()
                                        }
                                    }
                                }
                            };

                            let final_size = payload.len();
                            metadata.insert("final_size_bytes".to_string(), final_size.to_string());

                            let reduced_telemetry = crate::automutate::common::TelemetryData {
                                job_id: job_id_clone.clone(),
                                event_type: "trace_log_reduced".to_string(),
                                timestamp: chrono::Utc::now().timestamp(),
                                payload,
                                metadata,
                                typed_event: None,
                            };

                            info!(
                                "Reduced trace prepared ({} -> {} bytes) - available for controller pull",
                                original_size, final_size
                            );
                        });
                    }
                }
                Err(e) => {
                    error!("Failed to read trace events file: {}", e);
                }
            }
        } else {
            info!("No trace events file found (artifact may not have line tracing enabled)");
        }

        // Also collect from trace.log file (fallback if pipe wasn't available)
        let trace_log_path = telemetry_dir.join("trace.log");
        if trace_log_path.exists() {
            info!(
                "Found trace.log file, collecting binary protocol events: {:?}",
                trace_log_path
            );

            // Read as binary (new binary protocol format)
            match std::fs::read(&trace_log_path) {
                Ok(trace_bytes) => {
                    let mut file_trace_count = 0;
                    let mut checkpoint_count = 0;
                    let mut success_count = 0;
                    let mut failure_count = 0;

                    // Parse binary protocol records
                    let mut offset = 0;
                    while offset + 32 <= trace_bytes.len() {
                        // Read header (32 bytes)
                        let header_bytes = &trace_bytes[offset..offset + 32];

                        // Parse header fields (little-endian)
                        let magic = u32::from_le_bytes([
                            header_bytes[0],
                            header_bytes[1],
                            header_bytes[2],
                            header_bytes[3],
                        ]);

                        // Check magic (0x49535452 = 'ISTR')
                        if magic != 0x49535452 {
                            warn!(
                                "Invalid magic in trace.log at offset {}: 0x{:08x}, stopping parse",
                                offset, magic
                            );
                            break;
                        }

                        let event_type = u16::from_le_bytes([header_bytes[6], header_bytes[7]]);
                        let payload_len = u32::from_le_bytes([
                            header_bytes[28],
                            header_bytes[29],
                            header_bytes[30],
                            header_bytes[31],
                        ]);

                        offset += 32;

                        // Read payload
                        if offset + payload_len as usize > trace_bytes.len() {
                            warn!(
                                "Incomplete payload in trace.log at offset {}, expected {} bytes",
                                offset, payload_len
                            );
                            break;
                        }

                        let payload = &trace_bytes[offset..offset + payload_len as usize];
                        offset += payload_len as usize;

                        // Handle based on event type
                        match event_type {
                            1 => {
                                // LINE_TRACE: payload is "file:line:func"
                                if let Ok(payload_str) = std::str::from_utf8(payload) {
                                    let telemetry_data = crate::automutate::common::TelemetryData {
                                        job_id: job_id.clone(),
                                        event_type: "trace_line".to_string(),
                                        timestamp: chrono::Utc::now().timestamp_millis(),
                                        payload: payload_str.as_bytes().to_vec(),
                                        metadata: std::collections::HashMap::new(),
                                        typed_event: None,
                                    };
                                    telemetry_events.push(telemetry_data);
                                    file_trace_count += 1;
                                }
                            }
                            2 => {
                                // CHECKPOINT
                                if let Ok(checkpoint_name) = std::str::from_utf8(payload) {
                                    info!("[OK] CHECKPOINT from file: '{}'", checkpoint_name);
                                    checkpoint_count += 1;
                                }
                            }
                            3 => {
                                // SUCCESS
                                if let Ok(success_msg) = std::str::from_utf8(payload) {
                                    info!("[OK] ARTIFACT SUCCESS from file: '{}'", success_msg);
                                    success_count += 1;
                                }
                            }
                            4 => {
                                // FAILURE
                                if let Ok(failure_data) = std::str::from_utf8(payload) {
                                    let parts: Vec<&str> = failure_data.splitn(2, '|').collect();
                                    let message = parts.get(0).unwrap_or(&"unknown");
                                    let error_code = parts.get(1).unwrap_or(&"0");
                                    warn!(
                                        "[ERROR] ARTIFACT FAILURE from file: '{}' (error_code={})",
                                        message, error_code
                                    );
                                    failure_count += 1;
                                }
                            }
                            _ => {
                                debug!("Unknown event_type {} in trace.log", event_type);
                            }
                        }
                    }

                    info!(
                        "[OK] Collected from trace.log: {} line traces, {} checkpoints, {} success, {} failure",
                        file_trace_count, checkpoint_count, success_count, failure_count
                    );
                }
                Err(e) => {
                    warn!("Failed to read trace.log: {}", e);
                }
            }
        }

        // Collect BB coverage from disk
        // Look in artifact-specific telemetry directory where process ran
        let coverage_bin_path = telemetry_dir.join("coverage.bin");
        let coverage_bbs_path = telemetry_dir.join("coverage_bbs.txt");

        if coverage_bin_path.exists() && coverage_bbs_path.exists() {
            info!(
                "Found BB coverage files: {:?}, {:?}",
                coverage_bin_path, coverage_bbs_path
            );

            match helpers::collect_bb_coverage(&coverage_bin_path, &coverage_bbs_path, &job_id).await {
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
            warn!(
                "  Artifact may not be instrumented for BB coverage, or runtime did not flush files"
            );
        }

        // Collect API checkpoints from disk
        let checkpoints_path = telemetry_dir.join("checkpoints.log");
        if checkpoints_path.exists() {
            info!("Found API checkpoints file: {:?}", checkpoints_path);

            match helpers::collect_api_checkpoints(&checkpoints_path, &job_id).await {
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
            warn!(
                "  Artifact may not be instrumented for API tracing, or runtime did not flush file"
            );
        }

        let telemetry_count = telemetry_events.len() as i32;
        info!("Total telemetry events collected: {}", telemetry_count);

        // ====================================================================
        // Send final status with telemetry count
        // ====================================================================

        let final_status_type = if timed_out {
            "timeout"
        } else if exit_code == 0 {
            "success"
        } else {
            "error"
        };

        // Build detailed status message
        let final_details = if timed_out {
            format!("Process timed out after {}s", req.timeout_seconds)
        } else if exit_code == 0 {
            format!(
                "Process completed successfully, elapsed: {:.2}s",
                elapsed.as_secs_f64()
            )
        } else {
            // Error exit code - provide detailed information
            // Windows NTSTATUS codes can be negative (signed) or positive (unsigned representation)
            let error_type = match exit_code {
                // Special internal exit codes
                -2 => "Killed by AV/EDR (external termination)",
                -1 => "Process Wait Failed",
                // Signed NTSTATUS codes (negative)
                -1073741510 => "Access Violation (0xC0000005)",
                -1073741819 => "Access Denied (0xC0000022)",
                -1073741502 => "Invalid Image Format (0xC000007B)",
                -1073741515 => "DLL Not Found (0xC0000135)",
                -1073741701 => "Ordinal Not Found (0xC0000138)",
                -1073741571 => "Stack Overflow (0xC00000FD)",
                -1073740791 => "Application Error (0xC0000409)",
                // Common positive exit codes
                1 => "Generic Error",
                _ if exit_code < 0 => {
                    // Negative exit codes are usually Windows NTSTATUS codes
                    &format!("Windows Error (NTSTATUS: 0x{:08X})", exit_code as u32)
                }
                _ => "Unknown Error",
            };

            // Include stderr output if available
            let error_msg = if !stderr_output.is_empty() {
                // Truncate stderr to 200 chars for status report
                let stderr_preview = if stderr_output.len() > 200 {
                    format!("{}...", &stderr_output[..200])
                } else {
                    stderr_output.clone()
                };
                format!(
                    "Process failed with exit code {} ({}), elapsed: {:.2}s | Error: {}",
                    exit_code,
                    error_type,
                    elapsed.as_secs_f64(),
                    stderr_preview
                )
            } else {
                format!(
                    "Process failed with exit code {} ({}), elapsed: {:.2}s",
                    exit_code,
                    error_type,
                    elapsed.as_secs_f64()
                )
            };

            error_msg
        };

        // Log with appropriate level
        if timed_out {
            warn!("TIMEOUT: {} - {}", artifact_name, final_details);
        } else if exit_code == 0 {
            info!("SUCCESS: {} - {}", artifact_name, final_details);
        } else {
            warn!("ERROR: {} - {}", artifact_name, final_details);
        }

        info!(
            "Execution completed: {} telemetry events collected",
            telemetry_count
        );

        // Stream telemetry to controller if bidirectional stream is available
        if let Some(ref handler) = stream_handler {
            if !telemetry_events.is_empty() {
                info!(
                    "Streaming {} telemetry events to controller via bidirectional stream",
                    telemetry_count
                );

                debug!("[WORKER-TELEMETRY-SEND] run_id='{}' job_id='{}' events={}",
                    run_id, job_id, telemetry_count);

                // Create TelemetryBatch message
                let telemetry_batch = crate::automutate::common::TelemetryBatch {
                    job_id: job_id.clone(),
                    run_id: run_id.clone(),
                    events: telemetry_events.clone(),
                    is_final: true, // Final batch for this execution
                };

                // Stream to controller
                match handler.send_telemetry(telemetry_batch).await {
                    Ok(_) => {
                        info!("Successfully streamed {} telemetry events to controller", telemetry_count);
                    }
                    Err(e) => {
                        error!("Failed to stream telemetry to controller: {}", e);
                        warn!("Telemetry will be lost - controller should handle stream failures gracefully");
                        // Continue execution - telemetry failure shouldn't fail the job
                    }
                }
            } else {
                warn!(
                    "No telemetry events collected [job: {}, run: {}]",
                    job_id, run_id
                );
            }
        } else {
            // Legacy mode: no stream available (shouldn't happen in Phase 2)
            warn!(
                "No bidirectional stream available - telemetry NOT sent to controller [job: {}, run: {}]",
                job_id, run_id
            );
            if !telemetry_events.is_empty() {
                warn!(
                    "{} telemetry events collected but cannot be sent (stream unavailable)",
                    telemetry_count
                );
            }
        }

        // Reset RedEDR for next run (guard ensures cleanup on error, but we do it explicitly here)
        if let Err(e) = rededr_guard.reset_now().await {
            error!(
                "Failed to reset RedEDR: {} - Next execution may have contaminated events!",
                e
            );
            // Error is logged but we don't fail the RPC
        }

        // 10. Prepare output (include stderr if error occurred)
        let output = if timed_out {
            format!("Execution timed out after {}s", req.timeout_seconds)
        } else if exit_code == 0 {
            // Success - include stdout if available
            if !stdout_output.is_empty() {
                format!(
                    "Execution completed in {:.2}s\nOutput:\n{}",
                    elapsed.as_secs_f64(),
                    stdout_output
                )
            } else {
                format!("Execution completed in {:.2}s", elapsed.as_secs_f64())
            }
        } else {
            // Error
            let mut error_output = format!("Execution failed with exit code {}\n", exit_code);

            if !stderr_output.is_empty() {
                error_output.push_str(&format!("\nStderr:\n{}", stderr_output));
            }

            if !stdout_output.is_empty() {
                error_output.push_str(&format!("\nStdout:\n{}", stdout_output));
            }

            error_output
        };

        //TODO needed for lock
        //collector.release_lock().await.expect("Error");

        debug!("[WORKER-RESPONSE] Creating SampleResponse with run_id='{}' (will be overwritten by stream_handler)", run_id);

        // 11. Return response
        Ok(Response::new(SampleResponse {
            job_id,
            success: !timed_out && exit_code == 0,
            exit_code,
            output,
            telemetry_ids: vec![run_id.clone()],
            run_id: String::new(), // Empty for legacy RPC (will be populated by stream handler if called via stream)
        }))
    }
