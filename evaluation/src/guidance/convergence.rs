//! Guidance.Convergence — measures discovery decay and plateau behavior.
//!
//! Sub-metrics:
//! - Discovery decay ratio (2nd half evasion discoveries / 1st half)
//! - Plateau detection (round where rolling score stops improving)
//! - Cumulative unique configs over time
//! - Exploitation vs exploration ratio from selector rationale

use crate::helpers::{config_fingerprint, rolling_mean};
use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::HashSet;

pub struct Convergence;

impl EvalMetric for Convergence {
    fn metric_id(&self) -> &str {
        "guidance.convergence"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n < 4 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();
        let mid = n / 2;

        // 1. Discovery decay ratio
        let first_half_evasions = dataset.rounds[..mid]
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::Evasion)
            .count();
        let second_half_evasions = dataset.rounds[mid..]
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::Evasion)
            .count();

        let decay_ratio = if first_half_evasions > 0 {
            second_half_evasions as f64 / first_half_evasions as f64
        } else if second_half_evasions > 0 {
            f64::INFINITY // Improvement from zero
        } else {
            1.0 // Both zero
        };

        // Clamp for JSON serialization
        let decay_clamped = decay_ratio.min(10.0);

        results.push(MetricResult {
            metric_id: "guidance.convergence.decay_ratio".to_string(),
            axis: "guidance".to_string(),
            category: "convergence".to_string(),
            label: "Discovery decay ratio (2nd half evasions / 1st half, >1 = improving)"
                .to_string(),
            value: decay_clamped,
            details: json!({
                "first_half_evasions": first_half_evasions,
                "second_half_evasions": second_half_evasions,
            }),
            n,
        });

        // 2. Plateau detection
        let scores: Vec<f64> = dataset.rounds.iter().map(|r| r.evasion_score).collect();
        let window = 5.min(n);
        let trajectory = rolling_mean(&scores, window);

        // Plateau: first round where improvement over previous window is < threshold
        let improvement_threshold = 0.01;
        let plateau_round = trajectory.windows(2).enumerate().find_map(|(i, w)| {
            if (w[1] - w[0]).abs() < improvement_threshold {
                Some(i + window) // Round number (approx)
            } else {
                None
            }
        });

        let plateau_fraction = plateau_round.map_or(1.0, |r| r as f64 / n as f64);

        results.push(MetricResult {
            metric_id: "guidance.convergence.plateau_round".to_string(),
            axis: "guidance".to_string(),
            category: "convergence".to_string(),
            label: "Plateau onset (fraction of total rounds before score stalls)".to_string(),
            value: plateau_fraction,
            details: json!({
                "plateau_round": plateau_round,
                "total_rounds": n,
                "improvement_threshold": improvement_threshold,
            }),
            n,
        });

        // 3. Cumulative unique configs
        let mut seen = HashSet::new();
        let mut cumulative: Vec<usize> = Vec::with_capacity(n);
        for round in &dataset.rounds {
            let fp = config_fingerprint(&round.modules, &round.mutations);
            seen.insert(fp);
            cumulative.push(seen.len());
        }

        // Discovery rate in 2nd half vs 1st half
        let first_half_discoveries = cumulative[mid - 1];
        let second_half_discoveries = seen.len() - first_half_discoveries;

        let config_decay = if first_half_discoveries > 0 {
            second_half_discoveries as f64 / first_half_discoveries as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "guidance.convergence.config_discovery_decay".to_string(),
            axis: "guidance".to_string(),
            category: "convergence".to_string(),
            label: "Config discovery decay (2nd half new configs / 1st half)".to_string(),
            value: config_decay,
            details: json!({
                "total_unique": seen.len(),
                "first_half_configs": first_half_discoveries,
                "second_half_new": second_half_discoveries,
                "cumulative": cumulative,
            }),
            n,
        });

        // 4. Exploitation vs exploration ratio from selection rationale
        if !dataset.selections.is_empty() {
            let exploit_keywords = ["exploit", "best", "repeat", "refine", "top"];
            let explore_keywords = ["explore", "random", "epsilon", "new", "untried"];

            let mut exploit_count = 0usize;
            let mut explore_count = 0usize;

            for sel in &dataset.selections {
                let rationale_lower = sel.rationale.to_lowercase();
                let is_exploit = exploit_keywords.iter().any(|k| rationale_lower.contains(k));
                let is_explore = explore_keywords.iter().any(|k| rationale_lower.contains(k));

                if is_exploit {
                    exploit_count += 1;
                }
                if is_explore {
                    explore_count += 1;
                }
            }

            let total_classified = exploit_count + explore_count;
            let exploitation_ratio = if total_classified > 0 {
                exploit_count as f64 / total_classified as f64
            } else {
                0.5 // Neutral if unclassifiable
            };

            results.push(MetricResult {
                metric_id: "guidance.convergence.exploitation_ratio".to_string(),
                axis: "guidance".to_string(),
                category: "convergence".to_string(),
                label: "Exploitation ratio (exploit / exploit+explore from rationale)".to_string(),
                value: exploitation_ratio,
                details: json!({
                    "exploit": exploit_count,
                    "explore": explore_count,
                    "unclassified": dataset.selections.len() - exploit_count.max(explore_count),
                }),
                n,
            });
        }

        Ok(results)
    }
}
