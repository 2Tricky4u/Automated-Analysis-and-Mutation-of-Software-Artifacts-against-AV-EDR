//! Mutation selection strategies and triage token analysis.
//!
//! The [`Selector`] trait abstracts how each round's module and mutation
//! configuration is chosen. Four implementations are provided:
//!
//! - [`CoverageSelector`](coverage_selector::CoverageSelector): epsilon-greedy
//!   over module variants using in-memory evasion scores and differential
//!   categories.
//! - [`FuzzerSelector`](fuzzer_selector::FuzzerSelector): evolutionary/genetic
//!   algorithm with parameter-space exploration and crossover.
//! - [`TokenSelector`](token_selector::TokenSelector): token-guided selection
//!   using avoid/seek sets from async triage extraction.
//! - [`RandomSelector`](random_selector::RandomSelector): uniform random
//!   baseline for controlled evaluation.

pub mod accumulation;
pub mod coverage_selector;
pub mod extractor;
pub mod fuzzer_selector;
pub mod param_space;
pub mod random_selector;
pub mod scorer;
pub mod source_resolver;
pub mod token_diff;
pub mod token_selector;

use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
use crate::triage::accumulation::AccumulationConfig;
use crate::triage::fuzzer_selector::FuzzerConfig;
use async_trait::async_trait;
use std::collections::BTreeMap;

/// Selector output for one round.
///
/// Produced by a [`Selector`] implementation and consumed by
/// [`JobWorker`](crate::dispatch::job_worker::JobWorker) to configure
/// the next round's build.
///
/// - `modules` — which template modules to assemble.
/// - `mutations` — AST/IR/binary mutations to apply.
/// - `rationale` — human-readable explanation for logging.
#[derive(Debug, Clone)]
pub struct Selection {
    pub modules: ModuleSelectionSpec,
    pub mutations: Vec<MutationSpec>,
    pub rationale: String,
}

/// Which selector algorithm picks mutations.
///
/// - `Coverage` (default): epsilon-greedy over variants using evasion scores
/// - `Fuzzer`: evolutionary/genetic algorithm with parameter variation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SelectorType {
    #[default]
    Coverage,
    Fuzzer,
    Token,
    Random,
}

impl SelectorType {
    /// Parse a selector type from a string, defaulting to `Coverage`.
    ///
    /// Recognized values: `"fuzzer"`, `"token"`, `"random"`.
    /// Any other input (including empty) maps to `Coverage`.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "fuzzer" => Self::Fuzzer,
            "token" => Self::Token,
            "random" => Self::Random,
            _ => Self::Coverage,
        }
    }

    /// Return the canonical string representation of this selector type.
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Coverage => "coverage",
            Self::Fuzzer => "fuzzer",
            Self::Token => "token",
            Self::Random => "random",
        }
    }
}

/// Variation strategy for the selector.
///
/// - `MutationOnly` (default): modules stay fixed, mutations vary per round
/// - `Full`: both modules AND mutations vary per round
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VariationStrategy {
    #[default]
    MutationOnly,
    Full,
}

impl VariationStrategy {
    /// Parse a variation strategy from a string, defaulting to `MutationOnly`.
    ///
    /// Recognized value: `"full"`. Any other input maps to `MutationOnly`.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "full" => Self::Full,
            _ => Self::MutationOnly,
        }
    }

    /// Return the canonical string representation of this strategy.
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MutationOnly => "mutation",
            Self::Full => "full",
        }
    }
}

/// Configuration for the mutation search space explored by a [`Selector`].
///
/// Controls which module categories may vary across rounds, which AST/IR/binary
/// mutations are eligible for selection, and which mutations are always applied
/// after round 1 (the `fixed_mutations` list).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchSpace {
    /// Which selector algorithm to use: Coverage (default) or Fuzzer.
    #[serde(default)]
    pub selector: SelectorType,
    /// Variation strategy: MutationOnly (default) or Full.
    #[serde(default)]
    pub strategy: VariationStrategy,
    /// Module categories to vary (used in Full mode)
    pub variable_categories: Vec<String>,
    /// Optional subset of mutations to explore (empty = full catalog)
    #[serde(default = "default_mutation_pool")]
    pub mutation_pool: Vec<String>,
    /// Which modules mutations apply to (empty = all)
    #[serde(default)]
    pub mutation_targets: Vec<String>,
    /// Mutations always applied every round (after round 1).
    /// Default (PoC): all binary.* + llvm.* mutations.
    #[serde(default = "default_fixed_mutations")]
    pub fixed_mutations: Vec<String>,
    /// Config for FuzzerSelector (only used when strategy=Fuzzer).
    #[serde(default)]
    pub fuzzer_config: Option<FuzzerConfig>,
    /// Accumulation phase config. Controls progressive recipe building.
    /// When `enabled: false`, selectors stay in individual exploration forever.
    #[serde(default)]
    pub accumulation: AccumulationConfig,
}

fn default_fixed_mutations() -> Vec<String> {
    vec![
        //"llvm.nop_insert",
        "llvm.opaque_predicate",
        //"llvm.junk_block",
        "binary.rich_header",
        "binary.import_pad",
        "binary.resource_inject",
        "binary.section_rename",
        "binary.entropy_normalize",
        "binary.string_inject",
        "binary.size_pad",
        "binary.debug_dir",
        "binary.timestamp",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn default_mutation_pool() -> Vec<String> {
    vec![
        "ast.decon_rounds",
        "ast.fill_pattern",
        "ast.exec_decoy",
        "ast.timing_pattern",
        "ast.protection_transition",
        "ast.const_obfuscation",
        "ast.string_xor",
        "ast.benign_syscall_insert",
        "ast.benign_preamble",
        "ast.api_sequence_obfuscation",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

impl Default for SearchSpace {
    fn default() -> Self {
        Self {
            selector: SelectorType::Coverage,
            strategy: VariationStrategy::MutationOnly,
            variable_categories: vec!["deconditioner".to_string()],
            mutation_pool: default_mutation_pool(),
            mutation_targets: vec![],
            fixed_mutations: default_fixed_mutations(),
            fuzzer_config: None,
            accumulation: AccumulationConfig::default(),
        }
    }
}

/// Token-level guidance from async triage.
///
/// Produced by `extractor::extract_and_score()` in a background task,
/// sent to `JobWorker` via a channel, consumed by `TokenSelector`.
#[derive(Debug, Clone)]
pub struct TriageGuidance {
    pub avoid_tokens: Vec<String>,
    pub seek_tokens: Vec<String>,
}

/// Trait for mutation selection strategies.
///
/// `history` contains the job's completed round summaries. `guidance` carries
/// token-level avoid/seek sets from the async triage pipeline when available.
#[async_trait]
pub trait Selector: Send + Sync {
    /// Choose modules and mutations for the next round.
    ///
    /// * `job_id` — owning job, used for logging and seed derivation.
    /// * `round_number` — 1-based round index within the job.
    /// * `search_space` — eligible mutations and module categories.
    /// * `default_modules` — job-level default module selection.
    /// * `history` — completed round summaries keyed by round number.
    /// * `guidance` — optional token-level avoid/seek sets from async triage.
    async fn select(
        &self,
        job_id: &str,
        round_number: u32,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
        guidance: Option<&TriageGuidance>,
    ) -> Selection;
}
