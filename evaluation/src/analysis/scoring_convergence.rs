//! C4: Scoring Convergence over Rounds
//!
//! Measures how many rounds are needed before the triage signal stabilizes by
//! computing incremental token scores on first-N-rounds subsets.
//!
//! **RQ:** How many rounds until the triage signal stabilizes?
//!
//! **Output:** Overlap-vs-N curve; actionable-count-vs-N line chart

use crate::{EvalDataset, EvalMetric, MetricResult, TokenMatrixEntry};
use controller::triage::scorer::{build_guidance, compute_token_scores};
use serde_json::json;
use std::collections::HashSet;

pub struct ScoringConvergence;

fn trustworthy_pairs(entries: &[TokenMatrixEntry]) -> Vec<(Vec<String>, bool)> {
    entries
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect()
}

impl EvalMetric for ScoringConvergence {
    fn metric_id(&self) -> &str {
        "component.c4.scoring_convergence"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        if dataset.token_matrices.is_empty() {
            return Ok(vec![]);
        }

        let all_matrix = trustworthy_pairs(&dataset.token_matrices);
        if all_matrix.len() < 6 {
            return Ok(vec![]);
        }

        let n = dataset.rounds.len();
        let mut results = Vec::new();

        // Final scores (computed over all rounds) — the reference
        let final_scores = compute_token_scores(&all_matrix);
        let final_top5: HashSet<String> = final_scores
            .iter()
            .take(5)
            .map(|s| s.token.clone())
            .collect();
        let final_top10: HashSet<String> = final_scores
            .iter()
            .take(10)
            .map(|s| s.token.clone())
            .collect();
        let final_guidance = build_guidance(&final_scores, 1.5, 0.3);
        let final_actionable = final_guidance.avoid_tokens.len() + final_guidance.seek_tokens.len();

        // Checkpoints: first N rounds
        let checkpoints: Vec<usize> = vec![5, 10, 15, 20, 25, 30]
            .into_iter()
            .filter(|&cp| cp <= all_matrix.len())
            .collect();

        let mut convergence_data = Vec::new();

        for &cp in &checkpoints {
            let subset = &all_matrix[..cp];
            let scores_at_n = compute_token_scores(subset);

            // Top-5 overlap with final
            let top5_at_n: HashSet<String> = scores_at_n
                .iter()
                .take(5)
                .map(|s| s.token.clone())
                .collect();
            let top5_overlap = final_top5.intersection(&top5_at_n).count();
            let top5_jaccard = if final_top5.is_empty() && top5_at_n.is_empty() {
                1.0
            } else {
                let union = final_top5.union(&top5_at_n).count();
                top5_overlap as f64 / union.max(1) as f64
            };

            // Top-10 overlap
            let top10_at_n: HashSet<String> = scores_at_n
                .iter()
                .take(10)
                .map(|s| s.token.clone())
                .collect();
            let top10_overlap = final_top10.intersection(&top10_at_n).count();

            // Actionable count at this checkpoint
            let guidance_at_n = build_guidance(&scores_at_n, 1.5, 0.3);
            let actionable_at_n =
                guidance_at_n.avoid_tokens.len() + guidance_at_n.seek_tokens.len();

            // Lift variance at this checkpoint
            let lifts: Vec<f64> = scores_at_n.iter().map(|s| s.lift).collect();
            let mean_lift = if lifts.is_empty() {
                0.0
            } else {
                lifts.iter().sum::<f64>() / lifts.len() as f64
            };
            let lift_variance = if lifts.len() > 1 {
                lifts.iter().map(|l| (l - mean_lift).powi(2)).sum::<f64>() / lifts.len() as f64
            } else {
                0.0
            };

            convergence_data.push(json!({
                "rounds": cp,
                "top5_overlap": top5_overlap,
                "top5_overlap_frac": top5_overlap as f64 / 5.0,
                "top5_jaccard": top5_jaccard,
                "top10_overlap": top10_overlap,
                "top10_overlap_frac": top10_overlap as f64 / 10.0,
                "actionable_count": actionable_at_n,
                "avoid_count": guidance_at_n.avoid_tokens.len(),
                "seek_count": guidance_at_n.seek_tokens.len(),
                "total_scored": scores_at_n.len(),
                "mean_lift": mean_lift,
                "lift_variance": lift_variance,
                "top5_tokens": top5_at_n.iter().collect::<Vec<_>>(),
            }));
        }

        // 1. Top-5 convergence curve
        let final_overlap = convergence_data
            .last()
            .and_then(|d| d["top5_overlap_frac"].as_f64())
            .unwrap_or(0.0);

        results.push(MetricResult {
            metric_id: "component.c4.scoring_convergence.top5_overlap".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Top-5 token overlap convergence (final overlap fraction)".to_string(),
            value: final_overlap,
            details: json!({
                "convergence_curve": convergence_data,
                "final_top5": final_top5.iter().collect::<Vec<_>>(),
                "checkpoints": checkpoints,
            }),
            n,
        });

        // 2. Actionable count convergence
        let actionable_stable_round = convergence_data
            .windows(2)
            .find_map(|w| {
                let a1 = w[0]["actionable_count"].as_u64().unwrap_or(0);
                let a2 = w[1]["actionable_count"].as_u64().unwrap_or(0);
                if a1 == a2 {
                    w[0]["rounds"].as_u64()
                } else {
                    None
                }
            })
            .unwrap_or(0);

        results.push(MetricResult {
            metric_id: "component.c4.scoring_convergence.actionable_stability".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Round where actionable token count stabilizes".to_string(),
            value: actionable_stable_round as f64,
            details: json!({
                "final_actionable": final_actionable,
                "actionable_curve": convergence_data.iter().map(|d| json!({
                    "rounds": d["rounds"],
                    "actionable": d["actionable_count"],
                })).collect::<Vec<_>>(),
                "stable_round": actionable_stable_round,
            }),
            n,
        });

        // 3. Lift variance convergence
        let lift_variances: Vec<f64> = convergence_data
            .iter()
            .filter_map(|d| d["lift_variance"].as_f64())
            .collect();
        let final_lift_var = *lift_variances.last().unwrap_or(&0.0);

        results.push(MetricResult {
            metric_id: "component.c4.scoring_convergence.lift_variance".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Lift variance convergence (final variance)".to_string(),
            value: final_lift_var,
            details: json!({
                "variance_curve": convergence_data.iter().map(|d| json!({
                    "rounds": d["rounds"],
                    "lift_variance": d["lift_variance"],
                    "mean_lift": d["mean_lift"],
                })).collect::<Vec<_>>(),
            }),
            n,
        });

        Ok(results)
    }
}
