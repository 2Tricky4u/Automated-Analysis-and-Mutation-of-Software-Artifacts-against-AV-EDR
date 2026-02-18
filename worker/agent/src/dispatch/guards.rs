use crate::telemetry;
use std::time::Duration;

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
            let base_url = self.collector.config().base_url.clone();

            // Try to use the existing tokio runtime (always available in execute_run context)
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                // Fire-and-forget: spawn cleanup on the existing runtime
                handle.spawn(async move {
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
            } else {
                // Fallback: no runtime available (shouldn't happen, but defensive)
                eprintln!("RedEDR cleanup skipped: no tokio runtime available in Drop");
                eprintln!("WARNING: RedEDR may be contaminated for the next run");
            }
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
        if self.should_kill
            && let Some(ref mut child) = self.child
        {
            // start_kill() is synchronous — sends kill signal without awaiting
            if let Err(e) = child.start_kill() {
                eprintln!("Failed to kill process in Drop: {}", e);
            } else {
                eprintln!("Process kill signal sent in Drop (error path)");
            }
        }
    }
}
