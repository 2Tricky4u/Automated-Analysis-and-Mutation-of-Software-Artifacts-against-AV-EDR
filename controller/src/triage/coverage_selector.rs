//! Coverage-driven mutation selector (v0).
//!
//! Pure logic — no ES dependency. Uses in-memory round history.
//!
//! Algorithm:
//! 1. Round 1 → return job defaults (control/baseline measurement)
//! 2. Round 2+ → filter history by `is_trustworthy()`, group by deconditioner variant
//! 3. Epsilon-greedy: explore untried variants first, then ε=0.3 random vs best

use super::{SearchSpace, Selection, Selector, TriageGuidance};
use crate::dispatch::types::{ModuleSelectionSpec, RoundSummary};
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

        // Only vary deconditioner for now
        if !search_space
            .variable_categories
            .iter()
            .any(|c| c == "deconditioner")
        {
            return Selection {
                modules: default_modules.clone(),
                mutations: vec![],
                rationale: "No variable categories include deconditioner".to_string(),
            };
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
            // Explore: pick an untried variant
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
            // All variants tried — epsilon-greedy
            let coin = Self::pseudo_random(100) as f64 / 100.0;
            if coin < EPSILON {
                // Explore: random variant
                let idx = Self::pseudo_random(DECONDITIONER_VARIANTS.len());
                let variant = DECONDITIONER_VARIANTS[idx];
                (
                    variant.to_string(),
                    format!("Exploring random: {} (epsilon={:.1})", variant, EPSILON),
                )
            } else {
                // Exploit: best mean_evasion_score
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

        // Build modules with chosen deconditioner
        let mut modules = default_modules.clone();
        modules.deconditioner = chosen_variant;

        Selection {
            modules,
            mutations: vec![],
            rationale,
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
        RoundSummary {
            round_id: RoundId(format!("r-{}", round_number)),
            round_number,
            mutations: vec![],
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
        }
    }

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
    async fn test_untried_variants_explored_first() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        // Only "none" has been tried
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

        // Should pick an untried variant (not "none")
        assert_ne!(
            selection.modules.deconditioner, "none",
            "Should explore untried variant, not repeat 'none'"
        );
        assert!(selection.rationale.contains("untried"));
        assert!(selection.rationale.contains("1/6 tried"));
    }

    #[tokio::test]
    async fn test_trustworthy_filtering() {
        let selector = CoverageSelector::new();
        let defaults = ModuleSelectionSpec::default();

        // Mix of trustworthy and untrustworthy rounds
        let mut history = BTreeMap::new();
        // Trustworthy: RealDetection for "none"
        history.insert(
            1,
            make_summary(1, "none", 0.1, DifferentialCategory::RealDetection),
        );
        // NOT trustworthy: InstrumentationArtifact for "alloc_loop" — should be ignored
        history.insert(
            2,
            make_summary(
                2,
                "alloc_loop",
                0.6,
                DifferentialCategory::InstrumentationArtifact,
            ),
        );
        // NOT trustworthy: Flaky for "alloc_exec" — should be ignored
        history.insert(
            3,
            make_summary(3, "alloc_exec", 0.3, DifferentialCategory::Flaky),
        );

        let selection = selector
            .select(
                "job-1",
                4,
                &SearchSpace::default(),
                &defaults,
                &history,
                None,
            )
            .await;

        // Only "none" was trustworthy, so 5 variants are untried
        // The selector should pick one of the untried ones
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

        // All 6 variants tried with different scores
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

        // Run selector many times — exploitation should pick alloc_exec (score 0.9)
        // At least some of the time (70% exploit, 30% random)
        let mut best_count = 0;
        for _ in 0..20 {
            let selection = selector
                .select(
                    "job-1",
                    8,
                    &SearchSpace::default(),
                    &defaults,
                    &history,
                    None,
                )
                .await;
            if selection.modules.deconditioner == "alloc_exec" {
                best_count += 1;
            }
        }

        // With ε=0.3, we'd expect ~70% exploit (alloc_exec) + ~17% random alloc_exec = ~72%
        // Be lenient: at least 5/20 should pick alloc_exec
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
            variable_categories: vec![],
        };

        let selection = selector
            .select("job-1", 5, &empty_search, &defaults, &history, None)
            .await;

        assert_eq!(selection.modules.deconditioner, defaults.deconditioner);
    }
}
