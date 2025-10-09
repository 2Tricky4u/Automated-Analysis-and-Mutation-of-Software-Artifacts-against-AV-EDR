/// Shared configuration library
///
/// Loads configuration from config.yml for all services
/// Ensures reproducibility and centralized configuration management

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub controller: ControllerConfig,
    pub worker: WorkerConfig,
    pub telemetry: TelemetryConfig,
    pub elasticsearch: ElasticsearchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerConfig {
    pub host: String,
    pub port: u16,
    pub max_jobs: u32,
    pub selector_port: u16,
    pub triage_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub controller_endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub etw_buffer_size_kb: u32,
    pub rededr_output_dir: String,
    pub watch_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElasticsearchConfig {
    pub hosts: Vec<String>,
    pub index_prefix: String,
}

impl AppConfig {
    /// Load configuration from config.yml
    ///
    /// Searches for config.yml in:
    /// 1. Current directory
    /// 2. /etc/edr-lab/
    /// 3. $HOME/.edr-lab/
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name("config").required(false))
            .add_source(config::File::with_name("/etc/edr-lab/config").required(false))
            .add_source(
                config::File::with_name(&format!(
                    "{}/.edr-lab/config",
                    std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
                ))
                .required(false),
            )
            .add_source(config::Environment::with_prefix("EDR"))
            .build()?;

        Ok(settings.try_deserialize()?)
    }

    /// Load configuration with defaults for development
    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|_| Self::default())
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            controller: ControllerConfig {
                host: "0.0.0.0".to_string(),
                port: 50051,
                max_jobs: 100,
                selector_port: 50054,
                triage_port: 50055,
            },
            worker: WorkerConfig {
                id: "worker-01".to_string(),
                host: "0.0.0.0".to_string(),
                port: 50052,
                controller_endpoint: "http://controller:50051".to_string(),
            },
            telemetry: TelemetryConfig {
                etw_buffer_size_kb: 1024,
                rededr_output_dir: "/var/lib/rededr/output".to_string(),
                watch_interval_secs: 5,
            },
            elasticsearch: ElasticsearchConfig {
                hosts: vec!["http://localhost:9200".to_string()],
                index_prefix: "edr-lab".to_string(),
            },
        }
    }
}
