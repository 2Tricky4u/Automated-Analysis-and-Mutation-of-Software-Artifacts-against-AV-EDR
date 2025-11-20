use edr_config::WorkerConfig;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::Mutex;
use tonic::{Request, Response, Status, transport::Server};
use tracing::{error, info, warn};

mod execution;
mod telemetry;

pub mod edr {
    pub mod common {
        tonic::include_proto!("edr.common");
    }
    pub mod controller {
        tonic::include_proto!("edr.controller");
    }
    pub mod worker {
        tonic::include_proto!("edr.worker");
    }
}

use edr::worker::{
    ArtifactChunk, HealthRequest, HealthResponse, PingRequest, PingResponse, SampleRequest,
    SampleResponse, TransferAck,
    worker_agent_server::{WorkerAgent, WorkerAgentServer},
};

// ============================================================================
// RAII Guards for Resource Cleanup
// ============================================================================

/// RAII guard that ensures RedEDR is reset on drop
/// This guarantees cleanup on all exit paths (success, error, panic)
struct RedEdrGuard {
    collector: telemetry::collectors::rededr::RedEdrCollector,
    reset_on_drop: bool,
}

impl RedEdrGuard {
    fn new(collector: telemetry::collectors::rededr::RedEdrCollector) -> Self {
        Self {
            collector,
            reset_on_drop: true,
        }
    }

    /// Get reference to collector for operations
    fn collector(&self) -> &telemetry::collectors::rededr::RedEdrCollector {
        &self.collector
    }

    /// Manually reset and prevent Drop cleanup (for normal exit path)
    async fn reset_now(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.reset_on_drop = false; // Prevent double reset in Drop
        self.collector.reset().await
    }
}

impl Drop for RedEdrGuard {
    fn drop(&mut self) {
        if self.reset_on_drop {
            // Best-effort cleanup on error path
            // Spawn blocking task since Drop can't be async
            let base_url = self.collector.config().base_url.clone();
            std::thread::spawn(move || {
                // Create minimal runtime for cleanup
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let client = reqwest::Client::builder()
                        .timeout(Duration::from_secs(10))
                        .build()
                        .unwrap();
                    let url = format!("{}/api/trace/reset", base_url);
                    if let Err(e) = client.post(&url).send().await {
                        eprintln!("RedEDR cleanup in Drop failed: {}", e);
                    } else {
                        eprintln!("RedEDR cleanup in Drop succeeded (error path)");
                    }
                });
            });
        }
    }
}

/// RAII guard that ensures monitor is stopped on drop
struct MonitorGuard {
    stop_tx: Option<tokio::sync::watch::Sender<bool>>,
    handle: Option<tokio::task::JoinHandle<()>>,
    event_consumer: Option<tokio::task::JoinHandle<()>>,
}

impl MonitorGuard {
    fn new(
        stop_tx: tokio::sync::watch::Sender<bool>,
        handle: tokio::task::JoinHandle<()>,
        event_consumer: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            stop_tx: Some(stop_tx),
            handle: Some(handle),
            event_consumer: Some(event_consumer),
        }
    }

    /// Stop monitoring gracefully
    async fn stop(mut self) {
        // Send stop signal
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }

        // Abort event consumer FIRST to prevent monitor from blocking on channel send
        if let Some(consumer) = self.event_consumer.take() {
            consumer.abort();
        }

        // Now wait for monitor to finish (won't block on channel)
        if let Some(handle) = self.handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }
    }
}

impl Drop for MonitorGuard {
    fn drop(&mut self) {
        // Best-effort cleanup
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(consumer) = self.event_consumer.take() {
            consumer.abort();
        }
    }
}

/// RAII guard that ensures child process is killed on drop
struct ProcessGuard {
    child: Option<tokio::process::Child>,
    should_kill: bool,
}

impl ProcessGuard {
    fn new(child: tokio::process::Child) -> Self {
        Self {
            child: Some(child),
            should_kill: true,
        }
    }

    /// Get mutable reference to child for waiting
    fn child_mut(&mut self) -> &mut tokio::process::Child {
        self.child.as_mut().expect("Child already taken")
    }

    /// Take child ownership and prevent kill on drop (for normal completion)
    fn disarm(mut self) -> tokio::process::Child {
        self.should_kill = false;
        self.child.take().expect("Child already taken")
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if self.should_kill {
            if let Some(mut child) = self.child.take() {
                // Spawn blocking task to kill process
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                        if let Err(e) = child.kill().await {
                            eprintln!("Failed to kill process in Drop: {}", e);
                        } else {
                            eprintln!("Process killed in Drop (error path)");
                        }
                    });
                });
            }
        }
    }
}

// ============================================================================
// Execution Lock Guard
// ============================================================================

/// RAII guard for single execution lock
/// Automatically releases lock on drop (success, error, or panic)
struct ExecutionLockGuard {
    lock: Arc<Mutex<ExecutionState>>,
}

impl Drop for ExecutionLockGuard {
    fn drop(&mut self) {
        // Release lock by spawning task (Drop can't be async)
        let lock = self.lock.clone();
        tokio::spawn(async move {
            let mut state = lock.lock().await;
            let job_id = state.current_job_id.take().unwrap_or_else(|| "unknown".to_string());
            let artifact = state.current_artifact.take().unwrap_or_else(|| "unknown".to_string());
            state.busy = false;
            info!(
                "🔓 Execution lock RELEASED: job_id={}, artifact={}",
                job_id, artifact
            );
        });
    }
}

/// Execution state for single-job worker
#[derive(Debug, Clone)]
pub struct ExecutionState {
    pub busy: bool,
    pub current_job_id: Option<String>,
    pub current_artifact: Option<String>,
}

// ============================================================================
// Worker Agent Service
// ============================================================================

#[derive(Clone)]
pub struct WorkerAgentService {
    worker_id: String,
    config: WorkerConfig,
    system_info: Arc<Mutex<System>>,
    /// Single execution lock - only ONE job can run at a time
    /// This ensures clean telemetry collection with no cross-contamination
    execution_lock: Arc<Mutex<ExecutionState>>,
}

impl WorkerAgentService {
    pub fn new(worker_id: String, config: WorkerConfig) -> Self {
        Self {
            worker_id,
            config,
            system_info: Arc::new(Mutex::new(System::new_all())),
            execution_lock: Arc::new(Mutex::new(ExecutionState {
                busy: false,
                current_job_id: None,
                current_artifact: None,
            })),
        }
    }

    /// Try to acquire execution lock for single job execution
    /// Returns Ok(guard) if lock acquired, Err if already busy
    async fn try_acquire_execution_lock(
        &self,
        job_id: String,
        artifact_name: String,
    ) -> Result<ExecutionLockGuard, String> {
        let mut state = self.execution_lock.lock().await;

        if state.busy {
            let current_job = state.current_job_id.as_deref().unwrap_or("unknown");
            let current_artifact = state.current_artifact.as_deref().unwrap_or("unknown");
            return Err(format!(
                "Worker is busy executing job_id={} artifact={}. This worker supports only ONE concurrent execution.",
                current_job, current_artifact
            ));
        }

        // Acquire lock
        state.busy = true;
        state.current_job_id = Some(job_id.clone());
        state.current_artifact = Some(artifact_name.clone());

        info!(
            "🔒 Execution lock ACQUIRED: job_id={}, artifact={}",
            job_id, artifact_name
        );

        Ok(ExecutionLockGuard {
            lock: self.execution_lock.clone(),
        })
    }

    /// Get current execution state (for health check)
    async fn get_execution_state(&self) -> ExecutionState {
        self.execution_lock.lock().await.clone()
    }
}

#[tonic::async_trait]
impl WorkerAgent for WorkerAgentService {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        let req = request.into_inner();
        let timestamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        info!("Ping received: {}", req.message);

        Ok(Response::new(PingResponse {
            message: format!("pong: {}", req.message),
            timestamp,
            server: format!("worker-agent/{}", self.worker_id),
        }))
    }

    async fn run_sample(
        &self,
        request: Request<SampleRequest>,
    ) -> Result<Response<SampleResponse>, Status> {
        let req = request.into_inner();
        let run_id = uuid::Uuid::new_v4().to_string();
        let job_id = req.job_id.clone();
        let artifact_name = format!("{}.exe", req.artifact_id);

        info!(
            "Received sample execution request: job_id={}, artifact_id={}",
            job_id, req.artifact_id
        );

        // ====================================================================
        // Phase 0: Acquire single execution lock (FAIL FAST if busy)
        // ====================================================================

        let _execution_lock = self
            .try_acquire_execution_lock(job_id.clone(), artifact_name.clone())
            .await
            .map_err(|msg| {
                warn!("❌ REJECTED: {}", msg);
                Status::resource_exhausted(msg)
            })?;

        info!(
            "✅ Starting sample execution: job_id={}, artifact_id={}",
            job_id, req.artifact_id
        );

        // Check if RedEDR is enabled
        if !self.config.telemetry.rededr.enabled {
            return Err(Status::failed_precondition(
                "RedEDR telemetry is disabled in config",
            ));
        }

        // ====================================================================
        // Phase 1: Setup with RAII guards for automatic cleanup
        // ====================================================================

        // 1. Resolve artifact_id to local path
        let artifact_path =
            std::path::Path::new("C:\\temp\\artifacts").join(format!("{}.exe", req.artifact_id));

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
                base_url: self.config.telemetry.rededr.base_url.clone(),
                flush_interval_ms: 1000,
                job_id: job_id.clone(),
                run_id: run_id.clone(),
            },
        );
        let rededr_guard = RedEdrGuard::new(collector);

        // 4. Sanity check: RedEDR should be clean (no leftover events from previous run)
        info!("Performing pre-run sanity check: RedEDR should be empty");
        let pre_run_events = rededr_guard
            .collector()
            .collect_all("sanity-check")
            .await
            .map_err(|e| Status::internal(format!("Failed to check RedEDR state: {}", e)))?;

        let leftover_count = pre_run_events.len();

        // Tolerate single initialization event (RedEdr startup noise)
        // More than 1 event = real contamination from previous run
        let has_real_contamination = leftover_count > 1;

        if leftover_count == 1 {
            info!(
                "✓ Sanity check: Found 1 event (likely initialization noise), ignoring and continuing"
            );
            // Don't send to controller, don't reset - just ignore and proceed
        } else if has_real_contamination {
            warn!(
                "⚠️  SANITY CHECK FAILED: Found {} leftover events in RedEDR before starting new run!",
                leftover_count
            );
            warn!("This indicates the previous run did not reset properly.");
            warn!(
                "Sending contaminated events to controller with metadata: job_id=contaminated, artifact_id=unknown"
            );

            // Send contaminated events with special metadata
            let contaminated_events: Vec<edr::common::TelemetryData> = pre_run_events
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

            // Send to controller (best effort with short timeout - don't block execution)
            match tokio::time::timeout(
                Duration::from_secs(2),
                self.send_telemetry_batch_to_controller(contaminated_events)
            ).await {
                Ok(Ok(())) => {
                    info!(
                        "Sent {} contaminated events to controller for debugging",
                        leftover_count
                    );
                }
                Ok(Err(e)) => {
                    warn!("Failed to send contaminated events to controller: {}", e);
                    warn!("Continuing with execution anyway (contamination handling is best-effort)");
                }
                Err(_) => {
                    warn!("Timeout sending contaminated events to controller (2s limit exceeded)");
                    warn!("Continuing with execution anyway (contamination handling is best-effort)");
                }
            }

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
            warn!("Force-resetting RedEDR to clear contaminated state (trace target already set)...");
            if let Err(e) = rededr_guard.collector().reset().await {
                error!("Failed to force-reset RedEDR: {}", e);
                return Err(Status::internal(format!(
                    "RedEDR is contaminated and reset failed: {}",
                    e
                )));
            }
            info!("RedEDR force-reset completed. Now watching: {}", artifact_name);
        } else {
            info!("✓ Pre-run sanity check passed: RedEDR is clean");
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

        // 4. Spawn process with guard (guard ensures kill if error occurs)
        let child = tokio::process::Command::new(&artifact_path)
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
        // Phase 2: Capture output streams
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
        // Phase 3: Start monitoring with guard
        // ====================================================================

        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

        let monitor = execution::monitor::ExecutionMonitor::new(
            run_id.clone(),
            job_id.clone(),
            self.worker_id.clone(),
            self.config.worker.ip_address.clone(),
            artifact_name.clone(),
            pid,
            self.config.telemetry.rededr.base_url.clone(),
            self.config.controller.controller_address.clone(),
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
        // Phase 4: Wait for process completion or timeout
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

                // Stop monitor immediately - process likely crashed/killed
                if let Some(guard) = monitor_guard.take() {
                    guard.stop().await;
                }

                (-1, false) // -1 = wait() failed
            }
            Err(_) => {
                // Timeout - kill process forcefully
                let pid = process_guard.child_mut().id();

                // On Windows, use taskkill /F /T to kill process tree forcefully
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

                // Also try Tokio's kill as backup
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

                (-1, true)
            }
        };

        // Disarm process guard since we've handled completion/timeout
        let _ = process_guard.disarm();

        let elapsed = start_time.elapsed();

        // ====================================================================
        // Phase 5: Collect output and cleanup monitoring
        // ====================================================================

        // Collect stdout and stderr
        let stdout_output = stdout_handle.await.unwrap_or_default();
        let stderr_output = stderr_handle.await.unwrap_or_default();

        // Log captured output (show beginning and end, truncate middle if too long)
        if !stdout_output.is_empty() {
            let formatted = if stdout_output.len() > 1000 {
                // Show first 400 chars and last 400 chars, truncate middle
                let first_part = &stdout_output[..400];
                let last_part = &stdout_output[stdout_output.len() - 400..];
                format!(
                    "{}\n\n... ({} bytes truncated) ...\n\n{}",
                    first_part,
                    stdout_output.len() - 800,
                    last_part
                )
            } else {
                stdout_output.clone()
            };
            info!("Process stdout:\n{}", formatted);
        }

        if !stderr_output.is_empty() {
            let formatted = if stderr_output.len() > 1000 {
                // Show first 400 chars and last 400 chars, truncate middle
                let first_part = &stderr_output[..400];
                let last_part = &stderr_output[stderr_output.len() - 400..];
                format!(
                    "{}\n\n... ({} bytes truncated) ...\n\n{}",
                    first_part,
                    stderr_output.len() - 800,
                    last_part
                )
            } else {
                stderr_output.clone()
            };
            warn!("Process stderr:\n{}", formatted);
        }

        // ====================================================================
        // Phase 6: Stop monitor and post-exit telemetry window (10 seconds)
        // ====================================================================
        // Stop monitor BEFORE telemetry window to prevent duplicate status reports
        // (Monitor would send "terminated" event while main execution sends final status)
        if let Some(guard) = monitor_guard.take() {
            guard.stop().await;
        }

        // Continue collecting telemetry for 10 seconds after process exit
        // This captures any late-arriving events (kernel buffer flush, EDR alerts, etc.)
        info!("Process exited. Waiting 10 seconds for late telemetry events...");
        tokio::time::sleep(Duration::from_secs(10)).await;
        info!("Telemetry collection window closed.");

        // ====================================================================
        // Phase 7: Collect telemetry and reset RedEDR (BEFORE final status)
        // ====================================================================

        // Collect full telemetry batch (best effort - don't fail job if collection fails)
        info!("Collecting telemetry events from RedEDR...");
        let telemetry_events = match rededr_guard
            .collector()
            .collect_all(&job_id)
            .await
        {
            Ok(events) => events,
            Err(e) => {
                error!("Failed to collect telemetry: {}", e);
                error!("Continuing with empty telemetry - execution status will still be reported");
                Vec::new() // Continue with empty telemetry instead of failing entire job
            }
        };

        let telemetry_count = telemetry_events.len() as i32;
        info!("Collected {} telemetry events", telemetry_count);

        // ====================================================================
        // Phase 7: Send final status with telemetry count
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
            warn!("⏱️  TIMEOUT: {} - {}", artifact_name, final_details);
        } else if exit_code == 0 {
            info!("✅ SUCCESS: {} - {}", artifact_name, final_details);
        } else {
            warn!("❌ ERROR: {} - {}", artifact_name, final_details);
        }

        // Send final status to controller with telemetry count (with timeout)
        match tokio::time::timeout(
            Duration::from_secs(2),
            self.send_final_status_to_controller(
                &job_id,
                &run_id,
                &artifact_name,
                pid,
                final_status_type,
                exit_code,
                &final_details,
                elapsed.as_secs() as i32,
                telemetry_count,
            )
        ).await {
            Ok(()) => { /* status sent successfully or logged error */ }
            Err(_) => {
                warn!("Timeout sending final status to controller (2s limit exceeded)");
                warn!("Controller may be slow or unreachable");
            }
        }

        // Send telemetry to controller (best effort with timeout - don't block on this)
        if !telemetry_events.is_empty() {
            match tokio::time::timeout(
                Duration::from_secs(3),
                self.send_telemetry_batch_to_controller(telemetry_events)
            ).await {
                Ok(Ok(())) => {
                    info!("✅ Successfully sent {} telemetry events to controller", telemetry_count);
                }
                Ok(Err(e)) => {
                    warn!("Failed to send telemetry to controller: {}", e);
                    warn!("Telemetry collected ({} events) but not sent - controller may be unavailable", telemetry_count);
                    // Don't fail the RPC - execution completed successfully
                }
                Err(_) => {
                    warn!("Timeout sending {} telemetry events to controller (3s limit exceeded)", telemetry_count);
                    warn!("Controller may not implement StreamTelemetry RPC or is unreachable");
                    // Don't fail the RPC - execution completed successfully
                }
            }
        }

        // Reset RedEDR for next run (guard ensures cleanup on error, but we do it explicitly here)
        if let Err(e) = rededr_guard.reset_now().await {
            error!(
                "Failed to reset RedEDR: {} - Next execution may have contaminated events!",
                e
            );
            // Error is logged but we don't fail the RPC - telemetry was already collected
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
            // Error - include both stdout and stderr
            let mut error_output = format!("Execution failed with exit code {}\n", exit_code);

            if !stderr_output.is_empty() {
                error_output.push_str(&format!("\nStderr:\n{}", stderr_output));
            }

            if !stdout_output.is_empty() {
                error_output.push_str(&format!("\nStdout:\n{}", stdout_output));
            }

            error_output
        };

        // 11. Return response
        Ok(Response::new(SampleResponse {
            job_id,
            success: !timed_out && exit_code == 0,
            exit_code,
            output,
            telemetry_ids: vec![run_id],
        }))
    }

    async fn health_check(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let _req = request.into_inner();

        // Refresh system information
        let mut sys = self.system_info.lock().await;
        sys.refresh_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );

        // Calculate average CPU usage across all cores
        let cpu_percent = sys.global_cpu_info().cpu_usage() as i32;

        // Calculate memory usage percentage
        let total_memory = sys.total_memory();
        let used_memory = sys.used_memory();
        let memory_percent = if total_memory > 0 {
            ((used_memory as f64 / total_memory as f64) * 100.0) as i32
        } else {
            0
        };

        // Get execution state (busy/idle)
        let exec_state = self.get_execution_state().await;
        let active_jobs = if exec_state.busy { 1 } else { 0 };

        // Determine health status
        // Unhealthy if: CPU > 95% OR memory > 95%
        let healthy = cpu_percent < 95 && memory_percent < 95;

        // Log health check with execution state
        if exec_state.busy {
            info!(
                "Health check: cpu={}%, mem={}%, status=BUSY (job_id={}, artifact={})",
                cpu_percent,
                memory_percent,
                exec_state.current_job_id.as_deref().unwrap_or("unknown"),
                exec_state.current_artifact.as_deref().unwrap_or("unknown")
            );
        } else if !healthy {
            warn!(
                "Health check UNHEALTHY: cpu={}%, mem={}%, status=IDLE",
                cpu_percent, memory_percent
            );
        }

        Ok(Response::new(HealthResponse {
            worker_id: self.worker_id.clone(),
            healthy,
            cpu_percent,
            memory_percent,
            active_jobs,
        }))
    }

    async fn send_artifact(
        &self,
        request: Request<tonic::Streaming<ArtifactChunk>>,
    ) -> Result<Response<TransferAck>, Status> {
        use sha2::{Digest, Sha256};

        let mut stream = request.into_inner();
        let mut chunks = Vec::new();
        let mut artifact_id = String::new();
        let mut expected_sha256 = String::new();

        info!("Starting artifact transfer...");

        // Receive all chunks
        while let Some(chunk) = stream.message().await? {
            if artifact_id.is_empty() {
                artifact_id = chunk.artifact_id.clone();
                expected_sha256 = chunk.sha256.clone();
                info!(
                    "Receiving artifact: id={}, total_chunks={}",
                    artifact_id, chunk.total_chunks
                );
            }
            chunks.push(chunk);
        }

        if chunks.is_empty() {
            return Err(Status::invalid_argument("No chunks received"));
        }

        // Sort chunks by index
        chunks.sort_by_key(|c| c.chunk_index);

        // Reassemble binary
        let file_data: Vec<u8> = chunks.iter().flat_map(|c| c.data.clone()).collect();

        info!(
            "Reassembled artifact: {} bytes from {} chunks",
            file_data.len(),
            chunks.len()
        );

        // Verify integrity
        let mut hasher = Sha256::new();
        hasher.update(&file_data);
        let actual_sha256 = format!("{:x}", hasher.finalize());

        if actual_sha256 != expected_sha256 {
            return Err(Status::data_loss(format!(
                "SHA256 mismatch: expected {}, got {}",
                expected_sha256, actual_sha256
            )));
        }

        // Write to disk (artifacts directory)
        let artifacts_dir = std::path::Path::new("C:\\temp\\artifacts");
        std::fs::create_dir_all(artifacts_dir).map_err(|e| {
            Status::internal(format!("Failed to create artifacts directory: {}", e))
        })?;

        let artifact_path = artifacts_dir.join(format!("{}.exe", artifact_id));
        std::fs::write(&artifact_path, &file_data)
            .map_err(|e| Status::internal(format!("Failed to write artifact to disk: {}", e)))?;

        info!(
            "Artifact stored: {} ({} bytes) at {:?}",
            artifact_id,
            file_data.len(),
            artifact_path
        );

        Ok(Response::new(TransferAck {
            received: true,
            chunks_received: chunks.len() as u32,
            error: String::new(),
            storage_path: artifact_path.to_string_lossy().to_string(),
        }))
    }
}

// Helper methods for WorkerAgentService
impl WorkerAgentService {
    /// Send telemetry batch to controller via StreamTelemetry RPC
    async fn send_telemetry_batch_to_controller(
        &self,
        telemetry_events: Vec<edr::common::TelemetryData>,
    ) -> Result<(), Status> {
        use edr::controller::controller_client::ControllerClient;
        use futures::stream;

        info!(
            "Sending {} telemetry events to controller...",
            telemetry_events.len()
        );

        // Ensure controller address has http:// scheme
        let controller_addr = if self
            .config
            .controller
            .controller_address
            .starts_with("http://")
            || self
                .config
                .controller
                .controller_address
                .starts_with("https://")
        {
            self.config.controller.controller_address.clone()
        } else {
            format!("http://{}", self.config.controller.controller_address)
        };

        // Connect to controller
        let mut client = ControllerClient::connect(controller_addr.clone())
            .await
            .map_err(|e| {
                error!(
                    "Failed to connect to controller at {}: {}",
                    controller_addr, e
                );
                Status::unavailable(format!("Controller unavailable: {}", e))
            })?;

        // Create stream from telemetry events
        let stream = stream::iter(telemetry_events);

        // Send batch via streaming RPC
        let response = client.stream_telemetry(stream).await.map_err(|e| {
            error!("Failed to stream telemetry to controller: {}", e);
            Status::internal(format!("Telemetry streaming failed: {}", e))
        })?;

        let ack = response.into_inner();
        info!(
            "Telemetry sent successfully: {} events acknowledged",
            ack.events_count
        );

        Ok(())
    }

    /// Send final status report to controller with exit code information
    async fn send_final_status_to_controller(
        &self,
        job_id: &str,
        run_id: &str,
        artifact_name: &str,
        pid: u32,
        status_type: &str,
        _exit_code: i32,
        details: &str,
        elapsed_seconds: i32,
        telemetry_events_count: i32,
    ) {
        use edr::controller::{StatusReport, controller_client::ControllerClient};

        // Ensure controller address has http:// scheme
        let controller_addr = if self
            .config
            .controller
            .controller_address
            .starts_with("http://")
            || self
                .config
                .controller
                .controller_address
                .starts_with("https://")
        {
            self.config.controller.controller_address.clone()
        } else {
            format!("http://{}", self.config.controller.controller_address)
        };

        let status_report = StatusReport {
            worker_id: self.worker_id.clone(),
            worker_ip: self.config.worker.ip_address.clone(),
            job_id: job_id.to_string(),
            run_id: run_id.to_string(),
            artifact_name: artifact_name.to_string(),
            pid: pid as i32,
            elapsed_seconds,
            process_alive: false,
            telemetry_events_count,
            event_type: status_type.to_string(),
            cpu_percent: 0,
            memory_mb: 0,
            details: details.to_string(),
        };

        // Try to connect and send final status report
        match ControllerClient::connect(controller_addr.clone()).await {
            Ok(mut client) => {
                if let Err(e) = client.report_status(Request::new(status_report)).await {
                    warn!("Failed to send final status to controller: {}", e);
                }
            }
            Err(e) => {
                warn!(
                    "Failed to connect to controller at {}: {}",
                    controller_addr, e
                );
            }
        }
    }
}

// NOTE: Streaming telemetry removed - now using batch collection at execution completion
// Telemetry is collected once after artifact execution and returned with RunResult

// NOTE: JobGuard removed - replaced with ExecutionLockGuard for single execution enforcement
// The ExecutionLockGuard ensures only ONE job can run at a time, preventing telemetry cross-contamination

/// Extract filename from path (cross-platform)
fn extract_filename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Load generated TOML config (auto-finds in standard locations)
    // Search order:
    //   1. AUTOMUTATE_WORKER_CONFIG env var (highest priority)
    //   2. C:\AutoMutate\worker.toml (VM deployment standard location)
    //   3. Auto-detect by hostname (e.g., automation/generated/win10-worker-01.toml)
    //   4. config/worker.toml (local development)
    //   5. automation/generated/win10-worker-01.toml (fallback)
    let config = WorkerConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load worker config: {}", e);
        eprintln!("");
        eprintln!(
            "Hostname: {}",
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN".to_string())
        );
        eprintln!("");
        eprintln!("Config search order:");
        eprintln!("  1. AUTOMUTATE_WORKER_CONFIG env var");
        eprintln!("  2. C:\\AutoMutate\\worker.toml");
        eprintln!("  3. automation/generated/<hostname>.toml (auto-detect)");
        eprintln!("  4. config/worker.toml");
        eprintln!("  5. automation/generated/win10-worker-01.toml");
        eprintln!("");
        eprintln!("Solutions:");
        eprintln!("  - Run: .\\automation\\scripts\\generate-configs.ps1");
        eprintln!("  - Deploy: Copy <hostname>.toml to C:\\AutoMutate\\worker.toml");
        eprintln!(
            "  - Or set: $env:AUTOMUTATE_WORKER_CONFIG=\"automation\\generated\\<hostname>.toml\""
        );
        std::process::exit(1);
    });

    let worker_id = config.worker.worker_id.clone();

    // Ensure controller address has http:// scheme for tonic
    let controller_addr = {
        let addr = config.controller.controller_address.clone();
        if addr.starts_with("http://") || addr.starts_with("https://") {
            addr
        } else {
            format!("http://{}", addr)
        }
    };

    // Worker listen port can be overridden via env var
    let worker_port = std::env::var("WORKER_PORT")
        .unwrap_or_else(|_| "50052".to_string())
        .parse::<u16>()
        .unwrap_or(50052);
    let addr = format!("0.0.0.0:{}", worker_port).parse()?;

    info!(
        "Worker configuration loaded successfully at {}",
        WorkerConfig::find_config_path()
    );
    info!("Worker ID: {}", worker_id);
    info!("Worker IP: {}", config.worker.ip_address);
    info!("OS Version: {}", config.worker.os_version);
    info!("Controller: {}", controller_addr);
    info!("Sandbox enabled: {}", config.harness.sandbox_enabled);
    info!("ETW enabled: {}", config.telemetry.etw.enabled);

    let agent = WorkerAgentService::new(worker_id.clone(), config.clone());

    info!("Worker Agent {} starting on {}", worker_id, addr);
    info!("Telemetry mode: Batch collection (no streaming)");

    if config.telemetry.rededr.enabled {
        info!(
            "RedEDR collector available: {}",
            config.telemetry.rededr.base_url
        );
        info!("RedEDR telemetry will be collected after each execution completes");
    } else {
        info!("RedEDR collector disabled in config");
    }

    // gRPC reflection for grpcurl
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(tonic::include_file_descriptor_set!("edr_descriptor"))
        .build_v1()?;

    Server::builder()
        .add_service(WorkerAgentServer::new(agent))
        .add_service(reflection_service)
        .serve(addr)
        .await?;

    Ok(())
}
