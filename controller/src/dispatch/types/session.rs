//! Job session and status types.
//!
//! [`JobSession`] holds all ephemeral runtime state for a single job,
//! including build configuration, selection parameters, round history,
//! and progress counters. [`JobOutcome`] captures the terminal state.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::SystemTime;

use super::config::ModularBuildSpec;
use super::ids::JobId;
use super::round::{DifferentialCategory, RoundSummary};
use crate::triage::SearchSpace;

// ============================================================================
// Job Session (ephemeral runtime state)
// ============================================================================

/// Ephemeral runtime state for a single mutation-exploration job.
///
/// Owned by a [`JobWorker`](crate::dispatch::job_worker::JobWorker) for
/// its entire lifetime. Lifecycle state (queued / running / finished) is
/// implied by placement in the orchestrator's data structures, not stored
/// here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSession {
    pub id: JobId,

    // Constraints / config
    pub target_os: Option<String>,
    pub required_capabilities: Vec<String>,

    // Build configuration
    pub build_spec: ModularBuildSpec,

    // Instrumented run trace mode (default: "lines")
    pub trace_mode: String,

    // Selection
    pub search_space: SearchSpace,

    // Progress
    pub current_round: u32,
    pub completed_rounds: u32,
    pub max_rounds: u32,
    pub stop_on_evasion: bool,

    // Shellcode checkpoints (INT3 breakpoints; None = disabled)
    pub sc_checkpoint_count: Option<u32>,

    /// Cache precomputed payload headers across rounds (default: true).
    /// Set to false for debugging to force fresh encoding every round.
    pub cache_payload: bool,

    /// MSVC-compatible build mode: clang-cl + link.exe.
    /// When true, produces PE binaries with genuine MSVC metadata (Rich header, linker version).
    pub msvc_compat: bool,

    /// Optional WSL path to vcvarsall.bat override.
    /// When empty, uses default VS 2022 Build Tools path.
    pub msvc_vcvarsall: String,

    // Completed round summaries
    pub rounds: BTreeMap<u32, RoundSummary>,
    pub last_round: Option<RoundSummary>,

    // Timestamps
    pub created_at: SystemTime,
    pub started_at: Option<SystemTime>,
}

impl JobSession {
    /// Create a new job session with default selection and progress state.
    pub fn new(id: impl Into<String>, max_rounds: u32, build_spec: ModularBuildSpec) -> Self {
        Self {
            id: JobId(id.into()),
            target_os: None,
            required_capabilities: Vec::new(),
            build_spec,
            trace_mode: "lines".to_string(),
            search_space: SearchSpace::default(),
            sc_checkpoint_count: None,
            cache_payload: true,
            msvc_compat: false,
            msvc_vcvarsall: String::new(),
            current_round: 0,
            completed_rounds: 0,
            max_rounds,
            stop_on_evasion: false,
            rounds: BTreeMap::new(),
            last_round: None,
            created_at: SystemTime::now(),
            started_at: None,
        }
    }

    /// Builder-pattern setter for OS and capability constraints.
    #[allow(dead_code)]
    pub fn with_constraints(mut self, os: Option<String>, caps: Vec<String>) -> Self {
        self.target_os = os;
        self.required_capabilities = caps;
        self
    }

    /// Record the job start timestamp (idempotent — only the first call takes effect).
    pub fn mark_started(&mut self) {
        if self.started_at.is_none() {
            self.started_at = Some(SystemTime::now());
        }
    }

    /// Returns `true` if the job should produce more rounds.
    ///
    /// Returns `false` when any of these conditions hold:
    /// 1. `current_round >= max_rounds` — round budget exhausted.
    /// 2. `stop_on_evasion` is set and the last round achieved full evasion.
    pub fn should_continue(&self) -> bool {
        if self.current_round >= self.max_rounds {
            return false;
        }
        // Only stop on real evasion, not InstrumentationArtifact
        if let Some(last) = &self.last_round
            && self.stop_on_evasion
            && last.differential_category == DifferentialCategory::Evasion
        {
            return false;
        }
        true
    }

    /// Increment the round counter and return `(round_number, round_id)`.
    pub fn start_round(&mut self) -> (u32, super::ids::RoundId) {
        self.current_round += 1;
        let rid = super::ids::RoundId(format!("{}-round-{}", self.id.0, self.current_round));
        (self.current_round, rid)
    }

    /// Record a completed round summary into the in-memory history.
    pub fn record_round_summary(&mut self, summary: RoundSummary) {
        self.rounds.insert(summary.round_number, summary.clone());
        self.last_round = Some(summary);
        self.completed_rounds += 1;
    }

    /// Create a lightweight snapshot for external visibility
    pub fn to_info(&self, status: JobStatus) -> JobInfo {
        JobInfo {
            id: self.id.clone(),
            status,
            current_round: self.current_round,
            completed_rounds: self.completed_rounds,
            max_rounds: self.max_rounds,
            target_os: self.target_os.clone(),
            started_at: self.started_at,
        }
    }
}

// ============================================================================
// Job Status & Outcome
// ============================================================================

/// Job execution status for tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Running,
    Completed,
    Stopped,
    Failed,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Stopped => write!(f, "stopped"),
            JobStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Terminal outcome of a job after its [`JobWorker`](crate::dispatch::job_worker::JobWorker) exits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobOutcome {
    Completed { rounds_completed: u32 },
    Stopped { reason: String },
    Failed { error: String },
}

impl JobOutcome {
    pub fn to_status(&self) -> JobStatus {
        match self {
            JobOutcome::Completed { .. } => JobStatus::Completed,
            JobOutcome::Stopped { .. } => JobStatus::Stopped,
            JobOutcome::Failed { .. } => JobStatus::Failed,
        }
    }
}

// ============================================================================
// Job Info (lightweight snapshot for registry)
// ============================================================================

/// Lightweight job info for API responses.
/// Snapshot of JobSession state for external visibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: JobId,
    pub status: JobStatus,
    pub current_round: u32,
    pub completed_rounds: u32,
    pub max_rounds: u32,
    pub target_os: Option<String>,
    pub started_at: Option<SystemTime>,
}
