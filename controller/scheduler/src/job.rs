// Job structure and state machine for scheduler queue system
// Phase 1: Basic implementation with in-memory storage

use crate::round::RoundSummary;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Job status for iterative mutation loop
/// Queued -> Running -> Completed/Failed/Stopped
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    /// Job is waiting in queue
    Queued,
    /// Job is actively running mutation rounds
    Running,
    /// All rounds completed successfully
    Completed,
    /// Job failed (unrecoverable error)
    Failed,
    /// User manually stopped job
    Stopped,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Queued => write!(f, "queued"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Failed => write!(f, "failed"),
            JobStatus::Stopped => write!(f, "stopped"),
        }
    }
}

/// Mutation specification to apply at build stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationSpec {
    /// Mutation ID (e.g., "ast.import_reshape")
    pub id: String,
    /// Mutation parameters (optional)
    pub params: Option<serde_json::Value>,
}

/// Module selection for modular template builds
/// Each field selects which variant to use for that module category
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModuleSelectionSpec {
    /// Carrier module: "alloc_rw_rx", "change_rw_rx", "peb_walk"
    pub carrier: String,
    /// Decoder module: "xor", "english"
    pub decoder: String,
    /// Anti-emulation module: "none", "sirallocalot", "timeraw"
    pub antiemulation: String,
    /// Guardrail module: "none", "env"
    pub guardrail: String,
    /// VirtualProtect module: "standard", "undersized"
    pub virtualprotect: String,
    /// Decoy module: "none", "winexec"
    pub decoy: String,
}

impl ModuleSelectionSpec {
    /// Create default module selection
    pub fn new() -> Self {
        Self {
            carrier: "alloc_rw_rx".to_string(),
            decoder: "xor".to_string(),
            antiemulation: "none".to_string(),
            guardrail: "none".to_string(),
            virtualprotect: "standard".to_string(),
            decoy: "none".to_string(),
        }
    }
}

/// Modular build specification for the new @MODULE marker system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModularBuildSpec {
    /// Module selection for assembly
    pub modules: ModuleSelectionSpec,
    /// Raw payload bytes (will be encoded)
    pub payload: Vec<u8>,
    /// Payload encoding type: "xor" or "english"
    pub encoding: String,
}

/// Job structure containing all information about a build/execute task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique job identifier (e.g., "job-000001")
    pub id: String,

    /// Current job status
    pub status: JobStatus,

    /// Template name (e.g., "rwx_direct", "eicar") - used for legacy SourceFile builds
    pub template_name: String,

    /// Source file path (e.g., "rwx_direct.c") - used for legacy SourceFile builds
    pub source_file: String,

    /// Modular build specification (new preferred mode)
    /// When set, template_name and source_file are ignored
    /// Uses @MODULE marker system for assembly
    pub modular_build: Option<ModularBuildSpec>,

    /// Mutations to apply at build stage
    pub mutations: Vec<MutationSpec>,

    /// Trace mode ("api+bb", "lines", "all", etc.)
    pub trace_mode: String,

    /// Target OS for execution (e.g., "win10", "win11", "ubuntu")
    /// If None, scheduler will auto-assign based on first available worker
    pub target_os: Option<String>,

    /// Required capabilities for worker selection (e.g., ["mde"], ["cortex"])
    /// Workers must have ALL listed capabilities to be selected
    /// If empty, no capability filtering is applied
    pub required_capabilities: Vec<String>,

    /// Priority (higher = earlier execution)
    pub priority: i32,

    /// Current round number (1-indexed, 0 means not started)
    pub current_round: u32,

    /// Maximum rounds to execute (stop condition)
    pub max_rounds: u32,

    /// Stop condition: evasion goal
    pub stop_on_evasion: bool, // If true, stop when not_detected

    /// Stop condition: detection goal (for testing)
    pub stop_on_detection: bool, // If true, stop when detected

    /// History of completed rounds
    pub rounds: Vec<RoundSummary>,

    /// Assigned worker ID (None if not assigned yet)
    pub worker_id: Option<String>,

    /// Artifact ID after build (SHA256)
    pub artifact_id: Option<String>,

    /// Run ID during execution (UUID)
    pub run_id: Option<String>,

    /// Job creation timestamp
    pub created_at: SystemTime,

    /// Job start timestamp (when building begins)
    pub started_at: Option<SystemTime>,

    /// Job completion timestamp
    pub completed_at: Option<SystemTime>,

    /// Error message if failed
    pub error: Option<String>,
}

impl Job {
    /// Create a new job with given parameters (legacy SourceFile mode)
    pub fn new(
        id: String,
        template_name: String,
        source_file: String,
        mutations: Vec<MutationSpec>,
        trace_mode: String,
        priority: i32,
        max_rounds: u32,
    ) -> Self {
        Job {
            id,
            status: JobStatus::Queued,
            template_name,
            source_file,
            modular_build: None, // Legacy mode - no modular build
            mutations,
            trace_mode,
            target_os: None,
            required_capabilities: Vec::new(),
            priority,
            current_round: 0,
            max_rounds,
            stop_on_evasion: false,
            stop_on_detection: false,
            rounds: Vec::new(),
            worker_id: None,
            artifact_id: None,
            run_id: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error: None,
        }
    }

    /// Create a new job with modular build specification (new preferred mode)
    pub fn new_modular(
        id: String,
        modular_build: ModularBuildSpec,
        mutations: Vec<MutationSpec>,
        trace_mode: String,
        priority: i32,
        max_rounds: u32,
    ) -> Self {
        Job {
            id,
            status: JobStatus::Queued,
            template_name: String::new(), // Not used for modular builds
            source_file: String::new(),   // Not used for modular builds
            modular_build: Some(modular_build),
            mutations,
            trace_mode,
            target_os: None,
            required_capabilities: Vec::new(),
            priority,
            current_round: 0,
            max_rounds,
            stop_on_evasion: false,
            stop_on_detection: false,
            rounds: Vec::new(),
            worker_id: None,
            artifact_id: None,
            run_id: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error: None,
        }
    }

    /// Check if this job uses modular build mode
    pub fn is_modular_build(&self) -> bool {
        self.modular_build.is_some()
    }

    /// Start job execution (transition to Running)
    pub fn start_running(&mut self) {
        self.status = JobStatus::Running;
        if self.started_at.is_none() {
            self.started_at = Some(SystemTime::now());
        }
    }

    /// Transition job to Completed state
    pub fn mark_completed(&mut self) {
        self.status = JobStatus::Completed;
        self.completed_at = Some(SystemTime::now());
    }

    /// Transition job to Failed state with error message
    pub fn mark_failed(&mut self, error: String) {
        self.status = JobStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(SystemTime::now());
    }

    /// Transition job to Stopped state (user request)
    pub fn mark_stopped(&mut self) {
        self.status = JobStatus::Stopped;
        self.completed_at = Some(SystemTime::now());
    }

    /// Check if job is in a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Stopped
        )
    }

    /// Determine if job should continue with next round
    pub fn should_continue(&self) -> bool {
        // Check max rounds limit
        if self.current_round >= self.max_rounds {
            return false;
        }

        // Check stop conditions based on last round results
        if let Some(last_round) = self.rounds.last() {
            // Stop if evasion achieved and flag is set
            if self.stop_on_evasion && !last_round.detected {
                return false;
            }

            // Stop if detection occurred and flag is set (for testing)
            if self.stop_on_detection && last_round.detected {
                return false;
            }
        }

        true
    }

    /// Start a new round
    pub fn start_round(&mut self) {
        self.current_round += 1;
        if self.status == JobStatus::Queued {
            self.status = JobStatus::Running;
        }
    }

    /// Complete a round and store summary
    pub fn complete_round(&mut self, round_summary: RoundSummary) {
        self.rounds.push(round_summary);
    }

    /// Get progress percentage (0-100)
    pub fn progress_percent(&self) -> u32 {
        if self.max_rounds == 0 {
            return 0;
        }
        ((self.current_round as f32 / self.max_rounds as f32) * 100.0) as u32
    }

    /// Get elapsed time since job creation
    pub fn elapsed_seconds(&self) -> u64 {
        self.created_at.elapsed().map(|d| d.as_secs()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests;
