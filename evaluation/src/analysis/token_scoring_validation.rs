//! I8: Token Scoring Validation
//!
//! Validates lift computation accuracy and guidance classification correctness.
//!
//! **Claim:** Lift computation is mathematically correct;
//! degenerate inputs handled gracefully.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct TokenScoringValidation;

impl InfraMetric for TokenScoringValidation {
    fn metric_id(&self) -> &str {
        "infra.i8.token_scoring"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.token_scoring {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I8.1: Lift accuracy (max absolute error)
        let errors: Vec<f64> = results.iter().map(|r| r.lift_error).collect();
        let max_error = errors.iter().cloned().fold(f64::MIN, f64::max);
        let mean_error = errors.iter().sum::<f64>() / errors.len() as f64;

        let case_details: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "test_case": r.test_case,
                    "input_rounds": r.input_rounds,
                    "expected_lift": r.expected_lift,
                    "computed_lift": r.computed_lift,
                    "lift_error": r.lift_error,
                    "pass": r.lift_error < 1e-6,
                })
            })
            .collect();

        // Value: 1.0 - max_error (clamped to [0,1] — closer to 1.0 means more accurate)
        let accuracy = (1.0 - max_error).max(0.0).min(1.0);

        metrics.push(MetricResult {
            metric_id: "infra.i8.token_scoring.lift_accuracy".to_string(),
            axis: "infrastructure".to_string(),
            category: "token_scoring".to_string(),
            label: "Lift computation accuracy (1 - max_error)".to_string(),
            value: accuracy,
            details: json!({
                "max_absolute_error": max_error,
                "mean_absolute_error": mean_error,
                "test_cases": case_details,
                "all_pass": max_error < 1e-6,
            }),
            n,
        });

        // I8.2: Guidance classification correctness
        let correct_count = results.iter().filter(|r| r.guidance_correct).count();
        let correctness = correct_count as f64 / n as f64;

        let incorrect: Vec<serde_json::Value> = results
            .iter()
            .filter(|r| !r.guidance_correct)
            .map(|r| {
                json!({
                    "test_case": r.test_case,
                    "expected_lift": r.expected_lift,
                    "computed_lift": r.computed_lift,
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i8.token_scoring.guidance_correctness".to_string(),
            axis: "infrastructure".to_string(),
            category: "token_scoring".to_string(),
            label: "Avoid/seek classification accuracy".to_string(),
            value: correctness,
            details: json!({
                "correct": correct_count,
                "total": n,
                "incorrect_cases": incorrect,
            }),
            n,
        });

        Ok(metrics)
    }
}
