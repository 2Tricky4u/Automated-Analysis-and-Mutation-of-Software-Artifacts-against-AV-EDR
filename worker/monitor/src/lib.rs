/// Monitor service for outcome labeling and metrics collection
///
/// Implements CLAUDE.md Section 3: Monitor component
/// "Monitor: labels outcomes: detected | not_detected | noisy | crashed, returns metrics."
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Detected,
    NotDetected,
    Noisy,
    Crashed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMetrics {
    pub cpu_pct: f64,
    pub mem_mb: u64,
    pub detection_latency_ms: Option<u64>,
}

/// Monitor is responsible for:
/// - Watching execution outcomes in real-time
/// - Labeling runs based on telemetry heuristics
/// - Collecting performance metrics
/// - Providing labeled outcomes to the Triage Engine
pub struct Monitor {}

impl Monitor {
    pub fn new() -> Self {
        Self {}
    }

    /// Labels the outcome of a run based on telemetry analysis
    ///
    /// # Arguments
    /// * `run_id` - The unique run identifier
    ///
    /// # Returns
    /// Tuple of (Outcome, RunMetrics) or error
    ///
    /// # TODO
    /// - Query Elasticsearch for telemetry events for this run_id
    /// - Apply heuristics:
    ///   - detected: EDR alert or process kill
    ///   - not_detected: clean execution, no alerts
    ///   - noisy: high telemetry volume, many false positives
    ///   - crashed: abnormal termination
    /// - Compute metrics from telemetry timestamps
    pub async fn label_outcome(
        &self,
        _run_id: &str,
    ) -> Result<(Outcome, RunMetrics), Box<dyn std::error::Error>> {
        // TODO: Implement automated labeling based on telemetry
        Ok((
            Outcome::NotDetected,
            RunMetrics {
                cpu_pct: 0.0,
                mem_mb: 0,
                detection_latency_ms: None,
            },
        ))
    }
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}
