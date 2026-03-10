//! Random mutation selector — zero-intelligence baseline for evaluation.
//!
//! Uniformly samples modules and mutations with no learning from history.
//! Deterministic from seed (job_id + round_number) for reproducible runs.
//!
//! Algorithm:
//! 1. Round 1 → return defaults (baseline control measurement)
//! 2. Round 2+ → uniform random choices via SeededRng, ignoring history/guidance:
//!    - Pick a random deconditioner variant
//!    - Include all fixed_mutations (same as other selectors)
//!    - Pick a random subset (1–3) from mutation_pool with random params
//!    - If Full strategy: also randomize deconditioner

use super::coverage_selector::DECONDITIONER_VARIANTS;
use super::{SearchSpace, Selection, Selector, TriageGuidance, VariationStrategy};
use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
use crate::triage::accumulation::{AccumulationPhase, determine_phase, effective_max_recipe_size};
use crate::triage::param_space::{SeededRng, default_registry, find_param_space};
use async_trait::async_trait;
use std::collections::BTreeMap;

/// Stateless random selector — evaluation baseline.
///
/// All randomness comes from `SeededRng::new(job_id, round_number)`,
/// making selections fully deterministic and reproducible.
pub struct RandomSelector;

impl Default for RandomSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl RandomSelector {
    pub fn new() -> Self {
        Self
    }

    /// Sample params for a mutation ID using the seeded RNG.
    fn sample_mutation_params(mutation_id: &str, rng: &mut SeededRng) -> Option<serde_json::Value> {
        let registry = default_registry();
        find_param_space(&registry, mutation_id).and_then(|ps| ps.sample_params(rng))
    }

    /// Build fixed mutation specs with seeded params.
    fn fixed_mutation_specs(search_space: &SearchSpace, rng: &mut SeededRng) -> Vec<MutationSpec> {
        search_space
            .fixed_mutations
            .iter()
            .map(|id| MutationSpec {
                id: id.clone(),
                params: Self::sample_mutation_params(id, rng),
            })
            .collect()
    }

    /// Pick a random subset (1–3) from the mutation pool.
    fn random_pool_mutations(pool: &[String], rng: &mut SeededRng) -> (Vec<MutationSpec>, String) {
        if pool.is_empty() {
            return (vec![], "No exploration pool".to_string());
        }

        let count = 1 + rng.next_usize(pool.len().min(3));
        let count = count.min(pool.len());

        // Fisher-Yates partial shuffle to pick `count` unique items
        let mut indices: Vec<usize> = (0..pool.len()).collect();
        for i in 0..count {
            let j = i + rng.next_usize(indices.len() - i);
            indices.swap(i, j);
        }

        let chosen: Vec<MutationSpec> = indices[..count]
            .iter()
            .map(|&idx| {
                let id = &pool[idx];
                MutationSpec {
                    id: id.clone(),
                    params: Self::sample_mutation_params(id, rng),
                }
            })
            .collect();

        let names: Vec<&str> = chosen.iter().map(|m| m.id.as_str()).collect();
        let rationale = format!("Random: picked {} from pool [{}]", count, names.join(", "));
        (chosen, rationale)
    }

    /// Random accumulation: random recipe of growing size k.
    ///
    /// k = min(round_number - pool_size - 1, max_recipe_size), clamped to [1, pool_size].
    /// No history used — tests whether growing recipes provide value.
    fn random_accumulated(
        search_space: &SearchSpace,
        round_number: u32,
        rng: &mut SeededRng,
    ) -> (Vec<MutationSpec>, String) {
        let pool = &search_space.mutation_pool;
        if pool.is_empty() {
            return (vec![], "No exploration pool".to_string());
        }

        let pool_size = pool.len();
        let max_size = effective_max_recipe_size(&search_space.accumulation, pool_size);
        let k_raw = (round_number as usize).saturating_sub(pool_size + 1);
        let k = k_raw.clamp(1, max_size).min(pool_size);

        // Fisher-Yates partial shuffle to pick k unique items
        let mut indices: Vec<usize> = (0..pool.len()).collect();
        for i in 0..k {
            let j = i + rng.next_usize(indices.len() - i);
            indices.swap(i, j);
        }

        let chosen: Vec<MutationSpec> = indices[..k]
            .iter()
            .map(|&idx| {
                let id = &pool[idx];
                MutationSpec {
                    id: id.clone(),
                    params: Self::sample_mutation_params(id, rng),
                }
            })
            .collect();

        let names: Vec<&str> = chosen.iter().map(|m| m.id.as_str()).collect();
        let rationale = format!(
            "Random accumulated: k={} from pool [{}]",
            k,
            names.join(", ")
        );
        (chosen, rationale)
    }

    /// Random module selection: pick a uniform random deconditioner.
    fn random_modules(
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        rng: &mut SeededRng,
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

        let idx = rng.next_usize(DECONDITIONER_VARIANTS.len());
        let variant = DECONDITIONER_VARIANTS[idx];

        let mut modules = default_modules.clone();
        modules.deconditioner = variant.to_string();
        (modules, format!("Random module: {}", variant))
    }
}

#[async_trait]
impl Selector for RandomSelector {
    async fn select(
        &self,
        job_id: &str,
        round_number: u32,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        _history: &BTreeMap<u32, RoundSummary>,
        _guidance: Option<&TriageGuidance>,
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

        // Deterministic RNG from job_id + round_number
        let mut rng = SeededRng::new(job_id, round_number);

        // Build mutations: fixed + pool selection based on phase
        let mut mutations = Self::fixed_mutation_specs(search_space, &mut rng);
        let (pool_mutations, pool_rationale) = match phase {
            AccumulationPhase::Baseline => unreachable!(),
            AccumulationPhase::IndividualExploration => {
                Self::random_pool_mutations(&search_space.mutation_pool, &mut rng)
            }
            AccumulationPhase::Accumulation => {
                Self::random_accumulated(search_space, round_number, &mut rng)
            }
        };
        mutations.extend(pool_mutations);

        match search_space.strategy {
            VariationStrategy::MutationOnly => Selection {
                modules: default_modules.clone(),
                mutations,
                rationale: format!(
                    "Random | Fixed: {} | {}",
                    search_space.fixed_mutations.len(),
                    pool_rationale
                ),
            },
            VariationStrategy::Full => {
                let (modules, module_rationale) =
                    Self::random_modules(search_space, default_modules, &mut rng);
                Selection {
                    modules,
                    mutations,
                    rationale: format!(
                        "{} | Fixed: {} | {}",
                        module_rationale,
                        search_space.fixed_mutations.len(),
                        pool_rationale
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{DifferentialCategory, RoundId};
    use std::time::SystemTime;

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
    async fn test_round_1_returns_baseline() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let selection = selector
            .select(
                "job-1",
                1,
                &SearchSpace::default(),
                &defaults,
                &history,
                None,
            )
            .await;

        assert_eq!(selection.modules, defaults);
        assert!(selection.mutations.is_empty());
        assert!(selection.rationale.contains("baseline"));
    }

    #[tokio::test]
    async fn test_random_produces_mutations() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let ss = SearchSpace::default();
        let fixed_count = ss.fixed_mutations.len();

        let selection = selector
            .select("job-1", 2, &ss, &defaults, &history, None)
            .await;

        // Should have fixed mutations + at least 1 from pool
        assert!(
            selection.mutations.len() > fixed_count,
            "Should have fixed ({}) + pool mutations, got {}",
            fixed_count,
            selection.mutations.len()
        );
        // First N should be fixed
        for (i, fixed_id) in ss.fixed_mutations.iter().enumerate() {
            assert_eq!(
                &selection.mutations[i].id, fixed_id,
                "Position {} should be fixed mutation {}",
                i, fixed_id
            );
        }
        // Remaining should be from pool
        for m in &selection.mutations[fixed_count..] {
            assert!(
                ss.mutation_pool.contains(&m.id),
                "Pool mutation {} should be in pool {:?}",
                m.id,
                ss.mutation_pool
            );
        }
    }

    #[tokio::test]
    async fn test_deterministic_from_seed() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = SearchSpace::default();

        let sel1 = selector
            .select("job-abc", 5, &ss, &defaults, &history, None)
            .await;
        let sel2 = selector
            .select("job-abc", 5, &ss, &defaults, &history, None)
            .await;

        // Same job_id + round_number → identical selection
        assert_eq!(sel1.mutations.len(), sel2.mutations.len());
        for (a, b) in sel1.mutations.iter().zip(sel2.mutations.iter()) {
            assert_eq!(a.id, b.id, "Mutation IDs should match");
            assert_eq!(a.params, b.params, "Mutation params should match");
        }
        assert_eq!(
            sel1.modules.deconditioner, sel2.modules.deconditioner,
            "Module selection should be identical"
        );
    }

    #[tokio::test]
    async fn test_ignores_history() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let ss = SearchSpace::default();

        // Empty history
        let empty_history = BTreeMap::new();
        let sel_empty = selector
            .select("job-1", 3, &ss, &defaults, &empty_history, None)
            .await;

        // Rich history (should be ignored)
        let mut rich_history = BTreeMap::new();
        rich_history.insert(
            1,
            make_summary(
                1,
                "alloc_exec",
                0.9,
                DifferentialCategory::Evasion,
                vec!["ast.fill_pattern".to_string()],
            ),
        );
        rich_history.insert(
            2,
            make_summary(
                2,
                "none",
                0.1,
                DifferentialCategory::RealDetection,
                vec!["ast.decon_rounds".to_string()],
            ),
        );
        let sel_rich = selector
            .select("job-1", 3, &ss, &defaults, &rich_history, None)
            .await;

        // Same seed → same output regardless of history
        assert_eq!(sel_empty.mutations.len(), sel_rich.mutations.len());
        for (a, b) in sel_empty.mutations.iter().zip(sel_rich.mutations.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.params, b.params);
        }
    }

    #[tokio::test]
    async fn test_full_strategy_randomizes_modules() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let ss = SearchSpace {
            strategy: VariationStrategy::Full,
            variable_categories: vec!["deconditioner".to_string()],
            ..Default::default()
        };

        // Run across many rounds — should see module variation
        let mut seen_variants = std::collections::HashSet::new();
        for round in 2..30 {
            let selection = selector
                .select("job-variety", round, &ss, &defaults, &history, None)
                .await;
            seen_variants.insert(selection.modules.deconditioner.clone());
        }

        assert!(
            seen_variants.len() > 1,
            "Full strategy should produce multiple deconditioner variants, got {:?}",
            seen_variants
        );
    }

    #[tokio::test]
    async fn test_mutation_only_keeps_default_modules() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = SearchSpace::default(); // MutationOnly

        for round in 2..10 {
            let selection = selector
                .select("job-1", round, &ss, &defaults, &history, None)
                .await;
            assert_eq!(
                selection.modules, defaults,
                "MutationOnly should preserve default modules (round {})",
                round
            );
        }
    }

    // ==========================================================================
    // Accumulation phase tests
    // ==========================================================================

    #[tokio::test]
    async fn test_random_accumulation_growing_k() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = SearchSpace::default();
        let pool_size = ss.mutation_pool.len();

        // Accumulation starts at pool_size+2
        let mut sizes = Vec::new();
        for round in (pool_size as u32 + 2)..=(pool_size as u32 + 6) {
            let selection = selector
                .select("job-growth", round, &ss, &defaults, &history, None)
                .await;
            let pool_count = selection
                .mutations
                .iter()
                .filter(|m| ss.mutation_pool.contains(&m.id))
                .count();
            sizes.push(pool_count);
        }

        // k should grow: round pool_size+2 → k=1, pool_size+3 → k=2, etc.
        assert_eq!(sizes[0], 1, "First accumulation round should have k=1");
        assert!(
            sizes[1] >= 2,
            "Second accumulation round should have k>=2, got {}",
            sizes[1]
        );
    }

    #[tokio::test]
    async fn test_random_accumulation_respects_max_recipe_size() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let mut ss = SearchSpace::default();
        ss.accumulation.max_recipe_size = Some(3);
        let pool_size = ss.mutation_pool.len();

        // Round far into accumulation (would be k=50 without clamping)
        let round = pool_size as u32 + 50;
        let selection = selector
            .select("job-max", round, &ss, &defaults, &history, None)
            .await;
        let pool_count = selection
            .mutations
            .iter()
            .filter(|m| ss.mutation_pool.contains(&m.id))
            .count();
        assert!(
            pool_count <= 3,
            "Pool mutations ({}) should respect max_recipe_size=3",
            pool_count
        );
    }

    #[tokio::test]
    async fn test_random_accumulation_disabled() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let mut ss = SearchSpace::default();
        ss.accumulation.enabled = false;
        let pool_size = ss.mutation_pool.len();

        // Round well past pool_size+1
        let round = pool_size as u32 + 10;
        let selection = selector
            .select("job-disabled", round, &ss, &defaults, &history, None)
            .await;

        // Should NOT contain "accumulated" — stays in individual exploration
        assert!(
            !selection.rationale.contains("accumulated"),
            "Disabled accumulation should not use accumulated logic. Got: {}",
            selection.rationale
        );
    }

    #[tokio::test]
    async fn test_random_accumulation_deterministic() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = SearchSpace::default();
        let pool_size = ss.mutation_pool.len();

        let round = pool_size as u32 + 3;
        let sel1 = selector
            .select("job-det", round, &ss, &defaults, &history, None)
            .await;
        let sel2 = selector
            .select("job-det", round, &ss, &defaults, &history, None)
            .await;

        assert_eq!(sel1.mutations.len(), sel2.mutations.len());
        for (a, b) in sel1.mutations.iter().zip(sel2.mutations.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.params, b.params);
        }
    }

    #[tokio::test]
    async fn test_different_rounds_produce_different_selections() {
        let selector = RandomSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();
        let ss = SearchSpace::default();

        let sel2 = selector
            .select("job-1", 2, &ss, &defaults, &history, None)
            .await;
        let sel3 = selector
            .select("job-1", 3, &ss, &defaults, &history, None)
            .await;

        // Different round numbers should (almost certainly) produce different pool mutations
        let pool2: Vec<&str> = sel2.mutations[ss.fixed_mutations.len()..]
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        let pool3: Vec<&str> = sel3.mutations[ss.fixed_mutations.len()..]
            .iter()
            .map(|m| m.id.as_str())
            .collect();

        // With different seeds, at least mutation IDs or params should differ
        let ids_differ = pool2 != pool3;
        let params_differ = sel2
            .mutations
            .iter()
            .zip(sel3.mutations.iter())
            .any(|(a, b)| a.id != b.id || a.params != b.params);
        assert!(
            ids_differ || params_differ,
            "Different rounds should produce different selections"
        );
    }
}
