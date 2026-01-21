/// Execution Monitor - Lightweight Status Tracking
///
/// Monitors artifact execution without streaming full telemetry.
/// Polls RedEDR /api/stats every 2-5 seconds to track:
/// - Event count (detect stuck: no growth)
/// - Process status (alive/dead)
/// - Resource usage (CPU, memory)
///
/// Sends status updates via StreamHandler if available (new architecture),
/// otherwise skips remote reporting (worker-only mode).
use crate::automutate::worker::{ExecutionStatus, MonitorEvent};
use crate::stream_handler::StreamHandler;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info, warn};

pub struct ExecutionMonitor {
    pub run_id: String,
    pub job_id: String,
    pub worker_id: String,
    pub worker_ip: String,
    pub artifact_name: String,
    pub pid: u32,
    pub rededr_base_url: String,
    pub stream_handler: Option<Arc<StreamHandler>>,
    pub start_time: Instant,
    pub timeout_seconds: i32,
    client: reqwest::Client,
    sys: Arc<tokio::sync::Mutex<sysinfo::System>>,
}

impl ExecutionMonitor {
    pub fn new(
        run_id: String,
        job_id: String,
        worker_id: String,
        worker_ip: String,
        artifact_name: String,
        pid: u32,
        rededr_base_url: String,
        stream_handler: Option<Arc<StreamHandler>>,
        timeout_seconds: i32,
    ) -> Self {
        use sysinfo::System;

        Self {
            run_id,
            job_id,
            worker_id,
            worker_ip,
            artifact_name,
            pid,
            rededr_base_url,
            stream_handler,
            start_time: Instant::now(),
            timeout_seconds,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .expect("Failed to create HTTP client"),
            sys: std::sync::Arc::new(tokio::sync::Mutex::new(System::new())),
        }
    }

    /// Start monitoring task (runs in background until stop signal)
    pub async fn start(
        self,
        mut stop_rx: tokio::sync::watch::Receiver<bool>,
        event_tx: Sender<MonitorEvent>,
    ) {
        info!(
            "Starting execution monitor for run_id={}, pid={}",
            self.run_id, self.pid
        );

        let mut interval = tokio::time::interval(Duration::from_secs(3));
        let mut last_event_count = 0;
        let mut idle_count = 0;

        let timeout_thr = 5;
        let idle_count_thr = 3;

        // Send initial "started" event
        let started_details = format!("Process started: pid={}", self.pid);

        // Send started status to controller
        let initial_status = ExecutionStatus {
            pid: self.pid as i32,
            elapsed_seconds: 0,
            process_alive: true,
            cpu_percent: 0,
            memory_mb: 0,
            telemetry_events_count: 0,
            last_activity: "started".to_string(),
        };
        self.send_status_to_controller("started", &initial_status, &started_details)
            .await;

        // Also send to event channel for local logging
        let _ = event_tx
            .send(MonitorEvent {
                event_type: "started".to_string(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                details: started_details,
                status: Some(initial_status),
            })
            .await;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match self.collect_status().await {
                        Ok(status) => {
                            let event_count = status.telemetry_events_count;
                            let process_is_alive = status.process_alive;

                            // Detect stuck state (no new events for 3+ heartbeats = 9+ seconds)
                            if event_count == last_event_count && process_is_alive {
                                idle_count += 1;
                            } else {
                                idle_count = 0;
                            }
                            last_event_count = event_count;

                            // Detect approaching timeout (within 5 seconds of timeout threshold)
                            let approaching_timeout = status.elapsed_seconds >= (self.timeout_seconds - timeout_thr);

                            let event_type = if approaching_timeout && process_is_alive {
                                "approaching_timeout".to_string()
                            } else if idle_count >= idle_count_thr && status.elapsed_seconds > 0{
                                "stuck".to_string()
                            } else if process_is_alive {
                                "heartbeat".to_string()
                            } else {
                                "terminated".to_string()
                            };

                            let details = format!(
                                "pid={}, events={}, cpu={}%, mem={}MB, elapsed={}s{}{}",
                                status.pid,
                                status.telemetry_events_count,
                                status.cpu_percent,
                                status.memory_mb,
                                status.elapsed_seconds,
                                if approaching_timeout { " [TIMEOUT APPROACHING]" } else { "" },
                                if idle_count >= 3 { " [STUCK?]" } else { "" }
                            );

                            debug!("{}: {}", event_type, details);

                            // Send status report to controller
                            self.send_status_to_controller(&event_type, &status, &details).await;

                            if event_tx.send(MonitorEvent {
                                event_type: event_type.clone(),
                                timestamp: chrono::Utc::now().timestamp_millis(),
                                details,
                                status: Some(status),
                            }).await.is_err() {
                                warn!("Monitor channel closed, stopping monitor");
                                break;
                            }

                            // Stop monitoring after sending "terminated" event
                            if !process_is_alive {
                                info!("Process terminated (pid={}), stopping monitor", self.pid);
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Failed to collect status: {}", e);
                            // Continue monitoring even on errors
                        }
                    }
                }
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        info!("Monitor received stop signal for run_id={}", self.run_id);

                        // Don't send status here - the main code will send the correct
                        // final status (timeout/error/success) after stopping the monitor.
                        // Sending "completed" here causes confusion when stopped due to timeout.

                        break;
                    }
                }
            }
        }

        info!("Execution monitor stopped for run_id={}", self.run_id);
    }

    async fn collect_status(
        &self,
    ) -> Result<ExecutionStatus, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Check if process still alive
        let process_alive = self.is_process_alive(self.pid);

        // 2. Get CPU/memory usage
        let (cpu_percent, memory_mb) = if process_alive {
            self.get_process_metrics(self.pid).await.unwrap_or((0, 0))
        } else {
            (0, 0)
        };

        // 3. Query RedEDR for event count (lightweight, best effort)
        let stats_url = format!("{}/api/stats", self.rededr_base_url);
        let events_count = match self.client.get(&stats_url).send().await {
            Ok(response) => match response.text().await {
                Ok(body_text) => {
                    if body_text.is_empty() {
                        debug!("RedEDR /api/stats returned empty response");
                        0
                    } else {
                        match serde_json::from_str::<serde_json::Value>(&body_text) {
                            Ok(stats) => stats["events_count"].as_i64().unwrap_or(0) as i32,
                            Err(e) => {
                                warn!(
                                    "Failed to parse RedEDR /api/stats JSON: {} | Body: {}",
                                    e,
                                    &body_text.chars().take(200).collect::<String>()
                                );
                                0
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read RedEDR /api/stats response body: {}", e);
                    0
                }
            },
            Err(e) => {
                warn!("Failed to fetch RedEDR /api/stats: {}", e);
                0
            }
        };

        // 4. Last activity (use event count as proxy)
        let last_activity = if events_count > 0 {
            "active".to_string()
        } else {
            "idle".to_string()
        };

        Ok(ExecutionStatus {
            pid: self.pid as i32,
            elapsed_seconds: self.start_time.elapsed().as_secs() as i32,
            process_alive,
            cpu_percent,
            memory_mb,
            telemetry_events_count: events_count,
            last_activity,
        })
    }

    /// Send execution status via StreamHandler if available
    /// Sends all execution details: job_id, run_id, artifact, pid, metrics, etc.
    async fn send_status_to_controller(
        &self,
        event_type: &str,
        status: &ExecutionStatus,
        details: &str,
    ) {
        if let Some(handler) = &self.stream_handler {
            // Send comprehensive execution status via bidirectional stream
            let send_future = handler.send_execution_status(
                self.job_id.clone(),
                self.run_id.clone(),
                self.artifact_name.clone(),
                status.pid,
                status.elapsed_seconds,
                status.process_alive,
                status.telemetry_events_count,
                event_type.to_string(),
                status.cpu_percent,
                status.memory_mb,
                details.to_string(),
            );

            // Wrap in timeout to prevent monitor from blocking indefinitely
            match tokio::time::timeout(Duration::from_secs(1), send_future).await {
                Ok(Ok(())) => {
                    // Success - status sent via stream
                    debug!(
                        "Monitor: Execution status sent via stream [event: {}, job: {}]",
                        event_type, self.job_id
                    );
                }
                Ok(Err(e)) => {
                    warn!(
                        "[!] Monitor: Failed to send execution status via stream [event: {}, job: {}]: {}",
                        event_type, self.job_id, e
                    );
                }
                Err(_) => {
                    warn!(
                        "[!] Monitor: Execution status send timeout (>1s) [event: {}, job: {}]",
                        event_type, self.job_id
                    );
                    warn!("   Stream may be blocked - monitoring will continue");
                }
            }
        } else {
            // No StreamHandler available - worker-only mode
            // Status is still available via local event channel for logging
            debug!(
                "Monitor: No StreamHandler, execution status available locally only [event: {}, job: {}, elapsed: {}s]",
                event_type, self.job_id, status.elapsed_seconds
            );
        }
    }

    fn is_process_alive(&self, pid: u32) -> bool {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            };

            unsafe {
                let handle_result = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
                match handle_result {
                    Ok(handle) if !handle.is_invalid() => {
                        let _ = CloseHandle(handle);
                        true
                    }
                    _ => false,
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Fallback for non-Windows (use sysinfo or similar)
            true
        }
    }

    async fn get_process_metrics(
        &self,
        pid: u32,
    ) -> Result<(i32, i32), Box<dyn std::error::Error + Send + Sync>> {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate};

        let pid_obj = Pid::from_u32(pid);

        // Lock the shared System instance and refresh only the specific process
        let mut sys = self.sys.lock().await;

        // Refresh only this PID
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid_obj]),
            /* remove_dead_processes */ false,
            ProcessRefreshKind::everything(),
        );

        if let Some(process) = sys.process(pid_obj) {
            // CPU usage (percentage, 0-100 per core, so can be > 100 on multi-core)
            let cpu_percent = process.cpu_usage() as i32;

            // Memory in MB (convert from bytes)
            let memory_mb = (process.memory() / 1024 / 1024) as i32;

            Ok((cpu_percent, memory_mb))
        } else {
            // Process not found (might have just terminated)
            Ok((0, 0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitor_creation() {
        let monitor = ExecutionMonitor::new(
            "run-test-001".to_string(),
            "job-test-001".to_string(),
            "worker-01".to_string(),
            "10.200.200.11".to_string(),
            "test-artifact.exe".to_string(),
            1234,
            "http://localhost:8081".to_string(),
            None, // No StreamHandler in test
            30,   // timeout_seconds
        );

        assert_eq!(monitor.run_id, "run-test-001");
        assert_eq!(monitor.job_id, "job-test-001");
        assert_eq!(monitor.worker_id, "worker-01");
        assert_eq!(monitor.worker_ip, "10.200.200.11");
        assert_eq!(monitor.artifact_name, "test-artifact.exe");
        assert_eq!(monitor.pid, 1234);
        assert_eq!(monitor.rededr_base_url, "http://localhost:8081");
        assert!(monitor.stream_handler.is_none());
        assert_eq!(monitor.timeout_seconds, 30);
    }
}
