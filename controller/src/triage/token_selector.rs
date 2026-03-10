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
use crate::triage::accumulation::{
    AccumulationPhase, compute_marginal_contributions, determine_phase, effective_max_recipe_size,
    perturb_recipe_params, reconstruct_best_recipe,
};
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
///
/// Uses [`TriageGuidance`] avoid/seek sets produced by the async triage
/// pipeline to bias both module variant and mutation selection toward
/// evasion-correlated configurations. Falls back to
/// [`CoverageSelector`] when no
/// guidance is available yet (early rounds before the first triage completes).
pub struct TokenSelector;

impl Default for TokenSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenSelector {
    /// Create a new token-guided selector (no internal state).
    pub fn new() -> Self {
        Self
    }

    fn make_rng() -> crate::triage::param_space::SeededRng {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64;
        crate::triage::param_space::SeededRng::from_raw(nanos.max(1))
    }

    fn sample_mutation_params(
        mutation_id: &str,
        rng: &mut crate::triage::param_space::SeededRng,
    ) -> Option<serde_json::Value> {
        let registry = default_registry();
        find_param_space(&registry, mutation_id).and_then(|ps| ps.sample_params(rng))
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
        rng: &mut crate::triage::param_space::SeededRng,
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
        let (chosen, rationale) = if rng.coin(EPSILON) {
            let idx = rng.next_usize(scored.len());
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
        rng: &mut crate::triage::param_space::SeededRng,
    ) -> (Vec<MutationSpec>, String) {
        let avoid_set: HashSet<&str> = guidance.avoid_tokens.iter().map(|s| s.as_str()).collect();
        let seek_set: HashSet<&str> = guidance.seek_tokens.iter().map(|s| s.as_str()).collect();

        // Fixed mutations always applied
        let mut mutations: Vec<MutationSpec> = search_space
            .fixed_mutations
            .iter()
            .map(|id| MutationSpec {
                id: id.clone(),
                params: Self::sample_mutation_params(id, rng),
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
        let (chosen, rationale) = if rng.coin(EPSILON) {
            let idx = rng.next_usize(scored.len());
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
            id: chosen.clone(),
            params: Self::sample_mutation_params(&chosen, rng),
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
    /// Token-guided accumulation: build on best recipe using token bias.
    ///
    /// 1. Reconstruct best recipe + marginals
    /// 2. Token-biased pruning: prune mutations in avoid_tokens AND below threshold
    /// 3. Token-biased addition: score candidates by marginal + seek/avoid + novelty
    /// 4. Epsilon-greedy on scored candidates
    fn token_accumulated(
        &self,
        search_space: &SearchSpace,
        history: &BTreeMap<u32, RoundSummary>,
        guidance: &TriageGuidance,
        rng: &mut crate::triage::param_space::SeededRng,
    ) -> (Vec<MutationSpec>, String) {
        let avoid_set: HashSet<&str> = guidance.avoid_tokens.iter().map(|s| s.as_str()).collect();
        let seek_set: HashSet<&str> = guidance.seek_tokens.iter().map(|s| s.as_str()).collect();

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
        let config = &search_space.accumulation;

        // Fixed mutations always applied
        let mut mutations: Vec<MutationSpec> = search_space
            .fixed_mutations
            .iter()
            .map(|id| MutationSpec {
                id: id.clone(),
                params: Self::sample_mutation_params(id, rng),
            })
            .collect();

        // 1. Reconstruct best recipe
        let (best_recipe, best_score) = reconstruct_best_recipe(history, &fixed_set);
        let marginals = compute_marginal_contributions(history, &fixed_set);

        // 2. Token-biased pruning: prune if in avoid_tokens AND below marginal threshold
        let mut recipe: Vec<MutationSpec> = best_recipe
            .iter()
            .filter(|m| {
                let token = format!("mutation:{}", m.id);
                let is_avoided = avoid_set.contains(token.as_str());
                let marginal = marginals.get(&m.id).copied().unwrap_or(0.0);
                // If avoided AND marginal is below threshold, prune
                if is_avoided && marginal < config.prune_threshold {
                    return false;
                }
                // Otherwise apply standard pruning
                marginal >= config.prune_threshold || !marginals.contains_key(&m.id)
            })
            .cloned()
            .collect();

        let max_size = effective_max_recipe_size(config, pool.len());

        // 3. Token-biased addition: score candidates not yet in recipe
        let recipe_ids: HashSet<&str> = recipe.iter().map(|m| m.id.as_str()).collect();
        let mut candidates: Vec<(String, f64)> = pool
            .iter()
            .filter(|m| !recipe_ids.contains(*m))
            .map(|m| {
                let token = format!("mutation:{}", m);
                let mut score = marginals.get(*m).copied().unwrap_or(0.0);

                // Token bias
                if seek_set.contains(token.as_str()) {
                    score += 0.3;
                }
                if avoid_set.contains(token.as_str()) {
                    score -= 0.5;
                }
                // Novelty bonus for untried
                if !marginals.contains_key(*m) {
                    score += 0.4;
                }

                (m.to_string(), score)
            })
            .collect();
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // 4. Epsilon-greedy addition
        if !candidates.is_empty() && recipe.len() < max_size {
            let (chosen, _chosen_score) = if rng.coin(EPSILON) {
                let idx = rng.next_usize(candidates.len());
                candidates[idx].clone()
            } else {
                candidates[0].clone()
            };

            recipe.push(MutationSpec {
                id: chosen.clone(),
                params: Self::sample_mutation_params(&chosen, rng),
            });
        } else if !candidates.is_empty() && recipe.len() >= max_size {
            // Replace worst mutation
            let worst_idx = recipe
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let ma = marginals.get(&a.id).copied().unwrap_or(0.0);
                    let mb = marginals.get(&b.id).copied().unwrap_or(0.0);
                    ma.partial_cmp(&mb).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i);
            if let Some(idx) = worst_idx {
                let (chosen, _) = if rng.coin(EPSILON) {
                    let ci = rng.next_usize(candidates.len());
                    candidates[ci].clone()
                } else {
                    candidates[0].clone()
                };
                recipe[idx] = MutationSpec {
                    id: chosen.clone(),
                    params: Self::sample_mutation_params(&chosen, rng),
                };
            }
        }

        // Perturb params
        perturb_recipe_params(&mut recipe, rng, config.perturb_intensity, 0.3);

        let rationale = format!(
            "Token accumulation: recipe={}, best={:.2}, pruned={}",
            recipe.len(),
            best_score,
            best_recipe.len().saturating_sub(recipe.len()),
        );

        mutations.extend(recipe);
        (
            mutations,
            format!(
                "Token: {} fixed | {}",
                search_space.fixed_mutations.len(),
                rationale,
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
        let phase = determine_phase(
            round_number,
            search_space.mutation_pool.len(),
            &search_space.accumulation,
        );

        // Baseline: no mutations
        if phase == AccumulationPhase::Baseline {
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

        let mut rng = Self::make_rng();

        match phase {
            AccumulationPhase::Baseline => unreachable!(),
            AccumulationPhase::IndividualExploration => {
                // Token-guided individual exploration (existing logic)
                match search_space.strategy {
                    VariationStrategy::MutationOnly => {
                        let (mutations, rationale) =
                            self.token_guided_mutations(search_space, history, guidance, &mut rng);
                        Selection {
                            modules: default_modules.clone(),
                            mutations,
                            rationale,
                        }
                    }
                    VariationStrategy::Full => {
                        let (modules, module_rationale) = self.token_guided_modules(
                            search_space,
                            default_modules,
                            history,
                            guidance,
                            &mut rng,
                        );
                        let (mutations, mutation_rationale) =
                            self.token_guided_mutations(search_space, history, guidance, &mut rng);
                        Selection {
                            modules,
                            mutations,
                            rationale: format!(
                                "Module: {} | {}",
                                module_rationale, mutation_rationale
                            ),
                        }
                    }
                }
            }
            AccumulationPhase::Accumulation => {
                // Token-guided accumulation
                match search_space.strategy {
                    VariationStrategy::MutationOnly => {
                        let (mutations, rationale) = self.token_accumulated(
                            search_space,
                            history,
                            guidance,
                            &mut rng,
                        );
                        Selection {
                            modules: default_modules.clone(),
                            mutations,
                            rationale,
                        }
                    }
                    VariationStrategy::Full => {
                        let (modules, module_rationale) = self.token_guided_modules(
                            search_space,
                            default_modules,
                            history,
                            guidance,
                            &mut rng,
                        );
                        let (mutations, mutation_rationale) = self.token_accumulated(
                            search_space,
                            history,
                            guidance,
                            &mut rng,
                        );
                        Selection {
                            modules,
                            mutations,
                            rationale: format!(
                                "Module: {} | {}",
                                module_rationale, mutation_rationale
                            ),
                        }
                    }
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

        // Run multiple times (100 iterations for statistical stability)
        let mut fill_pattern_count = 0;
        let mut decon_rounds_count = 0;
        for _ in 0..100 {
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

        // fill_pattern should be strongly favored over decon_rounds
        // Expected: ~74 fill_pattern vs ~4 decon_rounds (with ε=0.3, 7-item pool)
        assert!(
            fill_pattern_count > decon_rounds_count * 2,
            "fill_pattern ({}) should be strongly favored over decon_rounds ({})",
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

    // ==========================================================================
    // Accumulation phase tests
    // ==========================================================================

    #[tokio::test]
    async fn test_token_accumulation_uses_guidance() {
        let selector = TokenSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let ss = SearchSpace::default();

        // Build individual exploration history
        let mut history = BTreeMap::new();
        history.insert(
            1,
            make_summary(1, "none", 0.2, DifferentialCategory::RealDetection, vec![]),
        );
        for (i, pool_id) in ss.mutation_pool.iter().enumerate() {
            let mut mutations: Vec<String> = ss.fixed_mutations.clone();
            mutations.push(pool_id.clone());
            let score = 0.5;
            history.insert(
                (i + 2) as u32,
                make_summary(
                    (i + 2) as u32,
                    "none",
                    score,
                    DifferentialCategory::Evasion,
                    mutations,
                ),
            );
        }

        let guidance = TriageGuidance {
            avoid_tokens: vec!["mutation:ast.decon_rounds".to_string()],
            seek_tokens: vec!["mutation:ast.fill_pattern".to_string()],
        };

        let round = ss.mutation_pool.len() as u32 + 2;
        let selection = selector
            .select("job-1", round, &ss, &defaults, &history, Some(&guidance))
            .await;

        assert!(
            selection.rationale.contains("Token accumulation"),
            "Should be in token accumulation phase. Got: {}",
            selection.rationale
        );
    }

    #[tokio::test]
    async fn test_token_accumulation_disabled_stays_exploration() {
        let selector = TokenSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let mut ss = SearchSpace::default();
        ss.accumulation.enabled = false;

        let guidance = TriageGuidance {
            avoid_tokens: vec![],
            seek_tokens: vec![],
        };

        // Round well past pool_size+1
        let round = ss.mutation_pool.len() as u32 + 5;
        let selection = selector
            .select("job-1", round, &ss, &defaults, &history, Some(&guidance))
            .await;

        // Should NOT be in accumulation phase
        assert!(
            !selection.rationale.contains("accumulation"),
            "Disabled accumulation should not produce accumulation rationale. Got: {}",
            selection.rationale
        );
    }
}
