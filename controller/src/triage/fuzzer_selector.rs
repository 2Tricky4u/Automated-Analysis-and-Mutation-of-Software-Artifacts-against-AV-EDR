//! FuzzerSelector — evolutionary mutation exploration with parameter variation.
//!
//! Treats mutation recipes as evolving chromosomes — exploring parameter spaces,
//! combinations, and learning from round-by-round feedback via a genetic algorithm.
//!
//! Stateless design: reconstructs population from `history` each call.
//! Deterministic given a seeded RNG (job_id + round_number).

use super::coverage_selector::select_modules;
use super::{SearchSpace, Selection, Selector, TriageGuidance, VariationStrategy};
use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
use crate::triage::accumulation::{
    AccumulationPhase, compute_marginal_contributions, compute_recipe_diversity, determine_phase,
    effective_max_recipe_size, prune_recipe, reconstruct_best_recipe,
};
use crate::triage::param_space::{SeededRng, default_registry, find_param_space};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Configuration for the FuzzerSelector's genetic algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzerConfig {
    /// Population size (recipes per generation). Default: 10.
    #[serde(default = "default_population_size")]
    pub population_size: usize,
    /// Number of elite recipes kept across generations. Default: 2.
    #[serde(default = "default_elitism")]
    pub elitism: usize,
    /// Probability of param perturbation during recipe mutation. Default: 0.3.
    #[serde(default = "default_param_mutation_rate")]
    pub param_mutation_rate: f64,
    /// Probability of adding/removing a mutation during recipe mutation. Default: 0.2.
    #[serde(default = "default_structural_mutation_rate")]
    pub structural_mutation_rate: f64,
    /// Min mutations from pool per recipe. Default: 1.
    #[serde(default = "default_min_pool")]
    pub min_pool_mutations: usize,
    /// Max mutations from pool per recipe. Default: 5.
    #[serde(default = "default_max_pool")]
    pub max_pool_mutations: usize,
    /// Whether to vary params on fixed mutations too. Default: true.
    #[serde(default = "default_vary_fixed")]
    pub vary_fixed_params: bool,
}

fn default_population_size() -> usize {
    10
}
fn default_elitism() -> usize {
    2
}
fn default_param_mutation_rate() -> f64 {
    0.3
}
fn default_structural_mutation_rate() -> f64 {
    0.2
}
fn default_min_pool() -> usize {
    1
}
fn default_max_pool() -> usize {
    5
}
fn default_vary_fixed() -> bool {
    true
}

impl Default for FuzzerConfig {
    fn default() -> Self {
        Self {
            population_size: default_population_size(),
            elitism: default_elitism(),
            param_mutation_rate: default_param_mutation_rate(),
            structural_mutation_rate: default_structural_mutation_rate(),
            min_pool_mutations: default_min_pool(),
            max_pool_mutations: default_max_pool(),
            vary_fixed_params: default_vary_fixed(),
        }
    }
}

/// A mutation recipe — a set of mutations with specific params (genome).
#[derive(Debug, Clone)]
struct Recipe {
    mutations: Vec<MutationSpec>,
    fitness: Option<f64>,
    generation: u32,
    rationale: String,
}

/// Stateless evolutionary selector.
///
/// Treats mutation recipes as chromosomes and evolves them across rounds using
/// tournament selection, crossover, and param/structural mutation operators.
/// All state is reconstructed from the `history` map on each call, so the
/// struct itself carries no mutable data. Deterministic given a seeded RNG
/// derived from `(job_id, round_number)`.
pub struct FuzzerSelector;

impl Default for FuzzerSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl FuzzerSelector {
    /// Create a new evolutionary selector (no internal state).
    pub fn new() -> Self {
        Self
    }

    /// Generate a random recipe from the search space.
    fn random_recipe(
        &self,
        search_space: &SearchSpace,
        config: &FuzzerConfig,
        rng: &mut SeededRng,
    ) -> Recipe {
        let registry = default_registry();
        let mut mutations = Vec::new();

        // Fixed mutations (always included)
        for id in &search_space.fixed_mutations {
            let params = if config.vary_fixed_params {
                find_param_space(&registry, id).and_then(|ps| ps.sample_params(rng))
            } else {
                None
            };
            mutations.push(MutationSpec {
                id: id.clone(),
                params,
            });
        }

        // Random subset from pool
        let pool = &search_space.mutation_pool;
        if !pool.is_empty() {
            let max = config.max_pool_mutations.min(pool.len());
            let min = config.min_pool_mutations.min(max);
            let count = if min == max {
                min
            } else {
                min + rng.next_usize(max - min + 1)
            };

            // Shuffle pool indices and pick first `count`
            let mut indices: Vec<usize> = (0..pool.len()).collect();
            // Fisher-Yates shuffle
            for i in (1..indices.len()).rev() {
                let j = rng.next_usize(i + 1);
                indices.swap(i, j);
            }

            for &idx in indices.iter().take(count) {
                let id = &pool[idx];
                let params = find_param_space(&registry, id).and_then(|ps| ps.sample_params(rng));
                mutations.push(MutationSpec {
                    id: id.clone(),
                    params,
                });
            }
        }

        let pool_count = mutations.len() - search_space.fixed_mutations.len();
        Recipe {
            mutations,
            fitness: None,
            generation: 0,
            rationale: format!(
                "random seed ({}+{} pool mutations)",
                search_space.fixed_mutations.len(),
                pool_count
            ),
        }
    }

    /// Evolve a new recipe from history using genetic operators.
    ///
    /// Retained for backward compatibility when accumulation is disabled;
    /// the main path now uses `evolve_accumulated` which adds elite injection
    /// and growth bias.
    #[allow(dead_code)]
    fn evolve_recipe(
        &self,
        search_space: &SearchSpace,
        history: &BTreeMap<u32, RoundSummary>,
        config: &FuzzerConfig,
        rng: &mut SeededRng,
    ) -> Recipe {
        let registry = default_registry();

        // Reconstruct population from history
        let mut population: Vec<Recipe> = history
            .values()
            .filter(|s| s.differential_category.is_trustworthy())
            .map(|s| Recipe {
                mutations: s.mutation_specs.clone(),
                fitness: Some(s.evasion_score),
                generation: 0, // historical
                rationale: String::new(),
            })
            .collect();

        // If population is too small, fall back to random
        if population.len() < 2 {
            return self.random_recipe(search_space, config, rng);
        }

        // Sort by fitness descending
        population.sort_by(|a, b| {
            b.fitness
                .unwrap_or(0.0)
                .partial_cmp(&a.fitness.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Compute generation number
        let generation = (history.len() as u32).saturating_sub(config.population_size as u32) + 1;

        // Select parents via tournament selection
        let parent_a = self.tournament_select(&population, rng);
        let parent_b = self.tournament_select(&population, rng);

        // Crossover
        let mut offspring = self.crossover(&parent_a, &parent_b, search_space, rng);

        // Param mutation
        if rng.coin(config.param_mutation_rate) && !offspring.mutations.is_empty() {
            let idx = rng.next_usize(offspring.mutations.len());
            let spec = &offspring.mutations[idx];
            if let Some(ps) = find_param_space(&registry, &spec.id) {
                let new_params = ps.perturb_params(spec.params.as_ref(), rng, 0.2);
                offspring.mutations[idx].params = new_params;
            }
        }

        // Structural mutation: add or remove a pool mutation
        if rng.coin(config.structural_mutation_rate) {
            let pool_mutations: Vec<&MutationSpec> = offspring
                .mutations
                .iter()
                .filter(|m| search_space.mutation_pool.contains(&m.id))
                .collect();
            let pool_count = pool_mutations.len();

            if pool_count > config.min_pool_mutations && rng.coin(0.5) {
                // Remove a random pool mutation
                let pool_ids: Vec<String> = pool_mutations.iter().map(|m| m.id.clone()).collect();
                let remove_idx = rng.next_usize(pool_ids.len());
                let remove_id = &pool_ids[remove_idx];
                offspring.mutations.retain(|m| m.id != *remove_id);
            } else if pool_count < config.max_pool_mutations {
                // Add a random pool mutation not already present
                let present: Vec<&str> =
                    offspring.mutations.iter().map(|m| m.id.as_str()).collect();
                let candidates: Vec<&String> = search_space
                    .mutation_pool
                    .iter()
                    .filter(|id| !present.contains(&id.as_str()))
                    .collect();
                if !candidates.is_empty() {
                    let idx = rng.next_usize(candidates.len());
                    let id = candidates[idx];
                    let params =
                        find_param_space(&registry, id).and_then(|ps| ps.sample_params(rng));
                    offspring.mutations.push(MutationSpec {
                        id: id.clone(),
                        params,
                    });
                }
            }
        }

        let pa_fitness = parent_a
            .fitness
            .map(|f| format!("{:.2}", f))
            .unwrap_or("?".into());
        let pb_fitness = parent_b
            .fitness
            .map(|f| format!("{:.2}", f))
            .unwrap_or("?".into());
        offspring.generation = generation;
        offspring.rationale = format!(
            "evolved gen={} (parents: {}, {})",
            generation, pa_fitness, pb_fitness
        );
        offspring
    }

    /// Individual exploration: test one pool mutation per round (rounds 2..=pool_size+1).
    ///
    /// Round N maps to pool index (N-2) % pool.len(). Includes fixed mutations.
    fn individual_exploration(
        &self,
        round_number: u32,
        search_space: &SearchSpace,
        rng: &mut SeededRng,
    ) -> Recipe {
        let registry = default_registry();
        let mut mutations = Vec::new();

        // Fixed mutations
        let config = search_space.fuzzer_config.clone().unwrap_or_default();
        for id in &search_space.fixed_mutations {
            let params = if config.vary_fixed_params {
                find_param_space(&registry, id).and_then(|ps| ps.sample_params(rng))
            } else {
                None
            };
            mutations.push(MutationSpec {
                id: id.clone(),
                params,
            });
        }

        // Pick one pool mutation based on round index
        let pool = &search_space.mutation_pool;
        if !pool.is_empty() {
            let idx = ((round_number as usize).saturating_sub(2)) % pool.len();
            let id = &pool[idx];
            let params = find_param_space(&registry, id).and_then(|ps| ps.sample_params(rng));
            mutations.push(MutationSpec {
                id: id.clone(),
                params,
            });
        }

        Recipe {
            mutations,
            fitness: None,
            generation: 0,
            rationale: format!("individual exploration round {}", round_number,),
        }
    }

    /// Accumulation phase: evolve with elite injection and growth bias.
    ///
    /// 1. Check diversity — restart if converged
    /// 2. Inject best recipe as elite member of the population
    /// 3. Bias structural mutation toward growth (70% add / 30% remove)
    /// 4. Prune marginals below threshold before crossover
    fn evolve_accumulated(
        &self,
        search_space: &SearchSpace,
        history: &BTreeMap<u32, RoundSummary>,
        config: &FuzzerConfig,
        rng: &mut SeededRng,
    ) -> Recipe {
        let registry = default_registry();
        let fixed_set: HashSet<&str> = search_space
            .fixed_mutations
            .iter()
            .map(|s| s.as_str())
            .collect();
        let acc_config = &search_space.accumulation;
        let max_size = effective_max_recipe_size(acc_config, search_space.mutation_pool.len());

        // Diversity check: restart if converged
        let diversity = compute_recipe_diversity(history, &fixed_set, acc_config.stagnation_window);
        if diversity < acc_config.diversity_threshold {
            let mut recipe = self.random_recipe(search_space, config, rng);
            recipe.rationale = format!(
                "diversity restart (div={:.2}) | {}",
                diversity, recipe.rationale
            );
            return recipe;
        }

        // Reconstruct best recipe and marginals
        let (best_recipe, best_score) = reconstruct_best_recipe(history, &fixed_set);
        let marginals = compute_marginal_contributions(history, &fixed_set);
        let pruned_best = prune_recipe(&best_recipe, &marginals, acc_config.prune_threshold);

        // Reconstruct population from history (sliding window: most recent first)
        let window_size = config.population_size.max(2) * 2;
        let mut population: Vec<Recipe> = history
            .values()
            .rev() // Most recent first
            .filter(|s| s.differential_category.is_trustworthy())
            .take(window_size) // Cap population to sliding window
            .map(|s| Recipe {
                mutations: s.mutation_specs.clone(),
                fitness: Some(s.evasion_score),
                generation: 0,
                rationale: String::new(),
            })
            .collect();

        // Inject best recipe as elite (always survives)
        let mut elite_mutations: Vec<MutationSpec> = search_space
            .fixed_mutations
            .iter()
            .map(|id| {
                let params = if config.vary_fixed_params {
                    // Inherit from best recipe if available
                    pruned_best
                        .iter()
                        .find(|m| m.id == *id)
                        .and_then(|m| m.params.clone())
                } else {
                    None
                };
                MutationSpec {
                    id: id.clone(),
                    params,
                }
            })
            .collect();
        elite_mutations.extend(pruned_best.clone());
        population.push(Recipe {
            mutations: elite_mutations,
            fitness: Some(best_score),
            generation: 0,
            rationale: "elite (best recipe)".to_string(),
        });

        // Sort by fitness descending
        population.sort_by(|a, b| {
            b.fitness
                .unwrap_or(0.0)
                .partial_cmp(&a.fitness.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if population.len() < 2 {
            return self.random_recipe(search_space, config, rng);
        }

        let generation =
            (history.len() as u32).saturating_sub(search_space.mutation_pool.len() as u32) + 1;

        // Tournament selection + crossover
        let parent_a = self.tournament_select(&population, rng);
        let parent_b = self.tournament_select(&population, rng);
        let mut offspring = self.crossover(&parent_a, &parent_b, search_space, rng);

        // Param mutation
        if rng.coin(config.param_mutation_rate) && !offspring.mutations.is_empty() {
            let idx = rng.next_usize(offspring.mutations.len());
            let spec = &offspring.mutations[idx];
            if let Some(ps) = find_param_space(&registry, &spec.id) {
                let new_params =
                    ps.perturb_params(spec.params.as_ref(), rng, acc_config.perturb_intensity);
                offspring.mutations[idx].params = new_params;
            }
        }

        // Structural mutation with growth bias (70% add / 30% remove when under max_size)
        if rng.coin(config.structural_mutation_rate) {
            let pool_mutations: Vec<&MutationSpec> = offspring
                .mutations
                .iter()
                .filter(|m| search_space.mutation_pool.contains(&m.id))
                .collect();
            let pool_count = pool_mutations.len();

            // Bias toward growth: 70% add, 30% remove
            let grow = rng.coin(0.7);
            if !grow && pool_count > config.min_pool_mutations {
                // Remove
                let pool_ids: Vec<String> = pool_mutations.iter().map(|m| m.id.clone()).collect();
                // Prefer removing worst-marginal mutation
                let remove_id = pool_ids
                    .iter()
                    .min_by(|a, b| {
                        let ma = marginals.get(a.as_str()).copied().unwrap_or(0.0);
                        let mb = marginals.get(b.as_str()).copied().unwrap_or(0.0);
                        ma.partial_cmp(&mb).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .cloned();
                if let Some(rid) = remove_id {
                    offspring.mutations.retain(|m| m.id != rid);
                }
            } else if grow && pool_count < max_size {
                // Add
                let present: Vec<&str> =
                    offspring.mutations.iter().map(|m| m.id.as_str()).collect();
                let candidates: Vec<&String> = search_space
                    .mutation_pool
                    .iter()
                    .filter(|id| !present.contains(&id.as_str()))
                    .collect();
                if !candidates.is_empty() {
                    let idx = rng.next_usize(candidates.len());
                    let id = candidates[idx];
                    let params =
                        find_param_space(&registry, id).and_then(|ps| ps.sample_params(rng));
                    offspring.mutations.push(MutationSpec {
                        id: id.clone(),
                        params,
                    });
                }
            }
        }

        let pa_fitness = parent_a
            .fitness
            .map(|f| format!("{:.2}", f))
            .unwrap_or("?".into());
        let pb_fitness = parent_b
            .fitness
            .map(|f| format!("{:.2}", f))
            .unwrap_or("?".into());
        offspring.generation = generation;
        offspring.rationale = format!(
            "accumulated gen={} (parents: {}, {}, elite={:.2})",
            generation, pa_fitness, pb_fitness, best_score
        );
        offspring
    }

    /// Tournament selection: pick 2 random candidates, return the fitter one.
    fn tournament_select(&self, population: &[Recipe], rng: &mut SeededRng) -> Recipe {
        let a = rng.next_usize(population.len());
        let b = rng.next_usize(population.len());
        let fa = population[a].fitness.unwrap_or(0.0);
        let fb = population[b].fitness.unwrap_or(0.0);
        if fa >= fb {
            population[a].clone()
        } else {
            population[b].clone()
        }
    }

    /// Crossover: combine mutations from two parents.
    ///
    /// - Fixed mutations: always included, params picked randomly from either parent
    /// - Pool mutations: union of both parents' pool mutations, params from random parent
    fn crossover(
        &self,
        parent_a: &Recipe,
        parent_b: &Recipe,
        search_space: &SearchSpace,
        rng: &mut SeededRng,
    ) -> Recipe {
        let mut mutations = Vec::new();
        let fixed_set: std::collections::HashSet<&str> = search_space
            .fixed_mutations
            .iter()
            .map(|s| s.as_str())
            .collect();

        // Fixed mutations: pick params from random parent
        for fixed_id in &search_space.fixed_mutations {
            let a_spec = parent_a.mutations.iter().find(|m| m.id == *fixed_id);
            let b_spec = parent_b.mutations.iter().find(|m| m.id == *fixed_id);
            let params = if rng.coin(0.5) {
                a_spec.and_then(|s| s.params.clone())
            } else {
                b_spec.and_then(|s| s.params.clone())
            };
            mutations.push(MutationSpec {
                id: fixed_id.clone(),
                params,
            });
        }

        // Pool mutations: union from both parents
        let a_pool: Vec<&MutationSpec> = parent_a
            .mutations
            .iter()
            .filter(|m| !fixed_set.contains(m.id.as_str()))
            .collect();
        let b_pool: Vec<&MutationSpec> = parent_b
            .mutations
            .iter()
            .filter(|m| !fixed_set.contains(m.id.as_str()))
            .collect();

        // Collect all unique pool mutation IDs
        let mut seen = std::collections::HashSet::new();
        for spec in a_pool.iter().chain(b_pool.iter()) {
            if seen.insert(spec.id.clone()) {
                // For shared mutations, pick params from random parent
                let a_version = parent_a.mutations.iter().find(|m| m.id == spec.id);
                let b_version = parent_b.mutations.iter().find(|m| m.id == spec.id);
                let params = match (a_version, b_version) {
                    (Some(a), Some(b)) => {
                        if rng.coin(0.5) {
                            a.params.clone()
                        } else {
                            b.params.clone()
                        }
                    }
                    (Some(a), None) => a.params.clone(),
                    (None, Some(b)) => b.params.clone(),
                    (None, None) => spec.params.clone(),
                };
                mutations.push(MutationSpec {
                    id: spec.id.clone(),
                    params,
                });
            }
        }

        Recipe {
            mutations,
            fitness: None,
            generation: 0,
            rationale: "crossover".to_string(),
        }
    }
}

/// Baseline selection for round 1 — empty mutations, default modules.
///
/// Returns the unmodified `default_modules` with zero mutations so the first
/// round serves as a control measurement.
fn baseline_selection(default_modules: &ModuleSelectionSpec) -> Selection {
    Selection {
        modules: default_modules.clone(),
        mutations: vec![],
        rationale: "Round 1: baseline control (defaults)".to_string(),
    }
}

#[async_trait]
impl Selector for FuzzerSelector {
    async fn select(
        &self,
        job_id: &str,
        round_number: u32,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
        _guidance: Option<&TriageGuidance>,
    ) -> Selection {
        let phase = determine_phase(
            round_number,
            search_space.mutation_pool.len(),
            &search_space.accumulation,
        );

        if phase == AccumulationPhase::Baseline {
            return baseline_selection(default_modules);
        }

        let config = search_space.fuzzer_config.clone().unwrap_or_default();
        let mut rng = SeededRng::new(job_id, round_number);

        let recipe = match phase {
            AccumulationPhase::Baseline => unreachable!(),
            AccumulationPhase::IndividualExploration => {
                self.individual_exploration(round_number, search_space, &mut rng)
            }
            AccumulationPhase::Accumulation => {
                self.evolve_accumulated(search_space, history, &config, &mut rng)
            }
        };

        // Module selection (reuse shared epsilon-greedy logic)
        let (modules, mod_rationale) = match search_space.strategy {
            VariationStrategy::Full => {
                select_modules(search_space, default_modules, history, &mut |n| {
                    rng.next_usize(n)
                })
            }
            VariationStrategy::MutationOnly => {
                (default_modules.clone(), "modules fixed".to_string())
            }
        };

        Selection {
            modules,
            mutations: recipe.mutations,
            rationale: format!(
                "Fuzzer {}: {} | {}",
                recipe.generation, recipe.rationale, mod_rationale
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{DifferentialCategory, RoundId};
    use std::time::SystemTime;

    fn make_fuzzer_summary(
        round_number: u32,
        evasion_score: f64,
        category: DifferentialCategory,
        mutation_specs: Vec<MutationSpec>,
    ) -> RoundSummary {
        RoundSummary {
            round_id: RoundId(format!("r-{}", round_number)),
            round_number,
            mutations: mutation_specs.iter().map(|m| m.id.clone()).collect(),
            mutation_specs,
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

    fn default_fuzzer_search_space() -> SearchSpace {
        SearchSpace {
            strategy: VariationStrategy::Full,
            fuzzer_config: Some(FuzzerConfig {
                population_size: 5,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_round_1_is_baseline() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = default_fuzzer_search_space();

        let selection = selector
            .select("job-1", 1, &ss, &defaults, &history, None)
            .await;

        assert!(selection.mutations.is_empty());
        assert!(selection.rationale.contains("baseline"));
    }

    #[tokio::test]
    async fn test_seeding_phase_generates_diverse_recipes() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = default_fuzzer_search_space();

        let mut recipes: Vec<Vec<String>> = Vec::new();
        // Rounds 2-6 are seeding phase (population_size=5)
        for round in 2..=6 {
            let selection = selector
                .select("job-1", round, &ss, &defaults, &history, None)
                .await;
            assert!(
                !selection.mutations.is_empty(),
                "Round {} should produce mutations",
                round
            );
            let ids: Vec<String> = selection.mutations.iter().map(|m| m.id.clone()).collect();
            recipes.push(ids);
        }

        // Check diversity: not all recipes identical (params will differ due to seeded RNG)
        let first = &recipes[0];
        let all_same = recipes.iter().all(|r| r == first);
        // With different round seeds, we should get at least some variation
        // (in pool mutations or params — IDs might be same if pool is small)
        assert!(
            !all_same || recipes.len() <= 1,
            "Seeding phase should produce diverse recipes"
        );
    }

    #[tokio::test]
    async fn test_seeding_phase_includes_fixed_mutations() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = default_fuzzer_search_space();
        let fixed_count = ss.fixed_mutations.len();

        let selection = selector
            .select("job-1", 2, &ss, &defaults, &history, None)
            .await;

        // First N mutations should be the fixed set
        for (i, fixed_id) in ss.fixed_mutations.iter().enumerate() {
            assert_eq!(
                &selection.mutations[i].id, fixed_id,
                "Position {} should be fixed mutation {}",
                i, fixed_id
            );
        }
        // Should have at least fixed + 1 pool mutation
        assert!(
            selection.mutations.len() > fixed_count,
            "Should have fixed + pool mutations"
        );
    }

    #[tokio::test]
    async fn test_seeding_phase_varies_params() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = default_fuzzer_search_space();

        let mut some_have_params = false;
        for round in 2..=6 {
            let selection = selector
                .select("job-1", round, &ss, &defaults, &history, None)
                .await;
            for m in &selection.mutations {
                if m.params.is_some() {
                    some_have_params = true;
                    break;
                }
            }
        }
        assert!(
            some_have_params,
            "Seeding phase should produce mutations with params"
        );
    }

    #[tokio::test]
    async fn test_evolution_favors_high_fitness() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let ss = SearchSpace {
            strategy: VariationStrategy::Full,
            fuzzer_config: Some(FuzzerConfig {
                population_size: 3,
                ..Default::default()
            }),
            ..Default::default()
        };

        // Create history with varied fitness
        let good_specs = vec![
            MutationSpec {
                id: "llvm.nop_insert".to_string(),
                params: None,
            },
            MutationSpec {
                id: "ast.decon_rounds".to_string(),
                params: None,
            },
        ];
        let bad_specs = vec![
            MutationSpec {
                id: "llvm.nop_insert".to_string(),
                params: None,
            },
            MutationSpec {
                id: "ast.exec_decoy".to_string(),
                params: None,
            },
        ];

        let mut history = BTreeMap::new();
        // High fitness recipes
        history.insert(
            2,
            make_fuzzer_summary(2, 0.9, DifferentialCategory::Evasion, good_specs.clone()),
        );
        history.insert(
            3,
            make_fuzzer_summary(3, 0.85, DifferentialCategory::Evasion, good_specs.clone()),
        );
        // Low fitness recipe
        history.insert(
            4,
            make_fuzzer_summary(4, 0.1, DifferentialCategory::RealDetection, bad_specs),
        );

        // Evolution rounds should tend to inherit from high-fitness parents
        let mut inherited_good = 0;
        for round in 5..=14 {
            let selection = selector
                .select("job-1", round, &ss, &defaults, &history, None)
                .await;
            let ids: Vec<&str> = selection.mutations.iter().map(|m| m.id.as_str()).collect();
            if ids.contains(&"ast.decon_rounds") {
                inherited_good += 1;
            }
        }
        // Tournament selection should favor high-fitness parents
        assert!(
            inherited_good >= 3,
            "Evolution should inherit from high-fitness parents, got {}/10",
            inherited_good
        );
    }

    #[tokio::test]
    async fn test_param_perturbation_stays_in_range() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let ss = SearchSpace {
            strategy: VariationStrategy::Full,
            fuzzer_config: Some(FuzzerConfig {
                population_size: 2,
                param_mutation_rate: 1.0, // Always mutate params
                ..Default::default()
            }),
            ..Default::default()
        };

        let specs = vec![
            MutationSpec {
                id: "llvm.nop_insert".to_string(),
                params: Some(serde_json::json!({"density": "0.5"})),
            },
            MutationSpec {
                id: "ast.decon_rounds".to_string(),
                params: Some(serde_json::json!({"count": "100", "method": "fixed"})),
            },
        ];

        let mut history = BTreeMap::new();
        history.insert(
            2,
            make_fuzzer_summary(2, 0.8, DifferentialCategory::Evasion, specs.clone()),
        );
        history.insert(
            3,
            make_fuzzer_summary(3, 0.7, DifferentialCategory::Evasion, specs),
        );

        for round in 4..=20 {
            let selection = selector
                .select("job-1", round, &ss, &defaults, &history, None)
                .await;
            for m in &selection.mutations {
                if let Some(params) = &m.params {
                    if m.id == "llvm.nop_insert"
                        && let Some(d) = params.get("density").and_then(|v| v.as_str())
                    {
                        let val: f64 = d.parse().unwrap_or(0.0);
                        assert!(
                            (0.0..=1.0).contains(&val),
                            "nop_insert density {} out of [0.0, 1.0]",
                            val
                        );
                    }
                    if m.id == "ast.decon_rounds"
                        && let Some(c) = params.get("count").and_then(|v| v.as_str())
                    {
                        let val: i64 = c.parse().unwrap_or(0);
                        assert!(
                            (5..=500).contains(&val),
                            "decon_rounds count {} out of [5, 500]",
                            val
                        );
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn test_crossover_merges_parents() {
        let selector = FuzzerSelector::new();
        let ss = SearchSpace::default();
        let mut rng = SeededRng::new("test", 42);

        let parent_a = Recipe {
            mutations: vec![
                MutationSpec {
                    id: "llvm.nop_insert".to_string(),
                    params: None,
                },
                MutationSpec {
                    id: "ast.decon_rounds".to_string(),
                    params: None,
                },
            ],
            fitness: Some(0.8),
            generation: 0,
            rationale: String::new(),
        };
        let parent_b = Recipe {
            mutations: vec![
                MutationSpec {
                    id: "llvm.nop_insert".to_string(),
                    params: None,
                },
                MutationSpec {
                    id: "ast.exec_decoy".to_string(),
                    params: None,
                },
            ],
            fitness: Some(0.7),
            generation: 0,
            rationale: String::new(),
        };

        let offspring = selector.crossover(&parent_a, &parent_b, &ss, &mut rng);
        let ids: Vec<&str> = offspring.mutations.iter().map(|m| m.id.as_str()).collect();

        // Should contain fixed mutations + union of pool mutations
        assert!(
            ids.contains(&"llvm.nop_insert"),
            "Should contain shared fixed mutation"
        );
    }

    #[tokio::test]
    async fn test_elitism_preserves_best() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let ss = SearchSpace {
            strategy: VariationStrategy::Full,
            fuzzer_config: Some(FuzzerConfig {
                population_size: 3,
                ..Default::default()
            }),
            ..Default::default()
        };

        // Create a history where one recipe has very high fitness
        let elite_specs = vec![MutationSpec {
            id: "llvm.nop_insert".to_string(),
            params: Some(serde_json::json!({"density": "0.99"})),
        }];

        let mut history = BTreeMap::new();
        history.insert(
            2,
            make_fuzzer_summary(2, 1.0, DifferentialCategory::Evasion, elite_specs),
        );
        history.insert(
            3,
            make_fuzzer_summary(3, 0.1, DifferentialCategory::RealDetection, vec![]),
        );
        history.insert(
            4,
            make_fuzzer_summary(4, 0.1, DifferentialCategory::RealDetection, vec![]),
        );

        // The elite recipe's traits should appear in evolved offspring
        let mut found_elite_trait = 0;
        for round in 5..=14 {
            let selection = selector
                .select("job-1", round, &ss, &defaults, &history, None)
                .await;
            for m in &selection.mutations {
                if m.id == "llvm.nop_insert"
                    && let Some(params) = &m.params
                    && let Some(d) = params.get("density").and_then(|v| v.as_str())
                {
                    let val: f64 = d.parse().unwrap_or(0.0);
                    if val > 0.7 {
                        found_elite_trait += 1;
                    }
                }
            }
        }
        // Tournament selection heavily favors the 1.0-fitness recipe
        assert!(
            found_elite_trait >= 1,
            "Elite traits should be preserved through evolution"
        );
    }

    #[tokio::test]
    async fn test_deterministic_from_seed() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let ss = default_fuzzer_search_space();
        let history = BTreeMap::new();

        let sel1 = selector
            .select("job-1", 3, &ss, &defaults, &history, None)
            .await;
        let sel2 = selector
            .select("job-1", 3, &ss, &defaults, &history, None)
            .await;

        // Same job_id + round + history → same mutations
        assert_eq!(sel1.mutations.len(), sel2.mutations.len());
        for (a, b) in sel1.mutations.iter().zip(sel2.mutations.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.params, b.params);
        }
    }

    // ==========================================================================
    // Accumulation phase tests
    // ==========================================================================

    #[tokio::test]
    async fn test_individual_exploration_one_per_round() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = default_fuzzer_search_space();

        // Rounds 2..=pool_size+1 should each produce exactly 1 pool mutation
        for round in 2..=(ss.mutation_pool.len() as u32 + 1) {
            let selection = selector
                .select("job-1", round, &ss, &defaults, &history, None)
                .await;
            let pool_count = selection
                .mutations
                .iter()
                .filter(|m| ss.mutation_pool.contains(&m.id))
                .count();
            assert_eq!(
                pool_count, 1,
                "Individual exploration round {} should have exactly 1 pool mutation, got {}",
                round, pool_count
            );
        }
    }

    #[tokio::test]
    async fn test_accumulation_phase_uses_elite() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let ss = SearchSpace {
            strategy: VariationStrategy::Full,
            fuzzer_config: Some(FuzzerConfig {
                population_size: 5,
                ..Default::default()
            }),
            ..Default::default()
        };

        // Build individual exploration history
        let mut history = BTreeMap::new();
        history.insert(
            1,
            make_fuzzer_summary(1, 0.2, DifferentialCategory::RealDetection, vec![]),
        );
        for (i, pool_id) in ss.mutation_pool.iter().enumerate() {
            let specs = vec![MutationSpec {
                id: pool_id.clone(),
                params: None,
            }];
            let score = if pool_id == "ast.fill_pattern" {
                0.9
            } else {
                0.2
            };
            let cat = if score > 0.5 {
                DifferentialCategory::Evasion
            } else {
                DifferentialCategory::RealDetection
            };
            history.insert(
                (i + 2) as u32,
                make_fuzzer_summary((i + 2) as u32, score, cat, specs),
            );
        }

        // Round in accumulation phase
        let round = ss.mutation_pool.len() as u32 + 2;
        let selection = selector
            .select("job-1", round, &ss, &defaults, &history, None)
            .await;

        assert!(
            selection.rationale.contains("accumulated"),
            "Should be in accumulation phase. Got: {}",
            selection.rationale
        );
    }

    #[tokio::test]
    async fn test_accumulation_disabled_stays_exploration_fuzzer() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let mut ss = default_fuzzer_search_space();
        ss.accumulation.enabled = false;

        // Round well past pool_size+1
        let round = ss.mutation_pool.len() as u32 + 5;
        let selection = selector
            .select("job-1", round, &ss, &defaults, &history, None)
            .await;

        // Individual exploration: exactly 1 pool mutation
        let pool_count = selection
            .mutations
            .iter()
            .filter(|m| ss.mutation_pool.contains(&m.id))
            .count();
        assert_eq!(
            pool_count, 1,
            "Disabled accumulation should stay in individual exploration, got {} pool mutations",
            pool_count
        );
    }

    #[tokio::test]
    async fn test_structural_mutation_adds_removes() {
        let selector = FuzzerSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let ss = SearchSpace {
            strategy: VariationStrategy::Full,
            fuzzer_config: Some(FuzzerConfig {
                population_size: 2,
                structural_mutation_rate: 1.0, // Always mutate structure
                ..Default::default()
            }),
            ..Default::default()
        };

        let specs = vec![
            MutationSpec {
                id: "llvm.nop_insert".to_string(),
                params: None,
            },
            MutationSpec {
                id: "ast.decon_rounds".to_string(),
                params: None,
            },
            MutationSpec {
                id: "ast.exec_decoy".to_string(),
                params: None,
            },
        ];

        let mut history = BTreeMap::new();
        history.insert(
            2,
            make_fuzzer_summary(2, 0.5, DifferentialCategory::RealDetection, specs.clone()),
        );
        history.insert(
            3,
            make_fuzzer_summary(3, 0.5, DifferentialCategory::RealDetection, specs),
        );

        let mut mutation_counts = std::collections::HashSet::new();
        for round in 4..=20 {
            let selection = selector
                .select("job-1", round, &ss, &defaults, &history, None)
                .await;
            let pool_count = selection
                .mutations
                .iter()
                .filter(|m| ss.mutation_pool.contains(&m.id))
                .count();
            mutation_counts.insert(pool_count);
        }

        // With 100% structural mutation rate, we should see variation in pool mutation count
        assert!(
            mutation_counts.len() > 1,
            "Structural mutation should vary pool mutation count, got {:?}",
            mutation_counts
        );
    }
}
