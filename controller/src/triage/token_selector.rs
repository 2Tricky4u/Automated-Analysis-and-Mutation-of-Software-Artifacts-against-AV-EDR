//! Token-guided mutation selector.
//!
//! Uses `TriageGuidance` (avoid/seek token sets) to bias module and mutation
//! selection. Falls back to `CoverageSelector` behavior when guidance is `None`
//! (first rounds before triage completes).
//!
//! Algorithm:
//! 1. Round 1 → baseline (same as CoverageSelector)
//! 2. No guidance → delegate to CoverageSelector (graceful degradation)
//! 3. With guidance → token-aware epsilon-greedy:
//!    - Score module variants by token overlap with avoid/seek
//!    - Score mutation candidates by token overlap
//!    - Epsilon-greedy (ε=0.3) on the scored list

use super::coverage_selector::CoverageSelector;
use super::{SearchSpace, Selection, Selector, TriageGuidance, VariationStrategy};
use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
use crate::triage::param_space::{default_registry, find_param_space};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// Exploration rate for epsilon-greedy selection.
const EPSILON: f64 = 0.3;

/// All available deconditioner variants (mirrors coverage_selector).
const DECONDITIONER_VARIANTS: &[&str] = &[
    "none",
    "alloc_loop",
    "alloc_exec",
    "thread_alloc",
    "mixed_apis",
    "entropy_flood",
];

/// Token-guided selector.
pub struct TokenSelector;

impl Default for TokenSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenSelector {
    pub fn new() -> Self {
        Self
    }

    fn pseudo_random(n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        nanos as usize % n
    }

    fn sample_mutation_params(mutation_id: &str) -> Option<serde_json::Value> {
        let registry = default_registry();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64;
        let mut rng = crate::triage::param_space::SeededRng::from_raw(nanos.max(1));
        find_param_space(&registry, mutation_id).and_then(|ps| ps.sample_params(&mut rng))
    }

    /// Token-guided module selection for deconditioner.
    ///
    /// Scores each variant by:
    /// - Historical evasion score (mean)
    /// - Token penalty/bonus from guidance
    /// - Novelty bonus for untried variants
    fn token_guided_modules(
        &self,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
        guidance: &TriageGuidance,
    ) -> (ModuleSelectionSpec, String) {
        if !search_space
            .variable_categories
            .iter()
            .any(|c| c == "deconditioner")
        {
            return (
                default_modules.clone(),
                "No variable categories include deconditioner".to_string(),
            );
        }

        let avoid_set: HashSet<&str> = guidance.avoid_tokens.iter().map(|s| s.as_str()).collect();
        let seek_set: HashSet<&str> = guidance.seek_tokens.iter().map(|s| s.as_str()).collect();

        // Collect per-variant stats from trustworthy history
        let mut stats: HashMap<String, (u32, f64)> = HashMap::new(); // (count, total_score)
        for summary in history.values() {
            if !summary.differential_category.is_trustworthy() {
                continue;
            }
            let variant = summary.modules.deconditioner.clone();
            let entry = stats.entry(variant).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += summary.evasion_score;
        }

        // Score each variant
        let mut scored: Vec<(String, f64)> = DECONDITIONER_VARIANTS
            .iter()
            .map(|v| {
                let token = format!("module:deconditioner={}", v);
                let mut score = 0.0;

                // Historical performance
                if let Some((count, total)) = stats.get(*v) {
                    score += total / *count as f64;
                } else {
                    // Novelty bonus for untried
                    score += 0.4;
                }

                // Token guidance
                if avoid_set.contains(token.as_str()) {
                    score -= 0.5;
                }
                if seek_set.contains(token.as_str()) {
                    score += 0.3;
                }

                (v.to_string(), score)
            })
            .collect();

        // Sort descending by score
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Epsilon-greedy
        let coin = Self::pseudo_random(100) as f64 / 100.0;
        let (chosen, rationale) = if coin < EPSILON {
            let idx = Self::pseudo_random(scored.len());
            let variant = &scored[idx].0;
            (
                variant.clone(),
                format!(
                    "Token-guided random: {} (score={:.2}, ε={:.1})",
                    variant, scored[idx].1, EPSILON
                ),
            )
        } else {
            let best = &scored[0];
            (
                best.0.clone(),
                format!("Token-guided best: {} (score={:.2})", best.0, best.1),
            )
        };

        let mut modules = default_modules.clone();
        modules.deconditioner = chosen;
        (modules, rationale)
    }

    /// Token-guided mutation selection.
    ///
    /// Fixed mutations always applied. Explored mutation from pool scored by
    /// token overlap with avoid/seek.
    fn token_guided_mutations(
        &self,
        search_space: &SearchSpace,
        history: &BTreeMap<u32, RoundSummary>,
        guidance: &TriageGuidance,
    ) -> (Vec<MutationSpec>, String) {
        let avoid_set: HashSet<&str> = guidance.avoid_tokens.iter().map(|s| s.as_str()).collect();
        let seek_set: HashSet<&str> = guidance.seek_tokens.iter().map(|s| s.as_str()).collect();

        // Fixed mutations always applied
        let mut mutations: Vec<MutationSpec> = search_space
            .fixed_mutations
            .iter()
            .map(|id| MutationSpec {
                id: id.clone(),
                params: Self::sample_mutation_params(id),
            })
            .collect();

        let fixed_set: HashSet<&str> = search_space
            .fixed_mutations
            .iter()
            .map(|s| s.as_str())
            .collect();

        let pool: Vec<&str> = search_space
            .mutation_pool
            .iter()
            .map(|s| s.as_str())
            .collect();

        if pool.is_empty() {
            return (
                mutations,
                format!(
                    "Token: {} fixed | No exploration pool",
                    search_space.fixed_mutations.len()
                ),
            );
        }

        // Collect per-mutation stats from trustworthy history
        let mut mutation_stats: HashMap<String, (u32, f64)> = HashMap::new();
        for summary in history.values() {
            if !summary.differential_category.is_trustworthy() {
                continue;
            }
            let explored: Vec<&String> = summary
                .mutations
                .iter()
                .filter(|m| !fixed_set.contains(m.as_str()))
                .collect();
            if explored.len() != 1 {
                continue;
            }
            let key = explored[0].clone();
            let entry = mutation_stats.entry(key).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += summary.evasion_score;
        }

        // Score each pool mutation
        let mut scored: Vec<(String, f64)> = pool
            .iter()
            .map(|m| {
                let token = format!("mutation:{}", m);
                let mut score = 0.0;

                // Historical performance
                if let Some((count, total)) = mutation_stats.get(*m) {
                    score += total / *count as f64;
                } else {
                    // Novelty bonus for untried
                    score += 0.4;
                }

                // Token guidance
                if avoid_set.contains(token.as_str()) {
                    score -= 0.5;
                }
                if seek_set.contains(token.as_str()) {
                    score += 0.3;
                }

                (m.to_string(), score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Epsilon-greedy
        let coin = Self::pseudo_random(100) as f64 / 100.0;
        let (chosen, rationale) = if coin < EPSILON {
            let idx = Self::pseudo_random(scored.len());
            (
                scored[idx].0.clone(),
                format!(
                    "Token mutation: random {} (score={:.2})",
                    scored[idx].0, scored[idx].1
                ),
            )
        } else {
            let best = &scored[0];
            (
                best.0.clone(),
                format!("Token mutation: best {} (score={:.2})", best.0, best.1),
            )
        };

        mutations.push(MutationSpec {
            id: chosen,
            params: Self::sample_mutation_params(&scored[0].0),
        });

        (
            mutations,
            format!(
                "Token: {} fixed | {}",
                search_space.fixed_mutations.len(),
                rationale
            ),
        )
    }
}

#[async_trait]
impl Selector for TokenSelector {
    async fn select(
        &self,
        job_id: &str,
        round_number: u32,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
        guidance: Option<&TriageGuidance>,
    ) -> Selection {
        // Round 1: always baseline (same as CoverageSelector)
        if round_number <= 1 {
            return Selection {
                modules: default_modules.clone(),
                mutations: vec![],
                rationale: "Round 1: baseline control (defaults)".to_string(),
            };
        }

        // No guidance yet: delegate to CoverageSelector behavior
        let guidance = match guidance {
            Some(g) => g,
            None => {
                return CoverageSelector::new()
                    .select(
                        job_id,
                        round_number,
                        search_space,
                        default_modules,
                        history,
                        None,
                    )
                    .await;
            }
        };

        // Token-guided selection
        match search_space.strategy {
            VariationStrategy::MutationOnly => {
                let (mutations, rationale) =
                    self.token_guided_mutations(search_space, history, guidance);
                Selection {
                    modules: default_modules.clone(),
                    mutations,
                    rationale,
                }
            }
            VariationStrategy::Full => {
                let (modules, module_rationale) =
                    self.token_guided_modules(search_space, default_modules, history, guidance);
                let (mutations, mutation_rationale) =
                    self.token_guided_mutations(search_space, history, guidance);
                Selection {
                    modules,
                    mutations,
                    rationale: format!("Module: {} | {}", module_rationale, mutation_rationale),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{DifferentialCategory, RoundId};

    fn make_summary(
        round_number: u32,
        deconditioner: &str,
        evasion_score: f64,
        category: DifferentialCategory,
        mutations: Vec<String>,
    ) -> RoundSummary {
        RoundSummary {
            round_id: RoundId(format!("r-{}", round_number)),
            round_number,
            mutation_specs: mutations
                .iter()
                .map(|id| MutationSpec {
                    id: id.clone(),
                    params: None,
                })
                .collect(),
            mutations,
            modules: ModuleSelectionSpec {
                deconditioner: deconditioner.to_string(),
                ..ModuleSelectionSpec::default()
            },
            detected: category.is_detected(),
            behavior_match: true,
            evasion_score,
            differential_category: category,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        }
    }

    #[tokio::test]
    async fn test_no_guidance_delegates_to_coverage() {
        let selector = TokenSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let mut history = BTreeMap::new();
        history.insert(
            1,
            make_summary(1, "none", 0.2, DifferentialCategory::RealDetection, vec![]),
        );

        let selection = selector
            .select(
                "job-1",
                2,
                &SearchSpace::default(),
                &defaults,
                &history,
                None,
            )
            .await;

        // Should produce mutations (CoverageSelector behavior for round 2+)
        assert!(
            !selection.mutations.is_empty(),
            "No guidance should delegate to CoverageSelector and produce mutations"
        );
    }

    #[tokio::test]
    async fn test_avoids_high_lift_module() {
        let selector = TokenSelector::new();
        let defaults = ModuleSelectionSpec::default();

        let mut history = BTreeMap::new();
        // Give alloc_loop a high evasion score so it would normally be selected
        history.insert(
            1,
            make_summary(1, "alloc_loop", 0.9, DifferentialCategory::Evasion, vec![]),
        );
        // But guidance says avoid it
        let guidance = TriageGuidance {
            avoid_tokens: vec!["module:deconditioner=alloc_loop".to_string()],
            seek_tokens: vec!["module:deconditioner=entropy_flood".to_string()],
        };

        let search_space = SearchSpace {
            strategy: VariationStrategy::Full,
            variable_categories: vec!["deconditioner".to_string()],
            ..Default::default()
        };

        // Run multiple times — alloc_loop should be deprioritized
        let mut alloc_loop_count = 0;
        for _ in 0..30 {
            let selection = selector
                .select(
                    "job-1",
                    3,
                    &search_space,
                    &defaults,
                    &history,
                    Some(&guidance),
                )
                .await;
            if selection.modules.deconditioner == "alloc_loop" {
                alloc_loop_count += 1;
            }
        }

        // alloc_loop should be rare (only from epsilon exploration)
        assert!(
            alloc_loop_count < 15,
            "alloc_loop should be deprioritized (got {} out of 30)",
            alloc_loop_count
        );
    }

    #[tokio::test]
    async fn test_seeks_low_lift_mutation() {
        let selector = TokenSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let guidance = TriageGuidance {
            avoid_tokens: vec!["mutation:ast.decon_rounds".to_string()],
            seek_tokens: vec!["mutation:ast.fill_pattern".to_string()],
        };

        let search_space = SearchSpace::default();

        // Run multiple times
        let mut fill_pattern_count = 0;
        let mut decon_rounds_count = 0;
        for _ in 0..30 {
            let selection = selector
                .select(
                    "job-1",
                    2,
                    &search_space,
                    &defaults,
                    &history,
                    Some(&guidance),
                )
                .await;

            let explored: Vec<&MutationSpec> = selection
                .mutations
                .iter()
                .filter(|m| !search_space.fixed_mutations.contains(&m.id))
                .collect();
            for m in &explored {
                if m.id == "ast.fill_pattern" {
                    fill_pattern_count += 1;
                }
                if m.id == "ast.decon_rounds" {
                    decon_rounds_count += 1;
                }
            }
        }

        // fill_pattern should be favored over decon_rounds
        assert!(
            fill_pattern_count > decon_rounds_count,
            "fill_pattern ({}) should be favored over decon_rounds ({})",
            fill_pattern_count,
            decon_rounds_count,
        );
    }

    #[tokio::test]
    async fn test_round_1_baseline() {
        let selector = TokenSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let guidance = TriageGuidance {
            avoid_tokens: vec!["module:deconditioner=none".to_string()],
            seek_tokens: vec![],
        };

        let selection = selector
            .select(
                "job-1",
                1,
                &SearchSpace::default(),
                &defaults,
                &history,
                Some(&guidance),
            )
            .await;

        assert!(
            selection.mutations.is_empty(),
            "Round 1 should always be baseline"
        );
        assert_eq!(
            selection.modules, defaults,
            "Round 1 should use default modules"
        );
    }
}
