//! Guidance.FeedbackQuality — measures how well token feedback correlates with outcomes.
//!
//! Sub-metrics:
//! - Pearson correlation between coverage and evasion score
//! - Avoid token count from build_guidance
//! - Guidance avoidance rate: fraction of rounds after guidance that avoid top-avoid tokens

use crate::helpers::pearson_correlation;
use crate::{EvalDataset, EvalMetric, MetricResult, TokenMatrixEntry};
use controller::triage::scorer::{build_guidance, compute_token_scores};
use serde_json::json;

pub struct FeedbackQuality;

fn trustworthy_pairs(entries: &[TokenMatrixEntry]) -> Vec<(Vec<String>, bool)> {
    entries
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect()
}

impl EvalMetric for FeedbackQuality {
    fn metric_id(&self) -> &str {
        "guidance.feedback_quality"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Coverage ↔ evasion score correlation
        let coverage_scores: Vec<(f64, f64)> = dataset
            .rounds
            .iter()
            .filter_map(|r| r.coverage_percent.map(|c| (c, r.evasion_score)))
            .collect();

        if coverage_scores.len() >= 4 {
            let x: Vec<f64> = coverage_scores.iter().map(|(c, _)| *c).collect();
            let y: Vec<f64> = coverage_scores.iter().map(|(_, e)| *e).collect();
            let corr = pearson_correlation(&x, &y);

            results.push(MetricResult {
                metric_id: "guidance.feedback_quality.coverage_correlation".to_string(),
                axis: "guidance".to_string(),
                category: "feedback_quality".to_string(),
                label: "Coverage ↔ evasion score Pearson correlation".to_string(),
                value: corr,
                details: json!({
                    "n_pairs": coverage_scores.len(),
                }),
                n,
            });
        }

        // 2. Token guidance quality from build_guidance
        if !dataset.token_matrices.is_empty() {
            let matrix = trustworthy_pairs(&dataset.token_matrices);
            if matrix.len() >= 4 {
                let scores = compute_token_scores(&matrix);
                let guidance = build_guidance(&scores, 1.5, 0.3);

                let guidance_strength = (guidance.avoid_tokens.len() + guidance.seek_tokens.len())
                    as f64
                    / scores.len().max(1) as f64;

                results.push(MetricResult {
                    metric_id: "guidance.feedback_quality.guidance_strength".to_string(),
                    axis: "guidance".to_string(),
                    category: "feedback_quality".to_string(),
                    label: "Guidance strength (avoid+seek tokens / total scored)".to_string(),
                    value: guidance_strength,
                    details: json!({
                        "avoid_count": guidance.avoid_tokens.len(),
                        "seek_count": guidance.seek_tokens.len(),
                        "total_scored": scores.len(),
                        "avoid_tokens": &guidance.avoid_tokens[..guidance.avoid_tokens.len().min(10)],
                        "seek_tokens": &guidance.seek_tokens[..guidance.seek_tokens.len().min(10)],
                    }),
                    n,
                });

                // 3. Avoidance rate: after midpoint, do rounds avoid the top-avoid tokens?
                let midpoint = dataset.token_matrices.len() / 2;
                if midpoint > 0 && !guidance.avoid_tokens.is_empty() {
                    let post_guidance = &dataset.token_matrices[midpoint..];
                    let top_avoid: Vec<&str> = guidance
                        .avoid_tokens
                        .iter()
                        .take(5)
                        .map(|s: &String| s.as_str())
                        .collect();

                    let mut avoided = 0usize;
                    let mut total = 0usize;
                    for entry in post_guidance {
                        total += 1;
                        let has_avoid =
                            entry.tokens.iter().any(|t| top_avoid.contains(&t.as_str()));
                        if !has_avoid {
                            avoided += 1;
                        }
                    }

                    let avoidance_rate = if total > 0 {
                        avoided as f64 / total as f64
                    } else {
                        0.0
                    };

                    results.push(MetricResult {
                        metric_id: "guidance.feedback_quality.avoidance_rate".to_string(),
                        axis: "guidance".to_string(),
                        category: "feedback_quality".to_string(),
                        label: "Post-guidance avoidance rate (rounds without top-5 avoid tokens)"
                            .to_string(),
                        value: avoidance_rate,
                        details: json!({
                            "avoided": avoided,
                            "total_post_guidance": total,
                            "top_avoid_tokens": top_avoid,
                        }),
                        n,
                    });
                }
            }
        }

        Ok(results)
    }
}
