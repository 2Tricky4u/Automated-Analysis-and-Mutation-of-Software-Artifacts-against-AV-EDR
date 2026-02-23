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
pub mod source_resolver;

use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
use async_trait::async_trait;
use std::collections::BTreeMap;

/// Selector output for one round.
#[derive(Debug, Clone)]
pub struct Selection {
    pub modules: ModuleSelectionSpec,
    pub mutations: Vec<MutationSpec>,
    pub rationale: String,
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
    /// Variation strategy: MutationOnly (default) or Full
    #[serde(default)]
    pub strategy: VariationStrategy,
    /// Module categories to vary (used in Full mode)
    pub variable_categories: Vec<String>,
    /// Optional subset of mutations to explore (empty = full catalog)
    #[serde(default)]
    pub mutation_pool: Vec<String>,
    /// Which modules mutations apply to (empty = all)
    #[serde(default)]
    pub mutation_targets: Vec<String>,
}

impl Default for SearchSpace {
    fn default() -> Self {
        Self {
            strategy: VariationStrategy::MutationOnly,
            variable_categories: vec!["deconditioner".to_string()],
            mutation_pool: vec![],
            mutation_targets: vec![],
        }
    }
}

/// Future: token-level guidance from async triage. None in v0.
#[allow(dead_code)]
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
