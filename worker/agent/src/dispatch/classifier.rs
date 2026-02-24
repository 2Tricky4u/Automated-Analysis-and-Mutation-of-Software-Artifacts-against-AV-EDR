//! Detection outcome classifier (v2).
//!
//! Replaces the legacy exit-code-only detection logic with a decision tree
//! that uses checkpoints, ETW event recency, and NTSTATUS codes to produce
//! fine-grained `DetectionVerdict` values.
//!
//! Key changes from v1:
//! - `CarrierBlocked` removed: without dry-run, carrier failures are
//!   indistinguishable from AV kills → `KilledPrePayload`. With dry-run
//!   confirming same error → `InfraError`.
//! - Synthetic exit codes are namespaced constants (EXIT_WAIT_FAILED, EXIT_NO_CODE,
//!   EXIT_TIMEOUT, EXIT_INFRA) defined in `types.rs`.
//! - `dry_run_exit_code` parameter added for future dry-run integration.
//! - `extract_evidence()` matches `event_type` against all checkpoint variants.
//! - `detection_outcome()` strings: `KILLED_PRE_PAYLOAD`, `KILLED_POST_PAYLOAD`.

pub use automutate_common::DetectionVerdict;

use crate::automutate::common::TelemetryData;
use crate::dispatch::types::{EXIT_INFRA, EXIT_NO_CODE, EXIT_TIMEOUT, EXIT_WAIT_FAILED};
use chrono::NaiveDateTime;

/// Fraction of total execution time: if the last ETW event falls within the
/// final `ETW_RECENCY_TAIL_FRACTION` of the execution window, the process was
/// "active" at timeout. E.g., 0.10 = last 10% of wall time.
const ETW_RECENCY_TAIL_FRACTION: f64 = 0.10;

/// Absolute floor (seconds) for the recency window, so very short runs
/// (< 5s) don't classify as idle just because the fraction is tiny.
const ETW_RECENCY_FLOOR_SECS: f64 = 0.5;

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

/// All signals available for classification.
#[allow(dead_code)]
struct ClassificationEvidence {
    exit_code: i32,
    timed_out: bool,
    dry_run_exit_code: Option<i32>,
    has_launching_checkpoint: bool,
    last_checkpoint: Option<String>,
    /// Earliest ETW event timestamp (unix seconds) from RedEDR payload `date` field.
    first_etw_ts: Option<f64>,
    /// Latest ETW event timestamp (unix seconds) from RedEDR payload `date` field.
    last_etw_ts: Option<f64>,
    /// Process wall time in seconds.
    elapsed_secs: f64,
}

/// Extract classification evidence from telemetry events.
///
/// Matches `event_type` against `"checkpoint"`, `"artifact_success"`, and
/// `"artifact_failure"` — all three carry a `CheckpointEvent` typed_event.
///
/// Returns `(has_launching, last_checkpoint, first_etw_ts, last_etw_ts)`.
fn extract_evidence(
    telemetry_events: &[TelemetryData],
) -> (bool, Option<String>, Option<f64>, Option<f64>) {
    let mut has_launching = false;
    let mut last_checkpoint: Option<String> = None;
    let mut min_etw_ts: Option<f64> = None;
    let mut max_etw_ts: Option<f64> = None;

    for event in telemetry_events {
        match event.event_type.as_str() {
            "checkpoint" | "artifact_success" | "artifact_failure" => {
                if let Some(crate::automutate::common::telemetry_data::TypedEvent::Checkpoint(
                    ref cp,
                )) = event.typed_event
                {
                    let name = &cp.name;
                    last_checkpoint = Some(name.clone());

                    if name.eq_ignore_ascii_case("Launching") {
                        has_launching = true;
                    }
                }
            }
            "etw" => {
                // Try to extract original event timestamp from payload JSON `date` field.
                // Format: "YYYY-MM-DD-HH-MM-SS"
                if let Some(source) = event.metadata.get("source")
                    && source == "rededr"
                    && let Some(ts) = parse_rededr_date(&event.payload)
                {
                    min_etw_ts = Some(match min_etw_ts {
                        Some(prev) => prev.min(ts),
                        None => ts,
                    });
                    max_etw_ts = Some(match max_etw_ts {
                        Some(prev) => prev.max(ts),
                        None => ts,
                    });
                }
            }
            _ => {}
        }
    }

    (has_launching, last_checkpoint, min_etw_ts, max_etw_ts)
}

/// Parse the `date` field from a RedEDR event payload JSON.
/// Format: "YYYY-MM-DD-HH-MM-SS" → unix timestamp (seconds, f64).
fn parse_rededr_date(payload: &[u8]) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let date_str = v.get("date")?.as_str()?;
    let dt = NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d-%H-%M-%S").ok()?;
    Some(dt.and_utc().timestamp() as f64)
}

/// Classify the detection outcome using the revised v2 decision tree.
///
/// Decision order:
/// 1. Timeout → TimeoutActive / TimeoutIdle
/// 2. EXIT_INFRA → InfraError
/// 3. exit_code == 0 → CleanExit
/// 4. dry_run_exit_code matches exit_code (non-zero) → InfraError
/// 5. EXIT_WAIT_FAILED → KilledPrePayload (conservative)
/// 6. EXIT_NO_CODE → KilledPrePayload / KilledPostPayload (checkpoint-based)
/// 7. EXIT_TIMEOUT (without timed_out) → TimeoutActive / TimeoutIdle (defensive)
/// 8. AV NTSTATUS → KilledPrePayload / KilledPostPayload
/// 9. Crash NTSTATUS → Crashed
/// 10. Otherwise → KilledPrePayload / KilledPostPayload (based on checkpoint)
fn classify_outcome(ev: &ClassificationEvidence) -> DetectionVerdict {
    // Step 1: Timeout
    if ev.timed_out {
        // Determine if the process was still generating ETW activity near the end.
        // Use the ETW event span (first..last) relative to elapsed wall time:
        // if the last event falls within the final TAIL_FRACTION of execution, it's active.
        let etw_recent = match (ev.first_etw_ts, ev.last_etw_ts) {
            (Some(first), Some(last)) if ev.elapsed_secs > 0.0 => {
                let etw_span = last - first;
                let tail_window =
                    (ev.elapsed_secs * ETW_RECENCY_TAIL_FRACTION).max(ETW_RECENCY_FLOOR_SECS);
                // Active if the last ETW event is within tail_window of the end of the ETW span
                // relative to total execution time. Simplified: was there ETW activity in the
                // final fraction of wall time?
                (ev.elapsed_secs - etw_span) < tail_window
            }
            (Some(_), Some(_)) => true, // zero elapsed but had ETW → active
            _ => false,                 // no ETW events at all → idle
        };

        return if etw_recent || ev.has_launching_checkpoint {
            DetectionVerdict::TimeoutActive
        } else {
            DetectionVerdict::TimeoutIdle
        };
    }

    // Step 2: Infrastructure error (never executed)
    if ev.exit_code == EXIT_INFRA {
        return DetectionVerdict::InfraError;
    }

    // Step 3: Clean exit
    if ev.exit_code == 0 {
        return DetectionVerdict::CleanExit;
    }

    // Step 4: Dry-run gate — matching non-zero exit code → confirmed non-AV failure
    if let Some(dry_run_code) = ev.dry_run_exit_code
        && dry_run_code == ev.exit_code
    {
        return DetectionVerdict::InfraError;
    }

    // Step 5: exit_code == EXIT_WAIT_FAILED (wait() failed)
    // Conservative: could be static detection (file quarantined before spawn)
    if ev.exit_code == EXIT_WAIT_FAILED {
        return DetectionVerdict::KilledPrePayload;
    }

    // Step 5b: EXIT_NO_CODE — process was externally terminated (no exit code)
    // Use checkpoint to distinguish pre/post payload
    if ev.exit_code == EXIT_NO_CODE {
        return if ev.has_launching_checkpoint {
            DetectionVerdict::KilledPostPayload
        } else {
            DetectionVerdict::KilledPrePayload
        };
    }

    // Step 5c: EXIT_TIMEOUT without timed_out flag (defensive — shouldn't happen)
    if ev.exit_code == EXIT_TIMEOUT {
        return if ev.has_launching_checkpoint {
            DetectionVerdict::TimeoutActive
        } else {
            DetectionVerdict::TimeoutIdle
        };
    }

    // Step 6: Known AV NTSTATUS codes
    if AV_NTSTATUS_CODES.contains(&ev.exit_code) {
        return if ev.has_launching_checkpoint {
            DetectionVerdict::KilledPostPayload
        } else {
            DetectionVerdict::KilledPrePayload
        };
    }

    // Step 7: Crash NTSTATUS codes
    if CRASH_NTSTATUS_CODES.contains(&ev.exit_code) {
        return DetectionVerdict::Crashed;
    }

    // Step 8: Unknown nonzero exit code
    if ev.has_launching_checkpoint {
        DetectionVerdict::KilledPostPayload
    } else {
        DetectionVerdict::KilledPrePayload
    }
}

/// Public entry point: classify a run using exit code, timeout, elapsed time, and telemetry.
///
/// `dry_run_exit_code`: If the artifact was previously run without AV and produced
/// this exit code, pass it here. If `Some(x)` and `x == exit_code`, the classifier
/// returns `InfraError` (artifact fails identically without AV → not a detection).
///
/// Returns `(verdict, last_checkpoint)`.
pub fn classify_run(
    exit_code: i32,
    timed_out: bool,
    elapsed_ms: f64,
    telemetry_events: &[TelemetryData],
    dry_run_exit_code: Option<i32>,
) -> (DetectionVerdict, Option<String>) {
    let (has_launching, last_checkpoint, first_etw_ts, last_etw_ts) =
        extract_evidence(telemetry_events);

    let evidence = ClassificationEvidence {
        exit_code,
        timed_out,
        dry_run_exit_code,
        has_launching_checkpoint: has_launching,
        last_checkpoint: last_checkpoint.clone(),
        first_etw_ts,
        last_etw_ts,
        elapsed_secs: elapsed_ms / 1000.0,
    };

    let verdict = classify_outcome(&evidence);
    (verdict, last_checkpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a ClassificationEvidence for testing the decision tree directly.
    ///
    /// `first_etw_ts` and `last_etw_ts` are absolute timestamps (seconds).
    /// `elapsed_secs` is total wall time.
    fn evidence(
        exit_code: i32,
        timed_out: bool,
        has_launching: bool,
        dry_run_exit_code: Option<i32>,
        etw_range: Option<(f64, f64)>,
        elapsed_secs: f64,
    ) -> ClassificationEvidence {
        ClassificationEvidence {
            exit_code,
            timed_out,
            dry_run_exit_code,
            has_launching_checkpoint: has_launching,
            last_checkpoint: None,
            first_etw_ts: etw_range.map(|(f, _)| f),
            last_etw_ts: etw_range.map(|(_, l)| l),
            elapsed_secs,
        }
    }

    // ── Step 1: Timeout paths ─────────────────────────────────────────────

    #[test]
    fn test_timeout_active_etw_spans_full_execution() {
        // ETW events span 0..30s out of 30s execution → last 10% (3s) has activity → active
        let ev = evidence(EXIT_TIMEOUT, true, false, None, Some((0.0, 30.0)), 30.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::TimeoutActive);
        assert!(!DetectionVerdict::TimeoutActive.is_detected());
    }

    #[test]
    fn test_timeout_active_etw_near_end() {
        // ETW events span 0..29s out of 30s → gap=1s, tail_window=3s → active
        let ev = evidence(EXIT_TIMEOUT, true, false, None, Some((0.0, 29.0)), 30.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::TimeoutActive);
    }

    #[test]
    fn test_timeout_active_with_launching_checkpoint() {
        // No ETW but launching checkpoint → active (payload ran)
        let ev = evidence(EXIT_TIMEOUT, true, true, None, None, 30.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::TimeoutActive);
    }

    #[test]
    fn test_timeout_idle_etw_stopped_early() {
        // ETW events span 0..5s out of 30s → gap=25s, tail_window=3s → idle
        let ev = evidence(EXIT_TIMEOUT, true, false, None, Some((0.0, 5.0)), 30.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::TimeoutIdle);
        assert!(!DetectionVerdict::TimeoutIdle.is_detected());
    }

    #[test]
    fn test_timeout_idle_no_etw_at_all() {
        let ev = evidence(EXIT_TIMEOUT, true, false, None, None, 30.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::TimeoutIdle);
    }

    #[test]
    fn test_timeout_short_run_uses_floor() {
        // 2s execution, ETW spans 0..1.8s → gap=0.2s, floor=0.5s → active
        let ev = evidence(EXIT_TIMEOUT, true, false, None, Some((0.0, 1.8)), 2.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::TimeoutActive);
    }

    // ── Step 2: CleanExit ─────────────────────────────────────────────────

    #[test]
    fn test_clean_exit() {
        let ev = evidence(0, false, true, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::CleanExit);
        assert!(!DetectionVerdict::CleanExit.is_detected());
    }

    // ── Step 3: Dry-run gate ──────────────────────────────────────────────

    #[test]
    fn test_dry_run_matching_exit_code_is_infra_error() {
        // Artifact exits with code 1 both with and without AV → InfraError
        let ev = evidence(1, false, false, Some(1), None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::InfraError);
        assert!(!DetectionVerdict::InfraError.is_detected());
    }

    #[test]
    fn test_dry_run_different_exit_code_falls_through() {
        // Dry-run exited 0, but with AV exits 1 → not InfraError, falls to step 7
        let ev = evidence(1, false, false, Some(0), None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPrePayload);
    }

    #[test]
    fn test_dry_run_none_falls_through() {
        // No dry-run data, exit code 1 → falls to step 7
        let ev = evidence(1, false, false, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPrePayload);
    }

    #[test]
    fn test_dry_run_matching_wait_failed_is_infra_error() {
        // Both dry-run and AV run fail with EXIT_WAIT_FAILED → InfraError (spawn always fails)
        let ev = evidence(EXIT_WAIT_FAILED, false, false, Some(EXIT_WAIT_FAILED), None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::InfraError);
    }

    // ── Step 2: EXIT_INFRA ────────────────────────────────────────────────

    #[test]
    fn test_exit_infra_is_infra_error() {
        let ev = evidence(EXIT_INFRA, false, false, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::InfraError);
    }

    // ── Step 5: EXIT_WAIT_FAILED (conservative) ─────────────────────────

    #[test]
    fn test_exit_code_wait_failed_without_dry_run_is_killed_pre() {
        let ev = evidence(EXIT_WAIT_FAILED, false, false, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPrePayload);
        assert!(DetectionVerdict::KilledPrePayload.is_detected());
    }

    // ── Step 5b: EXIT_NO_CODE ───────────────────────────────────────────

    #[test]
    fn test_exit_no_code_without_launching_is_killed_pre() {
        let ev = evidence(EXIT_NO_CODE, false, false, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPrePayload);
    }

    #[test]
    fn test_exit_no_code_with_launching_is_killed_post() {
        let ev = evidence(EXIT_NO_CODE, false, true, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPostPayload);
    }

    // ── Step 5c: EXIT_TIMEOUT (defensive) ───────────────────────────────

    #[test]
    fn test_exit_timeout_without_timed_out_flag() {
        let ev = evidence(EXIT_TIMEOUT, false, false, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::TimeoutIdle);
    }

    #[test]
    fn test_exit_timeout_with_launching() {
        let ev = evidence(EXIT_TIMEOUT, false, true, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::TimeoutActive);
    }

    // ── Step 6: AV NTSTATUS codes ─────────────────────────────────────────

    #[test]
    fn test_av_ntstatus_pre_payload() {
        let ev = evidence(0xC0000906_u32 as i32, false, false, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPrePayload);
        assert!(DetectionVerdict::KilledPrePayload.is_detected());
    }

    #[test]
    fn test_av_ntstatus_post_payload() {
        let ev = evidence(0xC0000907_u32 as i32, false, true, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPostPayload);
        assert!(DetectionVerdict::KilledPostPayload.is_detected());
    }

    // ── Step 7: Crash NTSTATUS ────────────────────────────────────────────

    #[test]
    fn test_crash_ntstatus() {
        let ev = evidence(0xC0000005_u32 as i32, false, false, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::Crashed);
        assert!(DetectionVerdict::Crashed.is_detected());
    }

    // ── Step 8: Unknown nonzero exit code ─────────────────────────────────

    #[test]
    fn test_unknown_exit_code_no_launching() {
        let ev = evidence(42, false, false, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPrePayload);
    }

    #[test]
    fn test_unknown_exit_code_with_launching() {
        let ev = evidence(42, false, true, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPostPayload);
    }

    #[test]
    fn test_carrier_exit_codes_without_dry_run_are_killed_pre() {
        // Namespaced carrier exit codes (30-33) without dry-run fall through to step 8 → KilledPrePayload
        for &code in &[30, 31, 32, 33] {
            let ev = evidence(code, false, false, None, None, 1000.0);
            assert_eq!(
                classify_outcome(&ev),
                DetectionVerdict::KilledPrePayload,
                "exit_code={} without dry-run should be KilledPrePayload",
                code
            );
        }
    }

    #[test]
    fn test_carrier_exit_code_with_launching_is_killed_post() {
        // exit_code=30 but launching checkpoint present → killed post payload
        let ev = evidence(30, false, true, None, None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::KilledPostPayload);
    }

    #[test]
    fn test_carrier_exit_code_with_dry_run_match_is_infra() {
        // exit_code=31, dry-run also 31 → InfraError (artifact broken, not AV)
        let ev = evidence(31, false, false, Some(31), None, 1000.0);
        assert_eq!(classify_outcome(&ev), DetectionVerdict::InfraError);
    }

    // ── Verdict methods ───────────────────────────────────────────────────

    #[test]
    fn test_verdict_roundtrip() {
        let verdicts = [
            DetectionVerdict::KilledPrePayload,
            DetectionVerdict::KilledPostPayload,
            DetectionVerdict::Crashed,
            DetectionVerdict::TimeoutActive,
            DetectionVerdict::TimeoutIdle,
            DetectionVerdict::CleanExit,
            DetectionVerdict::InfraError,
        ];
        for v in &verdicts {
            let s = v.as_str();
            let parsed = DetectionVerdict::from_verdict_str(s);
            assert_eq!(parsed, Some(*v), "Roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn test_detection_outcome_strings() {
        assert_eq!(
            DetectionVerdict::KilledPrePayload.detection_outcome(),
            "KILLED_PRE_PAYLOAD"
        );
        assert_eq!(
            DetectionVerdict::KilledPostPayload.detection_outcome(),
            "KILLED_POST_PAYLOAD"
        );
        assert_eq!(DetectionVerdict::Crashed.detection_outcome(), "CRASHED");
        assert_eq!(
            DetectionVerdict::TimeoutActive.detection_outcome(),
            "TIMEOUT_ACTIVE"
        );
        assert_eq!(
            DetectionVerdict::TimeoutIdle.detection_outcome(),
            "TIMEOUT_IDLE"
        );
        assert_eq!(
            DetectionVerdict::CleanExit.detection_outcome(),
            "FULL_EVASION"
        );
        assert_eq!(
            DetectionVerdict::InfraError.detection_outcome(),
            "INFRA_ERROR"
        );
    }

    #[test]
    fn test_parse_rededr_date_valid() {
        let payload = br#"{"date": "2024-06-15-14-30-45", "other": "data"}"#;
        let ts = parse_rededr_date(payload);
        assert!(ts.is_some());
        // 2024-06-15 14:30:45 UTC
        let expected = NaiveDateTime::parse_from_str("2024-06-15-14-30-45", "%Y-%m-%d-%H-%M-%S")
            .unwrap()
            .and_utc()
            .timestamp() as f64;
        assert_eq!(ts.unwrap(), expected);
    }

    #[test]
    fn test_parse_rededr_date_missing() {
        let payload = br#"{"other": "data"}"#;
        assert!(parse_rededr_date(payload).is_none());
    }

    #[test]
    fn test_parse_rededr_date_invalid_format() {
        let payload = br#"{"date": "not-a-date"}"#;
        assert!(parse_rededr_date(payload).is_none());
    }

    #[test]
    fn test_parse_rededr_date_bad_json() {
        let payload = b"not json at all";
        assert!(parse_rededr_date(payload).is_none());
    }
}
