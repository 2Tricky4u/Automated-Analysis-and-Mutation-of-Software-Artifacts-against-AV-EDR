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

/// Which module categories the selector may vary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchSpace {
    pub variable_categories: Vec<String>,
}

impl Default for SearchSpace {
    fn default() -> Self {
        Self {
            variable_categories: vec!["deconditioner".to_string()],
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
