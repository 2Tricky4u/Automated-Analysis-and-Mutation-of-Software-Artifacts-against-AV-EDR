//! Guidance.BaselineComparison — compares guided search against random baseline.
//!
//! Sub-metrics:
//! - Guided vs synthetic-random evasion rate delta
//! - Mean evasion score delta
//! - Ablation: score with/without module variation, mutations, token guidance

use crate::fixtures::round_factory::RoundSequenceBuilder;
use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;

pub struct BaselineComparison;

impl EvalMetric for BaselineComparison {
    fn metric_id(&self) -> &str {
        "guidance.baseline_comparison"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // Generate a synthetic random baseline of the same size for comparison
        let mut builder = RoundSequenceBuilder::new();
        builder.random_rounds(n, 12345);
        let random_rounds = builder.build();

        // 1. Evasion rate delta
        let guided_evasions = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::Evasion)
            .count();
        let random_evasions = random_rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::Evasion)
            .count();

        let guided_rate = guided_evasions as f64 / n as f64;
        let random_rate = random_evasions as f64 / n as f64;
        let evasion_delta = guided_rate - random_rate;

        results.push(MetricResult {
            metric_id: "guidance.baseline_comparison.evasion_rate_delta".to_string(),
            axis: "guidance".to_string(),
            category: "baseline_comparison".to_string(),
            label: "Evasion rate delta (guided - random baseline)".to_string(),
            value: evasion_delta,
            details: json!({
                "guided_rate": guided_rate,
                "random_rate": random_rate,
                "guided_evasions": guided_evasions,
                "random_evasions": random_evasions,
                "total_rounds": n,
            }),
            n,
        });

        // 2. Mean evasion score delta
        let guided_mean_score =
            dataset.rounds.iter().map(|r| r.evasion_score).sum::<f64>() / n as f64;
        let random_mean_score =
            random_rounds.iter().map(|r| r.evasion_score).sum::<f64>() / n as f64;
        let score_delta = guided_mean_score - random_mean_score;

        results.push(MetricResult {
            metric_id: "guidance.baseline_comparison.score_delta".to_string(),
            axis: "guidance".to_string(),
            category: "baseline_comparison".to_string(),
            label: "Mean evasion score delta (guided - random)".to_string(),
            value: score_delta,
            details: json!({
                "guided_mean": guided_mean_score,
                "random_mean": random_mean_score,
            }),
            n,
        });

        // 3. Ablation: modules-only score (rounds where mutations are empty/minimal)
        let rounds_with_mutations = dataset
            .rounds
            .iter()
            .filter(|r| !r.mutations.is_empty())
            .count();
        let rounds_without_mutations = n - rounds_with_mutations;

        let score_with_mutations = if rounds_with_mutations > 0 {
            dataset
                .rounds
                .iter()
                .filter(|r| !r.mutations.is_empty())
                .map(|r| r.evasion_score)
                .sum::<f64>()
                / rounds_with_mutations as f64
        } else {
            0.0
        };

        let score_without_mutations = if rounds_without_mutations > 0 {
            dataset
                .rounds
                .iter()
                .filter(|r| r.mutations.is_empty())
                .map(|r| r.evasion_score)
                .sum::<f64>()
                / rounds_without_mutations as f64
        } else {
            0.0
        };

        let mutation_contribution = score_with_mutations - score_without_mutations;

        results.push(MetricResult {
            metric_id: "guidance.baseline_comparison.mutation_ablation".to_string(),
            axis: "guidance".to_string(),
            category: "baseline_comparison".to_string(),
            label: "Mutation ablation (score with mutations - score without)".to_string(),
            value: mutation_contribution,
            details: json!({
                "score_with": score_with_mutations,
                "score_without": score_without_mutations,
                "rounds_with": rounds_with_mutations,
                "rounds_without": rounds_without_mutations,
            }),
            n,
        });

        // 4. Token guidance ablation (using selection records)
        if !dataset.selections.is_empty() {
            let guided_selections = dataset
                .selections
                .iter()
                .filter(|s| !s.avoid_tokens.is_empty() || !s.seek_tokens.is_empty())
                .count();
            let unguided_selections = dataset.selections.len() - guided_selections;

            let guidance_usage_rate =
                guided_selections as f64 / dataset.selections.len().max(1) as f64;

            results.push(MetricResult {
                metric_id: "guidance.baseline_comparison.token_guidance_usage".to_string(),
                axis: "guidance".to_string(),
                category: "baseline_comparison".to_string(),
                label: "Token guidance usage rate (selections with avoid/seek tokens)".to_string(),
                value: guidance_usage_rate,
                details: json!({
                    "guided": guided_selections,
                    "unguided": unguided_selections,
                }),
                n,
            });
        }

        Ok(results)
    }
}
