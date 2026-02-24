//! Data models for dispatch system.
//!
//! JobSession: ephemeral runtime state (no persistence needed)
//! RoundSpec: immutable mutation recipe
//! RoundAgg: ephemeral join state until both runs complete
//! RunEnvelope: dispatch envelope for the per-worker pool

use automutate_common::{DetectionVerdict, has_launched};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::triage::SearchSpace;

// ============================================================================
// Build Configuration
// ============================================================================

/// Module selection for modular template assembly
/// Uses "none" for disabled optional modules (matches build crate)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleSelectionSpec {
    pub carrier: String,
    pub decoder: String,
    #[serde(default = "default_none")]
    pub antiemulation: String,
    #[serde(default = "default_none")]
    pub deconditioner: String,
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

impl ModuleSelectionSpec {
    /// Build from a proto `ModuleSelection`, falling back to defaults for empty fields.
    pub fn from_proto_or_default(proto: &crate::automutate::controller::ModuleSelection) -> Self {
        let d = Self::default();
        fn pick(proto_val: &str, default: String) -> String {
            if proto_val.is_empty() {
                default
            } else {
                proto_val.to_string()
            }
        }
        Self {
            carrier: pick(&proto.carrier, d.carrier),
            decoder: pick(&proto.decoder, d.decoder),
            antiemulation: pick(&proto.antiemulation, d.antiemulation),
            deconditioner: pick(&proto.deconditioner, d.deconditioner),
            guardrail: pick(&proto.guardrail, d.guardrail),
            virtualprotect: pick(&proto.virtualprotect, d.virtualprotect),
            decoy: pick(&proto.decoy, d.decoy),
        }
    }
}

impl Default for ModuleSelectionSpec {
    fn default() -> Self {
        // Must match actual module files in build/templates/modules/
        Self {
            carrier: "alloc_rw_rx".to_string(), // alloc_rw_rx, change_rw_rx, peb_walk
            decoder: "xor".to_string(),         // xor, english
            antiemulation: "none".to_string(),  // none, sirallocalot, timeraw
            deconditioner: "none".to_string(),  // none, alloc_loop
            guardrail: "none".to_string(),      // none, env
            virtualprotect: "standard".to_string(), // standard, undersized
            decoy: "none".to_string(),          // none, calc, winexec
        }
    }
}

impl From<ModuleSelectionSpec> for build::ModuleSelection {
    fn from(spec: ModuleSelectionSpec) -> Self {
        Self {
            carrier: spec.carrier,
            decoder: spec.decoder,
            antiemulation: spec.antiemulation,
            deconditioner: spec.deconditioner,
            guardrail: spec.guardrail,
            virtualprotect: spec.virtualprotect,
            decoy: spec.decoy,
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

// TODO: migrate callers from TargetId("...".into()) to TargetId::new("...")
#[allow(dead_code)]
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

// TODO: migrate callers from JobId("...".into()) to JobId::new("...")
#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

// ============================================================================
// Differential Category (CLAUDE.md Section 5)
// ============================================================================

/// Classifies a round based on the two-run differential protocol.
///
/// | Baseline | Instrumented | Category                |
/// |----------|-------------|-------------------------|
/// | Detected | Detected    | RealDetection           |
/// | Not det. | Detected    | InstrumentationArtifact |
/// | Detected | Not det.    | Flaky                   |
/// | Not det. | Not det.    | Evasion                 |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DifferentialCategory {
    RealDetection,
    InstrumentationArtifact,
    Flaky,
    Evasion,
}

impl DifferentialCategory {
    pub fn from_runs(baseline_detected: bool, instrumented_detected: bool) -> Self {
        match (baseline_detected, instrumented_detected) {
            (true, true) => Self::RealDetection,
            (false, true) => Self::InstrumentationArtifact,
            (true, false) => Self::Flaky,
            (false, false) => Self::Evasion,
        }
    }

    /// True only for RealDetection — the only trustworthy "detected" signal.
    pub fn is_detected(self) -> bool {
        matches!(self, Self::RealDetection)
    }

    /// True for categories where the result is trustworthy for the feedback loop.
    /// Used by future token scorer / mutation selector (FEEDBACK-LOOP-PLAN.md).
    #[allow(dead_code)]
    pub fn is_trustworthy(self) -> bool {
        matches!(self, Self::RealDetection | Self::Evasion)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RealDetection => "real_detection",
            Self::InstrumentationArtifact => "instrumentation_artifact",
            Self::Flaky => "flaky",
            Self::Evasion => "evasion",
        }
    }
}

impl std::fmt::Display for DifferentialCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Summary of a completed round
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundSummary {
    pub round_id: RoundId,
    pub round_number: u32,
    pub mutations: Vec<String>,
    pub modules: ModuleSelectionSpec,
    pub detected: bool,
    pub behavior_match: bool,
    pub evasion_score: f64,
    pub differential_category: DifferentialCategory,
    pub completed_at: SystemTime,
    /// Dryrun exit code (None if no dryrun result available)
    #[serde(default)]
    pub dry_run_exit_code: Option<i32>,
    /// Whether a dryrun result was used for this round
    #[serde(default)]
    pub has_dryrun: bool,
    /// Authoritative detection verdict (e.g. "detected", "mutation_failed")
    #[serde(default)]
    pub detection_verdict: String,
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
            trace_mode: "lines".to_string(),
            search_space: SearchSpace::default(),
            sc_checkpoint_count: None,
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

    // TODO: wire into job submission gRPC handler for OS/capability constraints
    #[allow(dead_code)]
    pub fn with_constraints(mut self, os: Option<String>, caps: Vec<String>) -> Self {
        self.target_os = os;
        self.required_capabilities = caps;
        self
    }

    pub fn mark_started(&mut self) {
        if self.started_at.is_none() {
            self.started_at = Some(SystemTime::now());
        }
    }

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

    pub fn start_round(&mut self) -> (u32, RoundId) {
        self.current_round += 1;
        let rid = RoundId(format!("{}-round-{}", self.id.0, self.current_round));
        (self.current_round, rid)
    }

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
    pub modules: ModuleSelectionSpec,
}

/// Minimal outcome from a run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub detected: bool,
    pub exit_code: i32,
    pub error: Option<String>,
    /// Payload executed to completion (evasion signal from worker)
    pub success: bool,
    /// Wall-clock execution time in milliseconds
    pub elapsed_ms: f64,
    /// Fine-grained classifier verdict (e.g. "killed_pre_payload"), empty for legacy
    #[serde(default)]
    pub detection_verdict: String,
    /// Last checkpoint reached before exit (e.g. "Launching")
    #[serde(default)]
    pub last_checkpoint: String,
}

/// Ephemeral join state for a round until both runs finish
#[derive(Debug, Clone)]
pub struct RoundAgg {
    pub spec: RoundSpec,
    pub baseline_run_id: RunId,
    pub instrumented_run_id: RunId,
    pub baseline: Option<RunOutcome>,
    pub instrumented: Option<RunOutcome>,
    pub baseline_vm_id: String,
    pub instrumented_vm_id: String,
    pub started_at: SystemTime,
    /// Timeout for the run in milliseconds, used for survival_ratio in evasion score
    pub timeout_ms: u64,
    /// Pre-instrumentation assembled C source for line trace resolution.
    pub assembled_source: Option<String>,
    /// Controller-side build artifact paths for post-round cleanup.
    pub baseline_artifact_path: PathBuf,
    pub instrumented_artifact_path: PathBuf,
    // --- Dryrun fields ---
    /// Run ID for the dryrun (always produced; may sit unclaimed in pool)
    pub dryrun_run_id: RunId,
    /// Dryrun result (None until dryrun worker returns)
    pub dryrun: Option<RunOutcome>,
    /// VM that executed the dryrun
    pub dryrun_vm_id: String,
    /// Grace period deadline: set when baseline+instrumented are done, dryrun still pending.
    /// If the deadline passes before dryrun arrives, the round finalizes without it.
    pub dryrun_deadline: Option<Instant>,
}

impl RoundAgg {
    pub fn is_complete(&self) -> bool {
        self.baseline.is_some() && self.instrumented.is_some()
    }

    /// Compute round summary from completed runs using the differential protocol.
    ///
    /// If a dryrun result is available, `override_with_dryrun()` produces the
    /// authoritative verdict. Otherwise the worker's provisional verdict is used.
    pub fn to_summary(&self) -> Option<RoundSummary> {
        let baseline = self.baseline.as_ref()?;
        let instrumented = self.instrumented.as_ref()?;

        let dry_run_exit_code = self.dryrun.as_ref().map(|d| d.exit_code);
        let has_dryrun = self.dryrun.is_some();

        // Compute authoritative verdict using dry-run if available
        let effective_verdict = if let Some(dryrun) = &self.dryrun {
            override_with_dryrun(dryrun, baseline)
        } else if !baseline.detection_verdict.is_empty() {
            // No dry-run: use worker's provisional verdict string
            DetectionVerdict::from_verdict_str(&baseline.detection_verdict)
                .unwrap_or(DetectionVerdict::Ambiguous)
        } else {
            // Legacy: no verdict string, fall back to worker's detected boolean
            if baseline.detected {
                DetectionVerdict::Detected
            } else {
                DetectionVerdict::Evasion
            }
        };

        let effective_baseline_detected = effective_verdict.is_detected();

        // Differential category from CLAUDE.md Section 5
        let category =
            DifferentialCategory::from_runs(effective_baseline_detected, instrumented.detected);
        let detected = category.is_detected();

        // Behavior match: exit codes agree AND detected status agrees
        let behavior_match = baseline.exit_code == instrumented.exit_code
            && effective_baseline_detected == instrumented.detected;

        let evasion_score = self.compute_evasion_score(baseline, instrumented, category);

        Some(RoundSummary {
            round_id: self.spec.id.clone(),
            round_number: self.spec.round_number,
            mutations: self.spec.mutations.iter().map(|m| m.id.clone()).collect(),
            modules: self.spec.modules.clone(),
            detected,
            behavior_match,
            evasion_score,
            differential_category: category,
            completed_at: SystemTime::now(),
            dry_run_exit_code,
            has_dryrun,
            detection_verdict: effective_verdict.as_str().to_string(),
        })
    }

    /// Composite evasion score with per-category ranges.
    ///
    /// | Category               | Range     |
    /// |------------------------|-----------|
    /// | RealDetection          | 0.0–0.4   |
    /// | Flaky                  | 0.0–0.3   |
    /// | InstrumentationArtifact| 0.5–0.7   |
    /// | Evasion                | 0.6–1.0   |
    fn compute_evasion_score(
        &self,
        baseline: &RunOutcome,
        instrumented: &RunOutcome,
        category: DifferentialCategory,
    ) -> f64 {
        let timeout = self.timeout_ms.max(100 * 1000) as f64;
        let survival_ratio = (baseline.elapsed_ms / timeout).clamp(0.0, 1.0);
        let payload_reached = if baseline.exit_code == 0 { 1.0 } else { 0.0 };
        let exits_match = baseline.exit_code == instrumented.exit_code;
        let detected_match = baseline.detected == instrumented.detected;
        let behavior_match_val = if exits_match && detected_match {
            1.0
        } else {
            0.0
        };

        match category {
            DifferentialCategory::RealDetection => 0.4 * survival_ratio,
            DifferentialCategory::InstrumentationArtifact => 0.5 + 0.2 * survival_ratio,
            DifferentialCategory::Flaky => 0.3 * survival_ratio,
            DifferentialCategory::Evasion => 0.6 + 0.2 * payload_reached + 0.2 * behavior_match_val,
        }
    }
}

/// Resolve detection verdict using dryrun (clean VM) and baseline (AV VM) outcomes.
///
/// This tree never returns `Ambiguous` — dry-run always resolves ambiguity.
///
/// Decision tree:
///  1. Dryrun nonzero AND not timeout           → MutationFailed
///  2. Dryrun timeout AND !has_launched(dryrun)  → MutationFailed
///  3. Dryrun clean AND baseline clean           → Evasion
///  4. Dryrun clean AND baseline nonzero         → Detected
///  5. Both timeout AND has_launched(baseline)   → Evasion
///  6. Both timeout AND !has_launched(baseline)  → MutationFailed
///  7. Dryrun timeout+launched AND baseline !timeout:
///     baseline == 0                              → Anomaly
///     baseline != 0                              → Detected
///  8. Same nonzero exit code                    → InfraError
///  9. Different nonzero exit codes              → Detected
fn override_with_dryrun(dryrun: &RunOutcome, baseline: &RunOutcome) -> DetectionVerdict {
    let dr_timeout = dryrun.exit_code == -3; // EXIT_TIMEOUT
    let bl_timeout = baseline.exit_code == -3;
    let dr_clean = dryrun.exit_code == 0;
    let bl_clean = baseline.exit_code == 0;
    let dr_launched = has_launched(&dryrun.last_checkpoint);
    let bl_launched = has_launched(&baseline.last_checkpoint);

    // 1. Dryrun nonzero AND not timeout → artifact broken on clean VM
    if !dr_clean && !dr_timeout {
        return DetectionVerdict::MutationFailed;
    }

    // 2. Dryrun timeout AND didn't reach Launching → stalled loader = broken artifact
    if dr_timeout && !dr_launched {
        return DetectionVerdict::MutationFailed;
    }

    // 3. Both clean → evasion
    if dr_clean && bl_clean {
        return DetectionVerdict::Evasion;
    }

    // 4. Dryrun clean AND baseline nonzero → AV was the differentiator
    if dr_clean && !bl_clean {
        return DetectionVerdict::Detected;
    }

    // 5-6. Both timeout
    if dr_timeout && bl_timeout {
        return if bl_launched {
            // 5. Payload runs forever with or without AV
            DetectionVerdict::Evasion
        } else {
            // 6. Stalled identically → loader/artifact bug
            DetectionVerdict::MutationFailed
        };
    }

    // 7. Dryrun timeout+launched AND baseline not timeout
    if dr_timeout && dr_launched && !bl_timeout {
        return if bl_clean {
            // Finishes with AV but stalls without? Contradictory.
            DetectionVerdict::Anomaly
        } else {
            // AV killed what would have kept running
            DetectionVerdict::Detected
        };
    }

    // 8. Same nonzero exit code (both != 0, both not timeout at this point)
    if dryrun.exit_code == baseline.exit_code {
        return DetectionVerdict::InfraError;
    }

    // 9. Different nonzero exit codes → AV changed the failure mode
    DetectionVerdict::Detected
}

// ============================================================================
// Run: Envelope (dispatch queue item)
// ============================================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunType {
    Baseline,
    Instrumented,
    DryRun,
}

impl RunType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunType::Baseline => "baseline",
            RunType::Instrumented => "instrumented",
            RunType::DryRun => "dryrun",
        }
    }

    pub fn trace_mode(&self) -> &'static str {
        match self {
            RunType::Baseline => "off",
            RunType::Instrumented => "lines",
            RunType::DryRun => "off",
        }
    }

    pub fn is_dryrun(&self) -> bool {
        matches!(self, RunType::DryRun)
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
#[allow(dead_code)]
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

// ============================================================================
// Artifact Chunking
// ============================================================================

/// Split binary data into 4MB `ArtifactChunk` messages for gRPC streaming.
pub fn chunk_artifact(
    artifact_id: &str,
    data: &[u8],
) -> Vec<crate::automutate::common::ArtifactChunk> {
    use crate::automutate::common::ArtifactChunk;

    const CHUNK_SIZE: usize = 4 * 1024 * 1024;
    let total_chunks = data.len().div_ceil(CHUNK_SIZE);
    data.chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(i, chunk)| ArtifactChunk {
            artifact_id: artifact_id.to_string(),
            data: chunk.to_vec(),
            chunk_index: i as u32,
            total_chunks: total_chunks as u32,
            sha256: artifact_id.to_string(),
        })
        .collect()
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
            modules: ModuleSelectionSpec::default(),
            detected: false,
            behavior_match: true,
            evasion_score: 1.0,
            differential_category: DifferentialCategory::Evasion,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
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
            modules: ModuleSelectionSpec::default(),
        };

        let mut agg = RoundAgg {
            spec,
            baseline_run_id: RunId("r1-baseline".into()),
            instrumented_run_id: RunId("r1-instrumented".into()),
            baseline: None,
            instrumented: None,
            baseline_vm_id: String::new(),
            instrumented_vm_id: String::new(),
            started_at: SystemTime::now(),
            timeout_ms: 120_000,
            assembled_source: None,
            baseline_artifact_path: PathBuf::new(),
            instrumented_artifact_path: PathBuf::new(),
            dryrun_run_id: RunId("dryrun".into()),
            dryrun: None,
            dryrun_vm_id: String::new(),
            dryrun_deadline: None,
        };

        assert!(!agg.is_complete());
        assert!(agg.to_summary().is_none());

        agg.baseline = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
            success: true,
            elapsed_ms: 60_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        assert!(!agg.is_complete());

        agg.instrumented = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
            success: true,
            elapsed_ms: 62_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        assert!(agg.is_complete());

        let summary = agg.to_summary().unwrap();
        assert!(!summary.detected);
        assert!(summary.behavior_match);
        assert_eq!(summary.differential_category, DifferentialCategory::Evasion);
    }

    // ── ModularBuildSpec / ModuleSelectionSpec tests ─────────────────────────

    #[test]
    fn test_module_selection_spec_defaults() {
        let spec = ModuleSelectionSpec::default();
        assert_eq!(spec.carrier, "alloc_rw_rx");
        assert_eq!(spec.decoder, "xor");
        assert_eq!(spec.antiemulation, "none");
        assert_eq!(spec.deconditioner, "none");
        assert_eq!(spec.guardrail, "none");
        assert_eq!(spec.virtualprotect, "standard");
        assert_eq!(spec.decoy, "none");
    }

    #[test]
    fn test_modular_build_spec_serde_roundtrip() {
        let spec = ModularBuildSpec {
            modules: ModuleSelectionSpec::default(),
            payload_path: PathBuf::from("/tmp/test_payload.bin"),
            encoding: "xor".to_string(),
        };

        let json = serde_json::to_string(&spec).unwrap();
        let deserialized: ModularBuildSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.modules.carrier, "alloc_rw_rx");
        assert_eq!(deserialized.modules.decoder, "xor");
        assert_eq!(
            deserialized.payload_path,
            PathBuf::from("/tmp/test_payload.bin")
        );
        assert_eq!(deserialized.encoding, "xor");
    }

    #[test]
    fn test_module_selection_spec_serde_with_defaults() {
        // Partial JSON should fill in defaults for optional fields
        let json = r#"{"carrier":"peb_walk","decoder":"english"}"#;
        let spec: ModuleSelectionSpec = serde_json::from_str(json).unwrap();

        assert_eq!(spec.carrier, "peb_walk");
        assert_eq!(spec.decoder, "english");
        assert_eq!(
            spec.antiemulation, "none",
            "antiemulation should default to 'none'"
        );
        assert_eq!(spec.guardrail, "none", "guardrail should default to 'none'");
        assert_eq!(
            spec.virtualprotect, "standard",
            "virtualprotect should default to 'standard'"
        );
        assert_eq!(spec.decoy, "none", "decoy should default to 'none'");
    }

    #[test]
    fn test_modular_build_spec_encoding_default() {
        let json =
            r#"{"modules":{"carrier":"alloc_rw_rx","decoder":"xor"},"payload_path":"test.bin"}"#;
        let spec: ModularBuildSpec = serde_json::from_str(json).unwrap();

        assert_eq!(spec.encoding, "xor", "encoding should default to 'xor'");
    }

    // ── JobSession lifecycle edge cases ─────────────────────────────────────

    #[test]
    fn test_job_session_stop_on_evasion() {
        let mut job = JobSession::new("job-evasion", 10, test_build_spec());
        job.stop_on_evasion = true;

        let (_, rid) = job.start_round();
        // Record a round where detection was NOT triggered (evasion)
        job.record_round_summary(RoundSummary {
            round_id: rid,
            round_number: 1,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: false, // evasion
            behavior_match: true,
            evasion_score: 1.0,
            differential_category: DifferentialCategory::Evasion,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
        });

        assert!(
            !job.should_continue(),
            "Job should stop when stop_on_evasion=true and last round was evasion"
        );
    }

    #[test]
    fn test_job_session_continues_after_detection() {
        let mut job = JobSession::new("job-det", 10, test_build_spec());
        job.stop_on_evasion = true;

        let (_, rid) = job.start_round();
        job.record_round_summary(RoundSummary {
            round_id: rid,
            round_number: 1,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: true, // detected
            behavior_match: true,
            evasion_score: 0.0,
            differential_category: DifferentialCategory::RealDetection,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
        });

        assert!(
            job.should_continue(),
            "Job should continue when last round was detected (not evasion)"
        );
    }

    #[test]
    fn test_job_session_max_rounds_reached() {
        let mut job = JobSession::new("job-max", 2, test_build_spec());

        let (_, rid1) = job.start_round();
        job.record_round_summary(RoundSummary {
            round_id: rid1,
            round_number: 1,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: true,
            behavior_match: true,
            evasion_score: 0.0,
            differential_category: DifferentialCategory::RealDetection,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
        });

        let (_, rid2) = job.start_round();
        job.record_round_summary(RoundSummary {
            round_id: rid2,
            round_number: 2,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: true,
            behavior_match: true,
            evasion_score: 0.0,
            differential_category: DifferentialCategory::RealDetection,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
        });

        assert!(
            !job.should_continue(),
            "Job should stop when max_rounds reached"
        );
    }

    #[test]
    fn test_job_session_round_id_format() {
        let mut job = JobSession::new("test-job-42", 5, test_build_spec());

        let (num1, rid1) = job.start_round();
        assert_eq!(num1, 1);
        assert_eq!(rid1.0, "test-job-42-round-1");

        let (num2, rid2) = job.start_round();
        assert_eq!(num2, 2);
        assert_eq!(rid2.0, "test-job-42-round-2");
    }

    #[test]
    fn test_job_session_to_info() {
        let mut job = JobSession::new("info-job", 5, test_build_spec());
        job.mark_started();
        job.start_round();

        let info = job.to_info(JobStatus::Running);
        assert_eq!(info.id.0, "info-job");
        assert_eq!(info.status, JobStatus::Running);
        assert_eq!(info.current_round, 1);
        assert_eq!(info.max_rounds, 5);
        assert!(info.started_at.is_some());
    }

    // ── RoundAgg differential protocol tests ────────────────────────────────

    #[test]
    fn test_round_agg_detected_baseline_only() {
        let spec = RoundSpec {
            id: RoundId("r2".into()),
            job_id: JobId("j1".into()),
            round_number: 1,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
        };
        let agg = RoundAgg {
            spec,
            baseline_run_id: RunId("b".into()),
            instrumented_run_id: RunId("i".into()),
            baseline: Some(RunOutcome {
                detected: true,
                exit_code: 1,
                error: None,
                success: false,
                elapsed_ms: 5_000.0,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            }),
            instrumented: Some(RunOutcome {
                detected: false,
                exit_code: 0,
                error: None,
                success: true,
                elapsed_ms: 60_000.0,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            }),
            baseline_vm_id: String::new(),
            instrumented_vm_id: String::new(),
            started_at: SystemTime::now(),
            timeout_ms: 120_000,
            assembled_source: None,
            baseline_artifact_path: PathBuf::new(),
            instrumented_artifact_path: PathBuf::new(),
            dryrun_run_id: RunId("dryrun".into()),
            dryrun: None,
            dryrun_vm_id: String::new(),
            dryrun_deadline: None,
        };

        let summary = agg.to_summary().unwrap();
        // Per differential protocol: baseline only detected → Flaky (not trustworthy)
        assert_eq!(summary.differential_category, DifferentialCategory::Flaky);
        assert!(!summary.detected, "Flaky should not count as detected");
        // Different exit codes + different detected → behavior mismatch
        assert!(
            !summary.behavior_match,
            "Different exit codes → behavior mismatch"
        );
        // Flaky score = 0.3 * survival_ratio (5000/120000 ≈ 0.042)
        assert!(summary.evasion_score < 0.3, "Flaky score should be < 0.3");
    }

    #[test]
    fn test_round_agg_detected_instrumented_only() {
        let spec = RoundSpec {
            id: RoundId("r3".into()),
            job_id: JobId("j1".into()),
            round_number: 1,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
        };
        let agg = RoundAgg {
            spec,
            baseline_run_id: RunId("b".into()),
            instrumented_run_id: RunId("i".into()),
            baseline: Some(RunOutcome {
                detected: false,
                exit_code: 0,
                error: None,
                success: true,
                elapsed_ms: 100_000.0,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            }),
            instrumented: Some(RunOutcome {
                detected: true,
                exit_code: 1,
                error: None,
                success: false,
                elapsed_ms: 15_000.0,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            }),
            baseline_vm_id: String::new(),
            instrumented_vm_id: String::new(),
            started_at: SystemTime::now(),
            timeout_ms: 120_000,
            assembled_source: None,
            baseline_artifact_path: PathBuf::new(),
            instrumented_artifact_path: PathBuf::new(),
            dryrun_run_id: RunId("dryrun".into()),
            dryrun: None,
            dryrun_vm_id: String::new(),
            dryrun_deadline: None,
        };

        let summary = agg.to_summary().unwrap();
        // Instrumented detected but baseline not → InstrumentationArtifact
        // FIX: This should NOT count as detected (was the bug)
        assert_eq!(
            summary.differential_category,
            DifferentialCategory::InstrumentationArtifact
        );
        assert!(
            !summary.detected,
            "InstrumentationArtifact should NOT be marked as detected"
        );
        // InstrumentationArtifact score = 0.5 + 0.2*survival_ratio, in [0.5, 0.7]
        assert!(
            summary.evasion_score >= 0.5 && summary.evasion_score <= 0.7,
            "InstrumentationArtifact score should be in [0.5, 0.7], got {}",
            summary.evasion_score
        );
    }

    #[test]
    fn test_round_agg_full_evasion() {
        let spec = RoundSpec {
            id: RoundId("r4".into()),
            job_id: JobId("j1".into()),
            round_number: 1,
            mutations: vec![MutationSpec {
                id: "ast.string_xor".into(),
                params: None,
            }],
            modules: ModuleSelectionSpec::default(),
        };
        let agg = RoundAgg {
            spec,
            baseline_run_id: RunId("b".into()),
            instrumented_run_id: RunId("i".into()),
            baseline: Some(RunOutcome {
                detected: false,
                exit_code: 0,
                error: None,
                success: true,
                elapsed_ms: 120_000.0,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            }),
            instrumented: Some(RunOutcome {
                detected: false,
                exit_code: 0,
                error: None,
                success: true,
                elapsed_ms: 118_000.0,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            }),
            baseline_vm_id: String::new(),
            instrumented_vm_id: String::new(),
            started_at: SystemTime::now(),
            timeout_ms: 120_000,
            assembled_source: None,
            baseline_artifact_path: PathBuf::new(),
            instrumented_artifact_path: PathBuf::new(),
            dryrun_run_id: RunId("dryrun".into()),
            dryrun: None,
            dryrun_vm_id: String::new(),
            dryrun_deadline: None,
        };

        let summary = agg.to_summary().unwrap();
        assert!(!summary.detected);
        assert!(summary.behavior_match);
        assert_eq!(summary.differential_category, DifferentialCategory::Evasion);
        // Evasion: 0.6 + 0.2 (payload_reached: exit_code==0) + 0.2 (behavior_match) = 1.0
        assert_eq!(summary.evasion_score, 1.0, "Full evasion → score 1.0");
        assert_eq!(summary.mutations, vec!["ast.string_xor"]);
    }

    // ── RunType tests ───────────────────────────────────────────────────────

    #[test]
    fn test_run_type_trace_modes() {
        assert_eq!(RunType::Baseline.trace_mode(), "off");
        assert_eq!(RunType::Instrumented.trace_mode(), "lines");
        assert_eq!(RunType::Baseline.as_str(), "baseline");
        assert_eq!(RunType::Instrumented.as_str(), "instrumented");
    }

    // ── RunEnvelope / ArtifactRef serde ─────────────────────────────────────

    #[test]
    fn test_run_envelope_serde_roundtrip() {
        let envelope = RunEnvelope {
            run_id: RunId("run-1".into()),
            job_id: JobId("job-1".into()),
            round_id: RoundId("r-1".into()),
            round_number: 3,
            run_type: RunType::Baseline,
            artifact: ArtifactRef {
                path: PathBuf::from("/artifacts/abc123.exe"),
                sha256: Some("deadbeef".into()),
            },
            mutations: vec!["ast.string_xor".into()],
            timeout_seconds: 60,
            required_os: "windows10".into(),
            required_capabilities: vec!["defender".into()],
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let deserialized: RunEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.run_id.0, "run-1");
        assert_eq!(deserialized.round_number, 3);
        assert_eq!(deserialized.run_type, RunType::Baseline);
        assert_eq!(deserialized.artifact.sha256, Some("deadbeef".into()));
        assert_eq!(deserialized.mutations, vec!["ast.string_xor"]);
        assert_eq!(deserialized.timeout_seconds, 60);
    }

    // ── JobOutcome tests ────────────────────────────────────────────────────

    #[test]
    fn test_job_outcome_status_mapping() {
        assert_eq!(
            JobOutcome::Completed {
                rounds_completed: 5
            }
            .to_status(),
            JobStatus::Completed
        );
        assert_eq!(
            JobOutcome::Stopped {
                reason: "user".into()
            }
            .to_status(),
            JobStatus::Stopped
        );
        assert_eq!(
            JobOutcome::Failed {
                error: "oops".into()
            }
            .to_status(),
            JobStatus::Failed
        );
    }

    // ── Payload I/O with tempfile ───────────────────────────────────────────

    #[test]
    fn test_payload_path_read_success() {
        let payload_bytes = vec![0x90u8; 256];

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, &payload_bytes).unwrap();

        let spec = ModularBuildSpec {
            modules: ModuleSelectionSpec::default(),
            payload_path: tmp.path().to_path_buf(),
            encoding: "xor".to_string(),
        };

        // Simulate what job_worker does: read payload from path
        let read_bytes = std::fs::read(&spec.payload_path).unwrap();
        assert_eq!(read_bytes, payload_bytes);
    }

    #[test]
    fn test_payload_path_file_not_found() {
        let spec = ModularBuildSpec {
            modules: ModuleSelectionSpec::default(),
            payload_path: PathBuf::from("/nonexistent/path/payload.bin"),
            encoding: "xor".to_string(),
        };

        let result = std::fs::read(&spec.payload_path);
        assert!(result.is_err(), "Reading nonexistent payload should fail");
    }

    #[test]
    fn test_payload_path_empty_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // Don't write anything — file is empty

        let read_bytes = std::fs::read(tmp.path()).unwrap();
        assert!(read_bytes.is_empty(), "Empty file should read as empty vec");
    }

    #[test]
    fn test_payload_path_large_file() {
        let payload_bytes = vec![0xCCu8; 1_000_000]; // 1MB

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, &payload_bytes).unwrap();

        let read_bytes = std::fs::read(tmp.path()).unwrap();
        assert_eq!(read_bytes.len(), 1_000_000);
        assert_eq!(read_bytes[0], 0xCC);
    }

    // ── DifferentialCategory tests ───────────────────────────────────────────

    #[test]
    fn test_differential_category_from_runs() {
        assert_eq!(
            DifferentialCategory::from_runs(true, true),
            DifferentialCategory::RealDetection
        );
        assert_eq!(
            DifferentialCategory::from_runs(false, true),
            DifferentialCategory::InstrumentationArtifact
        );
        assert_eq!(
            DifferentialCategory::from_runs(true, false),
            DifferentialCategory::Flaky
        );
        assert_eq!(
            DifferentialCategory::from_runs(false, false),
            DifferentialCategory::Evasion
        );
    }

    #[test]
    fn test_differential_category_is_detected() {
        assert!(DifferentialCategory::RealDetection.is_detected());
        assert!(!DifferentialCategory::InstrumentationArtifact.is_detected());
        assert!(!DifferentialCategory::Flaky.is_detected());
        assert!(!DifferentialCategory::Evasion.is_detected());
    }

    #[test]
    fn test_differential_category_is_trustworthy() {
        assert!(DifferentialCategory::RealDetection.is_trustworthy());
        assert!(!DifferentialCategory::InstrumentationArtifact.is_trustworthy());
        assert!(!DifferentialCategory::Flaky.is_trustworthy());
        assert!(DifferentialCategory::Evasion.is_trustworthy());
    }

    #[test]
    fn test_differential_category_as_str() {
        assert_eq!(
            DifferentialCategory::RealDetection.as_str(),
            "real_detection"
        );
        assert_eq!(
            DifferentialCategory::InstrumentationArtifact.as_str(),
            "instrumentation_artifact"
        );
        assert_eq!(DifferentialCategory::Flaky.as_str(), "flaky");
        assert_eq!(DifferentialCategory::Evasion.as_str(), "evasion");
    }

    #[test]
    fn test_differential_category_real_detection_score() {
        let spec = RoundSpec {
            id: RoundId("r-rd".into()),
            job_id: JobId("j1".into()),
            round_number: 1,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
        };
        let agg = RoundAgg {
            spec,
            baseline_run_id: RunId("b".into()),
            instrumented_run_id: RunId("i".into()),
            baseline: Some(RunOutcome {
                detected: true,
                exit_code: -2,
                error: None,
                success: false,
                elapsed_ms: 50_000.0,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            }),
            instrumented: Some(RunOutcome {
                detected: true,
                exit_code: -2,
                error: None,
                success: false,
                elapsed_ms: 48_000.0,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            }),
            baseline_vm_id: String::new(),
            instrumented_vm_id: String::new(),
            started_at: SystemTime::now(),
            timeout_ms: 120_000,
            assembled_source: None,
            baseline_artifact_path: PathBuf::new(),
            instrumented_artifact_path: PathBuf::new(),
            dryrun_run_id: RunId("dryrun".into()),
            dryrun: None,
            dryrun_vm_id: String::new(),
            dryrun_deadline: None,
        };

        let summary = agg.to_summary().unwrap();
        assert_eq!(
            summary.differential_category,
            DifferentialCategory::RealDetection
        );
        assert!(summary.detected);
        // Score = 0.4 * (50/120) ≈ 0.167
        assert!(
            summary.evasion_score >= 0.0 && summary.evasion_score <= 0.4,
            "RealDetection score should be in [0.0, 0.4], got {}",
            summary.evasion_score
        );
    }

    #[test]
    fn test_evasion_score_gradient_on_detection() {
        // Quick kill (2s) vs slow kill (100s) should give different scores
        let make_agg = |elapsed: f64| -> RoundAgg {
            RoundAgg {
                spec: RoundSpec {
                    id: RoundId("r".into()),
                    job_id: JobId("j".into()),
                    round_number: 1,
                    mutations: vec![],
                    modules: ModuleSelectionSpec::default(),
                },
                baseline_run_id: RunId("b".into()),
                instrumented_run_id: RunId("i".into()),
                baseline: Some(RunOutcome {
                    detected: true,
                    exit_code: -2,
                    error: None,
                    success: false,
                    elapsed_ms: elapsed,
                    detection_verdict: String::new(),
                    last_checkpoint: String::new(),
                }),
                instrumented: Some(RunOutcome {
                    detected: true,
                    exit_code: -2,
                    error: None,
                    success: false,
                    elapsed_ms: elapsed,
                    detection_verdict: String::new(),
                    last_checkpoint: String::new(),
                }),
                baseline_vm_id: String::new(),
                instrumented_vm_id: String::new(),
                started_at: SystemTime::now(),
                timeout_ms: 120_000,
                assembled_source: None,
                baseline_artifact_path: PathBuf::new(),
                instrumented_artifact_path: PathBuf::new(),
                dryrun_run_id: RunId("dryrun".into()),
                dryrun: None,
                dryrun_vm_id: String::new(),
                dryrun_deadline: None,
            }
        };

        let quick_kill = make_agg(2_000.0).to_summary().unwrap();
        let slow_kill = make_agg(100_000.0).to_summary().unwrap();

        assert!(
            slow_kill.evasion_score > quick_kill.evasion_score,
            "Slow kill ({:.3}) should score higher than quick kill ({:.3})",
            slow_kill.evasion_score,
            quick_kill.evasion_score
        );

        // Quick kill: 0.4 * (2000/120000) ≈ 0.007
        assert!(quick_kill.evasion_score < 0.05);
        // Slow kill: 0.4 * (100000/120000) ≈ 0.333
        assert!(slow_kill.evasion_score > 0.3);
    }

    #[test]
    fn test_stop_on_evasion_does_not_stop_on_instrumentation_artifact() {
        let mut job = JobSession::new("job-instr-artifact", 10, test_build_spec());
        job.stop_on_evasion = true;

        let (_, rid) = job.start_round();
        // InstrumentationArtifact: baseline clean, instrumented detected
        job.record_round_summary(RoundSummary {
            round_id: rid,
            round_number: 1,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: false,
            behavior_match: false,
            evasion_score: 0.6,
            differential_category: DifferentialCategory::InstrumentationArtifact,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
        });

        assert!(
            job.should_continue(),
            "Job should NOT stop on InstrumentationArtifact even with stop_on_evasion=true"
        );
    }
}
