//! I12: Guidance Utilization Analysis
//!
//! Validates that token guidance materially changes selector behavior;
//! avoid tokens reduce selection of correlated mutations.
//!
//! **Claim:** Token guidance changes selector output in the expected direction.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct GuidanceUtilizationAnalysis;

impl InfraMetric for GuidanceUtilizationAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i12.guidance_utilization"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.guidance_utilization {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I12.1: Avoidance rate per selector (higher = guidance works)
        let avoidance_table: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "selector": r.selector_name,
                    "avoidance_rate": r.avoidance_rate,
                    "avoid_tokens": r.avoid_tokens,
                })
            })
            .collect();

        let mean_avoidance =
            results.iter().map(|r| r.avoidance_rate).sum::<f64>() / results.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i12.guidance_utilization.avoidance_rate".to_string(),
            axis: "infrastructure".to_string(),
            category: "guidance_utilization".to_string(),
            label: "Fraction of guided rounds avoiding avoid-correlated mutations".to_string(),
            value: mean_avoidance,
            details: json!({
                "mean_avoidance_rate": mean_avoidance,
                "by_selector": avoidance_table,
            }),
            n,
        });

        // I12.2: Seek adoption rate per selector
        let seek_table: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "selector": r.selector_name,
                    "seek_adoption_rate": r.seek_adoption_rate,
                    "seek_tokens": r.seek_tokens,
                })
            })
            .collect();

        let mean_seek =
            results.iter().map(|r| r.seek_adoption_rate).sum::<f64>() / results.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i12.guidance_utilization.seek_adoption_rate".to_string(),
            axis: "infrastructure".to_string(),
            category: "guidance_utilization".to_string(),
            label: "Fraction of guided rounds adopting seek-correlated mutations".to_string(),
            value: mean_seek,
            details: json!({
                "mean_seek_rate": mean_seek,
                "by_selector": seek_table,
            }),
            n,
        });

        // I12.3: Recipe delta — mean recipe difference with vs without guidance
        let delta_table: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                // Compute per-round mutation frequency comparison
                let mut without_freq: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                let mut with_freq: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();

                for round_muts in &r.mutations_without_guidance {
                    for m in round_muts {
                        *without_freq.entry(m.clone()).or_default() += 1;
                    }
                }
                for round_muts in &r.mutations_with_guidance {
                    for m in round_muts {
                        *with_freq.entry(m.clone()).or_default() += 1;
                    }
                }

                // Collect all mutations
                let mut all_muts: Vec<String> = without_freq
                    .keys()
                    .chain(with_freq.keys())
                    .cloned()
                    .collect();
                all_muts.sort();
                all_muts.dedup();

                let freq_comparison: Vec<serde_json::Value> = all_muts
                    .iter()
                    .map(|m| {
                        let wo = *without_freq.get(m).unwrap_or(&0);
                        let wi = *with_freq.get(m).unwrap_or(&0);
                        json!({
                            "mutation": m,
                            "without_guidance": wo,
                            "with_guidance": wi,
                            "delta": wi as i64 - wo as i64,
                        })
                    })
                    .collect();

                json!({
                    "selector": r.selector_name,
                    "recipe_jaccard_delta": r.recipe_jaccard_delta,
                    "rounds": r.rounds,
                    "mutation_frequencies": freq_comparison,
                })
            })
            .collect();

        let mean_delta = results.iter().map(|r| r.recipe_jaccard_delta).sum::<f64>()
            / results.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i12.guidance_utilization.recipe_delta".to_string(),
            axis: "infrastructure".to_string(),
            category: "guidance_utilization".to_string(),
            label: "Mean recipe difference with vs without guidance".to_string(),
            value: mean_delta,
            details: json!({
                "mean_recipe_delta": mean_delta,
                "by_selector": delta_table,
            }),
            n,
        });

        Ok(metrics)
    }
}
