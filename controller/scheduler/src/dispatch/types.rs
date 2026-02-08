//! Data models for dispatch system.
//!
//! JobSession: ephemeral runtime state (no persistence needed)
//! RoundSpec: immutable mutation recipe
//! RoundAgg: ephemeral join state until both runs complete
//! RunEnvelope: dispatch envelope for the per-worker pool

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

// ============================================================================
// Build Configuration
// ============================================================================

/// Module selection for modular template assembly
/// Uses "none" for disabled optional modules (matches build crate)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSelectionSpec {
    pub carrier: String,
    pub decoder: String,
    #[serde(default = "default_none")]
    pub antiemulation: String,
    #[serde(default = "default_none")]
    pub guardrail: String,
    #[serde(default = "default_standard")]
    pub virtualprotect: String,
    #[serde(default = "default_none")]
    pub decoy: String,
}

fn default_none() -> String {
    "none".to_string()
}

fn default_standard() -> String {
    "standard".to_string()
}

impl Default for ModuleSelectionSpec {
    fn default() -> Self {
        // Must match actual module files in build/templates/modules/
        Self {
            carrier: "alloc_rw_rx".to_string(),      // alloc_rw_rx, change_rw_rx, peb_walk
            decoder: "xor".to_string(),              // xor, english
            antiemulation: "none".to_string(),       // none, sirallocalot, timeraw
            guardrail: "none".to_string(),           // none, env
            virtualprotect: "standard".to_string(), // standard, undersized
            decoy: "none".to_string(),               // none, calc, winexec
        }
    }
}

/// Modular build specification for the @MODULE marker system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModularBuildSpec {
    /// Module selection for assembly
    pub modules: ModuleSelectionSpec,
    /// Path to raw payload file (.bin)
    pub payload_path: PathBuf,
    /// Payload encoding type: "xor" or "english"
    #[serde(default = "default_encoding")]
    pub encoding: String,
}

fn default_encoding() -> String {
    "xor".to_string()
}

// ============================================================================
// IDs (newtype wrappers for type safety)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoundId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

/// WorkerId - ephemeral session identity (new ID per connection)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerId(pub String);

// ============================================================================
// TargetId implementations
// ============================================================================

/// TargetId - stable machine identity (persists across reconnects)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TargetId(pub String);

impl TargetId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TargetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::borrow::Borrow<str> for TargetId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for TargetId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TargetId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl AsRef<str> for TargetId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// ============================================================================
// JobId implementations
// ============================================================================

impl JobId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::borrow::Borrow<str> for JobId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for JobId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for JobId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for JobId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ============================================================================
// RoundId implementations
// ============================================================================

impl RoundId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RoundId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for RoundId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// ============================================================================
// RunId implementations
// ============================================================================

impl RunId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for RunId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// ============================================================================
// WorkerId implementations
// ============================================================================

impl WorkerId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::borrow::Borrow<str> for WorkerId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for WorkerId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for WorkerId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for WorkerId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ============================================================================
// Job Session (ephemeral runtime state)
// ============================================================================

/// Summary of a completed round
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundSummary {
    pub round_id: RoundId,
    pub round_number: u32,
    pub mutations: Vec<String>,
    pub detected: bool,
    pub behavior_match: bool,
    pub evasion_score: f64,
    pub completed_at: SystemTime,
}

/// JobSession is ephemeral runtime state.
/// State (queued/running/finished) is implied by placement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSession {
    pub id: JobId,

    // Constraints / config
    pub target_os: Option<String>,
    pub required_capabilities: Vec<String>,

    // Build configuration
    pub build_spec: ModularBuildSpec,

    // Progress
    pub current_round: u32,
    pub max_rounds: u32,
    pub stop_on_evasion: bool,

    // Completed round summaries
    pub rounds: BTreeMap<u32, RoundSummary>,
    pub last_round: Option<RoundSummary>,

    // Timestamps
    pub created_at: SystemTime,
    pub started_at: Option<SystemTime>,
}

impl JobSession {
    pub fn new(id: impl Into<String>, max_rounds: u32, build_spec: ModularBuildSpec) -> Self {
        Self {
            id: JobId(id.into()),
            target_os: None,
            required_capabilities: Vec::new(),
            build_spec,
            current_round: 0,
            max_rounds,
            stop_on_evasion: false,
            rounds: BTreeMap::new(),
            last_round: None,
            created_at: SystemTime::now(),
            started_at: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_constraints(mut self, os: Option<String>, caps: Vec<String>) -> Self {
        self.target_os = os;
        self.required_capabilities = caps;
        self
    }

    #[allow(dead_code)]
    pub fn mark_started(&mut self) {
        if self.started_at.is_none() {
            self.started_at = Some(SystemTime::now());
        }
    }

    pub fn should_continue(&self) -> bool {
        if self.current_round >= self.max_rounds {
            return false;
        }
        if let Some(last) = &self.last_round {
            if self.stop_on_evasion && !last.detected {
                return false;
            }
        }
        true
    }

    pub fn start_round(&mut self) -> (u32, RoundId) {
        self.current_round += 1;
        let rid = RoundId(format!("{}-round-{}", self.id.0, self.current_round));
        (self.current_round, rid)
    }

    pub fn record_round_summary(&mut self, summary: RoundSummary) {
        self.rounds.insert(summary.round_number, summary.clone());
        self.last_round = Some(summary);
    }
}

// ============================================================================
// Round: Spec + Agg
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationSpec {
    pub id: String,
    pub params: Option<serde_json::Value>,
}

/// Immutable recipe/identity for a round
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundSpec {
    pub id: RoundId,
    pub job_id: JobId,
    pub round_number: u32,
    pub mutations: Vec<MutationSpec>,
}

/// Minimal outcome from a run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub detected: bool,
    pub exit_code: i32,
    pub error: Option<String>,
}

/// Ephemeral join state for a round until both runs finish
#[derive(Debug, Clone)]
pub struct RoundAgg {
    pub spec: RoundSpec,
    pub baseline_run_id: RunId,
    pub instrumented_run_id: RunId,
    pub baseline: Option<RunOutcome>,
    pub instrumented: Option<RunOutcome>,
}

impl RoundAgg {
    pub fn is_complete(&self) -> bool {
        self.baseline.is_some() && self.instrumented.is_some()
    }

    /// Compute round summary from completed runs
    pub fn to_summary(&self) -> Option<RoundSummary> {
        let baseline = self.baseline.as_ref()?;
        let instrumented = self.instrumented.as_ref()?;

        // Detected if either run was detected
        let detected = baseline.detected || instrumented.detected;

        // Behavior match if exit codes are the same (simplified)
        let behavior_match = baseline.exit_code == instrumented.exit_code;

        // Evasion score: 1.0 if not detected, 0.0 if detected
        let evasion_score = if detected { 0.0 } else { 1.0 };

        Some(RoundSummary {
            round_id: self.spec.id.clone(),
            round_number: self.spec.round_number,
            mutations: self.spec.mutations.iter().map(|m| m.id.clone()).collect(),
            detected,
            behavior_match,
            evasion_score,
            completed_at: SystemTime::now(),
        })
    }
}

// ============================================================================
// Run: Envelope (dispatch queue item)
// ============================================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunType {
    Baseline,
    Instrumented,
}

impl RunType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunType::Baseline => "baseline",
            RunType::Instrumented => "instrumented",
        }
    }

    pub fn trace_mode(&self) -> &'static str {
        match self {
            RunType::Baseline => "off",
            RunType::Instrumented => "lines",
        }
    }
}

impl std::fmt::Display for RunType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub sha256: Option<String>,
}

/// RunEnvelope: what we keep in the run pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEnvelope {
    pub run_id: RunId,
    pub job_id: JobId,
    pub round_id: RoundId,
    pub round_number: u32,
    pub run_type: RunType,
    pub artifact: ArtifactRef,
    pub mutations: Vec<String>,
    pub timeout_seconds: u32,
    pub required_os: String,
    pub required_capabilities: Vec<String>,
}

// ============================================================================
// Worker Info (from vm::manager)
// ============================================================================

#[derive(Debug, Clone)]
#[allow(dead_code)] // id field kept for logging/debugging
pub struct WorkerInfo {
    pub id: WorkerId,
    pub os: String,
    pub capabilities: Vec<String>,
}

// ============================================================================
// VM Info (for VMExecutor)
// ============================================================================

/// Information about a connected VM for VMExecutor.
/// Simplified version of WorkerInfo focused on execution capabilities.
#[derive(Debug, Clone)]
pub struct VMInfo {
    pub id: String,
    pub os: String,
    pub capabilities: Vec<String>,
}

impl From<&WorkerInfo> for VMInfo {
    fn from(info: &WorkerInfo) -> Self {
        Self {
            id: info.id.0.clone(),
            os: info.os.clone(),
            capabilities: info.capabilities.clone(),
        }
    }
}

// ============================================================================
// Job Outcome
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobOutcome {
    Completed { rounds_completed: u32 },
    Stopped { reason: String },
    Failed { error: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_build_spec() -> ModularBuildSpec {
        ModularBuildSpec {
            modules: ModuleSelectionSpec::default(),
            payload_path: PathBuf::from("test.bin"),
            encoding: "xor".to_string(),
        }
    }

    #[test]
    fn test_job_session_lifecycle() {
        let mut job = JobSession::new("job-1", 5, test_build_spec());
        assert!(job.should_continue());

        let (num, id) = job.start_round();
        assert_eq!(num, 1);
        assert!(id.0.contains("round-1"));

        job.record_round_summary(RoundSummary {
            round_id: id,
            round_number: 1,
            mutations: vec![],
            detected: false,
            behavior_match: true,
            evasion_score: 1.0,
            completed_at: SystemTime::now(),
        });

        assert_eq!(job.current_round, 1);
    }

    #[test]
    fn test_round_agg_completion() {
        let spec = RoundSpec {
            id: RoundId("r1".into()),
            job_id: JobId("j1".into()),
            round_number: 1,
            mutations: vec![],
        };

        let mut agg = RoundAgg {
            spec,
            baseline_run_id: RunId("r1-baseline".into()),
            instrumented_run_id: RunId("r1-instrumented".into()),
            baseline: None,
            instrumented: None,
        };

        assert!(!agg.is_complete());
        assert!(agg.to_summary().is_none());

        agg.baseline = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
        });
        assert!(!agg.is_complete());

        agg.instrumented = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
        });
        assert!(agg.is_complete());

        let summary = agg.to_summary().unwrap();
        assert!(!summary.detected);
        assert!(summary.behavior_match);
    }
}