//! I11: Selector Baseline Comparison Analysis
//!
//! Compares 4 selector strategies on identical synthetic history to validate
//! that guided selectors produce meaningfully different selections than Random.
//!
//! **Claim:** Guided selectors produce more diverse, higher-coverage selections
//! than the random baseline.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;
use std::collections::{HashMap, HashSet};

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

        // I11.5: Coverage trajectory — cumulative coverage at each round per selector
        let pool: HashSet<String> = results
            .iter()
            .flat_map(|r| r.per_round_mutations.iter().flatten().cloned())
            .collect();
        let pool_size = pool.len().max(1);

        let cov_trajectory: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                let mut seen: HashSet<String> = HashSet::new();
                let trajectory: Vec<serde_json::Value> = r
                    .per_round_mutations
                    .iter()
                    .enumerate()
                    .map(|(i, muts)| {
                        for m in muts {
                            seen.insert(m.clone());
                        }
                        let coverage = seen.intersection(&pool).count() as f64 / pool_size as f64;
                        json!({"round": i + 1, "coverage": coverage})
                    })
                    .collect();
                json!({"selector": r.selector_name, "trajectory": trajectory})
            })
            .collect();

        let mean_final_cov = results
            .iter()
            .map(|r| {
                let seen: HashSet<String> =
                    r.per_round_mutations.iter().flatten().cloned().collect();
                seen.intersection(&pool).count() as f64 / pool_size as f64
            })
            .sum::<f64>()
            / results.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i11.selector_comparison.coverage_trajectory".to_string(),
            axis: "infrastructure".to_string(),
            category: "selector_comparison".to_string(),
            label: "Cumulative mutation coverage trajectory per selector".to_string(),
            value: mean_final_cov,
            details: json!({"by_selector": cov_trajectory}),
            n,
        });

        // I11.6: Diversity trajectory — rolling pairwise Jaccard diversity (window=5)
        let div_trajectory: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                let trajectory: Vec<serde_json::Value> = (0..r.per_round_mutations.len())
                    .map(|i| {
                        let window_start = i.saturating_sub(4);
                        let window = &r.per_round_mutations[window_start..=i];
                        let diversity = crate::helpers::mean_pairwise_jaccard(window);
                        json!({"round": i + 1, "diversity": diversity})
                    })
                    .collect();
                json!({"selector": r.selector_name, "trajectory": trajectory})
            })
            .collect();

        let mean_final_div = results
            .iter()
            .map(|r| crate::helpers::mean_pairwise_jaccard(&r.per_round_mutations))
            .sum::<f64>()
            / results.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i11.selector_comparison.diversity_trajectory".to_string(),
            axis: "infrastructure".to_string(),
            category: "selector_comparison".to_string(),
            label: "Rolling pairwise Jaccard diversity trajectory (window=5)".to_string(),
            value: mean_final_div,
            details: json!({"by_selector": div_trajectory}),
            n,
        });

        // I11.7: Mutation frequency heatmap — mutation × selector frequency matrix
        let mut all_mutations: Vec<String> = results
            .iter()
            .flat_map(|r| r.per_round_mutations.iter().flatten().cloned())
            .collect::<HashSet<String>>()
            .into_iter()
            .collect();
        all_mutations.sort();

        let selector_names: Vec<String> = results.iter().map(|r| r.selector_name.clone()).collect();

        let frequencies: Vec<Vec<usize>> = results
            .iter()
            .map(|r| {
                let mut freq: HashMap<String, usize> = HashMap::new();
                for muts in &r.per_round_mutations {
                    for m in muts {
                        *freq.entry(m.clone()).or_default() += 1;
                    }
                }
                all_mutations
                    .iter()
                    .map(|m| *freq.get(m).unwrap_or(&0))
                    .collect()
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i11.selector_comparison.mutation_frequency_heatmap".to_string(),
            axis: "infrastructure".to_string(),
            category: "selector_comparison".to_string(),
            label: "Mutation selection frequency matrix (mutation × selector)".to_string(),
            value: 0.0,
            details: json!({
                "mutations": all_mutations,
                "selectors": selector_names,
                "frequencies": frequencies,
            }),
            n,
        });

        // I11.8: Coverage saturation — round at which each selector hits 80% pool coverage
        let saturation_data: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                let mut seen: HashSet<String> = HashSet::new();
                let mut saturation_round: Option<usize> = None;
                for (i, muts) in r.per_round_mutations.iter().enumerate() {
                    for m in muts {
                        seen.insert(m.clone());
                    }
                    let cov = seen.intersection(&pool).count() as f64 / pool_size as f64;
                    if cov >= 0.8 && saturation_round.is_none() {
                        saturation_round = Some(i + 1);
                    }
                }
                json!({
                    "selector": r.selector_name,
                    "saturation_round": saturation_round,
                })
            })
            .collect();

        let mean_saturation = {
            let vals: Vec<f64> = saturation_data
                .iter()
                .filter_map(|s| s["saturation_round"].as_u64().map(|v| v as f64))
                .collect();
            if vals.is_empty() {
                0.0
            } else {
                vals.iter().sum::<f64>() / vals.len() as f64
            }
        };

        metrics.push(MetricResult {
            metric_id: "infra.i11.selector_comparison.coverage_saturation".to_string(),
            axis: "infrastructure".to_string(),
            category: "selector_comparison".to_string(),
            label: "Round at which each selector hits 80% pool coverage".to_string(),
            value: mean_saturation,
            details: json!({"by_selector": saturation_data}),
            n,
        });

        Ok(metrics)
    }
}
