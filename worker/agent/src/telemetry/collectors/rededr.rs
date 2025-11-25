/// RedEDR HTTP API Collector
///
/// Polls the RedEDR HTTP API and transforms events to gRPC TelemetryData
/// for streaming to the controller.
///
/// Architecture:
/// 1. Polls GET /api/logs/rededr at flush_interval (1000ms default)
/// 2. Tracks last_seen event index to avoid duplicates
/// 3. Transforms RedEDR JSON events to protobuf TelemetryData
/// 4. Sends to mpsc channel for gRPC streaming
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info, warn};

/// Stack trace entry structure (from RedEDR stack_trace field)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackTraceEntry {
    #[serde(default)]
    pub addr: Option<u64>,
    #[serde(default)]
    pub addr_info: Option<String>,
    #[serde(default)]
    pub idx: Option<u32>,
}

/// RedEDR event structure (from HTTP API /api/logs/rededr)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedEdrEvent {
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub trace_id: Option<u64>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub func: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub tid: Option<u32>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub event_id: Option<u32>,
    #[serde(default)]
    pub callstack: Option<Vec<String>>,
    // Complex fields that need explicit typing
    #[serde(default)]
    pub stack_trace: Option<Vec<StackTraceEntry>>,
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    // Flexible metadata for other fields
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// RedEDR collector configuration
#[derive(Debug, Clone)]
pub struct RedEdrCollectorConfig {
    pub base_url: String,
    pub flush_interval_ms: u64,
    pub job_id: String,
    pub run_id: String,
}

/// RedEDR HTTP collector
pub struct RedEdrCollector {
    config: RedEdrCollectorConfig,
    client: reqwest::Client,
    seen_trace_ids: HashSet<u64>,
}

impl RedEdrCollector {
    pub fn new(config: RedEdrCollectorConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            client,
            seen_trace_ids: HashSet::new(),
        }
    }

    /// Get reference to config (for accessing base_url in Drop)
    pub fn config(&self) -> &RedEdrCollectorConfig {
        &self.config
    }

    /// Start polling RedEDR HTTP API and send events to channel
    pub async fn start(
        mut self,
        tx: Sender<crate::edr::common::TelemetryData>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            "Starting RedEDR collector: {} (flush_interval={}ms)",
            self.config.base_url, self.config.flush_interval_ms
        );

        let interval = Duration::from_millis(self.config.flush_interval_ms);

        loop {
            // Poll RedEDR API
            match self.fetch_events().await {
                Ok(events) => {
                    debug!("Fetched {} events from RedEDR", events.len());

                    // Filter out already-seen events (by trace_id)
                    let new_events: Vec<RedEdrEvent> = events
                        .into_iter()
                        .filter(|e| {
                            if let Some(trace_id) = e.trace_id {
                                !self.seen_trace_ids.contains(&trace_id)
                            } else {
                                true // Include events without trace_id
                            }
                        })
                        .collect();

                    info!("Processing {} new RedEDR events", new_events.len());

                    // Send new events to gRPC stream
                    for event in new_events {
                        // Track trace_id to avoid duplicates
                        if let Some(trace_id) = event.trace_id {
                            self.seen_trace_ids.insert(trace_id);
                        }

                        // Transform to TelemetryData protobuf
                        let telemetry = self.transform_event(&event);

                        // Send to channel (non-blocking)
                        if let Err(e) = tx.try_send(telemetry) {
                            warn!("Failed to send telemetry event: {}", e);
                            // Channel full or closed, skip this event
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to fetch RedEDR events: {}", e);
                    // Continue polling even on errors
                }
            }

            // Wait before next poll
            tokio::time::sleep(interval).await;
        }
    }

    /// Fetch events from RedEDR HTTP API
    async fn fetch_events(
        &self,
    ) -> Result<Vec<RedEdrEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/logs/rededr", self.config.base_url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()).into());
        }

        // Get response body as text first for better error diagnostics
        let body_text = response
            .text()
            .await
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        // Handle empty response (RedEDR might return empty string instead of [])
        if body_text.is_empty() {
            debug!("RedEDR returned empty response, treating as empty event list");
            return Ok(Vec::new());
        }

        // Try to parse JSON with detailed error reporting
        let events: Vec<RedEdrEvent> = serde_json::from_str(&body_text).map_err(|e| {
            // Log first 500 chars of response for debugging
            let preview = if body_text.len() > 500 {
                format!("{}... ({} bytes total)", &body_text[..500], body_text.len())
            } else {
                body_text.clone()
            };

            error!("JSON parse error: {}", e);
            error!("Response body preview: {}", preview);
            error!("Parse error at line {} column {}", e.line(), e.column());

            format!(
                "JSON parse error at line {} column {}: {} | Body preview: {}",
                e.line(),
                e.column(),
                e,
                preview
            )
        })?;

        Ok(events)
    }

    /// Start tracing target executables (call before artifact execution)
    pub async fn start_trace(
        &self,
        targets: Vec<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/trace/start", self.config.base_url);

        self.client
            .post(&url)
            .json(&serde_json::json!({"trace": targets}))
            .send()
            .await?;

        info!("Started RedEDR tracing for targets: {:?}", targets);
        Ok(())
    }

    /// Collect all events (call AFTER artifact execution completes)
    pub async fn collect_all(
        &self,
        job_id: &str,
    ) -> Result<Vec<crate::edr::common::TelemetryData>, Box<dyn std::error::Error + Send + Sync>>
    {
        info!("Collecting all RedEDR events for job_id={}", job_id);

        let events = self.fetch_events().await?;
        info!("Fetched {} events from RedEDR", events.len());

        let telemetry_events: Vec<crate::edr::common::TelemetryData> = events
            .iter()
            .map(|e| self.transform_event_with_job(job_id, e))
            .collect();

        Ok(telemetry_events)
    }

    /// Reset RedEDR state for next run
    pub async fn reset(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/trace/reset", self.config.base_url);

        // Create a client with longer timeout for reset (can take time to clear state)
        let reset_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        let response = reset_client
            .post(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to send reset request to {}: {}", url, e))?;

        if !response.status().is_success() {
            return Err(format!(
                "RedEDR reset returned error status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )
            .into());
        }

        info!("Reset RedEDR state successfully");
        Ok(())
    }

    /// Acquire exclusive lock for RedEDR tracing (prevents concurrent runs)
    pub async fn acquire_lock(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/lock/acquire", self.config.base_url);

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to send lock acquire request: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to acquire RedEDR lock: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )
            .into());
        }

        info!("Acquired RedEDR exclusive lock");
        Ok(())
    }

    /// Release exclusive lock for RedEDR tracing
    pub async fn release_lock(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/lock/release", self.config.base_url);

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to send lock release request: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to release RedEDR lock: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )
            .into());
        }

        info!("Released RedEDR exclusive lock");
        Ok(())
    }

    /// Transform RedEDR event to protobuf TelemetryData
    fn transform_event(&self, event: &RedEdrEvent) -> crate::edr::common::TelemetryData {
        self.transform_event_with_job(&self.config.job_id, event)
    }

    /// Transform RedEDR event to protobuf TelemetryData with custom job_id
    fn transform_event_with_job(
        &self,
        job_id: &str,
        event: &RedEdrEvent,
    ) -> crate::edr::common::TelemetryData {
        // Serialize entire event as JSON payload
        let payload = serde_json::to_vec(event).unwrap_or_default();

        // Extract metadata for quick filtering
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("source".to_string(), "rededr".to_string());

        if let Some(ref event_type) = event.r#type {
            metadata.insert("event_type".to_string(), event_type.clone());
        }
        if let Some(pid) = event.pid {
            metadata.insert("pid".to_string(), pid.to_string());
        }
        if let Some(tid) = event.tid {
            metadata.insert("tid".to_string(), tid.to_string());
        }
        if let Some(ref provider) = event.provider {
            metadata.insert("provider".to_string(), provider.clone());
        }
        if let Some(trace_id) = event.trace_id {
            metadata.insert("trace_id".to_string(), trace_id.to_string());
        }

        crate::edr::common::TelemetryData {
            job_id: job_id.to_string(),
            event_type: event
                .r#type
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            timestamp: chrono::Utc::now().timestamp_millis(),
            payload,
            metadata,
            typed_event: None,  // RedEDR events use generic payload, not typed events
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_event() {
        let config = RedEdrCollectorConfig {
            base_url: "http://localhost:8081".to_string(),
            flush_interval_ms: 1000,
            job_id: "job-000001".to_string(),
            run_id: "run-uuid-123".to_string(),
        };

        let collector = RedEdrCollector::new(config);

        let event = RedEdrEvent {
            date: Some("2025-11-02-15-30-00".to_string()),
            r#type: Some("etw".to_string()),
            trace_id: Some(42),
            target: Some("notepad.exe".to_string()),
            func: Some("NtAllocateVirtualMemory".to_string()),
            pid: Some(1234),
            tid: Some(5678),
            provider: Some("Microsoft-Windows-Kernel-Process".to_string()),
            event_id: Some(10),
            callstack: None,
            stack_trace: None,
            targets: None,
            extra: serde_json::Map::new(),
        };

        let telemetry = collector.transform_event(&event);

        assert_eq!(telemetry.job_id, "job-000001");
        assert_eq!(telemetry.event_type, "etw");
        assert_eq!(telemetry.metadata.get("source").unwrap(), "rededr");
        assert_eq!(telemetry.metadata.get("pid").unwrap(), "1234");
        assert_eq!(telemetry.metadata.get("trace_id").unwrap(), "42");
    }
}
