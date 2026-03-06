//! Detection outcome classifier (v3).
//!
//! Produces provisional `DetectionVerdict` values from local signals only.
//! No dry-run context — that override happens controller-side in `override_with_dryrun()`.
//!
//! Key changes from v2:
//! - Verdicts answer "was it detected?" clearly; ambiguity is explicit.
//! - `Ambiguous` replaces `Crashed` and catch-all `KilledPre/PostPayload` for
//!   crash NTSTATUS, carrier codes 30-39, and unknown nonzero exits.
//! - `Stalled` replaces `TimeoutIdle` for timeout without Launching checkpoint.
//! - ETW recency logic removed from verdict decision (kept for logging if desired).
//! - `dry_run_exit_code` and `elapsed_ms` parameters removed from `classify_run()`.

pub use automutate_common::DetectionVerdict;

use crate::automutate::common::TelemetryData;
use crate::execution::types::{EXIT_INFRA, EXIT_NO_CODE, EXIT_TIMEOUT, EXIT_WAIT_FAILED};
use automutate_common::has_launched;

/// Known AV/EDR NTSTATUS termination codes.
const AV_NTSTATUS_CODES: &[i32] = &[
    0xC0000906_u32 as i32, // STATUS_VIRUS_INFECTED
    0xC0000907_u32 as i32, // STATUS_VIRUS_DELETED
];

/// Known crash NTSTATUS codes.
const CRASH_NTSTATUS_CODES: &[i32] = &[
    0xC0000005_u32 as i32, // STATUS_ACCESS_VIOLATION
    0xC0000409_u32 as i32, // STATUS_STACK_BUFFER_OVERRUN
    0xC00000FD_u32 as i32, // STATUS_STACK_OVERFLOW
    0xC0000374_u32 as i32, // STATUS_HEAP_CORRUPTION
    0xC0000094_u32 as i32, // STATUS_INTEGER_DIVIDE_BY_ZERO
];

/// Simplified classification evidence.
struct ClassificationEvidence {
    exit_code: i32,
    timed_out: bool,
    has_launched: bool,
}

/// Extract classification evidence from telemetry events.
///
/// Scans checkpoint events to determine if the artifact reached the Launching
/// checkpoint or beyond, and what the last checkpoint was.
fn extract_evidence(telemetry_events: &[TelemetryData]) -> (bool, Option<String>) {
    let mut launched = false;
    let mut last_checkpoint: Option<String> = None;

    for event in telemetry_events {
        match event.event_type.as_str() {
            "checkpoint" | "artifact_success" | "artifact_failure" => {
                if let Some(crate::automutate::common::telemetry_data::TypedEvent::Checkpoint(
                    ref cp,
                )) = event.typed_event
                {
                    let name = &cp.name;
                    last_checkpoint = Some(name.clone());

                    if has_launched(name) {
                        launched = true;
                    }
                }
            }
            _ => {}
        }
    }

    (launched, last_checkpoint)
}

/// Classify the detection outcome using the v3 decision tree.
///
/// Decision order:
///  1. EXIT_INFRA (-4)                             → InfraError
///  2. EXIT_WAIT_FAILED (-1)                       → InfraError
///  3. exit_code 10-19 (guardrail)                 → InfraError
///  4. exit_code == 0                              → Evasion
///  5. timed_out + has_launched                     → Evasion
///  6. timed_out + !has_launched                    → Stalled
///  7. EXIT_NO_CODE (-2)                           → Detected
///  8. AV NTSTATUS (0xC0000906, 0xC0000907)       → Detected
///  9. Crash NTSTATUS (0xC0000005, etc.)           → Ambiguous
/// 10. exit_code 30-39 (carrier codes)             → Ambiguous
/// 11. Other nonzero                               → Ambiguous
fn classify_outcome(ev: &ClassificationEvidence) -> DetectionVerdict {
    // Infrastructure error (process never executed)
    if ev.exit_code == EXIT_INFRA {
        return DetectionVerdict::InfraError;
    }

    // Wait failed (spawn-level failure)
    if ev.exit_code == EXIT_WAIT_FAILED {
        return DetectionVerdict::InfraError;
    }

    // Guardrail rejection (exit codes 10-19)
    if (10..20).contains(&ev.exit_code) {
        return DetectionVerdict::InfraError;
    }

    // Clean exit (code 0)
    if ev.exit_code == 0 {
        return DetectionVerdict::Evasion;
    }

    // Timeout paths
    if ev.timed_out {
        return if ev.has_launched {
            DetectionVerdict::Evasion
        } else {
            DetectionVerdict::Stalled
        };
    }

    // EXIT_TIMEOUT without timed_out flag (defensive consistency check)
    if ev.exit_code == EXIT_TIMEOUT {
        return if ev.has_launched {
            DetectionVerdict::Evasion
        } else {
            DetectionVerdict::Stalled
        };
    }

    // EXIT_NO_CODE — process was externally terminated (no exit code)
    if ev.exit_code == EXIT_NO_CODE {
        return DetectionVerdict::Detected;
    }

    // Known AV NTSTATUS codes
    if AV_NTSTATUS_CODES.contains(&ev.exit_code) {
        return DetectionVerdict::Detected;
    }

    // Crash NTSTATUS codes — could be AV or genuine crash
    if CRASH_NTSTATUS_CODES.contains(&ev.exit_code) {
        return DetectionVerdict::Ambiguous;
    }

    // Carrier exit codes (30-39) — AV can block VirtualAlloc/VirtualProtect
    if (30..40).contains(&ev.exit_code) {
        return DetectionVerdict::Ambiguous;
    }

    // Unknown nonzero exit code — default to ambiguous
    DetectionVerdict::Ambiguous
}

/// Public entry point: classify a run using exit code, timeout, and telemetry.
///
/// Returns `(verdict, last_checkpoint)`.
pub fn classify_run(
    exit_code: i32,
    timed_out: bool,
    telemetry_events: &[TelemetryData],
) -> (DetectionVerdict, Option<String>) {
    let (launched, last_checkpoint) = extract_evidence(telemetry_events);

    let evidence = ClassificationEvidence {
        exit_code,
        timed_out,
        has_launched: launched,
    };

    let verdict = classify_outcome(&evidence);
    (verdict, last_checkpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build ClassificationEvidence for testing.
    fn evidence(exit_code: i32, timed_out: bool, launched: bool) -> ClassificationEvidence {
        ClassificationEvidence {
            exit_code,
            timed_out,
            has_launched: launched,
        }
    }

    // ── EXIT_INFRA ────────────────────────────────────────────────────────

    #[test]
    fn test_exit_infra_is_infra_error() {
        let ev = evidence(EXIT_INFRA, false, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::InfraError);
        assert!(!DetectionVerdict::InfraError.is_detected());
    }

    // ── EXIT_WAIT_FAILED ──────────────────────────────────────────────────

    #[test]
    fn test_exit_wait_failed_is_infra_error() {
        let ev = evidence(EXIT_WAIT_FAILED, false, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::InfraError);
    }

    // ── Guardrail rejection ───────────────────────────────────────────────

    #[test]
    fn test_guardrail_exit_codes_are_infra_error() {
        for code in 10..20 {
            let ev = evidence(code, false, false);
            assert_eq!(
                classify_outcome(&ev),
                DetectionVerdict::InfraError,
                "exit_code={} should be InfraError",
                code
            );
        }
    }

    // ── Clean exit ────────────────────────────────────────────────────────

    #[test]
    fn test_clean_exit_is_evasion() {
        let ev = evidence(0, false, true);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Evasion);
        assert!(!DetectionVerdict::Evasion.is_detected());
    }

    // ── Timeout paths ────────────────────────────────────────────────────

    #[test]
    fn test_timeout_with_launching_is_evasion() {
        let ev = evidence(EXIT_TIMEOUT, true, true);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Evasion);
    }

    #[test]
    fn test_timeout_without_launching_is_stalled() {
        let ev = evidence(EXIT_TIMEOUT, true, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Stalled);
        assert!(!DetectionVerdict::Stalled.is_detected());
    }

    // ── EXIT_TIMEOUT without timed_out flag (defensive) ─────────────────

    #[test]
    fn test_exit_timeout_without_timed_out_flag_launched() {
        let ev = evidence(EXIT_TIMEOUT, false, true);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Evasion);
    }

    #[test]
    fn test_exit_timeout_without_timed_out_flag_not_launched() {
        let ev = evidence(EXIT_TIMEOUT, false, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Stalled);
    }

    // ── EXIT_NO_CODE ──────────────────────────────────────────────────────

    #[test]
    fn test_exit_no_code_is_detected() {
        let ev = evidence(EXIT_NO_CODE, false, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Detected);
        assert!(DetectionVerdict::Detected.is_detected());
    }

    #[test]
    fn test_exit_no_code_with_launching_is_detected() {
        let ev = evidence(EXIT_NO_CODE, false, true);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Detected);
    }

    // ── AV NTSTATUS codes ─────────────────────────────────────────────────

    #[test]
    fn test_av_ntstatus_virus_infected_is_detected() {
        let ev = evidence(0xC0000906_u32 as i32, false, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Detected);
    }

    #[test]
    fn test_av_ntstatus_virus_deleted_is_detected() {
        let ev = evidence(0xC0000907_u32 as i32, false, true);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Detected);
    }

    // ── Crash NTSTATUS → Ambiguous ────────────────────────────────────────

    #[test]
    fn test_crash_ntstatus_is_ambiguous() {
        let ev = evidence(0xC0000005_u32 as i32, false, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Ambiguous);
        assert!(DetectionVerdict::Ambiguous.is_detected()); // conservative
    }

    #[test]
    fn test_stack_buffer_overrun_is_ambiguous() {
        let ev = evidence(0xC0000409_u32 as i32, false, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Ambiguous);
    }

    // ── Carrier exit codes (30-39) → Ambiguous ────────────────────────────

    #[test]
    fn test_carrier_exit_codes_are_ambiguous() {
        for &code in &[30, 31, 32, 33, 34, 35, 39] {
            let ev = evidence(code, false, false);
            assert_eq!(
                classify_outcome(&ev),
                DetectionVerdict::Ambiguous,
                "exit_code={} should be Ambiguous",
                code
            );
        }
    }

    // ── Unknown nonzero → Ambiguous ───────────────────────────────────────

    #[test]
    fn test_unknown_exit_code_is_ambiguous() {
        let ev = evidence(42, false, false);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Ambiguous);
    }

    #[test]
    fn test_unknown_exit_code_with_launching_is_ambiguous() {
        let ev = evidence(42, false, true);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Ambiguous);
    }

    // ── Verdict methods ───────────────────────────────────────────────────

    #[test]
    fn test_verdict_roundtrip() {
        let verdicts = [
            DetectionVerdict::Evasion,
            DetectionVerdict::Detected,
            DetectionVerdict::Ambiguous,
            DetectionVerdict::Stalled,
            DetectionVerdict::InfraError,
            DetectionVerdict::MutationFailed,
            DetectionVerdict::Anomaly,
        ];
        for v in &verdicts {
            let s = v.as_str();
            let parsed = DetectionVerdict::from_verdict_str(s);
            assert_eq!(parsed, Some(*v), "Roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn test_backward_compat_old_strings() {
        // Old strings map to new variants
        assert_eq!(
            DetectionVerdict::from_verdict_str("killed_pre_payload"),
            Some(DetectionVerdict::Detected)
        );
        assert_eq!(
            DetectionVerdict::from_verdict_str("killed_post_payload"),
            Some(DetectionVerdict::Detected)
        );
        assert_eq!(
            DetectionVerdict::from_verdict_str("crashed"),
            Some(DetectionVerdict::Ambiguous)
        );
        assert_eq!(
            DetectionVerdict::from_verdict_str("timeout_active"),
            Some(DetectionVerdict::Evasion)
        );
        assert_eq!(
            DetectionVerdict::from_verdict_str("timeout_idle"),
            Some(DetectionVerdict::Evasion)
        );
        assert_eq!(
            DetectionVerdict::from_verdict_str("clean_exit"),
            Some(DetectionVerdict::Evasion)
        );
    }

    #[test]
    fn test_is_detected_semantics() {
        assert!(!DetectionVerdict::Evasion.is_detected());
        assert!(DetectionVerdict::Detected.is_detected());
        assert!(DetectionVerdict::Ambiguous.is_detected()); // conservative
        assert!(!DetectionVerdict::Stalled.is_detected());
        assert!(!DetectionVerdict::InfraError.is_detected());
        assert!(!DetectionVerdict::MutationFailed.is_detected());
        assert!(!DetectionVerdict::Anomaly.is_detected());
    }
}
