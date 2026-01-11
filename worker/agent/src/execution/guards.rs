use crate::telemetry;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::info;

pub const DELAY: u64 = 10;

// ============================================================================
// RAII Guards for Resource Cleanup
// ============================================================================

/// RAII guard that ensures RedEDR is reset on drop
/// This guarantees cleanup on all exit paths (success, error, panic)
pub struct RedEdrGuard {
    collector: telemetry::collectors::rededr::RedEdrCollector,
    reset_on_drop: bool,
}

impl RedEdrGuard {
    pub fn new(collector: telemetry::collectors::rededr::RedEdrCollector) -> Self {
        Self {
            collector,
            reset_on_drop: true,
        }
    }

    /// Get reference to collector for operations
    pub fn collector(&self) -> &telemetry::collectors::rededr::RedEdrCollector {
        &self.collector
    }

    /// Manually reset and prevent Drop cleanup (for normal exit path)
    pub async fn reset_now(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.reset_on_drop = false; // Prevent double reset in Drop
        self.collector.reset().await
    }
}

impl Drop for RedEdrGuard {
    fn drop(&mut self) {
        if self.reset_on_drop {
            // Spawn blocking task
            let base_url = self.collector.config().base_url.clone();
            std::thread::spawn(move || {
                // Create minimal runtime for cleanup
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let client = reqwest::Client::builder()
                        .timeout(Duration::from_secs(DELAY))
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
pub struct MonitorGuard {
    stop_tx: Option<tokio::sync::watch::Sender<bool>>,
    handle: Option<tokio::task::JoinHandle<()>>,
    event_consumer: Option<tokio::task::JoinHandle<()>>,
}

impl MonitorGuard {
    pub fn new(
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
    pub async fn stop(mut self) {
        // Send stop signal
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }

        // Abort event consumer FIRST to prevent monitor from blocking on channel send
        if let Some(consumer) = self.event_consumer.take() {
            consumer.abort();
        }

        // Wait for monitor to finish
        if let Some(handle) = self.handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(DELAY), handle).await;
        }
    }
}

impl Drop for MonitorGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(consumer) = self.event_consumer.take() {
            consumer.abort();
        }
    }
}

/// RAII guard that ensures child process is killed on drop
pub struct ProcessGuard {
    child: Option<tokio::process::Child>,
    should_kill: bool,
}

impl ProcessGuard {
    pub fn new(child: tokio::process::Child) -> Self {
        Self {
            child: Some(child),
            should_kill: true,
        }
    }

    /// Get mutable reference to child for waiting
    pub fn child_mut(&mut self) -> &mut tokio::process::Child {
        self.child.as_mut().expect("Child already taken")
    }

    /// Take child ownership and prevent kill on drop
    pub fn disarm(mut self) -> tokio::process::Child {
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
/// Automatically releases lock on drop
pub struct ExecutionLockGuard {
    lock: Arc<Mutex<ExecutionState>>,
}

impl ExecutionLockGuard {
    pub fn new(lock: Arc<Mutex<ExecutionState>>) -> Self {
        Self { lock }
    }
}

impl Drop for ExecutionLockGuard {
    fn drop(&mut self) {
        // Release lock by spawning task
        let lock = self.lock.clone();
        tokio::spawn(async move {
            let mut state = lock.lock().await;
            let job_id = state
                .current_job_id
                .take()
                .unwrap_or_else(|| "unknown".to_string());
            let artifact = state
                .current_artifact
                .take()
                .unwrap_or_else(|| "unknown".to_string());
            state.busy = false;
            info!(
                "Execution lock RELEASED: job_id={}, artifact={}",
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
