//! Evaluation framework for fuzzer assessment.
//!
//! Measures the AutoMutate++ fuzzer along three axes:
//! - **Input**: expressiveness, validity, diversity of generated artifacts
//! - **Oracle**: precision, soundness, attribution, stability of detection verdicts
//! - **Guidance**: feedback quality, search efficiency, baseline comparison, convergence
//!
//! Each metric implements [`EvalMetric`] — a stateless, pure-computation trait.
//! All metrics operate on [`EvalDataset`], which can be constructed from live
//! `JobSession` data or loaded offline from JSON exports.

pub mod fixtures;
pub mod helpers;
pub mod report;

#[cfg(feature = "guidance")]
pub mod guidance;
#[cfg(feature = "input")]
pub mod input;
#[cfg(feature = "oracle")]
pub mod oracle;

use serde::{Deserialize, Serialize};

// Re-export production types used throughout evaluation.
pub use controller::dispatch::types::{
    DifferentialCategory, ModuleSelectionSpec, MutationSpec, RoundSummary,
};
pub use controller::triage::extractor::extract_round_tokens;
pub use controller::triage::scorer::{self, TokenScore};
pub use controller::triage::{Selection, TriageGuidance};

// ============================================================================
// Core Abstractions
// ============================================================================

/// Result of a single evaluation metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricResult {
    /// Dotted metric ID, e.g. "input.validity.rejection_rate"
    pub metric_id: String,
    /// Axis: "input", "oracle", or "guidance"
    pub axis: String,
    /// Sub-category, e.g. "validity"
    pub category: String,
    /// Human-readable label
    pub label: String,
    /// Primary numeric value (0.0–1.0 where applicable)
    pub value: f64,
    /// Breakdown details (per-module, per-mutation, etc.)
    pub details: serde_json::Value,
    /// Sample size
    pub n: usize,
}

/// Record of a selector decision for one round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionRecord {
    pub round_number: u32,
    pub rationale: String,
    pub modules: ModuleSelectionSpec,
    pub mutations: Vec<String>,
    pub avoid_tokens: Vec<String>,
    pub seek_tokens: Vec<String>,
}

/// One entry in the token-round matrix: tokens observed in a round + detection outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMatrixEntry {
    pub round_number: u32,
    pub tokens: Vec<String>,
    pub detected: bool,
    pub trustworthy: bool,
}

/// Telemetry-level tokens for a round (from ES or JSON export).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundTelemetryTokens {
    pub round_number: u32,
    pub api_tokens: Vec<String>,
    pub seq2_tokens: Vec<String>,
    pub etw_tokens: Vec<String>,
    pub image_tokens: Vec<String>,
}

/// Universal input to all metrics — constructible offline from JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalDataset {
    pub job_id: String,
    pub rounds: Vec<RoundSummary>,
    pub selections: Vec<SelectionRecord>,
    pub token_matrices: Vec<TokenMatrixEntry>,
    pub telemetry_tokens: Option<Vec<RoundTelemetryTokens>>,
}

/// Every metric implements this. Stateless, pure computation.
pub trait EvalMetric: Send + Sync {
    /// Dotted metric ID, e.g. "input.validity"
    fn metric_id(&self) -> &str;

    /// Evaluate the metric against the dataset. Returns one or more results
    /// (a metric may emit sub-metrics).
    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>>;
}

/// Returns all metrics enabled by compile-time features.
pub fn all_metrics() -> Vec<Box<dyn EvalMetric>> {
    let mut metrics: Vec<Box<dyn EvalMetric>> = Vec::new();

    #[cfg(feature = "input")]
    {
        metrics.push(Box::new(input::expressiveness::Expressiveness));
        metrics.push(Box::new(input::validity::Validity));
        metrics.push(Box::new(input::diversity::Diversity));
    }

    #[cfg(feature = "oracle")]
    {
        metrics.push(Box::new(oracle::precision::Precision));
        metrics.push(Box::new(oracle::soundness::Soundness));
        metrics.push(Box::new(oracle::attribution::Attribution));
        metrics.push(Box::new(oracle::stability::Stability));
    }

    #[cfg(feature = "guidance")]
    {
        metrics.push(Box::new(guidance::feedback_quality::FeedbackQuality));
        metrics.push(Box::new(guidance::search_efficiency::SearchEfficiency));
        metrics.push(Box::new(guidance::baseline_comparison::BaselineComparison));
        metrics.push(Box::new(guidance::convergence::Convergence));
    }

    metrics
}

/// Run all enabled metrics and collect results.
pub fn run_evaluation(dataset: &EvalDataset) -> Vec<MetricResult> {
    all_metrics()
        .iter()
        .flat_map(|m| m.evaluate(dataset).unwrap_or_default())
        .collect()
}
