//! I11: Selector Baseline Comparison Analysis
//!
//! Compares 4 selector strategies on identical synthetic history to validate
//! that guided selectors produce meaningfully different selections than Random.
//!
//! **Claim:** Guided selectors produce more diverse, higher-coverage selections
//! than the random baseline.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct SelectorComparisonAnalysis;

impl InfraMetric for SelectorComparisonAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i11.selector_comparison"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.selector_comparison {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I11.1: Mutation pool coverage by selector
        let coverage_table: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "selector": r.selector_name,
                    "coverage": r.mutation_pool_coverage,
                    "unique_sets": r.unique_mutation_sets,
                    "rounds": r.rounds_evaluated,
                })
            })
            .collect();

        let mean_coverage = results
            .iter()
            .map(|r| r.mutation_pool_coverage)
            .sum::<f64>()
            / results.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i11.selector_comparison.coverage_by_selector".to_string(),
            axis: "infrastructure".to_string(),
            category: "selector_comparison".to_string(),
            label: "Mutation pool coverage by selector strategy".to_string(),
            value: mean_coverage,
            details: json!({
                "by_selector": coverage_table,
            }),
            n,
        });

        // I11.2: Diversity by selector — mean pairwise Jaccard distance of round selections
        let diversity_table: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                let diversity = crate::helpers::mean_pairwise_jaccard(&r.per_round_mutations);
                json!({
                    "selector": r.selector_name,
                    "diversity": diversity,
                    "mean_recipe_size": r.mean_recipe_size,
                })
            })
            .collect();

        let mean_diversity: f64 = results
            .iter()
            .map(|r| crate::helpers::mean_pairwise_jaccard(&r.per_round_mutations))
            .sum::<f64>()
            / results.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i11.selector_comparison.diversity_by_selector".to_string(),
            axis: "infrastructure".to_string(),
            category: "selector_comparison".to_string(),
            label: "Mean pairwise Jaccard distance of round selections".to_string(),
            value: mean_diversity,
            details: json!({
                "by_selector": diversity_table,
            }),
            n,
        });

        // I11.3: Exploration rate by selector
        let exploration_table: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "selector": r.selector_name,
                    "exploration_rate": r.exploration_rate,
                })
            })
            .collect();

        let mean_exploration =
            results.iter().map(|r| r.exploration_rate).sum::<f64>() / results.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i11.selector_comparison.exploration_rate".to_string(),
            axis: "infrastructure".to_string(),
            category: "selector_comparison".to_string(),
            label: "Per-selector exploration fraction".to_string(),
            value: mean_exploration,
            details: json!({
                "by_selector": exploration_table,
            }),
            n,
        });

        // I11.4: Guided vs Random delta
        let random_coverage = results
            .iter()
            .find(|r| r.selector_name == "Random")
            .map(|r| r.mutation_pool_coverage)
            .unwrap_or(0.0);

        let guided_deltas: Vec<serde_json::Value> = results
            .iter()
            .filter(|r| r.selector_name != "Random")
            .map(|r| {
                json!({
                    "selector": r.selector_name,
                    "coverage": r.mutation_pool_coverage,
                    "delta": r.mutation_pool_coverage - random_coverage,
                })
            })
            .collect();

        let mean_delta = if !guided_deltas.is_empty() {
            results
                .iter()
                .filter(|r| r.selector_name != "Random")
                .map(|r| r.mutation_pool_coverage - random_coverage)
                .sum::<f64>()
                / results
                    .iter()
                    .filter(|r| r.selector_name != "Random")
                    .count()
                    .max(1) as f64
        } else {
            0.0
        };

        metrics.push(MetricResult {
            metric_id: "infra.i11.selector_comparison.guided_vs_random_delta".to_string(),
            axis: "infrastructure".to_string(),
            category: "selector_comparison".to_string(),
            label: "Coverage delta: guided selectors vs Random baseline".to_string(),
            value: mean_delta,
            details: json!({
                "random_coverage": random_coverage,
                "guided_deltas": guided_deltas,
            }),
            n,
        });

        Ok(metrics)
    }
}
