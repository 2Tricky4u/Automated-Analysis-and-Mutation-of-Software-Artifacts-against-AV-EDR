use serde::{Deserialize, Serialize};

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
