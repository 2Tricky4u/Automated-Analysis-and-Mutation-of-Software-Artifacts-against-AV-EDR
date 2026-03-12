//! I10: Oracle Stability Analysis
//!
//! Validates that token scoring and guidance classification are stable under
//! input permutation and incremental evaluation.
//!
//! **Claim:** Token scoring is deterministic, permutation-robust, and converges incrementally.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct OracleStabilityAnalysis;

impl InfraMetric for OracleStabilityAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i10.oracle_stability"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.oracle_stability {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I10.1: Determinism — all repeated runs identical
        let all_deterministic = results.iter().all(|r| r.repeated_deterministic);
        let determinism = if all_deterministic { 1.0 } else { 0.0 };

        metrics.push(MetricResult {
            metric_id: "infra.i10.oracle_stability.determinism".to_string(),
            axis: "infrastructure".to_string(),
            category: "oracle_stability".to_string(),
            label: "Scoring determinism (1.0 if all repeated runs identical)".to_string(),
            value: determinism,
            details: json!({
                "all_deterministic": all_deterministic,
                "per_case": results.iter().map(|r| json!({
                    "test_case": r.test_case,
                    "deterministic": r.repeated_deterministic,
                })).collect::<Vec<_>>(),
            }),
            n,
        });

        // I10.2: Permutation robustness — mean top-5 Jaccard across permutations
        let jaccard_values: Vec<f64> = results.iter().map(|r| r.permutation_top5_jaccard).collect();
        let mean_jaccard = jaccard_values.iter().sum::<f64>() / jaccard_values.len().max(1) as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i10.oracle_stability.permutation_robustness".to_string(),
            axis: "infrastructure".to_string(),
            category: "oracle_stability".to_string(),
            label: "Mean top-5 Jaccard similarity across round permutations".to_string(),
            value: mean_jaccard,
            details: json!({
                "mean_jaccard": mean_jaccard,
                "per_case": results.iter().map(|r| json!({
                    "test_case": r.test_case,
                    "jaccard": r.permutation_top5_jaccard,
                })).collect::<Vec<_>>(),
            }),
            n,
        });

        // I10.3: Incremental convergence — round fraction where Jaccard > 0.8 with final
        let mut convergence_points = Vec::new();
        for r in results {
            if !r.incremental_snapshots.is_empty() {
                let total = r.incremental_snapshots.len();
                let converged = r
                    .incremental_snapshots
                    .iter()
                    .filter(|s| s.jaccard_with_full > 0.8)
                    .count();
                convergence_points.push(converged as f64 / total as f64);
            }
        }
        let mean_convergence = if convergence_points.is_empty() {
            0.0
        } else {
            convergence_points.iter().sum::<f64>() / convergence_points.len() as f64
        };

        // Build convergence curve for plotting
        let curve_data: Vec<serde_json::Value> = results
            .iter()
            .flat_map(|r| {
                r.incremental_snapshots.iter().map(|s| {
                    json!({
                        "round_count": s.round_count,
                        "jaccard_with_full": s.jaccard_with_full,
                        "avoid_count": s.avoid_count,
                        "seek_count": s.seek_count,
                    })
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i10.oracle_stability.incremental_convergence".to_string(),
            axis: "infrastructure".to_string(),
            category: "oracle_stability".to_string(),
            label: "Fraction of incremental snapshots with Jaccard > 0.8".to_string(),
            value: mean_convergence,
            details: json!({
                "mean_convergence": mean_convergence,
                "convergence_curve": curve_data,
            }),
            n: convergence_points.len(),
        });

        // I10.4: Lift variance — mean per-token lift variance (lower = more stable)
        let variances: Vec<f64> = results
            .iter()
            .map(|r| r.permutation_lift_variance)
            .collect();
        let mean_variance = variances.iter().sum::<f64>() / variances.len().max(1) as f64;
        // Normalize: 1/(1+var) so high stability → value near 1.0
        let stability_score = 1.0 / (1.0 + mean_variance);

        metrics.push(MetricResult {
            metric_id: "infra.i10.oracle_stability.lift_variance".to_string(),
            axis: "infrastructure".to_string(),
            category: "oracle_stability".to_string(),
            label: "Lift stability score: 1/(1+mean_variance)".to_string(),
            value: stability_score,
            details: json!({
                "mean_lift_variance": mean_variance,
                "stability_score": stability_score,
                "per_case": results.iter().map(|r| json!({
                    "test_case": r.test_case,
                    "lift_variance": r.permutation_lift_variance,
                })).collect::<Vec<_>>(),
            }),
            n,
        });

        Ok(metrics)
    }
}
