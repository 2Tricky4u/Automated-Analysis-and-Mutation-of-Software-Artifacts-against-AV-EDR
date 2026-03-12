//! I13: Convergence Simulation Analysis
//!
//! Validates that the accumulation phase correctly transitions from
//! exploration to recipe building, and that recipes grow and stabilize.
//!
//! **Claim:** The accumulation phase produces monotonically growing recipes
//! that converge to a stable, high-scoring configuration.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct ConvergenceSimulationAnalysis;

impl InfraMetric for ConvergenceSimulationAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i13.convergence_simulation"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.convergence_simulation {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let mut metrics = Vec::new();

        // Use the first (primary) result for detailed analysis
        let primary = &results[0];

        // I13.1: Phase transition round — when does accumulation begin?
        let accumulation_round = primary
            .phase_transitions
            .iter()
            .find(|(_, phase)| phase == "Accumulation")
            .map(|(round, _)| *round)
            .unwrap_or(0);

        let phase_fraction = if primary.total_rounds > 0 {
            accumulation_round as f64 / primary.total_rounds as f64
        } else {
            0.0
        };

        metrics.push(MetricResult {
            metric_id: "infra.i13.convergence_simulation.phase_transition_round".to_string(),
            axis: "infrastructure".to_string(),
            category: "convergence_simulation".to_string(),
            label: "Round of first Accumulation phase entry (as fraction of total)".to_string(),
            value: phase_fraction,
            details: json!({
                "accumulation_round": accumulation_round,
                "total_rounds": primary.total_rounds,
                "all_transitions": primary.phase_transitions.iter().map(|(r, p)| {
                    json!({"round": r, "phase": p})
                }).collect::<Vec<_>>(),
            }),
            n: 1,
        });

        // I13.2: Recipe growth rate — slope of recipe size during accumulation
        let accum_sizes: Vec<(f64, f64)> = primary
            .recipe_size_trajectory
            .iter()
            .filter(|(round, _)| *round >= accumulation_round)
            .map(|(round, size)| (*round as f64, *size as f64))
            .collect();

        let growth_rate = if accum_sizes.len() >= 2 {
            // Simple linear regression slope
            let n_pts = accum_sizes.len() as f64;
            let sum_x: f64 = accum_sizes.iter().map(|(x, _)| x).sum();
            let sum_y: f64 = accum_sizes.iter().map(|(_, y)| y).sum();
            let sum_xy: f64 = accum_sizes.iter().map(|(x, y)| x * y).sum();
            let sum_xx: f64 = accum_sizes.iter().map(|(x, _)| x * x).sum();
            let denom = n_pts * sum_xx - sum_x * sum_x;
            if denom.abs() > f64::EPSILON {
                (n_pts * sum_xy - sum_x * sum_y) / denom
            } else {
                0.0
            }
        } else {
            0.0
        };

        metrics.push(MetricResult {
            metric_id: "infra.i13.convergence_simulation.recipe_growth_rate".to_string(),
            axis: "infrastructure".to_string(),
            category: "convergence_simulation".to_string(),
            label: "Recipe size growth rate during accumulation (mutations/round)".to_string(),
            value: growth_rate,
            details: json!({
                "growth_rate": growth_rate,
                "recipe_trajectory": primary.recipe_size_trajectory.iter().map(|(r, s)| {
                    json!({"round": r, "recipe_size": s})
                }).collect::<Vec<_>>(),
            }),
            n: accum_sizes.len(),
        });

        // I13.3: Diversity preservation — minimum diversity during accumulation
        let accum_diversity: Vec<f64> = primary
            .diversity_trajectory
            .iter()
            .filter(|(round, _)| *round >= accumulation_round)
            .map(|(_, div)| *div)
            .collect();

        let min_diversity = accum_diversity.iter().cloned().fold(f64::MAX, f64::min);
        let min_diversity = if min_diversity == f64::MAX {
            0.0
        } else {
            min_diversity
        };

        metrics.push(MetricResult {
            metric_id: "infra.i13.convergence_simulation.diversity_preservation".to_string(),
            axis: "infrastructure".to_string(),
            category: "convergence_simulation".to_string(),
            label: "Minimum recipe diversity during accumulation phase".to_string(),
            value: min_diversity,
            details: json!({
                "min_diversity": min_diversity,
                "diversity_trajectory": primary.diversity_trajectory.iter().map(|(r, d)| {
                    json!({"round": r, "diversity": d})
                }).collect::<Vec<_>>(),
            }),
            n: accum_diversity.len(),
        });

        // I13.4: Score plateau round — round where best score stops improving
        let scores: &Vec<(u32, f64)> = &primary.best_score_trajectory;
        let plateau_round = find_plateau_round(scores, 0.01);
        let plateau_fraction = if primary.total_rounds > 0 {
            plateau_round as f64 / primary.total_rounds as f64
        } else {
            0.0
        };

        metrics.push(MetricResult {
            metric_id: "infra.i13.convergence_simulation.score_plateau_round".to_string(),
            axis: "infrastructure".to_string(),
            category: "convergence_simulation".to_string(),
            label: "Round where best score plateaus (as fraction of total)".to_string(),
            value: plateau_fraction,
            details: json!({
                "plateau_round": plateau_round,
                "total_rounds": primary.total_rounds,
                "score_trajectory": scores.iter().map(|(r, s)| {
                    json!({"round": r, "score": s})
                }).collect::<Vec<_>>(),
                "marginal_contributions": primary.marginal_contribution_count.iter().map(|(r, c)| {
                    json!({"round": r, "contributing_mutations": c})
                }).collect::<Vec<_>>(),
            }),
            n: scores.len(),
        });

        Ok(metrics)
    }
}

/// Find the round where score improvement drops below threshold for 3+ consecutive rounds.
fn find_plateau_round(scores: &[(u32, f64)], threshold: f64) -> u32 {
    if scores.len() < 2 {
        return scores.first().map(|(r, _)| *r).unwrap_or(0);
    }

    let mut best_so_far = f64::MIN;
    let mut stagnant_count = 0u32;

    for (round, score) in scores {
        if *score > best_so_far + threshold {
            best_so_far = *score;
            stagnant_count = 0;
        } else {
            stagnant_count += 1;
            if stagnant_count >= 3 {
                return *round - 2; // Return the round where plateau started
            }
        }
    }

    // Never plateaued — return last round
    scores.last().map(|(r, _)| *r).unwrap_or(0)
}
