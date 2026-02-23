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

    /// Mutation selection logic — fuzzer-like individual exploration then epsilon-greedy.
    ///
    /// Algorithm mirrors module selection for consistency:
    /// 1. Determine pool: `search_space.mutation_pool` if non-empty, else `MUTATION_CATALOG`
    /// 2. Collect stats: from trustworthy history, key = sorted mutation IDs joined by `,`
    /// 3. Untried phase: explore individual mutations one-at-a-time
    /// 4. Epsilon-greedy phase (ε=0.3): exploit best individual mutation or random
    fn select_mutations(
        &self,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
    ) -> Selection {
        // Determine mutation pool
        let pool: Vec<&str> = if !search_space.mutation_pool.is_empty() {
            search_space.mutation_pool.iter().map(|s| s.as_str()).collect()
        } else {
            MUTATION_CATALOG.to_vec()
        };

        // Collect stats from trustworthy rounds, keyed by mutation ID
        // For individual exploration, we key by single mutation ID
        let mut stats: HashMap<String, VariantStats> = HashMap::new();
        for summary in history.values() {
            if !summary.differential_category.is_trustworthy() {
                continue;
            }
            // Key = sorted mutations joined by comma (for single mutations, just the ID)
            let mut key_parts = summary.mutations.clone();
            key_parts.sort();
            let key = key_parts.join(",");
            if key.is_empty() {
                continue; // Skip baseline (no mutations)
            }
            let entry = stats.entry(key).or_insert(VariantStats {
                count: 0,
                total_evasion_score: 0.0,
            });
            entry.count += 1;
            entry.total_evasion_score += summary.evasion_score;
        }

        // Find untried individual mutations
        let untried: Vec<&str> = pool
            .iter()
            .filter(|m| !stats.contains_key(**m))
            .copied()
            .collect();

        let tried_count = pool.len() - untried.len();

        let (chosen_mutation, rationale) = if !untried.is_empty() {
            // Explore: try an untried individual mutation
            let idx = Self::pseudo_random(untried.len());
            let mutation = untried[idx];
            (
                mutation.to_string(),
                format!(
                    "Mutation: exploring untried {} ({}/{} tried)",
                    mutation,
                    tried_count,
                    pool.len()
                ),
            )
        } else {
            // All mutations tried — epsilon-greedy
            let coin = Self::pseudo_random(100) as f64 / 100.0;
            if coin < EPSILON {
                let idx = Self::pseudo_random(pool.len());
                let mutation = pool[idx];
                (
                    mutation.to_string(),
                    format!(
                        "Mutation: exploring random {} (epsilon={:.1})",
                        mutation, EPSILON
                    ),
                )
            } else {
                // Exploit: best mean_evasion_score among individual mutations
                let best = stats
                    .iter()
                    .filter(|(k, _)| !k.contains(',')) // Only individual mutations
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

        Selection {
            modules: default_modules.clone(),
            mutations: vec![MutationSpec {
                id: chosen_mutation,
                params: None,
            }],
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
        let mutation_selection =
            self.select_mutations(search_space, default_modules, history);

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

    // ==========================================================================
    // MutationOnly mode tests
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

        assert!(selection.mutations.is_empty(), "Round 1 should have no mutations");
        assert_eq!(selection.modules, defaults, "Round 1 modules = defaults");
    }

    #[tokio::test]
    async fn test_mutation_mode_explores_untried() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        // Round 1: baseline (no mutations)
        let mut history = BTreeMap::new();
        history.insert(
            1,
            make_summary(1, "none", 0.2, DifferentialCategory::RealDetection),
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

        // Should have exactly 1 mutation
        assert_eq!(selection.mutations.len(), 1, "Should select exactly 1 mutation");
        assert!(selection.rationale.contains("Mutation:"));
        assert!(selection.rationale.contains("untried"));
        // Modules should stay fixed
        assert_eq!(selection.modules, defaults, "MutationOnly: modules stay fixed");
    }

    #[tokio::test]
    async fn test_mutation_mode_modules_stay_fixed() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();
        let history = BTreeMap::new();

        // Run multiple rounds — modules should always be defaults
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
    async fn test_mutation_mode_exploits_best() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        // Create history where all mutations have been tried
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

        // Verify: all tried → selection always produces exactly 1 mutation from the catalog,
        // and it's either the best (exploitation) or any from the pool (exploration).
        // Note: pseudo_random uses subsec_nanos which has limited resolution on Windows,
        // so in a tight loop all iterations may take the same branch.
        let catalog_set: std::collections::HashSet<&str> =
            MUTATION_CATALOG.iter().copied().collect();

        for _ in 0..10 {
            let selection = selector
                .select(
                    "job-1",
                    (MUTATION_CATALOG.len() + 2) as u32,
                    &SearchSpace::default(),
                    &defaults,
                    &history,
                    None,
                )
                .await;
            assert_eq!(selection.mutations.len(), 1, "Should select exactly 1 mutation");
            assert!(
                catalog_set.contains(selection.mutations[0].id.as_str()),
                "Selected mutation {} should be from catalog",
                selection.mutations[0].id
            );
            assert_eq!(selection.modules, defaults, "Modules should stay fixed");
            // Rationale should indicate exploit or explore (epsilon-greedy)
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
            ..Default::default()
        };

        // Run multiple rounds — mutations should only come from the custom pool
        let mut seen = std::collections::HashSet::new();
        for round in 2..20 {
            let selection = selector
                .select("job-1", round, &search_space, &defaults, &history, None)
                .await;
            if !selection.mutations.is_empty() {
                seen.insert(selection.mutations[0].id.clone());
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

        // Should have mutations
        assert!(
            !selection.mutations.is_empty(),
            "Full mode should produce mutations"
        );

        // Rationale should mention both module and mutation
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

        // Module part should pick an untried variant
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
