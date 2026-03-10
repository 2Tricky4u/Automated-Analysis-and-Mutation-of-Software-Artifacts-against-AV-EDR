//! Accumulation utilities — shared logic for progressive recipe building.
//!
//! All 4 selectors use these utilities to transition from individual
//! exploration to recipe accumulation. The accumulation phase builds on
//! the best recipe from history, pruning low-marginal mutations and
//! adding new ones.
//!
//! Toggle: `AccumulationConfig.enabled` (default: `true`).
//! Set `false` to restore exact legacy behavior (no phase transition).

use crate::dispatch::types::{MutationSpec, RoundSummary};
use crate::triage::param_space::{SeededRng, default_registry, find_param_space};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Configuration for the accumulation phase.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccumulationConfig {
    /// Whether accumulation is enabled. Default: `true`.
    /// When `false`, selectors stay in IndividualExploration forever.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Maximum number of pool mutations in a recipe. Default: `None` (= pool.len()).
    #[serde(default)]
    pub max_recipe_size: Option<usize>,
    /// Marginal contribution threshold for pruning. Default: `-0.05`.
    /// Mutations with marginal contribution below this are pruned.
    #[serde(default = "default_prune_threshold")]
    pub prune_threshold: f64,
    /// Intensity of parameter perturbation during accumulation. Default: `0.15`.
    #[serde(default = "default_perturb_intensity")]
    pub perturb_intensity: f64,
    /// Window size for stagnation detection. Default: `5`.
    #[serde(default = "default_stagnation_window")]
    pub stagnation_window: usize,
    /// Diversity threshold below which a restart is triggered. Default: `0.15`.
    #[serde(default = "default_diversity_threshold")]
    pub diversity_threshold: f64,
}

fn default_enabled() -> bool {
    true
}
fn default_prune_threshold() -> f64 {
    -0.05
}
fn default_perturb_intensity() -> f64 {
    0.15
}
fn default_stagnation_window() -> usize {
    5
}
fn default_diversity_threshold() -> f64 {
    0.15
}

impl Default for AccumulationConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_recipe_size: None,
            prune_threshold: default_prune_threshold(),
            perturb_intensity: default_perturb_intensity(),
            stagnation_window: default_stagnation_window(),
            diversity_threshold: default_diversity_threshold(),
        }
    }
}

/// Accumulation phase for a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccumulationPhase {
    /// Round 1: no mutations, control measurement.
    Baseline,
    /// Rounds 2 to `pool_size + 1`: each pool mutation tested individually.
    IndividualExploration,
    /// Rounds `pool_size + 2` onward: build on best recipe.
    Accumulation,
}

/// Determine which phase a round belongs to.
///
/// - Round 1 is always Baseline.
/// - Rounds 2..=pool_size+1 are IndividualExploration.
/// - Rounds pool_size+2+ are Accumulation (only if `config.enabled`).
/// - When `config.enabled == false`, rounds pool_size+2+ stay IndividualExploration.
pub fn determine_phase(
    round_number: u32,
    pool_size: usize,
    config: &AccumulationConfig,
) -> AccumulationPhase {
    if round_number <= 1 {
        return AccumulationPhase::Baseline;
    }
    // No pool means nothing to accumulate — stay in exploration forever
    if pool_size == 0 {
        return AccumulationPhase::IndividualExploration;
    }
    let exploration_end = pool_size as u32 + 1;
    if round_number <= exploration_end {
        return AccumulationPhase::IndividualExploration;
    }
    if config.enabled {
        AccumulationPhase::Accumulation
    } else {
        AccumulationPhase::IndividualExploration
    }
}

/// Reconstruct the best recipe from history.
///
/// Finds the trustworthy round with the highest evasion score and returns
/// its pool mutations (excluding fixed mutations) plus the score.
/// Returns `(empty vec, 0.0)` if no trustworthy rounds exist.
pub fn reconstruct_best_recipe(
    history: &BTreeMap<u32, RoundSummary>,
    fixed_set: &HashSet<&str>,
) -> (Vec<MutationSpec>, f64) {
    let best = history
        .values()
        .filter(|s| s.differential_category.is_trustworthy())
        .max_by(|a, b| {
            a.evasion_score
                .partial_cmp(&b.evasion_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    match best {
        Some(summary) => {
            let pool_specs: Vec<MutationSpec> = summary
                .mutation_specs
                .iter()
                .filter(|m| !fixed_set.contains(m.id.as_str()))
                .cloned()
                .collect();
            (pool_specs, summary.evasion_score)
        }
        None => (vec![], 0.0),
    }
}

/// Compute marginal contributions of individual mutations from history.
///
/// For each pool mutation, computes how its presence correlates with evasion
/// score changes. Uses individual exploration rounds (exactly 1 pool mutation)
/// vs baseline (0 pool mutations) to estimate marginals.
///
/// Returns a map from mutation ID to marginal contribution (can be negative).
pub fn compute_marginal_contributions(
    history: &BTreeMap<u32, RoundSummary>,
    fixed_set: &HashSet<&str>,
) -> HashMap<String, f64> {
    let mut marginals: HashMap<String, f64> = HashMap::new();

    // Find baseline score (rounds with 0 pool mutations)
    let mut baseline_scores: Vec<f64> = Vec::new();
    let mut individual_scores: HashMap<String, Vec<f64>> = HashMap::new();

    for summary in history.values() {
        if !summary.differential_category.is_trustworthy() {
            continue;
        }
        let pool_mutations: Vec<&String> = summary
            .mutations
            .iter()
            .filter(|m| !fixed_set.contains(m.as_str()))
            .collect();

        match pool_mutations.len() {
            0 => baseline_scores.push(summary.evasion_score),
            1 => {
                individual_scores
                    .entry(pool_mutations[0].clone())
                    .or_default()
                    .push(summary.evasion_score);
            }
            _ => {} // Skip confounded multi-mutation rounds (ablation requires isolation)
        }
    }

    let baseline_mean = if baseline_scores.is_empty() {
        0.0
    } else {
        baseline_scores.iter().sum::<f64>() / baseline_scores.len() as f64
    };

    for (mutation_id, scores) in &individual_scores {
        let mean = scores.iter().sum::<f64>() / scores.len() as f64;
        marginals.insert(mutation_id.clone(), mean - baseline_mean);
    }

    marginals
}

/// Prune mutations with marginal contribution below threshold.
///
/// Returns the pruned recipe (mutations with marginals >= threshold or
/// mutations not found in marginals, which are kept by default).
pub fn prune_recipe(
    recipe: &[MutationSpec],
    marginals: &HashMap<String, f64>,
    threshold: f64,
) -> Vec<MutationSpec> {
    recipe
        .iter()
        .filter(|m| {
            match marginals.get(&m.id) {
                Some(&marginal) => marginal >= threshold,
                // Keep mutations we don't have data for
                None => true,
            }
        })
        .cloned()
        .collect()
}

/// Perturb params of mutations in a recipe.
///
/// Each mutation has `probability` chance of being perturbed with `intensity`.
pub fn perturb_recipe_params(
    recipe: &mut [MutationSpec],
    rng: &mut SeededRng,
    intensity: f64,
    probability: f64,
) {
    let registry = default_registry();
    for spec in recipe.iter_mut() {
        if rng.coin(probability) {
            if let Some(ps) = find_param_space(&registry, &spec.id) {
                spec.params = ps.perturb_params(spec.params.as_ref(), rng, intensity);
            }
        }
    }
}

/// Effective max recipe size, falling back to pool_size if not configured.
pub fn effective_max_recipe_size(config: &AccumulationConfig, pool_size: usize) -> usize {
    config.max_recipe_size.unwrap_or(pool_size).max(1)
}

/// Decaying epsilon: ε(r) = ε_min + (ε₀ - ε_min) / (1 + α * r_acc)
///
/// During IndividualExploration (round_number <= pool_size+1), returns ~ε₀.
/// After exploration, decays toward ε_min as rounds accumulate.
///
/// - ε₀ = 0.3 (initial exploration rate)
/// - ε_min = 0.05 (minimum exploration rate)
/// - α = 0.1 (decay rate)
pub fn decaying_epsilon(round_number: u32, pool_size: usize) -> f64 {
    let epsilon_initial = 0.3;
    let epsilon_min = 0.05;
    let decay_rate = 0.1;
    let exploration_end = pool_size as u32 + 1;
    let rounds_past = round_number.saturating_sub(exploration_end) as f64;
    epsilon_min + (epsilon_initial - epsilon_min) / (1.0 + decay_rate * rounds_past)
}

/// Compute diversity of recent recipes via mean pairwise Jaccard distance.
///
/// Returns a value in `[0.0, 1.0]`:
/// - `0.0` = all recent recipes are identical
/// - `1.0` = all recent recipes are completely disjoint
///
/// Only considers trustworthy rounds and excludes fixed mutations.
pub fn compute_recipe_diversity(
    history: &BTreeMap<u32, RoundSummary>,
    fixed_set: &HashSet<&str>,
    window: usize,
) -> f64 {
    let recent_sets: Vec<HashSet<&str>> = history
        .values()
        .rev()
        .filter(|s| s.differential_category.is_trustworthy())
        .take(window)
        .map(|s| {
            s.mutations
                .iter()
                .filter(|m| !fixed_set.contains(m.as_str()))
                .map(|m| m.as_str())
                .collect::<HashSet<_>>()
        })
        .collect();

    if recent_sets.len() < 2 {
        return 1.0; // Not enough data — assume diverse
    }

    let mut total_distance = 0.0;
    let mut pair_count = 0u32;

    for i in 0..recent_sets.len() {
        for j in (i + 1)..recent_sets.len() {
            let intersection = recent_sets[i].intersection(&recent_sets[j]).count();
            let union = recent_sets[i].union(&recent_sets[j]).count();
            let jaccard_distance = if union == 0 {
                0.0 // Both empty — identical
            } else {
                1.0 - (intersection as f64 / union as f64)
            };
            total_distance += jaccard_distance;
            pair_count += 1;
        }
    }

    if pair_count == 0 {
        1.0
    } else {
        total_distance / pair_count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{DifferentialCategory, ModuleSelectionSpec, RoundId};
    use std::time::SystemTime;

    fn make_summary(
        round_number: u32,
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
            modules: ModuleSelectionSpec::default(),
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

    // ========== determine_phase ==========

    #[test]
    fn test_determine_phase_baseline() {
        let config = AccumulationConfig::default();
        assert_eq!(determine_phase(1, 10, &config), AccumulationPhase::Baseline);
        assert_eq!(determine_phase(0, 10, &config), AccumulationPhase::Baseline);
    }

    #[test]
    fn test_determine_phase_individual_exploration() {
        let config = AccumulationConfig::default();
        // pool_size=10: rounds 2..=11 are IndividualExploration
        for round in 2..=11 {
            assert_eq!(
                determine_phase(round, 10, &config),
                AccumulationPhase::IndividualExploration,
                "Round {} should be IndividualExploration",
                round,
            );
        }
    }

    #[test]
    fn test_determine_phase_accumulation() {
        let config = AccumulationConfig::default();
        // pool_size=10: round 12+ is Accumulation
        assert_eq!(
            determine_phase(12, 10, &config),
            AccumulationPhase::Accumulation,
        );
        assert_eq!(
            determine_phase(100, 10, &config),
            AccumulationPhase::Accumulation,
        );
    }

    #[test]
    fn test_determine_phase_disabled_stays_exploration() {
        let config = AccumulationConfig {
            enabled: false,
            ..Default::default()
        };
        // With accumulation disabled, round 12+ stays IndividualExploration
        assert_eq!(
            determine_phase(12, 10, &config),
            AccumulationPhase::IndividualExploration,
        );
        assert_eq!(
            determine_phase(100, 10, &config),
            AccumulationPhase::IndividualExploration,
        );
    }

    #[test]
    fn test_determine_phase_empty_pool_stays_exploration() {
        let config = AccumulationConfig::default();
        // pool_size=0: never enter accumulation
        assert_eq!(
            determine_phase(2, 0, &config),
            AccumulationPhase::IndividualExploration,
        );
        assert_eq!(
            determine_phase(100, 0, &config),
            AccumulationPhase::IndividualExploration,
        );
    }

    #[test]
    fn test_determine_phase_small_pool() {
        let config = AccumulationConfig::default();
        // pool_size=1: round 2 is exploration, round 3+ is accumulation
        assert_eq!(
            determine_phase(2, 1, &config),
            AccumulationPhase::IndividualExploration,
        );
        assert_eq!(
            determine_phase(3, 1, &config),
            AccumulationPhase::Accumulation,
        );
    }

    // ========== reconstruct_best_recipe ==========

    #[test]
    fn test_reconstruct_best_recipe_empty_history() {
        let history = BTreeMap::new();
        let fixed_set = HashSet::new();
        let (recipe, score) = reconstruct_best_recipe(&history, &fixed_set);
        assert!(recipe.is_empty());
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_reconstruct_best_recipe_finds_highest() {
        let mut history = BTreeMap::new();
        let fixed_set: HashSet<&str> = ["binary.rich_header"].into_iter().collect();

        history.insert(
            2,
            make_summary(
                2,
                0.3,
                DifferentialCategory::RealDetection,
                vec![
                    "binary.rich_header".to_string(),
                    "ast.decon_rounds".to_string(),
                ],
            ),
        );
        history.insert(
            3,
            make_summary(
                3,
                0.8,
                DifferentialCategory::Evasion,
                vec![
                    "binary.rich_header".to_string(),
                    "ast.fill_pattern".to_string(),
                ],
            ),
        );
        history.insert(
            4,
            make_summary(
                4,
                0.5,
                DifferentialCategory::RealDetection,
                vec![
                    "binary.rich_header".to_string(),
                    "ast.string_xor".to_string(),
                ],
            ),
        );

        let (recipe, score) = reconstruct_best_recipe(&history, &fixed_set);
        assert_eq!(score, 0.8);
        assert_eq!(recipe.len(), 1);
        assert_eq!(recipe[0].id, "ast.fill_pattern");
    }

    #[test]
    fn test_reconstruct_best_recipe_skips_untrustworthy() {
        let mut history = BTreeMap::new();
        let fixed_set = HashSet::new();

        // High score but not trustworthy
        history.insert(
            2,
            make_summary(
                2,
                0.9,
                DifferentialCategory::InstrumentationArtifact,
                vec!["ast.decon_rounds".to_string()],
            ),
        );
        // Lower score but trustworthy
        history.insert(
            3,
            make_summary(
                3,
                0.4,
                DifferentialCategory::RealDetection,
                vec!["ast.fill_pattern".to_string()],
            ),
        );

        let (recipe, score) = reconstruct_best_recipe(&history, &fixed_set);
        assert_eq!(score, 0.4);
        assert_eq!(recipe[0].id, "ast.fill_pattern");
    }

    // ========== compute_marginal_contributions ==========

    #[test]
    fn test_marginal_contributions_basic() {
        let mut history = BTreeMap::new();
        let fixed_set: HashSet<&str> = ["binary.rich_header"].into_iter().collect();

        // Baseline (no pool mutations): score 0.2
        history.insert(
            1,
            make_summary(
                1,
                0.2,
                DifferentialCategory::RealDetection,
                vec!["binary.rich_header".to_string()],
            ),
        );
        // Individual with ast.decon_rounds: score 0.6 → marginal +0.4
        history.insert(
            2,
            make_summary(
                2,
                0.6,
                DifferentialCategory::Evasion,
                vec![
                    "binary.rich_header".to_string(),
                    "ast.decon_rounds".to_string(),
                ],
            ),
        );
        // Individual with ast.fill_pattern: score 0.1 → marginal -0.1
        history.insert(
            3,
            make_summary(
                3,
                0.1,
                DifferentialCategory::RealDetection,
                vec![
                    "binary.rich_header".to_string(),
                    "ast.fill_pattern".to_string(),
                ],
            ),
        );

        let marginals = compute_marginal_contributions(&history, &fixed_set);
        assert!(
            (marginals["ast.decon_rounds"] - 0.4).abs() < 1e-9,
            "decon_rounds marginal should be 0.4, got {}",
            marginals["ast.decon_rounds"]
        );
        assert!(
            (marginals["ast.fill_pattern"] - (-0.1)).abs() < 1e-9,
            "fill_pattern marginal should be -0.1, got {}",
            marginals["ast.fill_pattern"]
        );
    }

    // ========== prune_recipe ==========

    #[test]
    fn test_prune_recipe_removes_below_threshold() {
        let recipe = vec![
            MutationSpec {
                id: "ast.decon_rounds".to_string(),
                params: None,
            },
            MutationSpec {
                id: "ast.fill_pattern".to_string(),
                params: None,
            },
            MutationSpec {
                id: "ast.string_xor".to_string(),
                params: None,
            },
        ];
        let mut marginals = HashMap::new();
        marginals.insert("ast.decon_rounds".to_string(), 0.3);
        marginals.insert("ast.fill_pattern".to_string(), -0.1);
        marginals.insert("ast.string_xor".to_string(), 0.0);

        let pruned = prune_recipe(&recipe, &marginals, -0.05);
        let ids: Vec<&str> = pruned.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"ast.decon_rounds"));
        assert!(!ids.contains(&"ast.fill_pattern")); // -0.1 < -0.05
        assert!(ids.contains(&"ast.string_xor")); // 0.0 >= -0.05
    }

    #[test]
    fn test_prune_recipe_keeps_unknown_mutations() {
        let recipe = vec![MutationSpec {
            id: "ast.unknown".to_string(),
            params: None,
        }];
        let marginals = HashMap::new(); // no data
        let pruned = prune_recipe(&recipe, &marginals, -0.05);
        assert_eq!(pruned.len(), 1);
    }

    // ========== effective_max_recipe_size ==========

    #[test]
    fn test_effective_max_recipe_size_default() {
        let config = AccumulationConfig::default();
        assert_eq!(effective_max_recipe_size(&config, 10), 10);
    }

    #[test]
    fn test_effective_max_recipe_size_explicit() {
        let config = AccumulationConfig {
            max_recipe_size: Some(5),
            ..Default::default()
        };
        assert_eq!(effective_max_recipe_size(&config, 10), 5);
    }

    #[test]
    fn test_effective_max_recipe_size_clamped_to_1() {
        let config = AccumulationConfig {
            max_recipe_size: Some(0),
            ..Default::default()
        };
        assert_eq!(effective_max_recipe_size(&config, 10), 1);
    }

    // ========== perturb_recipe_params ==========

    #[test]
    fn test_perturb_recipe_params_modifies() {
        let mut recipe = vec![MutationSpec {
            id: "ast.decon_rounds".to_string(),
            params: Some(serde_json::json!({"count": "100", "method": "fixed"})),
        }];
        let mut rng = SeededRng::new("test", 42);
        // probability=1.0 guarantees perturbation
        perturb_recipe_params(&mut recipe, &mut rng, 0.3, 1.0);
        // Params should be present (may or may not have changed values, but structure preserved)
        assert!(recipe[0].params.is_some());
    }

    // ========== marginal contributions: multi-mutation exclusion ==========

    #[test]
    fn test_marginal_contributions_skips_multi_mutation_rounds() {
        let mut history = BTreeMap::new();
        let fixed_set: HashSet<&str> = ["binary.rich_header"].into_iter().collect();

        // Baseline: score 0.2
        history.insert(
            1,
            make_summary(
                1,
                0.2,
                DifferentialCategory::RealDetection,
                vec!["binary.rich_header".to_string()],
            ),
        );
        // Individual with ast.decon_rounds: score 0.6 → marginal +0.4
        history.insert(
            2,
            make_summary(
                2,
                0.6,
                DifferentialCategory::Evasion,
                vec![
                    "binary.rich_header".to_string(),
                    "ast.decon_rounds".to_string(),
                ],
            ),
        );
        // Multi-mutation round with high score — should NOT inflate marginals
        history.insert(
            3,
            make_summary(
                3,
                0.95,
                DifferentialCategory::Evasion,
                vec![
                    "binary.rich_header".to_string(),
                    "ast.decon_rounds".to_string(),
                    "ast.fill_pattern".to_string(),
                ],
            ),
        );

        let marginals = compute_marginal_contributions(&history, &fixed_set);
        // ast.decon_rounds should only get 0.6 - 0.2 = 0.4, NOT inflated by the 0.95 round
        assert!(
            (marginals["ast.decon_rounds"] - 0.4).abs() < 1e-9,
            "decon_rounds marginal should be 0.4, got {}",
            marginals["ast.decon_rounds"]
        );
        // ast.fill_pattern should have NO marginal entry (only appeared in multi-mutation round)
        assert!(
            !marginals.contains_key("ast.fill_pattern"),
            "fill_pattern should have no marginal (only in multi-mutation round)"
        );
    }

    // ========== decaying_epsilon ==========

    #[test]
    fn test_decaying_epsilon_during_exploration() {
        // During exploration phase, rounds_past = 0, epsilon ≈ 0.3
        let eps = decaying_epsilon(5, 10);
        assert!(
            (eps - 0.3).abs() < 0.01,
            "During exploration, ε should be ~0.3, got {}",
            eps
        );
    }

    #[test]
    fn test_decaying_epsilon_boundary_values() {
        let pool_size = 10;
        // Round 12 (1 past exploration): ε ≈ 0.05 + 0.25/(1+0.1*1) ≈ 0.277
        let eps12 = decaying_epsilon(12, pool_size);
        assert!(
            (eps12 - 0.277).abs() < 0.01,
            "ε(12,10) should be ~0.277, got {}",
            eps12
        );

        // Round 22 (11 past): ε ≈ 0.05 + 0.25/(1+1.1) ≈ 0.169
        let eps22 = decaying_epsilon(22, pool_size);
        assert!(
            eps22 < 0.2 && eps22 > 0.1,
            "ε(22,10) should be ~0.17, got {}",
            eps22
        );

        // Round 100 (89 past): ε ≈ 0.05 + 0.25/(1+8.9) ≈ 0.075
        let eps100 = decaying_epsilon(100, pool_size);
        assert!(
            eps100 < 0.1 && eps100 > 0.05,
            "ε(100,10) should approach 0.05, got {}",
            eps100
        );
    }

    #[test]
    fn test_decaying_epsilon_monotonically_decreasing() {
        let pool_size = 10;
        let mut prev = decaying_epsilon(12, pool_size);
        for r in 13..=100 {
            let eps = decaying_epsilon(r, pool_size);
            assert!(
                eps <= prev + 1e-12,
                "ε should be monotonically decreasing: ε({})={} > ε({})={}",
                r,
                eps,
                r - 1,
                prev
            );
            prev = eps;
        }
    }

    // ========== compute_recipe_diversity ==========

    #[test]
    fn test_diversity_identical_recipes() {
        let mut history = BTreeMap::new();
        let fixed_set: HashSet<&str> = ["binary.rich_header"].into_iter().collect();

        // All rounds have the same pool mutation
        for i in 1..=5 {
            history.insert(
                i,
                make_summary(
                    i,
                    0.5,
                    DifferentialCategory::Evasion,
                    vec![
                        "binary.rich_header".to_string(),
                        "ast.decon_rounds".to_string(),
                    ],
                ),
            );
        }

        let diversity = compute_recipe_diversity(&history, &fixed_set, 5);
        assert!(
            diversity.abs() < 1e-9,
            "Identical recipes should have diversity 0.0, got {}",
            diversity
        );
    }

    #[test]
    fn test_diversity_disjoint_recipes() {
        let mut history = BTreeMap::new();
        let fixed_set: HashSet<&str> = ["binary.rich_header"].into_iter().collect();

        let pool_mutations = [
            "ast.decon_rounds",
            "ast.fill_pattern",
            "ast.string_xor",
            "ast.exec_decoy",
            "ast.timing_pattern",
        ];
        for (i, m) in pool_mutations.iter().enumerate() {
            history.insert(
                (i + 1) as u32,
                make_summary(
                    (i + 1) as u32,
                    0.5,
                    DifferentialCategory::Evasion,
                    vec!["binary.rich_header".to_string(), m.to_string()],
                ),
            );
        }

        let diversity = compute_recipe_diversity(&history, &fixed_set, 5);
        assert!(
            (diversity - 1.0).abs() < 1e-9,
            "Disjoint recipes should have diversity 1.0, got {}",
            diversity
        );
    }

    #[test]
    fn test_diversity_insufficient_data() {
        let mut history = BTreeMap::new();
        let fixed_set = HashSet::new();

        history.insert(
            1,
            make_summary(1, 0.5, DifferentialCategory::Evasion, vec!["ast.decon_rounds".to_string()]),
        );

        // Only 1 recipe — should return 1.0 (assume diverse)
        let diversity = compute_recipe_diversity(&history, &fixed_set, 5);
        assert!(
            (diversity - 1.0).abs() < 1e-9,
            "Single recipe should return 1.0, got {}",
            diversity
        );
    }
}
