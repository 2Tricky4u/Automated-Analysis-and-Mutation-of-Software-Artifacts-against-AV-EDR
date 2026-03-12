//! I9: Input Diversity Analysis
//!
//! Measures pairwise structural distance between mutation outputs to validate
//! that mutation operators produce diverse artifacts from identical inputs.
//!
//! **Claim:** Mutation operators produce structurally diverse outputs.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct InputDiversityAnalysis;

impl InfraMetric for InputDiversityAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i9.input_diversity"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.input_diversity {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I9.1: Mean pairwise normalized distance
        let distances: Vec<f64> = results.iter().map(|r| r.normalized_distance).collect();
        let mean_distance = distances.iter().sum::<f64>() / distances.len().max(1) as f64;

        // Build heatmap data: collect unique mutations and organize distances
        let mut mutations: Vec<String> = Vec::new();
        for r in results {
            if !mutations.contains(&r.mutation_a) {
                mutations.push(r.mutation_a.clone());
            }
            if !mutations.contains(&r.mutation_b) {
                mutations.push(r.mutation_b.clone());
            }
        }
        mutations.sort();

        let size = mutations.len();
        let mut heatmap = vec![vec![0.0f64; size]; size];
        for r in results {
            if let (Some(i), Some(j)) = (
                mutations.iter().position(|m| m == &r.mutation_a),
                mutations.iter().position(|m| m == &r.mutation_b),
            ) {
                heatmap[i][j] = r.normalized_distance;
                heatmap[j][i] = r.normalized_distance;
            }
        }

        metrics.push(MetricResult {
            metric_id: "infra.i9.input_diversity.pairwise_distance".to_string(),
            axis: "infrastructure".to_string(),
            category: "input_diversity".to_string(),
            label: "Mean pairwise normalized distance between mutation outputs".to_string(),
            value: mean_distance,
            details: json!({
                "mean_distance": mean_distance,
                "min_distance": distances.iter().cloned().fold(f64::MAX, f64::min),
                "max_distance": distances.iter().cloned().fold(f64::MIN, f64::max),
                "heatmap": {
                    "labels": mutations,
                    "matrix": heatmap,
                },
            }),
            n,
        });

        // I9.2: Output uniqueness — fraction of pairs that produce different output
        let differ_count = results.iter().filter(|r| r.outputs_differ).count();
        let uniqueness = differ_count as f64 / n.max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i9.input_diversity.output_uniqueness".to_string(),
            axis: "infrastructure".to_string(),
            category: "input_diversity".to_string(),
            label: "Fraction of mutation pairs producing distinct output".to_string(),
            value: uniqueness,
            details: json!({
                "differ_count": differ_count,
                "total_pairs": n,
            }),
            n,
        });

        // I9.3: Param sensitivity — from AST mutation data if available
        if let Some(ast_results) = &dataset.ast_mutation {
            let mut mutation_outputs: std::collections::HashMap<String, Vec<i64>> =
                std::collections::HashMap::new();
            for r in ast_results {
                let base_id = r.mutation_id.split(':').next().unwrap_or(&r.mutation_id);
                mutation_outputs
                    .entry(base_id.to_string())
                    .or_default()
                    .push(r.line_delta);
            }
            let mutations_with_variants = mutation_outputs
                .values()
                .filter(|deltas| deltas.len() > 1)
                .count();
            let mutations_with_different_output = mutation_outputs
                .values()
                .filter(|deltas| deltas.len() > 1 && deltas.windows(2).any(|w| w[0] != w[1]))
                .count();
            let param_sensitivity = if mutations_with_variants > 0 {
                mutations_with_different_output as f64 / mutations_with_variants as f64
            } else {
                0.0
            };

            metrics.push(MetricResult {
                metric_id: "infra.i9.input_diversity.param_sensitivity".to_string(),
                axis: "infrastructure".to_string(),
                category: "input_diversity".to_string(),
                label: "Fraction of mutations where different params produce different output"
                    .to_string(),
                value: param_sensitivity,
                details: json!({
                    "mutations_with_variants": mutations_with_variants,
                    "mutations_with_different_output": mutations_with_different_output,
                }),
                n: mutations_with_variants,
            });
        }

        // I9.4: Encoding entropy spread (from payload encoding data if available)
        if let Some(encoding_results) = &dataset.payload_encoding {
            let entropies: Vec<f64> = encoding_results.iter().map(|r| r.encoded_entropy).collect();
            let min_entropy = entropies.iter().cloned().fold(f64::MAX, f64::min);
            let max_entropy = entropies.iter().cloned().fold(f64::MIN, f64::max);
            let spread = max_entropy - min_entropy;

            metrics.push(MetricResult {
                metric_id: "infra.i9.input_diversity.encoding_entropy_spread".to_string(),
                axis: "infrastructure".to_string(),
                category: "input_diversity".to_string(),
                label: "Entropy range across encoding types (bits)".to_string(),
                value: spread,
                details: json!({
                    "min_entropy": min_entropy,
                    "max_entropy": max_entropy,
                    "spread": spread,
                }),
                n: entropies.len(),
            });
        }

        Ok(metrics)
    }
}
