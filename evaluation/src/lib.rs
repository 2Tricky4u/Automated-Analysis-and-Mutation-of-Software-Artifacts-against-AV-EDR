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

/// Result of an input diversity benchmark (I9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputDiversityResult {
    pub mutation_a: String,
    pub mutation_b: String,
    pub line_delta_a: i64,
    pub line_delta_b: i64,
    pub normalized_distance: f64,
    pub outputs_differ: bool,
}

/// Result of an oracle stability benchmark (I10).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleStabilityResult {
    pub test_case: String,
    pub repeated_deterministic: bool,
    pub permutation_top5_jaccard: f64,
    pub permutation_lift_variance: f64,
    pub incremental_snapshots: Vec<IncrementalSnapshot>,
}

/// Snapshot of guidance state at a given round count (I10 sub-struct).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalSnapshot {
    pub round_count: usize,
    pub avoid_count: usize,
    pub seek_count: usize,
    pub jaccard_with_full: f64,
}

/// Result of a selector comparison benchmark (I11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorComparisonResult {
    pub selector_name: String,
    pub rounds_evaluated: usize,
    pub unique_mutation_sets: usize,
    pub mutation_pool_coverage: f64,
    pub mean_recipe_size: f64,
    pub exploration_rate: f64,
    pub per_round_mutations: Vec<Vec<String>>,
}

/// Result of a guidance utilization benchmark (I12).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuidanceUtilizationResult {
    pub selector_name: String,
    pub rounds: usize,
    pub mutations_without_guidance: Vec<Vec<String>>,
    pub mutations_with_guidance: Vec<Vec<String>>,
    pub avoid_tokens: Vec<String>,
    pub seek_tokens: Vec<String>,
    pub avoidance_rate: f64,
    pub seek_adoption_rate: f64,
    pub recipe_jaccard_delta: f64,
}

/// Result of a convergence simulation benchmark (I13).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvergenceSimulationResult {
    /// None = accumulation-only (original), Some = per-selector simulation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_name: Option<String>,
    pub total_rounds: usize,
    pub phase_transitions: Vec<(u32, String)>,
    pub recipe_size_trajectory: Vec<(u32, usize)>,
    pub diversity_trajectory: Vec<(u32, f64)>,
    pub best_score_trajectory: Vec<(u32, f64)>,
    pub marginal_contribution_count: Vec<(u32, usize)>,
}

/// Result of a line tracing instrumentation benchmark (I14).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineTracingResult {
    /// Label for the source input (e.g., "reference_c", "assembled_alloc_rw_rx")
    pub source_label: String,
    /// Number of lines in input source
    pub input_lines: usize,
    /// Number of lines in output (after injection)
    pub output_lines: usize,
    /// Number of trace calls injected
    pub trace_calls_injected: usize,
    /// Number of deferred trace calls (loop optimization)
    pub deferred_trace_calls: usize,
    /// Trace format used (binary or base64)
    pub trace_format: String,
    /// Transform time in microseconds (single run)
    pub transform_time_us: f64,
    /// Mean transform time across N iterations
    pub mean_transform_time_us: f64,
    /// Standard deviation of transform time
    pub stddev_transform_time_us: f64,
    /// Number of timing iterations
    pub iterations: usize,
    /// Whether the output parses as valid C (tree-sitter re-parse)
    pub output_valid: bool,
    /// Characters per microsecond (throughput metric)
    pub chars_per_us: f64,
}

/// Result of a shellcode checkpoint patching benchmark (I15).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScCheckpointResult {
    /// Shellcode file name (e.g., "calc64.bin")
    pub shellcode_name: String,
    /// Size of input shellcode in bytes
    pub shellcode_size: usize,
    /// Requested checkpoint count
    pub requested_checkpoints: u32,
    /// Actual checkpoints inserted (may be clamped)
    pub actual_checkpoints: usize,
    /// Size after stub prepend (stub_size + shellcode_size)
    pub size_with_stub: usize,
    /// Number of reachable instruction boundaries found
    pub reachable_boundaries: usize,
    /// Patch time in microseconds (disassemble + insert INT3s)
    pub patch_time_us: f64,
    /// Mean patch time across N iterations
    pub mean_patch_time_us: f64,
    /// Standard deviation of patch time
    pub stddev_patch_time_us: f64,
    /// Stub prepend time in microseconds
    pub stub_prepend_time_us: f64,
    /// C header generation time in microseconds
    pub header_gen_time_us: f64,
    /// Number of timing iterations
    pub iterations: usize,
    /// Bytes per microsecond (throughput metric)
    pub bytes_per_us: f64,
    /// Whether all patched offsets are at instruction boundaries
    pub boundary_correct: bool,
    /// Progress percentages of inserted checkpoints
    pub checkpoint_progress_pcts: Vec<u8>,
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
    pub input_diversity: Option<Vec<InputDiversityResult>>,
    pub oracle_stability: Option<Vec<OracleStabilityResult>>,
    pub selector_comparison: Option<Vec<SelectorComparisonResult>>,
    pub guidance_utilization: Option<Vec<GuidanceUtilizationResult>>,
    pub convergence_simulation: Option<Vec<ConvergenceSimulationResult>>,
    pub line_tracing: Option<Vec<LineTracingResult>>,
    pub sc_checkpoint: Option<Vec<ScCheckpointResult>>,
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
