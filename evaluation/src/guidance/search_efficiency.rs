//! Guidance.SearchEfficiency — measures how quickly the fuzzer finds evasions.
//!
//! Sub-metrics:
//! - Evasions per round (total evasion rate)
//! - Time-to-first-evasion (round number of first evasion)
//! - Evasions@N curves (cumulative evasions at N=5,10,20,50)
//! - Windowed evasion score trajectory

use crate::helpers::rolling_mean;
use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;

pub struct SearchEfficiency;

impl EvalMetric for SearchEfficiency {
    fn metric_id(&self) -> &str {
        "guidance.search_efficiency"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Evasions per round
        let evasion_count = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::Evasion)
            .count();
        let evasions_per_round = evasion_count as f64 / n as f64;

        results.push(MetricResult {
            metric_id: "guidance.search_efficiency.evasions_per_round".to_string(),
            axis: "guidance".to_string(),
            category: "search_efficiency".to_string(),
            label: "Evasions per round".to_string(),
            value: evasions_per_round,
            details: json!({
                "evasion_count": evasion_count,
                "total_rounds": n,
            }),
            n,
        });

        // 2. Time-to-first-evasion
        let first_evasion = dataset
            .rounds
            .iter()
            .find(|r| r.differential_category == DifferentialCategory::Evasion)
            .map(|r| r.round_number);

        let ttfe = first_evasion.unwrap_or(0) as f64;
        let ttfe_normalized = if n > 0 && first_evasion.is_some() {
            first_evasion.unwrap() as f64 / n as f64
        } else {
            1.0 // Never found evasion → worst case
        };

        results.push(MetricResult {
            metric_id: "guidance.search_efficiency.time_to_first_evasion".to_string(),
            axis: "guidance".to_string(),
            category: "search_efficiency".to_string(),
            label: "Time-to-first-evasion (round number, lower is better)".to_string(),
            value: ttfe,
            details: json!({
                "first_evasion_round": first_evasion,
                "normalized": ttfe_normalized,
                "found": first_evasion.is_some(),
            }),
            n,
        });

        // 3. Evasions@N curves
        let checkpoints = [5, 10, 20, 50];
        let mut at_n = serde_json::Map::new();

        for &cp in &checkpoints {
            if n >= cp {
                let count = dataset.rounds[..cp]
                    .iter()
                    .filter(|r| r.differential_category == DifferentialCategory::Evasion)
                    .count();
                at_n.insert(format!("evasions@{}", cp), json!(count));
            }
        }

        if !at_n.is_empty() {
            // Value = best evasions@N rate across checkpoints
            let best_rate = checkpoints
                .iter()
                .filter(|&&cp| n >= cp)
                .map(|&cp| {
                    let count = dataset.rounds[..cp]
                        .iter()
                        .filter(|r| r.differential_category == DifferentialCategory::Evasion)
                        .count();
                    count as f64 / cp as f64
                })
                .fold(0.0f64, f64::max);

            results.push(MetricResult {
                metric_id: "guidance.search_efficiency.evasions_at_n".to_string(),
                axis: "guidance".to_string(),
                category: "search_efficiency".to_string(),
                label: "Best evasion rate across N checkpoints".to_string(),
                value: best_rate,
                details: json!({ "at_n": at_n }),
                n,
            });
        }

        // 4. Windowed evasion score trajectory
        let scores: Vec<f64> = dataset.rounds.iter().map(|r| r.evasion_score).collect();
        let window = 5.min(n);
        let trajectory = rolling_mean(&scores, window);

        if trajectory.len() >= 2 {
            let first = trajectory[0];
            let last = *trajectory.last().unwrap();
            let improvement = last - first;

            results.push(MetricResult {
                metric_id: "guidance.search_efficiency.score_trajectory".to_string(),
                axis: "guidance".to_string(),
                category: "search_efficiency".to_string(),
                label: "Evasion score trajectory improvement (last - first rolling mean)"
                    .to_string(),
                value: improvement,
                details: json!({
                    "window_size": window,
                    "first_window_avg": first,
                    "last_window_avg": last,
                    "trajectory": trajectory,
                }),
                n,
            });
        }

        Ok(results)
    }
}
