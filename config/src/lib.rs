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
    //pub triage: TriageConfig,
    //pub mutator: MutatorConfig,
    pub scheduler: SchedulerConfig,
    //pub corpus: CorpusConfig,
    pub logging: LoggingConfig,
    //pub metrics: MetricsConfig,
    //pub telemetry: TelemetryConfig,
    //pub differential: DifferentialConfig,
    //pub experiments: ExperimentsConfig,
}

/// Worker configuration (loaded from worker.toml)
/// Deployed to: C:\AutoMutate\worker.toml (Windows VMs)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub worker: WorkerIdentityConfig,
    //#[serde(default)]
    //pub controller: Option<ControllerEndpointConfig>,
    //pub harness: HarnessConfig,
    pub telemetry: WorkerTelemetryConfig,
    //pub build: BuildConfig,
    pub storage: StorageConfig,
    pub logging: LoggingConfig,
    //pub health: HealthConfig,
    //pub security: SecurityConfig,
}

// === Controller Sub-Configs ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind_address: String,
    //pub max_connections: u32,
    //pub request_timeout_secs: u64,
    //#[serde(default)]
    //pub tls_enabled: bool,
    //#[serde(default)]
    //pub tls_cert_path: Option<String>,
    //#[serde(default)]
    //pub tls_key_path: Option<String>,
    //#[serde(default)]
    //pub require_client_cert: bool,
    //#[serde(default)]
    //pub client_ca_cert_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElasticsearchConfig {
    pub url: String,
    //pub index_prefix: String,
    //pub etw_index: String,
    //pub rededr_index: String,
    //pub runs_index: String,
    //pub bulk_size: usize,
    //pub bulk_timeout_ms: u64,
    //pub bulk_max_retries: u32,
    //#[serde(default)]
    //pub ilm_enabled: bool,
    //#[serde(default)]
    //pub ilm_max_age_days: u32,
    //#[serde(default)]
    //pub ilm_max_size_gb: u32,
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
    //pub queue_capacity: usize,
    pub max_concurrent_runs_per_worker: u32,
    pub run_timeout_secs: u64,
    pub poll_interval_ms: u64,
    pub health_timeout_seconds: u64,
    //pub max_retries: u32,
    //pub retry_backoff_secs: u64,
    /// Format: [{id: "win10-worker-01", address: "10.200.200.11:50052", enabled: true}]
    #[serde(default)]
    pub workers: Vec<WorkerEntryConfig>, //TODO still useful?
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerEntryConfig {
    pub id: String,
    pub address: String,
    #[serde(default = "default_worker_enabled")]
    pub enabled: bool,
}

fn default_worker_enabled() -> bool {
    true
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
    //pub format: String,
    //#[serde(default)]
    //pub file_enabled: bool,
    //#[serde(default)]
    //pub file_path: Option<String>,
    //#[serde(default)]
    //pub file_rotation: Option<String>,
    //#[serde(default)]
    //pub file_retention_days: Option<u32>,
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
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
}

fn default_listen_port() -> u16 {
    50052 // todo in config
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
    //pub stream_buffer_size: usize,
    //pub flush_interval_ms: u64,
    //pub etw: EtwConfig,
    //pub eventlog: EventLogConfig,
    //pub defender: DefenderConfig,
    pub rededr: RedEdrConfig,
    //pub api_tracing: ApiTracingConfig,
    //pub bb_coverage: BbCoverageConfig,
    //pub line_tracing: LineTracingConfig,
    //pub last_seen: LastSeenConfig,
    //#[serde(default)]
    //pub external: ExternalTelemetryConfig,
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
    //pub enabled: bool,
    pub base_url: String,
    //pub data_path: String,
    //pub file_watch_enabled: bool,
    /// If true, fail immediately when RedEDR has >1 leftover events (contamination).
    /// Default: false (attempt reset and continue).
    #[serde(default)]
    pub strict_contamination_check: bool,
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
    //pub max_artifact_age_days: u32,
    //pub max_log_age_days: u32,
    //pub max_storage_gb: u32,
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

// === Detection Verdict (shared between worker and controller) ===

/// Fine-grained detection verdict from the classifier.
///
/// Used by the worker's classifier to produce verdicts and by the controller
/// to derive `detection_outcome` and `detected` fields for ElasticSearch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionVerdict {
    /// Killed/failed before "Launching" checkpoint (loader detected by AV)
    KilledPrePayload,
    /// Killed after "Launching" checkpoint (behavioral detection post-payload)
    KilledPostPayload,
    /// Crash NTSTATUS (conservative: treated as detected)
    Crashed,
    /// Timeout + recent ETW or Launching checkpoint (probable evasion)
    TimeoutActive,
    /// Timeout + no recent activity (stuck/dead/ambiguous)
    TimeoutIdle,
    /// Exit code 0 (full evasion)
    CleanExit,
    /// Confirmed non-AV failure (dry-run exit_code matches, or infra issue)
    InfraError,
}

impl DetectionVerdict {
    /// Whether this verdict means the artifact was detected.
    pub fn is_detected(self) -> bool {
        matches!(
            self,
            Self::KilledPrePayload | Self::KilledPostPayload | Self::Crashed
        )
    }

    /// Map to ES `detection_outcome` string.
    pub fn detection_outcome(self) -> &'static str {
        match self {
            Self::KilledPrePayload => "KILLED_PRE_PAYLOAD",
            Self::KilledPostPayload => "KILLED_POST_PAYLOAD",
            Self::Crashed => "CRASHED",
            Self::TimeoutActive => "TIMEOUT_ACTIVE",
            Self::TimeoutIdle => "TIMEOUT_IDLE",
            Self::CleanExit => "FULL_EVASION",
            Self::InfraError => "INFRA_ERROR",
        }
    }

    /// Short string identifier for this verdict (used in proto/ES fields).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KilledPrePayload => "killed_pre_payload",
            Self::KilledPostPayload => "killed_post_payload",
            Self::Crashed => "crashed",
            Self::TimeoutActive => "timeout_active",
            Self::TimeoutIdle => "timeout_idle",
            Self::CleanExit => "clean_exit",
            Self::InfraError => "infra_error",
        }
    }

    /// Parse from string (inverse of `as_str`).
    pub fn from_verdict_str(s: &str) -> Option<Self> {
        match s {
            "killed_pre_payload" => Some(Self::KilledPrePayload),
            "killed_post_payload" => Some(Self::KilledPostPayload),
            "crashed" => Some(Self::Crashed),
            "timeout_active" => Some(Self::TimeoutActive),
            "timeout_idle" => Some(Self::TimeoutIdle),
            "clean_exit" => Some(Self::CleanExit),
            "infra_error" => Some(Self::InfraError),
            _ => None,
        }
    }
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

    pub fn find_config_path() -> String {
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

    pub fn find_config_path() -> String {
        // 1. Environment variable (highest priority)
        if let Ok(path) = std::env::var("AUTOMUTATE_WORKER_CONFIG")
            && Path::new(&path).exists()
        {
            return path;
        }

        // 2. VM deployment path (standard location after deployment)
        if Path::new("C:\\AutoMutate\\worker.toml").exists() {
            return "C:\\AutoMutate\\worker.toml".to_string();
        }

        // 3. Auto-detect based on hostname (matches generated config filename)
        if let Some(config_path) = Self::auto_detect_config_by_hostname() {
            return config_path;
        }

        // 5. Template fallback (first worker config found)
        "automation/generated/win10-worker-01.toml".to_string()
    }

    /// Auto-detect worker config by matching machine hostname to config filename
    ///
    /// Example: If hostname is "WIN10-WORKER-01", looks for "automation/generated/win10-worker-01.toml"
    fn auto_detect_config_by_hostname() -> Option<String> {
        // Get machine hostname
        let hostname = Self::get_hostname()?;
        let hostname_lower = hostname.to_lowercase();

        // Check automation/generated/ directory
        let generated_dir = Path::new("automation/generated");
        if !generated_dir.exists() {
            return None;
        }

        // Try direct match: <hostname>.toml (case-insensitive)
        let direct_path = generated_dir.join(format!("{}.toml", hostname_lower));
        if direct_path.exists() {
            return Some(direct_path.to_string_lossy().to_string());
        }

        // Fallback: Search for any config file that matches the hostname pattern
        for entry in std::fs::read_dir(generated_dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();

            if let Some(filename) = path.file_name() {
                let filename_str = filename.to_string_lossy().to_lowercase();
                // Check if filename (without .toml) matches hostname
                if let Some(stem) = filename_str.strip_suffix(".toml")
                    && stem == hostname_lower
                {
                    return Some(path.to_string_lossy().to_string());
                }
            }
        }

        None
    }

    /// Get machine hostname (Windows-compatible, no external dependencies)
    fn get_hostname() -> Option<String> {
        // Use standard library environment variable (works on Windows)
        if let Ok(hostname) = std::env::var("COMPUTERNAME") {
            return Some(hostname);
        }

        // Fallback: Try HOSTNAME env var (Unix-style, also sometimes set on Windows)
        if let Ok(hostname) = std::env::var("HOSTNAME") {
            return Some(hostname);
        }

        None
    }

    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|_| Self::default())
    }
}

// === Defaults ===

impl Default for ControllerConfig {
    fn default() -> Self {
        // Load from template if available, otherwise use hardcoded defaults
        Self::from_file("automation/generated/controller.toml")
            .unwrap_or_else(|_| Self::hardcoded_default())
    }
}

impl ControllerConfig {
    fn hardcoded_default() -> Self {
        Self {
            server: ServerConfig {
                bind_address: "0.0.0.0:50051".to_string(),
            },
            elasticsearch: ElasticsearchConfig {
                url: "http://localhost:9200".to_string(),
            },
            //triage: TriageConfig {
            //    model_type: "forest".to_string(),
            //    confidence_threshold: 0.7,
            //    min_support_count: 10,
            //    max_hypotheses: 10,
            //    feature_importance_method: "shap".to_string(),
            //},
            //mutator: MutatorConfig {
            //    selector_weights: [
            //        ("coverage".to_string(), 1.5),
            //        ("evasion".to_string(), 3.0),
            //        ("similarity".to_string(), 0.5),
            //    ]
            //    .iter()
            //    .cloned()
            //    .collect(),
            //    max_mutations_per_artifact: 5,
            //    probabilities: MutationProbabilities {
            //        ast_transform: 0.7,
            //        binary_transform: 0.5,
            //        behavioral_transform: 0.6,
            //    },
            //    ast: AstTransformConfig {
            //        control_flow_jitter: true,
            //        opaque_predicates: true,
            //        constant_encoding: true,
            //        import_reshaping: true,
            //        api_indirection: true,
            //    },
            //    binary: BinaryTransformConfig {
            //        splice_enabled: true,
            //        shellcode_reencoding: true,
            //        preserve_semantics: true,
            //    },
            //    behavioral: BehavioralTransformConfig {
            //        benign_preambles: true,
            //        staged_execution: true,
            //        timing_randomization: true,
            //    },
            //},
            scheduler: SchedulerConfig {
                max_concurrent_runs_per_worker: 1,
                run_timeout_secs: 300,
                poll_interval_ms: 500,
                health_timeout_seconds: 30,
                workers: vec![],
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
            //differential: DifferentialConfig {
            //    enabled: true,
            //    layers: vec![
            //        "event_counts".to_string(),
            //        "api_sequences".to_string(),
            //        "bb_coverage".to_string(),
            //        "argument_patterns".to_string(),
            //    ],
            //    lift_threshold: 1.2,
            //    min_confidence: 0.5,
            //},
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
                ip_address: "10.200.200.100".to_string(),
                os_version: "windows11".to_string(),
                listen_port: 50052,
            },
            telemetry: WorkerTelemetryConfig {
                rededr: RedEdrConfig {
                    base_url: "http://localhost:8080".to_string(),
                    strict_contamination_check: false,
                },
            },
            storage: StorageConfig {
                artifacts_path: "C:\\AutoMutate\\artifacts".to_string(),
                results_path: "C:\\AutoMutate\\results".to_string(),
                logs_path: "C:\\AutoMutate\\logs".to_string(),
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
        }
    }
}
