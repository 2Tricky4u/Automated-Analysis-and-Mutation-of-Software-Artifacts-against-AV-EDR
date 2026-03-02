//! Input.Validity — measures artifact build/execution success rate.
//!
//! Sub-metrics:
//! - Rejection rate: fraction of rounds that are MutationFailed or PayloadFailed
//! - Semantic execution rate: fraction that reached meaningful execution
//! - Per-mutation failure correlation: which mutations correlate with failure

use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::HashMap;

pub struct Validity;

impl EvalMetric for Validity {
    fn metric_id(&self) -> &str {
        "input.validity"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Rejection rate: MutationFailed + PayloadFailed / total
        let broken_count = dataset
            .rounds
            .iter()
            .filter(|r| {
                matches!(
                    r.differential_category,
                    DifferentialCategory::MutationFailed | DifferentialCategory::PayloadFailed
                )
            })
            .count();

        let rejection_rate = broken_count as f64 / n as f64;
        results.push(MetricResult {
            metric_id: "input.validity.rejection_rate".to_string(),
            axis: "input".to_string(),
            category: "validity".to_string(),
            label: "Rejection rate (broken artifacts / total)".to_string(),
            value: rejection_rate,
            details: json!({
                "broken": broken_count,
                "mutation_failed": dataset.rounds.iter()
                    .filter(|r| r.differential_category == DifferentialCategory::MutationFailed)
                    .count(),
                "payload_failed": dataset.rounds.iter()
                    .filter(|r| r.differential_category == DifferentialCategory::PayloadFailed)
                    .count(),
                "total": n,
            }),
            n,
        });

        // 2. Semantic execution rate: rounds that were NOT broken and NOT infra errors
        let executed_count = dataset
            .rounds
            .iter()
            .filter(|r| {
                !matches!(
                    r.differential_category,
                    DifferentialCategory::MutationFailed | DifferentialCategory::PayloadFailed
                )
            })
            .count();

        let execution_rate = executed_count as f64 / n as f64;
        results.push(MetricResult {
            metric_id: "input.validity.execution_rate".to_string(),
            axis: "input".to_string(),
            category: "validity".to_string(),
            label: "Semantic execution rate (non-broken / total)".to_string(),
            value: execution_rate,
            details: json!({
                "executed": executed_count,
                "total": n,
            }),
            n,
        });

        // 3. Per-mutation failure correlation
        let mut mutation_total: HashMap<String, usize> = HashMap::new();
        let mut mutation_broken: HashMap<String, usize> = HashMap::new();

        for round in &dataset.rounds {
            let is_broken = matches!(
                round.differential_category,
                DifferentialCategory::MutationFailed | DifferentialCategory::PayloadFailed
            );

            for m in &round.mutations {
                let key: String = m.clone();
                *mutation_total.entry(key.clone()).or_default() += 1;
                if is_broken {
                    *mutation_broken.entry(key).or_default() += 1;
                }
            }
        }

        let mut failure_rates: Vec<(String, f64, usize, usize)> = mutation_total
            .iter()
            .map(|(m, total)| {
                let broken = *mutation_broken.get(m).unwrap_or(&0);
                let rate = broken as f64 / *total as f64;
                (m.clone(), rate, broken, *total)
            })
            .collect();

        failure_rates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        results.push(MetricResult {
            metric_id: "input.validity.mutation_failure_correlation".to_string(),
            axis: "input".to_string(),
            category: "validity".to_string(),
            label: "Per-mutation failure rate".to_string(),
            value: failure_rates
                .first()
                .map(|(_, rate, _, _)| *rate)
                .unwrap_or(0.0),
            details: json!({
                "per_mutation": failure_rates.iter().map(|(m, rate, broken, total)| {
                    json!({
                        "mutation": m,
                        "failure_rate": rate,
                        "broken": broken,
                        "total": total,
                    })
                }).collect::<Vec<_>>(),
            }),
            n,
        });

        Ok(results)
    }
}
