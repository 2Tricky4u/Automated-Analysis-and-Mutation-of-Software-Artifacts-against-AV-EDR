//! Oracle.Attribution — measures token-level detection attribution quality.
//!
//! Sub-metrics:
//! - Token lift ranking via compute_token_scores
//! - Top-5 token stability across sliding windows
//! - Counterfactual test: detection rate with/without top token

use crate::{EvalDataset, EvalMetric, MetricResult, TokenMatrixEntry};
use controller::triage::scorer::{build_guidance, compute_token_scores};
use serde_json::json;
use std::collections::HashSet;

pub struct Attribution;

/// Build the (tokens, detected) tuples from trustworthy token matrix entries.
fn trustworthy_matrix(entries: &[TokenMatrixEntry]) -> Vec<(Vec<String>, bool)> {
    entries
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect()
}

impl EvalMetric for Attribution {
    fn metric_id(&self) -> &str {
        "oracle.attribution"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        if dataset.token_matrices.is_empty() {
            return Ok(vec![]);
        }

        let n = dataset.token_matrices.len();
        let matrix = trustworthy_matrix(&dataset.token_matrices);

        if matrix.len() < 4 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Token lift ranking — compute scores and report top tokens
        let scores = compute_token_scores(&matrix);
        let guidance = build_guidance(&scores, 1.5, 0.3);

        let top_avoid: Vec<serde_json::Value> = scores
            .iter()
            .filter(|s| s.lift > 1.5 && s.confidence > 0.3)
            .take(10)
            .map(|s| {
                json!({
                    "token": s.token,
                    "lift": s.lift,
                    "confidence": s.confidence,
                    "importance": s.importance,
                })
            })
            .collect();

        let has_meaningful_tokens = !scores.is_empty();
        let top_importance = scores.first().map(|s| s.importance).unwrap_or(0.0);

        results.push(MetricResult {
            metric_id: "oracle.attribution.token_ranking".to_string(),
            axis: "oracle".to_string(),
            category: "attribution".to_string(),
            label: "Token lift ranking quality (top importance score)".to_string(),
            value: top_importance,
            details: json!({
                "total_tokens_scored": scores.len(),
                "avoid_tokens": guidance.avoid_tokens.len(),
                "seek_tokens": guidance.seek_tokens.len(),
                "top_avoid": top_avoid,
                "has_meaningful_tokens": has_meaningful_tokens,
            }),
            n,
        });

        // 2. Top-5 token stability across sliding windows
        let window_size = matrix.len() / 2;
        if window_size >= 4 {
            let first_half = &matrix[..window_size];
            let second_half = &matrix[window_size..];

            let scores_first = compute_token_scores(first_half);
            let scores_second = compute_token_scores(second_half);

            let top5_first: HashSet<String> = scores_first
                .iter()
                .take(5)
                .map(|s| s.token.clone())
                .collect();
            let top5_second: HashSet<String> = scores_second
                .iter()
                .take(5)
                .map(|s| s.token.clone())
                .collect();

            let overlap = top5_first.intersection(&top5_second).count();
            let stability = overlap as f64 / 5.0;

            results.push(MetricResult {
                metric_id: "oracle.attribution.top5_stability".to_string(),
                axis: "oracle".to_string(),
                category: "attribution".to_string(),
                label: "Top-5 token stability (overlap between 1st and 2nd half)".to_string(),
                value: stability,
                details: json!({
                    "overlap": overlap,
                    "first_half_top5": top5_first.iter().collect::<Vec<_>>(),
                    "second_half_top5": top5_second.iter().collect::<Vec<_>>(),
                    "window_size": window_size,
                }),
                n,
            });
        }

        // 3. Counterfactual test: detection rate with/without top token
        if let Some(top_token) = scores.first() {
            let with_token: Vec<&(Vec<String>, bool)> = matrix
                .iter()
                .filter(|(tokens, _)| tokens.contains(&top_token.token))
                .collect();
            let without_token: Vec<&(Vec<String>, bool)> = matrix
                .iter()
                .filter(|(tokens, _)| !tokens.contains(&top_token.token))
                .collect();

            let detect_rate_with = if !with_token.is_empty() {
                with_token.iter().filter(|(_, d)| *d).count() as f64 / with_token.len() as f64
            } else {
                0.0
            };

            let detect_rate_without = if !without_token.is_empty() {
                without_token.iter().filter(|(_, d)| *d).count() as f64 / without_token.len() as f64
            } else {
                0.0
            };

            let counterfactual_delta = detect_rate_with - detect_rate_without;

            results.push(MetricResult {
                metric_id: "oracle.attribution.counterfactual".to_string(),
                axis: "oracle".to_string(),
                category: "attribution".to_string(),
                label: "Counterfactual delta (detection rate with vs without top token)"
                    .to_string(),
                value: counterfactual_delta,
                details: json!({
                    "top_token": top_token.token,
                    "detect_rate_with": detect_rate_with,
                    "detect_rate_without": detect_rate_without,
                    "n_with": with_token.len(),
                    "n_without": without_token.len(),
                }),
                n,
            });
        }

        Ok(results)
    }
}
