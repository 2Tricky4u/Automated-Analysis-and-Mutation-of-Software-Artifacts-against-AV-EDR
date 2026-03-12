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

pub mod analysis;
pub mod build_bench;
pub mod campaign_runner;
pub mod es_query;
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
    #[allow(unused_mut)]
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

// ============================================================================
// Infrastructure Evaluation Types
// ============================================================================

use std::collections::HashMap;

/// Result of a payload encoding benchmark (I1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadEncodingResult {
    pub encoding_type: String,
    pub payload_size: usize,
    pub encoded_size: usize,
    pub encoded_entropy: f64,
    pub roundtrip_correct: bool,
    pub encode_time_us: f64,
    pub header_compiles: bool,
}

/// Result of an AST mutation benchmark (I2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstMutationResult {
    pub mutation_id: String,
    pub input_lines: usize,
    pub output_lines: usize,
    pub line_delta: i64,
    pub input_ast_nodes: usize,
    pub output_ast_nodes: usize,
    pub parse_valid: bool,
    pub compile_success: Option<bool>,
    pub transform_time_us: f64,
}

/// Result of an IR mutation benchmark (I3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrMutationResult {
    pub mutation_id: String,
    pub density: f32,
    pub input_lines: usize,
    pub output_lines: usize,
    pub insertions: usize,
    pub survives_o2: Option<bool>,
    pub deterministic: bool,
    pub transform_time_us: f64,
}

/// Result of a binary mutation benchmark (I4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryMutationResult {
    pub mutation_id: String,
    pub input_size: u64,
    pub output_size: u64,
    pub pe_valid: bool,
    pub section_count_delta: i32,
    pub import_count_delta: i32,
    pub text_entropy_before: f64,
    pub text_entropy_after: f64,
    pub transform_time_us: f64,
}

/// Result of a template assembly benchmark (I5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateAssemblyResult {
    pub modules: HashMap<String, String>,
    pub markers_resolved: bool,
    pub output_lines: usize,
    pub assembly_time_us: f64,
}

/// Result of an instrumentation overhead benchmark (I6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentationResult {
    pub carrier: String,
    pub baseline_size: u64,
    pub instrumented_size: u64,
    pub size_ratio: f64,
    pub build_time_baseline_ms: f64,
    pub build_time_instrumented_ms: f64,
}

/// Result of a token extraction benchmark (I7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenExtractionResult {
    pub input_doc_count: usize,
    pub output_token_count: usize,
    pub category_counts: HashMap<String, usize>,
    pub categories_active: usize,
    pub extraction_time_us: f64,
    pub deterministic: bool,
}

/// Result of a token scoring validation test (I8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenScoringResult {
    pub test_case: String,
    pub input_rounds: usize,
    pub expected_lift: f64,
    pub computed_lift: f64,
    pub lift_error: f64,
    pub guidance_correct: bool,
}

/// Dataset for infrastructure-level evaluation (parallel to EvalDataset).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InfraEvalDataset {
    pub payload_encoding: Option<Vec<PayloadEncodingResult>>,
    pub ast_mutation: Option<Vec<AstMutationResult>>,
    pub ir_mutation: Option<Vec<IrMutationResult>>,
    pub binary_mutation: Option<Vec<BinaryMutationResult>>,
    pub template_assembly: Option<Vec<TemplateAssemblyResult>>,
    pub instrumentation: Option<Vec<InstrumentationResult>>,
    pub token_extraction: Option<Vec<TokenExtractionResult>>,
    pub token_scoring: Option<Vec<TokenScoringResult>>,
}

/// Every infrastructure metric implements this. Stateless, pure computation.
pub trait InfraMetric: Send + Sync {
    fn metric_id(&self) -> &str;
    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>>;
}

/// Returns all infrastructure-level metrics.
pub fn all_infra_metrics() -> Vec<Box<dyn InfraMetric>> {
    analysis::all_infra_analysis_metrics()
}

/// Run all infrastructure metrics and collect results.
pub fn run_infra_evaluation(dataset: &InfraEvalDataset) -> Vec<MetricResult> {
    all_infra_metrics()
        .iter()
        .flat_map(|m| m.evaluate(dataset).unwrap_or_default())
        .collect()
}
