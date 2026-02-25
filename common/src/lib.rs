use serde::{Deserialize, Serialize};

// === Detection Verdict (shared between worker and controller) ===

/// Detection verdict from the classifier.
///
/// Answers "was it detected?" clearly. Ambiguity is explicit.
/// Used by the worker's classifier to produce provisional verdicts and by the
/// controller's dry-run override to produce authoritative verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionVerdict {
    /// Exit 0, or timeout with Launching/beyond checkpoint (full evasion)
    Evasion,
    /// AV NTSTATUS, EXIT_NO_CODE, or dry-run proves AV kill
    Detected,
    /// Crash NTSTATUS, carrier codes 30-39, unknown nonzero — need dry-run to resolve
    Ambiguous,
    /// Timeout without reaching Launching checkpoint (stuck/stalled loader)
    Stalled,
    /// Engine/setup failure, guardrail rejection
    InfraError,
    /// Dry-run proved artifact broken on clean VM
    MutationFailed,
    /// Contradictory signals (e.g. works better with AV)
    Anomaly,
}

impl DetectionVerdict {
    /// Whether this verdict means the artifact was detected.
    /// Conservative: ambiguous defaults to detected.
    pub fn is_detected(self) -> bool {
        matches!(self, Self::Detected | Self::Ambiguous)
    }

    /// Short string identifier for this verdict (used in proto/ES fields).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Evasion => "evasion",
            Self::Detected => "detected",
            Self::Ambiguous => "ambiguous",
            Self::Stalled => "stalled",
            Self::InfraError => "infra_error",
            Self::MutationFailed => "mutation_failed",
            Self::Anomaly => "anomaly",
        }
    }

    /// Parse from string (inverse of `as_str`), with backward compat for old ES strings.
    pub fn from_verdict_str(s: &str) -> Option<Self> {
        match s {
            // New strings
            "evasion" => Some(Self::Evasion),
            "detected" => Some(Self::Detected),
            "ambiguous" => Some(Self::Ambiguous),
            "stalled" => Some(Self::Stalled),
            "infra_error" => Some(Self::InfraError),
            "mutation_failed" => Some(Self::MutationFailed),
            "anomaly" => Some(Self::Anomaly),
            // Backward compat: old strings → new variants
            "killed_pre_payload" | "killed_post_payload" => Some(Self::Detected),
            "crashed" => Some(Self::Ambiguous),
            "timeout_active" | "timeout_idle" | "clean_exit" => Some(Self::Evasion),
            _ => None,
        }
    }
}

/// True if the artifact reached carrier execution (Launching checkpoint or beyond).
/// Used by both worker classifier and controller dry-run override.
pub fn has_launched(last_checkpoint: &str) -> bool {
    let lc = last_checkpoint.to_ascii_lowercase();
    lc == "launching" || lc == "payload_executed" || lc.starts_with("sc_checkpoint")
}
