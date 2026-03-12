//! I4: Binary Mutation Analysis
//!
//! Evaluates 9 PE transforms for validity, size impact, entropy shift, and feature deltas.
//!
//! **Claim:** 9 PE transforms preserve PE validity; shift feature vector toward benign profiles.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct BinaryMutationAnalysis;

impl InfraMetric for BinaryMutationAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i4.binary_mutation"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.binary_mutation {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I4.1: PE validity
        let valid_count = results.iter().filter(|r| r.pe_valid).count();
        let validity = valid_count as f64 / n as f64;

        let validity_table: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "mutation_id": r.mutation_id,
                    "pe_valid": r.pe_valid,
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i4.binary_mutation.pe_validity".to_string(),
            axis: "infrastructure".to_string(),
            category: "binary_mutation".to_string(),
            label: "Fraction of mutations producing valid PE".to_string(),
            value: validity,
            details: json!({
                "valid": valid_count,
                "total": n,
                "validity_table": validity_table,
            }),
            n,
        });

        // I4.2: Size impact per mutation
        let size_deltas: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                let delta = r.output_size as i64 - r.input_size as i64;
                let ratio = r.output_size as f64 / r.input_size.max(1) as f64;
                json!({
                    "mutation_id": r.mutation_id,
                    "input_size": r.input_size,
                    "output_size": r.output_size,
                    "size_delta": delta,
                    "size_ratio": ratio,
                })
            })
            .collect();

        let mean_ratio = results
            .iter()
            .map(|r| r.output_size as f64 / r.input_size.max(1) as f64)
            .sum::<f64>()
            / n as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i4.binary_mutation.size_impact".to_string(),
            axis: "infrastructure".to_string(),
            category: "binary_mutation".to_string(),
            label: "Per-mutation size delta table".to_string(),
            value: mean_ratio,
            details: json!({
                "mutations": size_deltas,
                "mean_size_ratio": mean_ratio,
            }),
            n,
        });

        // I4.3: Entropy shift
        let entropy_shifts: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "mutation_id": r.mutation_id,
                    "entropy_before": r.text_entropy_before,
                    "entropy_after": r.text_entropy_after,
                    "entropy_delta": r.text_entropy_after - r.text_entropy_before,
                })
            })
            .collect();

        let mean_entropy_shift = results
            .iter()
            .map(|r| (r.text_entropy_after - r.text_entropy_before).abs())
            .sum::<f64>()
            / n as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i4.binary_mutation.entropy_shift".to_string(),
            axis: "infrastructure".to_string(),
            category: "binary_mutation".to_string(),
            label: "Mean absolute .text entropy shift".to_string(),
            value: mean_entropy_shift,
            details: json!({
                "shifts": entropy_shifts,
                "mean_abs_shift": mean_entropy_shift,
            }),
            n,
        });

        // I4.4: Feature heatmap (mutations × features)
        let heatmap: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "mutation_id": r.mutation_id,
                    "size_delta": r.output_size as i64 - r.input_size as i64,
                    "section_delta": r.section_count_delta,
                    "import_delta": r.import_count_delta,
                    "entropy_delta": r.text_entropy_after - r.text_entropy_before,
                    "transform_time_us": r.transform_time_us,
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i4.binary_mutation.feature_heatmap".to_string(),
            axis: "infrastructure".to_string(),
            category: "binary_mutation".to_string(),
            label: "9 mutations × 4 features heatmap".to_string(),
            value: n as f64,
            details: json!({
                "heatmap": heatmap,
                "features": ["size_delta", "section_delta", "import_delta", "entropy_delta"],
            }),
            n,
        });

        Ok(metrics)
    }
}
