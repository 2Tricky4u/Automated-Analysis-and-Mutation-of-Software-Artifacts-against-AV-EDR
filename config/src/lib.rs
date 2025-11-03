/// Shared configuration library for AutoMutate++
///
/// Loads TOML configuration files from automation/templates/
/// Aligns with deployment structure: WSL2 (Controller) + Hyper-V VMs (Workers)
///
/// Configuration files:
/// - Controller: automation/templates/controller.toml (deployed to WSL2)
/// - Worker: automation/templates/worker.toml (deployed to Windows VMs)
///
/// See automation/README.md for deployment details.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Controller configuration (loaded from controller.toml)
/// Deployed to: ~/automutate/config/controller.toml (WSL2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerConfig {
    pub server: ServerConfig,
    pub elasticsearch: ElasticsearchConfig,
    pub triage: TriageConfig,
    pub mutator: MutatorConfig,
    pub scheduler: SchedulerConfig,
    pub corpus: CorpusConfig,
    pub logging: LoggingConfig,
    pub metrics: MetricsConfig,
    pub telemetry: TelemetryConfig,
    pub differential: DifferentialConfig,
    pub experiments: ExperimentsConfig,
}

/// Worker configuration (loaded from worker.toml)
/// Deployed to: C:\AutoMutate\worker.toml (Windows VMs)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub worker: WorkerIdentityConfig,
    pub controller: ControllerEndpointConfig,
    pub harness: HarnessConfig,
    pub telemetry: WorkerTelemetryConfig,
    pub build: BuildConfig,
    pub storage: StorageConfig,
    pub logging: LoggingConfig,
    pub health: HealthConfig,
    pub security: SecurityConfig,
}

// === Controller Sub-Configs ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind_address: String,
    pub max_connections: u32,
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub tls_enabled: bool,
    #[serde(default)]
    pub tls_cert_path: Option<String>,
    #[serde(default)]
    pub tls_key_path: Option<String>,
    #[serde(default)]
    pub require_client_cert: bool,
    #[serde(default)]
    pub client_ca_cert_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElasticsearchConfig {
    pub url: String,
    pub index_prefix: String,
    pub etw_index: String,
    pub rededr_index: String,
    pub runs_index: String,
    pub bulk_size: usize,
    pub bulk_timeout_ms: u64,
    pub bulk_max_retries: u32,
    #[serde(default)]
    pub ilm_enabled: bool,
    #[serde(default)]
    pub ilm_max_age_days: u32,
    #[serde(default)]
    pub ilm_max_size_gb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageConfig {
    pub model_type: String,
    pub confidence_threshold: f64,
    pub min_support_count: u32,
    pub max_hypotheses: u32,
    pub feature_importance_method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutatorConfig {
    pub selector_weights: HashMap<String, f64>,
    pub max_mutations_per_artifact: u32,
    pub probabilities: MutationProbabilities,
    pub ast: AstTransformConfig,
    pub binary: BinaryTransformConfig,
    pub behavioral: BehavioralTransformConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationProbabilities {
    pub ast_transform: f64,
    pub binary_transform: f64,
    pub behavioral_transform: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstTransformConfig {
    pub control_flow_jitter: bool,
    pub opaque_predicates: bool,
    pub constant_encoding: bool,
    pub import_reshaping: bool,
    pub api_indirection: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryTransformConfig {
    pub splice_enabled: bool,
    pub shellcode_reencoding: bool,
    pub preserve_semantics: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehavioralTransformConfig {
    pub benign_preambles: bool,
    pub staged_execution: bool,
    pub timing_randomization: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub queue_capacity: usize,
    pub max_concurrent_runs_per_worker: u32,
    pub run_timeout_secs: u64,
    pub max_retries: u32,
    pub retry_backoff_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusConfig {
    pub storage_path: String,
    pub max_size: usize,
    pub prioritization: String,
    pub novelty_threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
    #[serde(default)]
    pub file_enabled: bool,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub file_rotation: Option<String>,
    #[serde(default)]
    pub file_retention_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    pub metrics_enabled: bool,
    pub metrics_bind_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub collection_interval_ms: u64,
    pub rededr_enabled: bool,
    pub rededr_data_path: String,
    pub api_tracing_enabled: bool,
    pub api_trace_format: String,
    pub bb_coverage_enabled: bool,
    pub bb_bitmap_size: usize,
    #[serde(default)]
    pub line_tracing_enabled: bool,
    #[serde(default)]
    pub line_trace_targeted: bool,
    #[serde(default)]
    pub line_trace_around_bb: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DifferentialConfig {
    pub enabled: bool,
    pub layers: Vec<String>,
    pub lift_threshold: f64,
    pub min_confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentsConfig {
    pub track_experiments: bool,
    pub experiment_metadata_required: bool,
    pub require_deterministic_seeds: bool,
    pub require_artifact_ids: bool,
}

// === Worker Sub-Configs ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerIdentityConfig {
    pub worker_id: String,
    pub ip_address: String,
    pub os_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerEndpointConfig {
    pub controller_address: String,
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub keepalive_interval_secs: u64,
    #[serde(default)]
    pub tls_enabled: bool,
    #[serde(default)]
    pub tls_ca_cert_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessConfig {
    pub working_directory: String,
    pub execution_timeout_secs: u64,
    pub cleanup_enabled: bool,
    pub sandbox_enabled: bool,
    pub sandbox_low_integrity: bool,
    pub sandbox_job_object: bool,
    pub monitor_children: bool,
    pub max_child_depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerTelemetryConfig {
    pub stream_buffer_size: usize,
    pub flush_interval_ms: u64,
    pub etw: EtwConfig,
    pub eventlog: EventLogConfig,
    pub defender: DefenderConfig,
    pub rededr: RedEdrConfig,
    pub api_tracing: ApiTracingConfig,
    pub bb_coverage: BbCoverageConfig,
    pub line_tracing: LineTracingConfig,
    pub last_seen: LastSeenConfig,
    #[serde(default)]
    pub external: ExternalTelemetryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtwConfig {
    pub enabled: bool,
    pub buffer_size_kb: u32,
    pub lost_event_threshold: u32,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLogConfig {
    pub enabled: bool,
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefenderConfig {
    pub enabled: bool,
    pub alert_polling_interval_ms: u64,
    pub scan_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedEdrConfig {
    pub enabled: bool,
    pub base_url: String,
    pub data_path: String,
    pub file_watch_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTracingConfig {
    pub enabled: bool,
    pub per_thread: bool,
    pub output_format: String,
    pub output_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BbCoverageConfig {
    pub enabled: bool,
    pub bitmap_size: usize,
    pub output_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineTracingConfig {
    pub enabled: bool,
    pub mode: String,
    pub output_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastSeenConfig {
    pub enabled: bool,
    pub ring_buffer_size: usize,
    pub flush_on_abnormal_exit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalTelemetryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cortex: CortexConfig,
    #[serde(default)]
    pub mde: MdeConfig,
    #[serde(default)]
    pub custom_http: CustomHttpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub bearer_token: String,
    #[serde(default)]
    pub use_mtls: bool,
    #[serde(default)]
    pub tls_cert_path: String,
    #[serde(default)]
    pub tls_key_path: String,
    #[serde(default)]
    pub tls_ca_path: String,
    #[serde(default)]
    pub batch_size: usize,
    #[serde(default)]
    pub flush_interval_secs: u64,
    #[serde(default)]
    pub retry_attempts: u32,
    #[serde(default)]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub use_cert_auth: bool,
    #[serde(default)]
    pub cert_path: String,
    #[serde(default)]
    pub cert_password: String,
    #[serde(default)]
    pub batch_size: usize,
    #[serde(default)]
    pub flush_interval_secs: u64,
    #[serde(default)]
    pub retry_attempts: u32,
    #[serde(default)]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomHttpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub batch_size: usize,
    #[serde(default)]
    pub flush_interval_secs: u64,
    #[serde(default)]
    pub retry_attempts: u32,
    #[serde(default)]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    pub rust_toolchain: String,
    pub llvm_version: String,
    pub default_trace_mode: String,
    pub optimization_level: String,
    pub debug_info: bool,
    pub strip_symbols: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub artifacts_path: String,
    pub results_path: String,
    pub logs_path: String,
    pub max_artifact_age_days: u32,
    pub max_log_age_days: u32,
    pub max_storage_gb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    pub health_check_interval_secs: u64,
    pub max_cpu_percent: u32,
    pub max_memory_percent: u32,
    pub max_disk_percent: u32,
    pub auto_revert_on_hang: bool,
    pub hang_detection_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub disable_network: bool,
    pub block_internet: bool,
    pub allow_controller_only: bool,
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    pub verify_dep: bool,
    pub verify_aslr: bool,
    pub verify_cfg: bool,
}

// === Implementation ===

impl ControllerConfig {
    /// Load Controller configuration from TOML file
    ///
    /// Default search paths (in order):
    /// 1. Path from --config CLI argument
    /// 2. AUTOMUTATE_CONTROLLER_CONFIG environment variable
    /// 3. ~/automutate/config/controller.toml (WSL2 deployment default)
    /// 4. automation/generated/controller.toml (generated from config.yaml)
    /// 5. ./config/controller.toml (local development)
    /// 6. automation/templates/controller.toml (template fallback)
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let path = Self::find_config_path();
        Self::from_file(&path)
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    fn find_config_path() -> String {
        // 1. Environment variable
        if let Ok(path) = std::env::var("AUTOMUTATE_CONTROLLER_CONFIG")
            && Path::new(&path).exists()
        {
            return path;
        }

        // 2. WSL2 deployment path
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let wsl_path = format!("{}/automutate/config/controller.toml", home);
        if Path::new(&wsl_path).exists() {
            return wsl_path;
        }

        // 3. Generated config (from generate-configs.ps1)
        if Path::new("automation/generated/controller.toml").exists() {
            return "automation/generated/controller.toml".to_string();
        }

        // 4. Local development
        if Path::new("config/controller.toml").exists() {
            return "config/controller.toml".to_string();
        }

        // 5. Template fallback
        "automation/templates/controller.toml".to_string()
    }

    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|_| Self::default())
    }
}

impl WorkerConfig {
    /// Load Worker configuration from TOML file
    ///
    /// Default search paths (in order):
    /// 1. Path from --config CLI argument
    /// 2. AUTOMUTATE_WORKER_CONFIG environment variable
    /// 3. C:\AutoMutate\worker.toml (Windows VM deployment default)
    /// 4. ./config/worker.toml (local development)
    /// 5. automation/templates/worker.toml (template fallback)
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let path = Self::find_config_path();
        Self::from_file(&path)
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    fn find_config_path() -> String {
        // 1. Environment variable
        if let Ok(path) = std::env::var("AUTOMUTATE_WORKER_CONFIG")
            && Path::new(&path).exists()
        {
            return path;
        }

        // 2. Windows VM deployment path
        #[cfg(target_os = "windows")]
        {
            let win_path = r"C:\AutoMutate\worker.toml";
            if Path::new(win_path).exists() {
                return win_path.to_string();
            }
        }

        // 3. Local development
        if Path::new("config/worker.toml").exists() {
            return "config/worker.toml".to_string();
        }

        // 4. Template fallback
        "automation/templates/worker.toml".to_string()
    }

    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|_| Self::default())
    }
}

// === Defaults ===

impl Default for ControllerConfig {
    fn default() -> Self {
        // Load from template if available, otherwise use hardcoded defaults
        Self::from_file("automation/templates/controller.toml")
            .unwrap_or_else(|_| Self::hardcoded_default())
    }
}

impl ControllerConfig {
    fn hardcoded_default() -> Self {
        Self {
            server: ServerConfig {
                bind_address: "0.0.0.0:50051".to_string(),
                max_connections: 100,
                request_timeout_secs: 300,
                tls_enabled: false,
                tls_cert_path: None,
                tls_key_path: None,
                require_client_cert: false,
                client_ca_cert_path: None,
            },
            elasticsearch: ElasticsearchConfig {
                url: "http://localhost:9200".to_string(),
                index_prefix: "automutate-".to_string(),
                etw_index: "etw-*".to_string(),
                rededr_index: "rededr-*".to_string(),
                runs_index: "runs".to_string(),
                bulk_size: 100,
                bulk_timeout_ms: 5000,
                bulk_max_retries: 3,
                ilm_enabled: true,
                ilm_max_age_days: 90,
                ilm_max_size_gb: 50,
            },
            triage: TriageConfig {
                model_type: "forest".to_string(),
                confidence_threshold: 0.7,
                min_support_count: 10,
                max_hypotheses: 10,
                feature_importance_method: "shap".to_string(),
            },
            mutator: MutatorConfig {
                selector_weights: [
                    ("coverage".to_string(), 1.5),
                    ("evasion".to_string(), 3.0),
                    ("similarity".to_string(), 0.5),
                ]
                .iter()
                .cloned()
                .collect(),
                max_mutations_per_artifact: 5,
                probabilities: MutationProbabilities {
                    ast_transform: 0.7,
                    binary_transform: 0.5,
                    behavioral_transform: 0.6,
                },
                ast: AstTransformConfig {
                    control_flow_jitter: true,
                    opaque_predicates: true,
                    constant_encoding: true,
                    import_reshaping: true,
                    api_indirection: true,
                },
                binary: BinaryTransformConfig {
                    splice_enabled: true,
                    shellcode_reencoding: true,
                    preserve_semantics: true,
                },
                behavioral: BehavioralTransformConfig {
                    benign_preambles: true,
                    staged_execution: true,
                    timing_randomization: true,
                },
            },
            scheduler: SchedulerConfig {
                queue_capacity: 1000,
                max_concurrent_runs_per_worker: 1,
                run_timeout_secs: 300,
                max_retries: 3,
                retry_backoff_secs: 30,
            },
            corpus: CorpusConfig {
                storage_path: "/var/lib/automutate/corpus".to_string(),
                max_size: 10000,
                prioritization: "hybrid".to_string(),
                novelty_threshold: 0.1,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                format: "json".to_string(),
                file_enabled: true,
                file_path: Some("/var/log/automutate/controller.log".to_string()),
                file_rotation: Some("daily".to_string()),
                file_retention_days: Some(30),
            },
            metrics: MetricsConfig {
                metrics_enabled: true,
                metrics_bind_address: "0.0.0.0:9090".to_string(),
            },
            telemetry: TelemetryConfig {
                collection_interval_ms: 100,
                rededr_enabled: true,
                rededr_data_path: "C:\\RedEDR\\Data".to_string(),
                api_tracing_enabled: true,
                api_trace_format: "newline-json".to_string(),
                bb_coverage_enabled: true,
                bb_bitmap_size: 65536,
                line_tracing_enabled: false,
                line_trace_targeted: false,
                line_trace_around_bb: vec![],
            },
            differential: DifferentialConfig {
                enabled: true,
                layers: vec![
                    "event_counts".to_string(),
                    "api_sequences".to_string(),
                    "bb_coverage".to_string(),
                    "argument_patterns".to_string(),
                ],
                lift_threshold: 1.2,
                min_confidence: 0.5,
            },
            experiments: ExperimentsConfig {
                track_experiments: true,
                experiment_metadata_required: true,
                require_deterministic_seeds: true,
                require_artifact_ids: true,
            },
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        // Load from template if available, otherwise use hardcoded defaults
        Self::from_file("automation/templates/worker.toml")
            .unwrap_or_else(|_| Self::hardcoded_default())
    }
}

impl WorkerConfig {
    fn hardcoded_default() -> Self {
        Self {
            worker: WorkerIdentityConfig {
                worker_id: "win11-worker-01".to_string(),
                ip_address: "192.168.200.100".to_string(),
                os_version: "windows11".to_string(),
            },
            controller: ControllerEndpointConfig {
                controller_address: "192.168.200.1:50051".to_string(),
                connect_timeout_secs: 30,
                request_timeout_secs: 300,
                keepalive_interval_secs: 30,
                tls_enabled: false,
                tls_ca_cert_path: None,
            },
            harness: HarnessConfig {
                working_directory: "C:\\AutoMutate\\runs".to_string(),
                execution_timeout_secs: 120,
                cleanup_enabled: true,
                sandbox_enabled: true,
                sandbox_low_integrity: true,
                sandbox_job_object: true,
                monitor_children: true,
                max_child_depth: 3,
            },
            telemetry: WorkerTelemetryConfig {
                stream_buffer_size: 10000,
                flush_interval_ms: 1000,
                etw: EtwConfig {
                    enabled: true,
                    buffer_size_kb: 1024,
                    lost_event_threshold: 100,
                    providers: vec![
                        "Microsoft-Windows-Kernel-Process".to_string(),
                        "Microsoft-Windows-Kernel-File".to_string(),
                        "Microsoft-Windows-Kernel-Network".to_string(),
                        "Microsoft-Windows-Threat-Intelligence".to_string(),
                    ],
                },
                eventlog: EventLogConfig {
                    enabled: true,
                    channels: vec![
                        "Security".to_string(),
                        "System".to_string(),
                        "Application".to_string(),
                        "Microsoft-Windows-Windows Defender/Operational".to_string(),
                    ],
                },
                defender: DefenderConfig {
                    enabled: true,
                    alert_polling_interval_ms: 500,
                    scan_timeout_secs: 60,
                },
                rededr: RedEdrConfig {
                    enabled: true,
                    base_url: "http://localhost:8080".to_string(),
                    data_path: "C:\\RedEDR\\Data".to_string(),
                    file_watch_enabled: true,
                },
                api_tracing: ApiTracingConfig {
                    enabled: true,
                    per_thread: true,
                    output_format: "newline-json".to_string(),
                    output_path: "C:\\AutoMutate\\traces".to_string(),
                },
                bb_coverage: BbCoverageConfig {
                    enabled: true,
                    bitmap_size: 65536,
                    output_path: "C:\\AutoMutate\\coverage".to_string(),
                },
                line_tracing: LineTracingConfig {
                    enabled: false,
                    mode: "off".to_string(),
                    output_path: "C:\\AutoMutate\\lines".to_string(),
                },
                last_seen: LastSeenConfig {
                    enabled: true,
                    ring_buffer_size: 100,
                    flush_on_abnormal_exit: true,
                },
                external: ExternalTelemetryConfig::default(),
            },
            build: BuildConfig {
                rust_toolchain: "stable".to_string(),
                llvm_version: "17".to_string(),
                default_trace_mode: "api+bb".to_string(),
                optimization_level: "2".to_string(),
                debug_info: false,
                strip_symbols: true,
            },
            storage: StorageConfig {
                artifacts_path: "C:\\AutoMutate\\artifacts".to_string(),
                results_path: "C:\\AutoMutate\\results".to_string(),
                logs_path: "C:\\AutoMutate\\logs".to_string(),
                max_artifact_age_days: 7,
                max_log_age_days: 30,
                max_storage_gb: 50,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                format: "json".to_string(),
                file_enabled: false,
                file_path: Some("C:\\AutoMutate\\logs\\worker.log".to_string()),
                file_rotation: Some("daily".to_string()),
                file_retention_days: Some(14),
            },
            health: HealthConfig {
                health_check_interval_secs: 60,
                max_cpu_percent: 90,
                max_memory_percent: 80,
                max_disk_percent: 90,
                auto_revert_on_hang: true,
                hang_detection_timeout_secs: 600,
            },
            security: SecurityConfig {
                disable_network: false,
                block_internet: true,
                allow_controller_only: true,
                allowed_ips: vec![],
                verify_dep: true,
                verify_aslr: true,
                verify_cfg: false,
            },
        }
    }
}

// === External Telemetry Defaults ===

impl Default for CortexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            bearer_token: String::new(),
            use_mtls: false,
            tls_cert_path: String::new(),
            tls_key_path: String::new(),
            tls_ca_path: String::new(),
            batch_size: 1000,
            flush_interval_secs: 10,
            retry_attempts: 3,
            timeout_secs: 30,
        }
    }
}

impl Default for MdeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "https://api.securitycenter.microsoft.com".to_string(),
            tenant_id: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            use_cert_auth: false,
            cert_path: String::new(),
            cert_password: String::new(),
            batch_size: 100,
            flush_interval_secs: 30,
            retry_attempts: 3,
            timeout_secs: 60,
        }
    }
}

impl Default for CustomHttpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            method: "POST".to_string(),
            headers: HashMap::new(),
            batch_size: 500,
            flush_interval_secs: 15,
            retry_attempts: 3,
            timeout_secs: 30,
        }
    }
}
