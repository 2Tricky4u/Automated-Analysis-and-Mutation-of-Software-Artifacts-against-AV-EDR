//! C1: Token Scoring Sensitivity Analysis
//!
//! Sweeps a 5×5 grid of `(lift_threshold, min_confidence)` parameters and reports
//! how many avoid/seek tokens are produced at each point.
//!
//! **RQ:** How sensitive is the feedback signal to hyperparameters?
//!
//! **Output:** Heatmap data (threshold × confidence → actionable count)

use crate::{EvalDataset, EvalMetric, MetricResult, TokenMatrixEntry};
use controller::triage::scorer::{build_guidance, compute_token_scores};
use serde_json::json;

pub struct TokenSensitivity;

/// Grid values for the sweep.
const LIFT_THRESHOLDS: &[f64] = &[1.2, 1.5, 2.0, 3.0, 5.0];
const MIN_CONFIDENCES: &[f64] = &[0.1, 0.2, 0.3, 0.5, 0.8];

fn trustworthy_pairs(entries: &[TokenMatrixEntry]) -> Vec<(Vec<String>, bool)> {
    entries
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect()
}

impl EvalMetric for TokenSensitivity {
    fn metric_id(&self) -> &str {
        "component.c1.token_sensitivity"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        if dataset.token_matrices.is_empty() {
            return Ok(vec![]);
        }

        let matrix = trustworthy_pairs(&dataset.token_matrices);
        if matrix.len() < 4 {
            return Ok(vec![]);
        }

        let scores = compute_token_scores(&matrix);
        if scores.is_empty() {
            return Ok(vec![]);
        }

        let n = dataset.rounds.len();
        let mut results = Vec::new();

        // 1. Heatmap: actionable token count at each (lift, confidence) pair
        let mut heatmap_rows = Vec::new();
        let mut max_actionable = 0usize;
        let mut min_actionable = usize::MAX;

        for &lift in LIFT_THRESHOLDS {
            let mut row = Vec::new();
            for &conf in MIN_CONFIDENCES {
                let guidance = build_guidance(&scores, lift, conf);
                let avoid = guidance.avoid_tokens.len();
                let seek = guidance.seek_tokens.len();
                let actionable = avoid + seek;
                max_actionable = max_actionable.max(actionable);
                min_actionable = min_actionable.min(actionable);

                row.push(json!({
                    "lift_threshold": lift,
                    "min_confidence": conf,
                    "avoid_count": avoid,
                    "seek_count": seek,
                    "actionable": actionable,
                    "avoid_tokens": &guidance.avoid_tokens[..guidance.avoid_tokens.len().min(5)],
                    "seek_tokens": &guidance.seek_tokens[..guidance.seek_tokens.len().min(5)],
                }));
            }
            heatmap_rows.push(row);
        }

        // Primary value: sensitivity range (max - min actionable across grid)
        let sensitivity_range = if min_actionable <= max_actionable {
            (max_actionable - min_actionable) as f64 / scores.len().max(1) as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "component.c1.token_sensitivity.heatmap".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Token sensitivity heatmap (5×5 lift × confidence grid)".to_string(),
            value: sensitivity_range,
            details: json!({
                "lift_thresholds": LIFT_THRESHOLDS,
                "min_confidences": MIN_CONFIDENCES,
                "heatmap": heatmap_rows,
                "total_scored_tokens": scores.len(),
                "max_actionable": max_actionable,
                "min_actionable": min_actionable,
            }),
            n,
        });

        // 2. Default operating point analysis (lift=1.5, confidence=0.3)
        let default_guidance = build_guidance(&scores, 1.5, 0.3);
        let default_actionable =
            default_guidance.avoid_tokens.len() + default_guidance.seek_tokens.len();
        let guidance_strength = default_actionable as f64 / scores.len().max(1) as f64;

        results.push(MetricResult {
            metric_id: "component.c1.token_sensitivity.default_operating_point".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Default operating point (lift=1.5, confidence=0.3)".to_string(),
            value: guidance_strength,
            details: json!({
                "avoid_count": default_guidance.avoid_tokens.len(),
                "seek_count": default_guidance.seek_tokens.len(),
                "total_scored": scores.len(),
                "avoid_tokens": default_guidance.avoid_tokens,
                "seek_tokens": default_guidance.seek_tokens,
            }),
            n,
        });

        // 3. Lift distribution of all scored tokens
        let lift_values: Vec<f64> = scores.iter().map(|s| s.lift).collect();
        let confidence_values: Vec<f64> = scores.iter().map(|s| s.confidence).collect();
        let importance_values: Vec<f64> = scores.iter().map(|s| s.importance).collect();

        let mean_lift = lift_values.iter().sum::<f64>() / lift_values.len() as f64;
        let mean_confidence =
            confidence_values.iter().sum::<f64>() / confidence_values.len() as f64;

        let lift_variance = lift_values
            .iter()
            .map(|l| (l - mean_lift).powi(2))
            .sum::<f64>()
            / lift_values.len() as f64;

        results.push(MetricResult {
            metric_id: "component.c1.token_sensitivity.lift_distribution".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Token lift distribution statistics".to_string(),
            value: lift_variance.sqrt(),
            details: json!({
                "mean_lift": mean_lift,
                "lift_stddev": lift_variance.sqrt(),
                "mean_confidence": mean_confidence,
                "top_10_tokens": scores.iter().take(10).map(|s| json!({
                    "token": s.token,
                    "lift": s.lift,
                    "confidence": s.confidence,
                    "importance": s.importance,
                    "n_detected": s.n_detected,
                    "n_total": s.n_total,
                })).collect::<Vec<_>>(),
                "all_lifts": lift_values,
                "all_confidences": confidence_values,
                "all_importances": importance_values,
            }),
            n,
        });

        Ok(results)
    }
}
