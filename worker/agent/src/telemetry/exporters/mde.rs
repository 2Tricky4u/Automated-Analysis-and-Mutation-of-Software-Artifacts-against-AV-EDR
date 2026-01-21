/// Microsoft Defender for Endpoint (MDE) Telemetry Exporter
///
/// Exports telemetry data to Microsoft Defender for Endpoint via the Security Center API.
/// Supports OAuth2 client credentials flow and certificate-based authentication.
///
/// References:
/// - MDE API: https://learn.microsoft.com/en-us/microsoft-365/security/defender-endpoint/apis-intro
/// - Authentication: https://learn.microsoft.com/en-us/microsoft-365/security/defender-endpoint/api-hello-world
use anyhow::Result;
use edr_config::MdeConfig;
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex;

/// OAuth2 token response
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

/// MDE custom detection event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdeEvent {
    #[serde(rename = "EventTime")]
    pub event_time: String, // ISO8601 format

    #[serde(rename = "MachineId")]
    pub machine_id: String,

    #[serde(rename = "ComputerName")]
    pub computer_name: String,

    #[serde(rename = "EventType")]
    pub event_type: String, // e.g., "ProcessCreation", "FileCreation", "NetworkConnection"

    #[serde(rename = "Severity")]
    pub severity: String, // "Informational", "Low", "Medium", "High"

    #[serde(rename = "Category")]
    pub category: String, // "AutoMutateTest"

    #[serde(rename = "Title")]
    pub title: String,

    #[serde(rename = "Description")]
    pub description: String,

    #[serde(rename = "AdditionalFields", skip_serializing_if = "Option::is_none")]
    pub additional_fields: Option<serde_json::Value>,
}

/// Token cache with expiry
struct TokenCache {
    token: Option<String>,
    expires_at: Option<SystemTime>,
}

/// MDE telemetry exporter
pub struct MdeExporter {
    config: MdeConfig,
    client: Client,
    token_cache: Arc<Mutex<TokenCache>>,
    buffer: Arc<Mutex<Vec<MdeEvent>>>,
}

impl MdeExporter {
    /// Create a new MDE exporter
    pub fn new(config: MdeConfig) -> Result<Self> {
        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(config.timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            config,
            client,
            token_cache: Arc::new(Mutex::new(TokenCache {
                token: None,
                expires_at: None,
            })),
            buffer: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Add a telemetry event to the buffer
    pub async fn push_event(&self, event: MdeEvent) -> Result<()> {
        let mut buffer = self.buffer.lock().await;
        buffer.push(event);

        // Auto-flush if buffer exceeds batch size
        if buffer.len() >= self.config.batch_size {
            drop(buffer); // Release lock before flushing
            self.flush().await?;
        }

        Ok(())
    }

    /// Flush buffered events to MDE
    pub async fn flush(&self) -> Result<()> {
        let mut buffer = self.buffer.lock().await;
        if buffer.is_empty() {
            return Ok(());
        }

        let events = buffer.drain(..).collect::<Vec<_>>();
        drop(buffer); // Release lock before network call

        self.send_batch(&events).await
    }

    /// Send a batch of events to MDE with retries
    async fn send_batch(&self, events: &[MdeEvent]) -> Result<()> {
        let mut last_error = None;

        for attempt in 0..=self.config.retry_attempts {
            match self.send_request(events).await {
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

    /// Send events to MDE Custom Detection API
    async fn send_request(&self, events: &[MdeEvent]) -> Result<()> {
        let token = self.get_access_token().await?;

        let endpoint = format!("{}/api/customdetections/events", self.config.endpoint);

        let response = self
            .client
            .post(&endpoint)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .json(&events)
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!(
                "MDE write failed: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }

        Ok(())
    }

    /// Get OAuth2 access token (cached)
    async fn get_access_token(&self) -> Result<String> {
        let mut cache = self.token_cache.lock().await;

        // Return cached token if still valid
        if let (Some(token), Some(expires_at)) = (&cache.token, cache.expires_at)
            && SystemTime::now() < expires_at
        {
            return Ok(token.clone());
        }

        // Acquire new token
        let token_response = if self.config.use_cert_auth {
            self.acquire_token_cert().await?
        } else {
            self.acquire_token_secret().await?
        };

        // Cache token with 5-minute buffer before expiry
        cache.token = Some(token_response.access_token.clone());
        cache.expires_at = Some(
            SystemTime::now() + Duration::from_secs(token_response.expires_in.saturating_sub(300)),
        );

        Ok(token_response.access_token)
    }

    /// Acquire token using client secret (OAuth2 client credentials)
    async fn acquire_token_secret(&self) -> Result<TokenResponse> {
        let token_url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.config.tenant_id
        );

        let params = [
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
            ("scope", "https://api.securitycenter.microsoft.com/.default"),
            ("grant_type", "client_credentials"),
        ];

        let response = self.client.post(&token_url).form(&params).send().await?;

        if !response.status().is_success() {
            anyhow::bail!(
                "MDE token acquisition failed: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(token_response)
    }

    /// Acquire token using certificate authentication
    async fn acquire_token_cert(&self) -> Result<TokenResponse> {
        // TODO: Implement certificate-based authentication
        // This requires:
        // 1. Loading .pfx certificate with password
        // 2. Creating JWT assertion signed with private key
        // 3. Sending JWT to Azure AD token endpoint
        //
        // For now, fall back to client secret
        anyhow::bail!(
            "Certificate authentication not yet implemented. Please use client_secret authentication."
        );
    }

    /// Convert ETW event to MDE custom detection event
    pub fn etw_to_mde_event(
        &self,
        run_id: &str,
        machine_id: &str,
        computer_name: &str,
        event: &EtwEvent,
    ) -> MdeEvent {
        MdeEvent {
            event_time: chrono::Utc::now().to_rfc3339(),
            machine_id: machine_id.to_string(),
            computer_name: computer_name.to_string(),
            event_type: format!("ETW-{}", event.provider),
            severity: "Informational".to_string(),
            category: "AutoMutateTest".to_string(),
            title: format!("ETW Event {} from Run {}", event.event_id, run_id),
            description: format!(
                "Provider: {}, PID: {}, Event ID: {}",
                event.provider, event.process_id, event.event_id
            ),
            additional_fields: Some(serde_json::json!({
                "run_id": run_id,
                "provider": event.provider,
                "event_id": event.event_id,
                "process_id": event.process_id,
                "timestamp": event.timestamp,
            })),
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
    async fn test_mde_exporter_creation() {
        let config = MdeConfig {
            enabled: true,
            endpoint: "https://api.securitycenter.microsoft.com".to_string(),
            tenant_id: "test-tenant-id".to_string(),
            client_id: "test-client-id".to_string(),
            client_secret: "test-client-secret".to_string(),
            use_cert_auth: false,
            cert_path: String::new(),
            cert_password: String::new(),
            batch_size: 100,
            flush_interval_secs: 30,
            retry_attempts: 3,
            timeout_secs: 60,
        };

        let exporter = MdeExporter::new(config);
        assert!(exporter.is_ok());
    }

    #[test]
    fn test_etw_to_mde_event() {
        let config = MdeConfig::default();
        let exporter = MdeExporter::new(config).unwrap();

        let etw_event = EtwEvent {
            provider: "Microsoft-Windows-Kernel-Process".to_string(),
            event_id: 1,
            process_id: 1234,
            timestamp: 123456789,
        };

        let mde_event =
            exporter.etw_to_mde_event("run-123", "machine-456", "WORKER-01", &etw_event);

        assert_eq!(mde_event.machine_id, "machine-456");
        assert_eq!(mde_event.computer_name, "WORKER-01");
        assert_eq!(mde_event.category, "AutoMutateTest");
        assert!(mde_event.additional_fields.is_some());
    }
}
