//! Triage module — mutation selection strategies.
//!
//! The `Selector` trait abstracts how round N+1's module configuration
//! is chosen based on rounds 1..N. Two planned implementations:
//!
//! - `CoverageSelector` (v0): epsilon-greedy over deconditioner variants,
//!   using in-memory round outcomes (evasion_score + DifferentialCategory).
//!
//! - `TokenSelector` (future): uses avoid/seek token sets from async triage,
//!   queries ES for token-level data.

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
use crate::triage::fuzzer_selector::FuzzerConfig;
use async_trait::async_trait;
use std::collections::BTreeMap;

/// Selector output for one round.
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
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "fuzzer" => Self::Fuzzer,
            "token" => Self::Token,
            "random" => Self::Random,
            _ => Self::Coverage,
        }
    }

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
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "full" => Self::Full,
            _ => Self::MutationOnly,
        }
    }

    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MutationOnly => "mutation",
            Self::Full => "full",
        }
    }
}

/// Which module categories the selector may vary + mutation exploration config.
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

/// Trait for selection strategies.
///
/// `history` is the job's completed rounds (from JobSession.rounds).
/// CoverageSelector uses it directly; future TokenSelector may ignore it
/// and query ES for token-level data instead.
#[async_trait]
pub trait Selector: Send + Sync {
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
