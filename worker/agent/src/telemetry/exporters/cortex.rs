/// Cortex Telemetry Exporter
///
/// Exports telemetry data to Cortex (Prometheus-compatible TSDB) via remote write protocol.
/// Supports both bearer token authentication and mTLS.
///
/// References:
/// - Prometheus Remote Write Spec: https://prometheus.io/docs/concepts/remote_write_spec/
/// - Cortex API: https://cortexmetrics.io/docs/api/

use anyhow::{Context, Result};
use edr_config::CortexConfig;
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// Prometheus-compatible time series sample
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub timestamp: i64,     // Unix timestamp in milliseconds
    pub value: f64,         // Metric value
}

/// Prometheus-compatible label
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    pub value: String,
}

/// Prometheus-compatible time series
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeries {
    pub labels: Vec<Label>,
    pub samples: Vec<Sample>,
}

/// Remote write request (will be protobuf-encoded in production)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteRequest {
    pub timeseries: Vec<TimeSeries>,
}

/// Cortex telemetry exporter
pub struct CortexExporter {
    config: CortexConfig,
    client: Client,
    buffer: Arc<Mutex<Vec<TimeSeries>>>,
}

impl CortexExporter {
    /// Create a new Cortex exporter
    pub fn new(config: CortexConfig) -> Result<Self> {
        let mut client_builder = ClientBuilder::new()
            .timeout(Duration::from_secs(config.timeout_secs))
            .connect_timeout(Duration::from_secs(10));

        // Configure mTLS if enabled
        if config.use_mtls {
            let cert = std::fs::read(&config.tls_cert_path)
                .with_context(|| format!("Failed to read cert: {}", config.tls_cert_path))?;
            let key = std::fs::read(&config.tls_key_path)
                .with_context(|| format!("Failed to read key: {}", config.tls_key_path))?;
            let ca = std::fs::read(&config.tls_ca_path)
                .with_context(|| format!("Failed to read CA: {}", config.tls_ca_path))?;

            let identity = reqwest::Identity::from_pem(&[cert, key].concat())?;
            let ca_cert = reqwest::Certificate::from_pem(&ca)?;

            client_builder = client_builder
                .identity(identity)
                .add_root_certificate(ca_cert);
        }

        let client = client_builder.build()?;

        Ok(Self {
            config,
            client,
            buffer: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Add a telemetry event to the buffer
    pub async fn push_event(&self, event: TimeSeries) -> Result<()> {
        let mut buffer = self.buffer.lock().await;
        buffer.push(event);

        // Auto-flush if buffer exceeds batch size
        if buffer.len() >= self.config.batch_size {
            drop(buffer); // Release lock before flushing
            self.flush().await?;
        }

        Ok(())
    }

    /// Flush buffered events to Cortex
    pub async fn flush(&self) -> Result<()> {
        let mut buffer = self.buffer.lock().await;
        if buffer.is_empty() {
            return Ok(());
        }

        let events = buffer.drain(..).collect::<Vec<_>>();
        drop(buffer); // Release lock before network call

        self.send_batch(&events).await
    }

    /// Send a batch of events to Cortex with retries
    async fn send_batch(&self, events: &[TimeSeries]) -> Result<()> {
        let request = WriteRequest {
            timeseries: events.to_vec(),
        };

        let mut last_error = None;

        for attempt in 0..=self.config.retry_attempts {
            match self.send_request(&request).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < self.config.retry_attempts {
                        tokio::time::sleep(Duration::from_secs(2_u64.pow(attempt))).await;
                    }
                }
            }
        }

        Err(last_error.unwrap())
    }

    /// Send a single remote write request
    async fn send_request(&self, request: &WriteRequest) -> Result<()> {
        let mut req = self
            .client
            .post(&self.config.endpoint)
            .header("Content-Type", "application/x-protobuf")
            .header("Content-Encoding", "snappy")
            .header("X-Prometheus-Remote-Write-Version", "0.1.0");

        // Add bearer token if not using mTLS
        if !self.config.use_mtls && !self.config.bearer_token.is_empty() {
            req = req.bearer_auth(&self.config.bearer_token);
        }

        // TODO: Encode as Protobuf + Snappy (for now, use JSON for development)
        let body = serde_json::to_vec(request)?;

        let response = req.body(body).send().await?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Cortex write failed: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }

        Ok(())
    }

    /// Convert ETW event to Prometheus time series
    pub fn etw_to_timeseries(&self, run_id: &str, event: &EtwEvent) -> TimeSeries {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        TimeSeries {
            labels: vec![
                Label {
                    name: "__name__".to_string(),
                    value: "automutate_etw_event".to_string(),
                },
                Label {
                    name: "run_id".to_string(),
                    value: run_id.to_string(),
                },
                Label {
                    name: "provider".to_string(),
                    value: event.provider.clone(),
                },
                Label {
                    name: "event_id".to_string(),
                    value: event.event_id.to_string(),
                },
                Label {
                    name: "process_id".to_string(),
                    value: event.process_id.to_string(),
                },
            ],
            samples: vec![Sample {
                timestamp,
                value: 1.0, // Count metric
            }],
        }
    }
}

/// Simplified ETW event struct (placeholder - replace with actual struct from telemetry module)
#[derive(Debug, Clone)]
pub struct EtwEvent {
    pub provider: String,
    pub event_id: u16,
    pub process_id: u32,
    pub timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cortex_exporter_creation() {
        let config = CortexConfig {
            enabled: true,
            endpoint: "https://cortex.example.com/api/v1/push".to_string(),
            bearer_token: "test_token".to_string(),
            use_mtls: false,
            tls_cert_path: String::new(),
            tls_key_path: String::new(),
            tls_ca_path: String::new(),
            batch_size: 100,
            flush_interval_secs: 10,
            retry_attempts: 3,
            timeout_secs: 30,
        };

        let exporter = CortexExporter::new(config);
        assert!(exporter.is_ok());
    }

    #[test]
    fn test_etw_to_timeseries() {
        let config = CortexConfig::default();
        let exporter = CortexExporter::new(config).unwrap();

        let event = EtwEvent {
            provider: "Microsoft-Windows-Kernel-Process".to_string(),
            event_id: 1,
            process_id: 1234,
            timestamp: 123456789,
        };

        let ts = exporter.etw_to_timeseries("run-123", &event);

        assert_eq!(ts.labels.len(), 5);
        assert_eq!(ts.labels[0].name, "__name__");
        assert_eq!(ts.labels[1].value, "run-123");
        assert_eq!(ts.samples.len(), 1);
        assert_eq!(ts.samples[0].value, 1.0);
    }
}
