//! Round types: spec, aggregation, differential protocol, and summary.
//!
//! A *round* is the atomic unit of the experiment loop. Each round produces
//! three correlated runs (baseline, instrumented, dryrun), aggregates their
//! outcomes via [`RoundAgg`], and yields a [`RoundSummary`] that feeds back
//! into the [`Selector`](crate::triage::Selector).

use automutate_common::{DetectionVerdict, has_launched};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use super::config::ModuleSelectionSpec;
use super::ids::{JobId, RoundId, RunId};

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
    /// Dryrun crash — artifact broken (has mutations, or no mutations + didn't launch)
    MutationFailed,
    /// Dryrun crash — .bin payload broken (no mutations, reached payload execution)
    PayloadFailed,
    /// Defender static file scan detected the artifact before execution
    StaticDetection,
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

    /// True for RealDetection and StaticDetection — trustworthy "detected" signals.
    pub fn is_detected(self) -> bool {
        matches!(self, Self::RealDetection | Self::StaticDetection)
    }

    /// True for categories where the result is trustworthy for the feedback loop.
    /// Used by future token scorer / mutation selector (FEEDBACK-LOOP-PLAN.md).
    #[allow(dead_code)]
    pub fn is_trustworthy(self) -> bool {
        matches!(
            self,
            Self::RealDetection | Self::Evasion | Self::StaticDetection
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RealDetection => "real_detection",
            Self::InstrumentationArtifact => "instrumentation_artifact",
            Self::Flaky => "flaky",
            Self::Evasion => "evasion",
            Self::MutationFailed => "mutation_failed",
            Self::PayloadFailed => "payload_failed",
            Self::StaticDetection => "static_detection",
        }
    }
}

impl std::fmt::Display for DifferentialCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// Mutation & Round Spec
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

// ============================================================================
// Run Outcome
// ============================================================================

/// Minimal outcome from a single run execution on a VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    /// Whether the EDR/Defender flagged the artifact.
    pub detected: bool,
    /// Process exit code returned by the worker agent.
    pub exit_code: i32,
    /// Infrastructure error message, if any (e.g. upload failure).
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

// ============================================================================
// Round Summary
// ============================================================================

/// Summary of a completed round
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundSummary {
    pub round_id: RoundId,
    pub round_number: u32,
    pub mutations: Vec<String>,
    /// Full mutation specs with params (for FuzzerSelector reconstruction).
    #[serde(default)]
    pub mutation_specs: Vec<MutationSpec>,
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
    /// Coverage percent from line trace (None until async coverage computation completes)
    #[serde(default)]
    pub coverage_percent: Option<f64>,
    /// Normalized time component of the evasion score, stored for async blended recomputation.
    /// For RealDetection/InstrumentationArtifact/Flaky: survival_ratio.
    /// For Evasion: 0.5 * payload_reached + 0.5 * behavior_match.
    #[serde(default)]
    pub time_factor: f64,
}

// ============================================================================
// Round Aggregator
// ============================================================================

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
    /// Artifact IDs and sizes for ES indexing.
    pub baseline_artifact_id: String,
    pub baseline_artifact_size: u64,
    pub instrumented_artifact_id: String,
    pub instrumented_artifact_size: u64,
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
    /// Set to true when Defender static scan detected the artifact before VM dispatch.
    /// Causes `to_summary()` to short-circuit with `StaticDetection` category.
    pub static_scan_detected: bool,
}

impl RoundAgg {
    /// Create a new RoundAgg with sensible defaults derived from the spec.
    pub fn new(spec: RoundSpec, timeout_ms: u64) -> Self {
        let round_id = &spec.id.0;
        Self {
            baseline_run_id: RunId(format!("{}-baseline", round_id)),
            instrumented_run_id: RunId(format!("{}-instrumented", round_id)),
            dryrun_run_id: RunId(format!("{}-dryrun", round_id)),
            spec,
            baseline: None,
            instrumented: None,
            baseline_vm_id: String::new(),
            instrumented_vm_id: String::new(),
            started_at: SystemTime::now(),
            timeout_ms,
            assembled_source: None,
            baseline_artifact_path: PathBuf::new(),
            instrumented_artifact_path: PathBuf::new(),
            baseline_artifact_id: String::new(),
            baseline_artifact_size: 0,
            instrumented_artifact_id: String::new(),
            instrumented_artifact_size: 0,
            dryrun: None,
            dryrun_vm_id: String::new(),
            dryrun_deadline: None,
            static_scan_detected: false,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.baseline.is_some() && self.instrumented.is_some()
    }

    /// Compute round summary from completed runs using the differential protocol.
    ///
    /// If a dryrun result is available, `override_with_dryrun()` produces the
    /// authoritative verdict. Otherwise the worker's provisional verdict is used.
    pub fn to_summary(&self) -> Option<RoundSummary> {
        // Short-circuit: Defender static scan detected the artifact before VM dispatch
        if self.static_scan_detected {
            return Some(RoundSummary {
                round_id: self.spec.id.clone(),
                round_number: self.spec.round_number,
                mutations: self.spec.mutations.iter().map(|m| m.id.clone()).collect(),
                mutation_specs: self.spec.mutations.clone(),
                modules: self.spec.modules.clone(),
                detected: true,
                behavior_match: true,
                evasion_score: 0.0,
                differential_category: DifferentialCategory::StaticDetection,
                completed_at: SystemTime::now(),
                dry_run_exit_code: None,
                has_dryrun: false,
                detection_verdict: "static_detection".to_string(),
                coverage_percent: None,
                time_factor: 0.0,
            });
        }

        let baseline = self.baseline.as_ref()?;
        let instrumented = self.instrumented.as_ref()?;

        let dry_run_exit_code = self.dryrun.as_ref().map(|d| d.exit_code);
        let has_dryrun = self.dryrun.is_some();

        // Compute authoritative verdict using dry-run if available
        let effective_verdict = if let Some(dryrun) = &self.dryrun {
            override_with_dryrun(
                dryrun,
                baseline,
                &instrumented.last_checkpoint,
            )
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

        // Short-circuit for broken artifacts: skip differential computation entirely
        let (category, detected, behavior_match, evasion_score, time_factor) = if effective_verdict
            .is_broken()
        {
            let cat = match effective_verdict {
                DetectionVerdict::PayloadFailed => DifferentialCategory::PayloadFailed,
                _ => DifferentialCategory::MutationFailed,
            };
            (cat, false, false, 0.0, 0.0)
        } else {
            let effective_baseline_detected = effective_verdict.is_detected();

            // Differential category from CLAUDE.md Section 5
            let cat =
                DifferentialCategory::from_runs(effective_baseline_detected, instrumented.detected);
            let det = cat.is_detected();

            // Behavior match: exit codes agree AND detected status agrees
            let bm = baseline.exit_code == instrumented.exit_code
                && effective_baseline_detected == instrumented.detected;

            let (es, tf) = self.compute_evasion_score(baseline, instrumented, cat);
            (cat, det, bm, es, tf)
        };

        Some(RoundSummary {
            round_id: self.spec.id.clone(),
            round_number: self.spec.round_number,
            mutations: self.spec.mutations.iter().map(|m| m.id.clone()).collect(),
            mutation_specs: self.spec.mutations.clone(),
            modules: self.spec.modules.clone(),
            detected,
            behavior_match,
            evasion_score,
            differential_category: category,
            completed_at: SystemTime::now(),
            dry_run_exit_code,
            has_dryrun,
            detection_verdict: effective_verdict.as_str().to_string(),
            coverage_percent: None,
            time_factor,
        })
    }

    /// Composite evasion score — delegates to the free function [`compute_evasion_score`].
    fn compute_evasion_score(
        &self,
        baseline: &RunOutcome,
        instrumented: &RunOutcome,
        category: DifferentialCategory,
    ) -> (f64, f64) {
        compute_evasion_score(self.timeout_ms, baseline, instrumented, category)
    }
}

/// Composite evasion score with per-category ranges.
/// Returns `(evasion_score, time_factor)` where `time_factor` is the
/// normalized time component stored for async blended recomputation.
///
/// This is the **single source of truth** for evasion score computation.
/// `RoundAgg::compute_evasion_score` delegates here.
///
/// | Category               | Range     |
/// |------------------------|-----------|
/// | RealDetection          | 0.0–0.4   |
/// | Flaky                  | 0.0–0.3   |
/// | InstrumentationArtifact| 0.5–0.7   |
/// | Evasion                | 0.6–1.0   |
pub(crate) fn compute_evasion_score(
    timeout_ms: u64,
    baseline: &RunOutcome,
    instrumented: &RunOutcome,
    category: DifferentialCategory,
) -> (f64, f64) {
    let timeout = timeout_ms.max(100 * 1000) as f64;
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
        DifferentialCategory::RealDetection => (0.4 * survival_ratio, survival_ratio),
        DifferentialCategory::InstrumentationArtifact => {
            (0.5 + 0.2 * survival_ratio, survival_ratio)
        }
        DifferentialCategory::Flaky => (0.3 * survival_ratio, survival_ratio),
        DifferentialCategory::Evasion => {
            let tf = 0.5 * payload_reached + 0.5 * behavior_match_val;
            (0.6 + 0.2 * payload_reached + 0.2 * behavior_match_val, tf)
        }
        DifferentialCategory::MutationFailed
        | DifferentialCategory::PayloadFailed
        | DifferentialCategory::StaticDetection => (0.0, 0.0),
    }
}

/// Compute blended evasion score: 70% coverage + 30% time.
///
/// Called asynchronously after coverage data becomes available.
/// The result replaces the time-only score in the selector history.
pub fn compute_blended_evasion_score(
    category: DifferentialCategory,
    coverage_percent: f64,
    time_factor: f64,
) -> f64 {
    let cov = (coverage_percent / 100.0).clamp(0.0, 1.0);
    let blend = 0.7 * cov + 0.3 * time_factor;
    match category {
        DifferentialCategory::RealDetection => 0.4 * blend,
        DifferentialCategory::InstrumentationArtifact => 0.5 + 0.2 * blend,
        DifferentialCategory::Flaky => 0.3 * blend,
        DifferentialCategory::Evasion => 0.6 + 0.4 * blend,
        DifferentialCategory::MutationFailed
        | DifferentialCategory::PayloadFailed
        | DifferentialCategory::StaticDetection => 0.0,
    }
}

/// Resolve detection verdict using dryrun (clean VM) and baseline (AV VM) outcomes.
///
/// This tree never returns `Ambiguous` — dry-run always resolves ambiguity.
///
/// `instrumented_checkpoint`: the instrumented run's `last_checkpoint` — the
/// authoritative source for how far the artifact progressed. The dryrun is a
/// bare process (no telemetry, no RedEDR) so it only provides an exit code.
/// The baseline may lack checkpoint telemetry too if AV kills early.
///
/// Decision tree:
///  1. Dryrun nonzero AND not timeout:
///     - no mutations AND launched                → PayloadFailed
///     - otherwise                                → MutationFailed
///  2. Dryrun timeout AND !launched               → MutationFailed
///  3. Dryrun clean AND baseline clean            → Evasion
///  4. Dryrun clean AND baseline nonzero          → Detected
///  5. Both timeout AND launched                  → Evasion
///  6. Both timeout AND !launched                 → MutationFailed
///  7. Dryrun timeout+launched AND baseline !timeout:
///     baseline == 0                              → Anomaly
///     baseline != 0                              → Detected
///  8. Same nonzero exit code                    → InfraError
///  9. Different nonzero exit codes              → Detected
pub(crate) fn override_with_dryrun(
    dryrun: &RunOutcome,
    baseline: &RunOutcome,
    instrumented_checkpoint: &str,
) -> DetectionVerdict {
    let dr_timeout = dryrun.exit_code == -3; // EXIT_TIMEOUT
    let bl_timeout = baseline.exit_code == -3;
    let dr_clean = dryrun.exit_code == 0;
    let bl_clean = baseline.exit_code == 0;

    // The instrumented run is the single source of truth for checkpoint progress.
    // Dryrun has no telemetry; baseline may lack it if AV kills early.
    let launched = has_launched(instrumented_checkpoint);

    // 1. Dryrun nonzero AND not timeout → artifact broken on clean VM
    if !dr_clean && !dr_timeout {
        // If instrumented run reached payload execution phase, the loader/mutations
        // worked fine — the crash is in the .bin payload itself, regardless of
        // whether mutations were applied.
        if launched {
            return DetectionVerdict::PayloadFailed;
        }
        return DetectionVerdict::MutationFailed;
    }

    // 2. Dryrun timeout AND didn't reach Launching → stalled loader = broken artifact
    if dr_timeout && !launched {
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
        return if launched {
            // 5. Payload runs forever with or without AV
            DetectionVerdict::Evasion
        } else {
            // 6. Stalled identically → loader/artifact bug
            DetectionVerdict::MutationFailed
        };
    }

    // 7. Dryrun timeout+launched AND baseline not timeout
    if dr_timeout && launched && !bl_timeout {
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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::super::config::{ModularBuildSpec, ModuleSelectionSpec};
    use super::*;
    use std::path::PathBuf;

    fn test_build_spec() -> ModularBuildSpec {
        ModularBuildSpec {
            modules: ModuleSelectionSpec::default(),
            payload_path: PathBuf::from("test.bin"),
            encoding: "xor".to_string(),
        }
    }

    fn test_round_spec(id: &str) -> RoundSpec {
        RoundSpec {
            id: RoundId(id.into()),
            job_id: JobId("j1".into()),
            round_number: 1,
            mutations: vec![],
            modules: ModuleSelectionSpec::default(),
        }
    }

    fn test_round_agg(spec: RoundSpec) -> RoundAgg {
        RoundAgg::new(spec, 120_000)
    }

    #[test]
    fn test_job_session_lifecycle() {
        use super::super::session::JobSession;
        let mut job = JobSession::new("job-1", 5, test_build_spec());
        assert!(job.should_continue());

        let (num, id) = job.start_round();
        assert_eq!(num, 1);
        assert!(id.0.contains("round-1"));

        job.record_round_summary(RoundSummary {
            round_id: id,
            round_number: 1,
            mutations: vec![],
            mutation_specs: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: false,
            behavior_match: true,
            evasion_score: 1.0,
            differential_category: DifferentialCategory::Evasion,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        });

        assert_eq!(job.current_round, 1);
    }

    #[test]
    fn test_round_agg_completion() {
        let mut agg = test_round_agg(test_round_spec("r1"));

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
        use super::super::session::JobSession;
        let mut job = JobSession::new("job-evasion", 10, test_build_spec());
        job.stop_on_evasion = true;

        let (_, rid) = job.start_round();
        job.record_round_summary(RoundSummary {
            round_id: rid,
            round_number: 1,
            mutations: vec![],
            mutation_specs: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: false,
            behavior_match: true,
            evasion_score: 1.0,
            differential_category: DifferentialCategory::Evasion,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        });

        assert!(
            !job.should_continue(),
            "Job should stop when stop_on_evasion=true and last round was evasion"
        );
    }

    #[test]
    fn test_job_session_continues_after_detection() {
        use super::super::session::JobSession;
        let mut job = JobSession::new("job-det", 10, test_build_spec());
        job.stop_on_evasion = true;

        let (_, rid) = job.start_round();
        job.record_round_summary(RoundSummary {
            round_id: rid,
            round_number: 1,
            mutations: vec![],
            mutation_specs: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: true,
            behavior_match: true,
            evasion_score: 0.0,
            differential_category: DifferentialCategory::RealDetection,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        });

        assert!(
            job.should_continue(),
            "Job should continue when last round was detected (not evasion)"
        );
    }

    #[test]
    fn test_job_session_max_rounds_reached() {
        use super::super::session::JobSession;
        let mut job = JobSession::new("job-max", 2, test_build_spec());

        let (_, rid1) = job.start_round();
        job.record_round_summary(RoundSummary {
            round_id: rid1,
            round_number: 1,
            mutations: vec![],
            mutation_specs: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: true,
            behavior_match: true,
            evasion_score: 0.0,
            differential_category: DifferentialCategory::RealDetection,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        });

        let (_, rid2) = job.start_round();
        job.record_round_summary(RoundSummary {
            round_id: rid2,
            round_number: 2,
            mutations: vec![],
            mutation_specs: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: true,
            behavior_match: true,
            evasion_score: 0.0,
            differential_category: DifferentialCategory::RealDetection,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        });

        assert!(
            !job.should_continue(),
            "Job should stop when max_rounds reached"
        );
    }

    #[test]
    fn test_job_session_round_id_format() {
        use super::super::session::JobSession;
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
        use super::super::session::{JobSession, JobStatus};
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
        let mut agg = test_round_agg(test_round_spec("r2"));
        agg.baseline = Some(RunOutcome {
            detected: true,
            exit_code: 1,
            error: None,
            success: false,
            elapsed_ms: 5_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        agg.instrumented = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
            success: true,
            elapsed_ms: 60_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });

        let summary = agg.to_summary().unwrap();
        assert_eq!(summary.differential_category, DifferentialCategory::Flaky);
        assert!(!summary.detected, "Flaky should not count as detected");
        assert!(
            !summary.behavior_match,
            "Different exit codes → behavior mismatch"
        );
        assert!(summary.evasion_score < 0.3, "Flaky score should be < 0.3");
    }

    #[test]
    fn test_round_agg_detected_instrumented_only() {
        let mut agg = test_round_agg(test_round_spec("r3"));
        agg.baseline = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
            success: true,
            elapsed_ms: 100_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        agg.instrumented = Some(RunOutcome {
            detected: true,
            exit_code: 1,
            error: None,
            success: false,
            elapsed_ms: 15_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });

        let summary = agg.to_summary().unwrap();
        assert_eq!(
            summary.differential_category,
            DifferentialCategory::InstrumentationArtifact
        );
        assert!(
            !summary.detected,
            "InstrumentationArtifact should NOT be marked as detected"
        );
        assert!(
            summary.evasion_score >= 0.5 && summary.evasion_score <= 0.7,
            "InstrumentationArtifact score should be in [0.5, 0.7], got {}",
            summary.evasion_score
        );
    }

    #[test]
    fn test_round_agg_full_evasion() {
        let mut spec = test_round_spec("r4");
        spec.mutations = vec![MutationSpec {
            id: "ast.string_xor".into(),
            params: None,
        }];
        let mut agg = test_round_agg(spec);
        agg.baseline = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
            success: true,
            elapsed_ms: 120_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        agg.instrumented = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
            success: true,
            elapsed_ms: 118_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });

        let summary = agg.to_summary().unwrap();
        assert!(!summary.detected);
        assert!(summary.behavior_match);
        assert_eq!(summary.differential_category, DifferentialCategory::Evasion);
        assert_eq!(summary.evasion_score, 1.0, "Full evasion → score 1.0");
        assert_eq!(summary.mutations, vec!["ast.string_xor"]);
    }

    // ── RunType tests ───────────────────────────────────────────────────────

    #[test]
    fn test_run_type_trace_modes() {
        use super::super::run::RunType;
        assert_eq!(RunType::Baseline.trace_mode(), "off");
        assert_eq!(RunType::Instrumented.trace_mode(), "lines");
        assert_eq!(RunType::Baseline.as_str(), "baseline");
        assert_eq!(RunType::Instrumented.as_str(), "instrumented");
    }

    // ── RunEnvelope / ArtifactRef serde ─────────────────────────────────────

    #[test]
    fn test_run_envelope_serde_roundtrip() {
        use super::super::run::{ArtifactRef, RunEnvelope, RunType};
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
        use super::super::session::{JobOutcome, JobStatus};
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
        assert!(!DifferentialCategory::MutationFailed.is_detected());
        assert!(!DifferentialCategory::PayloadFailed.is_detected());
        assert!(DifferentialCategory::StaticDetection.is_detected());
    }

    #[test]
    fn test_differential_category_is_trustworthy() {
        assert!(DifferentialCategory::RealDetection.is_trustworthy());
        assert!(!DifferentialCategory::InstrumentationArtifact.is_trustworthy());
        assert!(!DifferentialCategory::Flaky.is_trustworthy());
        assert!(DifferentialCategory::Evasion.is_trustworthy());
        assert!(!DifferentialCategory::MutationFailed.is_trustworthy());
        assert!(!DifferentialCategory::PayloadFailed.is_trustworthy());
        assert!(DifferentialCategory::StaticDetection.is_trustworthy());
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
        assert_eq!(
            DifferentialCategory::MutationFailed.as_str(),
            "mutation_failed"
        );
        assert_eq!(
            DifferentialCategory::PayloadFailed.as_str(),
            "payload_failed"
        );
        assert_eq!(
            DifferentialCategory::StaticDetection.as_str(),
            "static_detection"
        );
    }

    #[test]
    fn test_static_scan_detected_short_circuits_to_summary() {
        let mut agg = test_round_agg(test_round_spec("r-static"));
        agg.baseline_vm_id = "static_scan".to_string();
        agg.instrumented_vm_id = "static_scan".to_string();
        agg.static_scan_detected = true;

        let summary = agg.to_summary().unwrap();
        assert_eq!(
            summary.differential_category,
            DifferentialCategory::StaticDetection
        );
        assert!(summary.detected);
        assert!(summary.behavior_match);
        assert_eq!(summary.evasion_score, 0.0);
        assert_eq!(summary.detection_verdict, "static_detection");
        assert!(!summary.has_dryrun);
    }

    #[test]
    fn test_differential_category_real_detection_score() {
        let mut agg = test_round_agg(test_round_spec("r-rd"));
        agg.baseline = Some(RunOutcome {
            detected: true,
            exit_code: -2,
            error: None,
            success: false,
            elapsed_ms: 50_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        agg.instrumented = Some(RunOutcome {
            detected: true,
            exit_code: -2,
            error: None,
            success: false,
            elapsed_ms: 48_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });

        let summary = agg.to_summary().unwrap();
        assert_eq!(
            summary.differential_category,
            DifferentialCategory::RealDetection
        );
        assert!(summary.detected);
        assert!(
            summary.evasion_score >= 0.0 && summary.evasion_score <= 0.4,
            "RealDetection score should be in [0.0, 0.4], got {}",
            summary.evasion_score
        );
    }

    #[test]
    fn test_evasion_score_gradient_on_detection() {
        let make_agg = |elapsed: f64| -> RoundAgg {
            let mut agg = test_round_agg(test_round_spec("r"));
            agg.baseline = Some(RunOutcome {
                detected: true,
                exit_code: -2,
                error: None,
                success: false,
                elapsed_ms: elapsed,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            });
            agg.instrumented = Some(RunOutcome {
                detected: true,
                exit_code: -2,
                error: None,
                success: false,
                elapsed_ms: elapsed,
                detection_verdict: String::new(),
                last_checkpoint: String::new(),
            });
            agg
        };

        let quick_kill = make_agg(2_000.0).to_summary().unwrap();
        let slow_kill = make_agg(100_000.0).to_summary().unwrap();

        assert!(
            slow_kill.evasion_score > quick_kill.evasion_score,
            "Slow kill ({:.3}) should score higher than quick kill ({:.3})",
            slow_kill.evasion_score,
            quick_kill.evasion_score
        );

        assert!(quick_kill.evasion_score < 0.05);
        assert!(slow_kill.evasion_score > 0.3);
    }

    // ── Dryrun crash / PayloadFailed tests ─────────────────────────────────

    fn make_dryrun_crash_agg(
        mutations: Vec<MutationSpec>,
        dryrun_exit_code: i32,
        instrumented_last_checkpoint: &str,
    ) -> RoundAgg {
        let mut spec = test_round_spec("r-dr");
        spec.mutations = mutations;
        let mut agg = test_round_agg(spec);
        agg.baseline = Some(RunOutcome {
            detected: false,
            exit_code: -2,
            error: None,
            success: false,
            elapsed_ms: 5_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        agg.instrumented = Some(RunOutcome {
            detected: true,
            exit_code: -2,
            error: None,
            success: false,
            elapsed_ms: 5_000.0,
            detection_verdict: String::new(),
            last_checkpoint: instrumented_last_checkpoint.to_string(),
        });
        agg.dryrun = Some(RunOutcome {
            detected: false,
            exit_code: dryrun_exit_code,
            error: None,
            success: false,
            elapsed_ms: 3_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        agg
    }

    #[test]
    fn test_dryrun_crash_with_mutations_gives_mutation_failed() {
        let agg = make_dryrun_crash_agg(
            vec![MutationSpec {
                id: "ast.string_xor".into(),
                params: None,
            }],
            -1073741819,
            "",
        );

        let summary = agg.to_summary().unwrap();
        assert_eq!(
            summary.differential_category,
            DifferentialCategory::MutationFailed,
        );
        assert!(!summary.detected);
        assert!(!summary.behavior_match);
        assert_eq!(summary.evasion_score, 0.0);
        assert_eq!(summary.detection_verdict, "mutation_failed");
    }

    #[test]
    fn test_dryrun_crash_no_mutations_launched_gives_payload_failed() {
        let agg = make_dryrun_crash_agg(vec![], -1073741819, "payload_executed");

        let summary = agg.to_summary().unwrap();
        assert_eq!(
            summary.differential_category,
            DifferentialCategory::PayloadFailed,
        );
        assert!(!summary.detected);
        assert!(!summary.behavior_match);
        assert_eq!(summary.evasion_score, 0.0);
        assert_eq!(summary.detection_verdict, "payload_failed");
    }

    #[test]
    fn test_dryrun_crash_no_mutations_not_launched_gives_mutation_failed() {
        let agg = make_dryrun_crash_agg(vec![], -1073741819, "");

        let summary = agg.to_summary().unwrap();
        assert_eq!(
            summary.differential_category,
            DifferentialCategory::MutationFailed,
        );
        assert!(!summary.detected);
        assert_eq!(summary.evasion_score, 0.0);
        assert_eq!(summary.detection_verdict, "mutation_failed");
    }

    #[test]
    fn test_stop_on_evasion_does_not_stop_on_instrumentation_artifact() {
        use super::super::session::JobSession;
        let mut job = JobSession::new("job-instr-artifact", 10, test_build_spec());
        job.stop_on_evasion = true;

        let (_, rid) = job.start_round();
        job.record_round_summary(RoundSummary {
            round_id: rid,
            round_number: 1,
            mutations: vec![],
            mutation_specs: vec![],
            modules: ModuleSelectionSpec::default(),
            detected: false,
            behavior_match: false,
            evasion_score: 0.6,
            differential_category: DifferentialCategory::InstrumentationArtifact,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        });

        assert!(
            job.should_continue(),
            "Job should NOT stop on InstrumentationArtifact even with stop_on_evasion=true"
        );
    }

    // ── Blended evasion score tests ────────────────────────────────────────

    #[test]
    fn test_blended_evasion_score_real_detection_range() {
        // 0% coverage, tf=0 → 0.0
        assert_eq!(
            compute_blended_evasion_score(DifferentialCategory::RealDetection, 0.0, 0.0),
            0.0
        );
        // 100% coverage, tf=1.0 → 0.4 * (0.7 + 0.3) = 0.4
        let score = compute_blended_evasion_score(DifferentialCategory::RealDetection, 100.0, 1.0);
        assert!((score - 0.4).abs() < 1e-9, "Expected 0.4, got {}", score);
    }

    #[test]
    fn test_blended_evasion_score_evasion_range() {
        // 0% coverage, tf=0 → 0.6
        assert_eq!(
            compute_blended_evasion_score(DifferentialCategory::Evasion, 0.0, 0.0),
            0.6
        );
        // 100% coverage, tf=1.0 → 0.6 + 0.4 * 1.0 = 1.0
        let score = compute_blended_evasion_score(DifferentialCategory::Evasion, 100.0, 1.0);
        assert!((score - 1.0).abs() < 1e-9, "Expected 1.0, got {}", score);
    }

    #[test]
    fn test_blended_evasion_score_instrumentation_artifact_range() {
        let lo =
            compute_blended_evasion_score(DifferentialCategory::InstrumentationArtifact, 0.0, 0.0);
        let hi = compute_blended_evasion_score(
            DifferentialCategory::InstrumentationArtifact,
            100.0,
            1.0,
        );
        assert!((lo - 0.5).abs() < 1e-9, "Expected 0.5, got {}", lo);
        assert!((hi - 0.7).abs() < 1e-9, "Expected 0.7, got {}", hi);
    }

    #[test]
    fn test_blended_evasion_score_broken_categories() {
        assert_eq!(
            compute_blended_evasion_score(DifferentialCategory::MutationFailed, 50.0, 0.5),
            0.0
        );
        assert_eq!(
            compute_blended_evasion_score(DifferentialCategory::PayloadFailed, 80.0, 0.8),
            0.0
        );
        assert_eq!(
            compute_blended_evasion_score(DifferentialCategory::StaticDetection, 100.0, 1.0),
            0.0
        );
    }

    #[test]
    fn test_blended_score_weighting() {
        // 70% coverage, 0% time → blend = 0.7 * 0.7 = 0.49
        let score = compute_blended_evasion_score(DifferentialCategory::Evasion, 70.0, 0.0);
        let expected = 0.6 + 0.4 * (0.7 * 0.7);
        assert!(
            (score - expected).abs() < 1e-9,
            "Expected {}, got {}",
            expected,
            score
        );
    }

    #[test]
    fn test_blended_score_coverage_clamped() {
        // Coverage > 100 should be clamped
        let score = compute_blended_evasion_score(DifferentialCategory::Evasion, 150.0, 1.0);
        assert!((score - 1.0).abs() < 1e-9, "Expected 1.0, got {}", score);

        // Negative coverage should be clamped to 0
        let score = compute_blended_evasion_score(DifferentialCategory::Evasion, -10.0, 1.0);
        let expected = 0.6 + 0.4 * 0.3;
        assert!(
            (score - expected).abs() < 1e-9,
            "Expected {}, got {}",
            expected,
            score
        );
    }

    #[test]
    fn test_compute_evasion_score_returns_time_factor() {
        let mut agg = test_round_agg(test_round_spec("r-tf"));
        agg.baseline = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
            success: true,
            elapsed_ms: 120_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });
        agg.instrumented = Some(RunOutcome {
            detected: false,
            exit_code: 0,
            error: None,
            success: true,
            elapsed_ms: 118_000.0,
            detection_verdict: String::new(),
            last_checkpoint: String::new(),
        });

        let summary = agg.to_summary().unwrap();
        // Evasion with payload_reached=1.0, behavior_match=1.0 → tf = 1.0
        assert!(
            (summary.time_factor - 1.0).abs() < 1e-9,
            "Expected tf=1.0, got {}",
            summary.time_factor
        );
        assert!(summary.coverage_percent.is_none());
    }
}
