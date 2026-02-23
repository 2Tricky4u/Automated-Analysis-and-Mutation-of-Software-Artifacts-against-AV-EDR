//! Coverage-driven mutation selector (v0).
//!
//! Pure logic — no ES dependency. Uses in-memory round history.
//!
//! Supports two strategies via `VariationStrategy`:
//! - `MutationOnly` (default): modules fixed, mutations vary per round
//! - `Full`: both modules AND mutations vary per round
//!
//! Algorithm (both axes):
//! 1. Round 1 → return defaults (baseline control measurement)
//! 2. Round 2+ → filter history by `is_trustworthy()`, group by key
//! 3. Explore untried items one-at-a-time, then epsilon-greedy (ε=0.3)

use super::{SearchSpace, Selection, Selector, TriageGuidance, VariationStrategy};
use crate::dispatch::types::{ModuleSelectionSpec, MutationSpec, RoundSummary};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

/// Exploration rate for epsilon-greedy selection.
const EPSILON: f64 = 0.3;

/// All available deconditioner variants (matches build/templates/modules/deconditioner/*.c).
const DECONDITIONER_VARIANTS: &[&str] = &[
    "none",
    "alloc_loop",
    "alloc_exec",
    "thread_alloc",
    "mixed_apis",
    "entropy_flood",
];

/// Full mutation catalog — all implemented mutations across AST, LLVM IR, and Binary layers.
const MUTATION_CATALOG: &[&str] = &[
    // AST – global
    "ast.string_xor",
    // AST – marker-based
    "ast.decon_rounds",
    "ast.fill_pattern",
    "ast.exec_decoy",
    "ast.timing_pattern",
    "ast.protection_transition",
    // LLVM IR
    "llvm.nop_insert",
    "llvm.opaque_predicate",
    "llvm.junk_block",
    // Binary/PE
    "binary.rich_header",
    "binary.import_pad",
    "binary.resource_inject",
    "binary.section_rename",
    "binary.entropy_normalize",
    "binary.string_inject",
    "binary.size_pad",
    "binary.debug_dir",
    "binary.timestamp",
];

/// Per-variant statistics accumulated from trustworthy rounds.
#[derive(Debug, Clone)]
struct VariantStats {
    count: u32,
    total_evasion_score: f64,
}

impl VariantStats {
    fn mean_evasion_score(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.total_evasion_score / self.count as f64
        }
    }
}

/// Stateless coverage-driven selector.
///
/// All state comes via `select()` arguments — cheap to construct, easy to test.
pub struct CoverageSelector;

impl Default for CoverageSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverageSelector {
    pub fn new() -> Self {
        Self
    }

    /// Simple pseudo-random using subsec_nanos. Not cryptographic, good enough for exploration.
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

    /// Module selection logic (epsilon-greedy over deconditioner variants).
    /// Extracted from the original `select()` — zero logic changes.
    fn select_modules(
        &self,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
    ) -> (ModuleSelectionSpec, String) {
        // Only vary deconditioner for now
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

        // Collect stats from trustworthy rounds only
        let mut stats: HashMap<String, VariantStats> = HashMap::new();
        for summary in history.values() {
            if !summary.differential_category.is_trustworthy() {
                continue;
            }
            let variant = summary.modules.deconditioner.clone();
            let entry = stats.entry(variant).or_insert(VariantStats {
                count: 0,
                total_evasion_score: 0.0,
            });
            entry.count += 1;
            entry.total_evasion_score += summary.evasion_score;
        }

        // Find untried variants
        let untried: Vec<&str> = DECONDITIONER_VARIANTS
            .iter()
            .filter(|v| !stats.contains_key(**v))
            .copied()
            .collect();

        let tried_count = DECONDITIONER_VARIANTS.len() - untried.len();

        let (chosen_variant, rationale) = if !untried.is_empty() {
            let idx = Self::pseudo_random(untried.len());
            let variant = untried[idx];
            (
                variant.to_string(),
                format!(
                    "Exploring untried variant: {} ({}/{} tried)",
                    variant,
                    tried_count,
                    DECONDITIONER_VARIANTS.len()
                ),
            )
        } else {
            let coin = Self::pseudo_random(100) as f64 / 100.0;
            if coin < EPSILON {
                let idx = Self::pseudo_random(DECONDITIONER_VARIANTS.len());
                let variant = DECONDITIONER_VARIANTS[idx];
                (
                    variant.to_string(),
                    format!("Exploring random: {} (epsilon={:.1})", variant, EPSILON),
                )
            } else {
                let best = stats
                    .iter()
                    .max_by(|a, b| {
                        a.1.mean_evasion_score()
                            .partial_cmp(&b.1.mean_evasion_score())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(k, v)| (k.clone(), v.mean_evasion_score(), v.count))
                    .unwrap_or_else(|| ("none".to_string(), 0.0, 0));

                (
                    best.0.clone(),
                    format!(
                        "Exploiting best: {} (mean_evasion={:.2}, n={})",
                        best.0, best.1, best.2
                    ),
                )
            }
        };

        let mut modules = default_modules.clone();
        modules.deconditioner = chosen_variant;
        (modules, rationale)
    }

    /// Build the fixed mutation specs from search_space.fixed_mutations.
    fn fixed_mutation_specs(search_space: &SearchSpace) -> Vec<MutationSpec> {
        search_space
            .fixed_mutations
            .iter()
            .map(|id| MutationSpec {
                id: id.clone(),
                params: None,
            })
            .collect()
    }

    /// Explore one mutation from the pool using fuzzer-like individual exploration
    /// then epsilon-greedy. Returns None if pool is empty.
    ///
    /// Algorithm:
    /// 1. Collect stats from trustworthy history keyed by explored mutation ID
    /// 2. Untried phase: explore individual mutations one-at-a-time
    /// 3. Epsilon-greedy phase (ε=0.3): exploit best or random
    fn explore_one(
        pool: &[&str],
        history: &BTreeMap<u32, RoundSummary>,
        fixed_set: &std::collections::HashSet<&str>,
    ) -> Option<(MutationSpec, String)> {
        if pool.is_empty() {
            return None;
        }

        // Collect stats from trustworthy rounds, keyed by the explored mutation ID.
        // We identify the explored mutation as the one NOT in the fixed set.
        let mut stats: HashMap<String, VariantStats> = HashMap::new();
        for summary in history.values() {
            if !summary.differential_category.is_trustworthy() {
                continue;
            }
            // Find mutations that aren't in the fixed set — those are the explored ones
            let explored: Vec<&String> = summary
                .mutations
                .iter()
                .filter(|m| !fixed_set.contains(m.as_str()))
                .collect();
            // Only track rounds with exactly 1 explored mutation (our exploration pattern)
            if explored.len() != 1 {
                continue;
            }
            let key = explored[0].clone();
            let entry = stats.entry(key).or_insert(VariantStats {
                count: 0,
                total_evasion_score: 0.0,
            });
            entry.count += 1;
            entry.total_evasion_score += summary.evasion_score;
        }

        // Find untried individual mutations from pool
        let untried: Vec<&str> = pool
            .iter()
            .filter(|m| !stats.contains_key(**m))
            .copied()
            .collect();

        let tried_count = pool.len() - untried.len();

        let (chosen, rationale) = if !untried.is_empty() {
            let idx = CoverageSelector::pseudo_random(untried.len());
            let mutation = untried[idx];
            (
                mutation.to_string(),
                format!(
                    "Mutation: exploring untried {} ({}/{} tried)",
                    mutation, tried_count, pool.len()
                ),
            )
        } else {
            let coin = CoverageSelector::pseudo_random(100) as f64 / 100.0;
            if coin < EPSILON {
                let idx = CoverageSelector::pseudo_random(pool.len());
                let mutation = pool[idx];
                (
                    mutation.to_string(),
                    format!(
                        "Mutation: exploring random {} (epsilon={:.1})",
                        mutation, EPSILON
                    ),
                )
            } else {
                let best = stats
                    .iter()
                    .filter(|(k, _)| pool.contains(&k.as_str()))
                    .max_by(|a, b| {
                        a.1.mean_evasion_score()
                            .partial_cmp(&b.1.mean_evasion_score())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(k, v)| (k.clone(), v.mean_evasion_score(), v.count))
                    .unwrap_or_else(|| (pool[0].to_string(), 0.0, 0));

                (
                    best.0.clone(),
                    format!(
                        "Mutation: exploiting best {} (mean_evasion={:.2}, n={})",
                        best.0, best.1, best.2
                    ),
                )
            }
        };

        Some((
            MutationSpec {
                id: chosen,
                params: None,
            },
            rationale,
        ))
    }

    /// Mutation selection logic — combines fixed mutations + fuzzer-explored one from pool.
    ///
    /// Two-tier approach:
    /// 1. Start with fixed_mutations (always applied, e.g. binary.* + llvm.*)
    /// 2. Explore one mutation from mutation_pool (e.g. AST markers) via epsilon-greedy
    /// 3. If both are empty, fall back to full-catalog exploration (backward compat)
    fn select_mutations(
        &self,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
    ) -> Selection {
        let has_fixed = !search_space.fixed_mutations.is_empty();
        let has_pool = !search_space.mutation_pool.is_empty();

        // Backward compat: if both empty, do original full-catalog exploration
        if !has_fixed && !has_pool {
            let pool: Vec<&str> = MUTATION_CATALOG.to_vec();
            let fixed_set = std::collections::HashSet::new();
            return match Self::explore_one(&pool, history, &fixed_set) {
                Some((spec, rationale)) => Selection {
                    modules: default_modules.clone(),
                    mutations: vec![spec],
                    rationale,
                },
                None => Selection {
                    modules: default_modules.clone(),
                    mutations: vec![],
                    rationale: "No mutations available".to_string(),
                },
            };
        }

        // Two-tier: fixed + explored
        let mut mutations = Self::fixed_mutation_specs(search_space);
        let fixed_set: std::collections::HashSet<&str> = search_space
            .fixed_mutations
            .iter()
            .map(|s| s.as_str())
            .collect();

        let pool: Vec<&str> = search_space
            .mutation_pool
            .iter()
            .map(|s| s.as_str())
            .collect();

        let rationale = match Self::explore_one(&pool, history, &fixed_set) {
            Some((spec, rationale)) => {
                mutations.push(spec);
                format!(
                    "Fixed: {} mutations | {}",
                    search_space.fixed_mutations.len(),
                    rationale
                )
            }
            None => format!(
                "Fixed: {} mutations | No exploration pool",
                search_space.fixed_mutations.len()
            ),
        };

        Selection {
            modules: default_modules.clone(),
            mutations,
            rationale,
        }
    }

    /// Full mode: both modules AND mutations vary.
    fn select_full(
        &self,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
    ) -> Selection {
        let (modules, module_rationale) =
            self.select_modules(search_space, default_modules, history);
        let mutation_selection = self.select_mutations(search_space, default_modules, history);

        Selection {
            modules,
            mutations: mutation_selection.mutations,
            rationale: format!(
                "Module: {} | {}",
                module_rationale, mutation_selection.rationale
            ),
        }
    }
}

#[async_trait]
impl Selector for CoverageSelector {
    async fn select(
        &self,
        _job_id: &str,
        round_number: u32,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
        _guidance: Option<&TriageGuidance>,
    ) -> Selection {
        // Round 1: always use defaults (baseline control measurement)
        if round_number <= 1 {
            return Selection {
                modules: default_modules.clone(),
                mutations: vec![],
                rationale: "Round 1: baseline control (defaults)".to_string(),
            };
        }

        match search_space.strategy {
            VariationStrategy::MutationOnly => {
                self.select_mutations(search_space, default_modules, history)
            }
            VariationStrategy::Full => {
                self.select_full(search_space, default_modules, history)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{DifferentialCategory, RoundId};

    /// Helper: SearchSpace with no fixed/pool (backward-compat pure exploration).
    fn empty_search_space() -> SearchSpace {
        SearchSpace {
            fixed_mutations: vec![],
            mutation_pool: vec![],
            ..Default::default()
        }
    }

    fn make_summary(
        round_number: u32,
        deconditioner: &str,
        evasion_score: f64,
        category: DifferentialCategory,
    ) -> RoundSummary {
        make_summary_with_mutations(round_number, deconditioner, evasion_score, category, vec![])
    }

    fn make_summary_with_mutations(
        round_number: u32,
        deconditioner: &str,
        evasion_score: f64,
        category: DifferentialCategory,
        mutations: Vec<String>,
    ) -> RoundSummary {
        RoundSummary {
            round_id: RoundId(format!("r-{}", round_number)),
            round_number,
            mutations,
            modules: {
                let mut m = ModuleSelectionSpec::default();
                m.deconditioner = deconditioner.to_string();
                m
            },
            detected: category.is_detected(),
            behavior_match: true,
            evasion_score,
            differential_category: category,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
        }
    }

    // ==========================================================================
    // Strategy dispatch & defaults
    // ==========================================================================

    #[tokio::test]
    async fn test_round_1_returns_defaults() {
        let selector = CoverageSelector::new();
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

        assert_eq!(selection.modules.deconditioner, defaults.deconditioner);
        assert!(selection.rationale.contains("baseline"));
        assert!(selection.mutations.is_empty());
    }

    #[tokio::test]
    async fn test_default_strategy_is_mutation_only() {
        let ss = SearchSpace::default();
        assert_eq!(ss.strategy, VariationStrategy::MutationOnly);
    }

    #[tokio::test]
    async fn test_default_search_space_has_fixed_and_pool() {
        let ss = SearchSpace::default();
        assert_eq!(ss.fixed_mutations.len(), 12, "Default: 3 LLVM + 9 binary");
        assert_eq!(ss.mutation_pool.len(), 5, "Default: 5 AST markers");
        assert!(ss.fixed_mutations.contains(&"llvm.nop_insert".to_string()));
        assert!(ss.fixed_mutations.contains(&"binary.timestamp".to_string()));
        assert!(ss.mutation_pool.contains(&"ast.decon_rounds".to_string()));
    }

    // ==========================================================================
    // MutationOnly mode tests (backward compat — empty fixed/pool)
    // ==========================================================================

    #[tokio::test]
    async fn test_mutation_mode_round_1_baseline() {
        let selector = CoverageSelector::new();
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

        assert!(
            selection.mutations.is_empty(),
            "Round 1 should have no mutations"
        );
        assert_eq!(selection.modules, defaults, "Round 1 modules = defaults");
    }

    #[tokio::test]
    async fn test_mutation_mode_explores_untried_backward_compat() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        let mut history = BTreeMap::new();
        history.insert(
            1,
            make_summary(1, "none", 0.2, DifferentialCategory::RealDetection),
        );

        // Empty fixed/pool → full catalog exploration
        let selection = selector
            .select(
                "job-1",
                2,
                &empty_search_space(),
                &defaults,
                &history,
                None,
            )
            .await;

        assert_eq!(
            selection.mutations.len(),
            1,
            "Should select exactly 1 mutation"
        );
        assert!(selection.rationale.contains("Mutation:"));
        assert!(selection.rationale.contains("untried"));
        assert_eq!(
            selection.modules, defaults,
            "MutationOnly: modules stay fixed"
        );
    }

    #[tokio::test]
    async fn test_mutation_mode_modules_stay_fixed() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        for round in 2..5 {
            let selection = selector
                .select(
                    "job-1",
                    round,
                    &SearchSpace::default(),
                    &defaults,
                    &history,
                    None,
                )
                .await;

            assert_eq!(
                selection.modules, defaults,
                "MutationOnly: modules must always equal defaults (round {})",
                round
            );
        }
    }

    #[tokio::test]
    async fn test_mutation_mode_exploits_best_backward_compat() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        // Create history where all mutations have been tried (empty fixed/pool mode)
        let mut history = BTreeMap::new();
        for (i, mutation) in MUTATION_CATALOG.iter().enumerate() {
            let score = if *mutation == "binary.rich_header" {
                0.9
            } else {
                0.1
            };
            let category = if score > 0.5 {
                DifferentialCategory::Evasion
            } else {
                DifferentialCategory::RealDetection
            };
            history.insert(
                (i + 1) as u32,
                make_summary_with_mutations(
                    (i + 1) as u32,
                    "none",
                    score,
                    category,
                    vec![mutation.to_string()],
                ),
            );
        }

        let catalog_set: std::collections::HashSet<&str> =
            MUTATION_CATALOG.iter().copied().collect();

        for _ in 0..10 {
            let selection = selector
                .select(
                    "job-1",
                    (MUTATION_CATALOG.len() + 2) as u32,
                    &empty_search_space(),
                    &defaults,
                    &history,
                    None,
                )
                .await;
            assert_eq!(
                selection.mutations.len(),
                1,
                "Should select exactly 1 mutation"
            );
            assert!(
                catalog_set.contains(selection.mutations[0].id.as_str()),
                "Selected mutation {} should be from catalog",
                selection.mutations[0].id
            );
            assert_eq!(selection.modules, defaults, "Modules should stay fixed");
            assert!(
                selection.rationale.contains("Mutation:"),
                "Rationale should describe mutation selection"
            );
        }
    }

    #[tokio::test]
    async fn test_mutation_mode_custom_pool() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let search_space = SearchSpace {
            strategy: VariationStrategy::MutationOnly,
            mutation_pool: vec![
                "ast.string_xor".to_string(),
                "binary.rich_header".to_string(),
            ],
            fixed_mutations: vec![],
            ..Default::default()
        };

        // Run multiple rounds — explored mutations should only come from the custom pool
        let mut seen = std::collections::HashSet::new();
        for round in 2..20 {
            let selection = selector
                .select("job-1", round, &search_space, &defaults, &history, None)
                .await;
            for m in &selection.mutations {
                seen.insert(m.id.clone());
            }
        }

        for m in &seen {
            assert!(
                m == "ast.string_xor" || m == "binary.rich_header",
                "Mutation {} not in custom pool",
                m
            );
        }
    }

    // ==========================================================================
    // Fixed + Explored mode tests (new two-tier)
    // ==========================================================================

    #[tokio::test]
    async fn test_fixed_plus_explored() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        // Use defaults: 12 fixed (binary+LLVM) + 5 AST pool
        let ss = SearchSpace::default();
        let fixed_count = ss.fixed_mutations.len(); // 12
        let pool: Vec<String> = ss.mutation_pool.clone();

        let selection = selector
            .select("job-1", 2, &ss, &defaults, &history, None)
            .await;

        // Should have fixed_count + 1 (explored) mutations
        assert_eq!(
            selection.mutations.len(),
            fixed_count + 1,
            "Round 2+: {} fixed + 1 explored = {}",
            fixed_count,
            fixed_count + 1
        );

        // Last mutation should be from the pool
        let last = &selection.mutations[fixed_count].id;
        assert!(
            pool.contains(last),
            "Explored mutation {} should be in pool {:?}",
            last,
            pool
        );

        // First N should be the fixed mutations
        for (i, fixed) in ss.fixed_mutations.iter().enumerate() {
            assert_eq!(
                &selection.mutations[i].id, fixed,
                "Position {} should be fixed mutation {}",
                i, fixed
            );
        }
    }

    #[tokio::test]
    async fn test_fixed_only_no_pool() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let ss = SearchSpace {
            fixed_mutations: vec!["binary.rich_header".to_string(), "llvm.nop_insert".to_string()],
            mutation_pool: vec![],
            ..Default::default()
        };

        let selection = selector
            .select("job-1", 2, &ss, &defaults, &history, None)
            .await;

        assert_eq!(
            selection.mutations.len(),
            2,
            "Only fixed mutations, no exploration"
        );
        assert_eq!(selection.mutations[0].id, "binary.rich_header");
        assert_eq!(selection.mutations[1].id, "llvm.nop_insert");
        assert!(
            selection.rationale.contains("No exploration pool"),
            "Rationale should note empty pool. Got: {}",
            selection.rationale
        );
    }

    #[tokio::test]
    async fn test_round1_still_empty() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        // Even with fixed_mutations configured, round 1 should be empty (baseline)
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

        assert!(
            selection.mutations.is_empty(),
            "Round 1 must have 0 mutations regardless of config"
        );
    }

    // ==========================================================================
    // Full mode tests
    // ==========================================================================

    #[tokio::test]
    async fn test_full_mode_varies_both() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        let mut history = BTreeMap::new();
        history.insert(
            1,
            make_summary(1, "none", 0.2, DifferentialCategory::RealDetection),
        );

        let search_space = SearchSpace {
            strategy: VariationStrategy::Full,
            variable_categories: vec!["deconditioner".to_string()],
            ..Default::default()
        };

        let selection = selector
            .select("job-1", 2, &search_space, &defaults, &history, None)
            .await;

        assert!(
            !selection.mutations.is_empty(),
            "Full mode should produce mutations"
        );
        assert!(
            selection.rationale.contains("Module:"),
            "Full mode rationale should reference module selection"
        );
        assert!(
            selection.rationale.contains("Mutation:"),
            "Full mode rationale should reference mutation selection"
        );
    }

    // ==========================================================================
    // Legacy module-only tests (Full mode with deconditioner variation)
    // ==========================================================================

    #[tokio::test]
    async fn test_untried_variants_explored_first() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        let mut history = BTreeMap::new();
        history.insert(
            1,
            make_summary(1, "none", 0.2, DifferentialCategory::RealDetection),
        );

        let search_space = SearchSpace {
            strategy: VariationStrategy::Full,
            variable_categories: vec!["deconditioner".to_string()],
            ..Default::default()
        };

        let selection = selector
            .select("job-1", 2, &search_space, &defaults, &history, None)
            .await;

        assert_ne!(
            selection.modules.deconditioner, "none",
            "Should explore untried variant, not repeat 'none'"
        );
        assert!(
            selection.rationale.contains("untried variant"),
            "Module rationale should mention untried. Got: {}",
            selection.rationale
        );
    }

    #[tokio::test]
    async fn test_trustworthy_filtering() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        let mut history = BTreeMap::new();
        history.insert(
            1,
            make_summary(1, "none", 0.1, DifferentialCategory::RealDetection),
        );
        history.insert(
            2,
            make_summary(
                2,
                "alloc_loop",
                0.6,
                DifferentialCategory::InstrumentationArtifact,
            ),
        );
        history.insert(
            3,
            make_summary(3, "alloc_exec", 0.3, DifferentialCategory::Flaky),
        );

        let search_space = SearchSpace {
            strategy: VariationStrategy::Full,
            variable_categories: vec!["deconditioner".to_string()],
            ..Default::default()
        };

        let selection = selector
            .select("job-1", 4, &search_space, &defaults, &history, None)
            .await;

        assert!(
            selection.rationale.contains("untried"),
            "With only 1 trustworthy round, should still be exploring. Got: {}",
            selection.rationale
        );
        assert!(
            selection.rationale.contains("1/6 tried"),
            "Should show 1/6 tried. Got: {}",
            selection.rationale
        );
    }

    #[tokio::test]
    async fn test_exploitation_picks_best() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        let mut history = BTreeMap::new();
        let variants = [
            ("none", 0.1),
            ("alloc_loop", 0.3),
            ("alloc_exec", 0.9),
            ("thread_alloc", 0.2),
            ("mixed_apis", 0.4),
            ("entropy_flood", 0.5),
        ];
        for (i, (variant, score)) in variants.iter().enumerate() {
            let category = if *score > 0.5 {
                DifferentialCategory::Evasion
            } else {
                DifferentialCategory::RealDetection
            };
            history.insert(
                (i + 1) as u32,
                make_summary((i + 1) as u32, variant, *score, category),
            );
        }

        let search_space = SearchSpace {
            strategy: VariationStrategy::Full,
            variable_categories: vec!["deconditioner".to_string()],
            ..Default::default()
        };

        let mut best_count = 0;
        for _ in 0..20 {
            let selection = selector
                .select("job-1", 8, &search_space, &defaults, &history, None)
                .await;
            if selection.modules.deconditioner == "alloc_exec" {
                best_count += 1;
            }
        }

        assert!(
            best_count >= 5,
            "Expected exploitation to favor alloc_exec (score=0.9), got {} out of 20",
            best_count
        );
    }

    #[tokio::test]
    async fn test_no_variable_categories_returns_defaults() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        let empty_search = SearchSpace {
            strategy: VariationStrategy::Full,
            variable_categories: vec![],
            ..Default::default()
        };

        let selection = selector
            .select("job-1", 5, &empty_search, &defaults, &history, None)
            .await;

        assert_eq!(selection.modules.deconditioner, defaults.deconditioner);
    }
}
