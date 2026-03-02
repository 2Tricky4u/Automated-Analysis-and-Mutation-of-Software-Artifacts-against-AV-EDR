//! Oracle.Stability — measures reproducibility of detection outcomes.
//!
//! Sub-metrics:
//! - Flaky rate (Flaky category / trustworthy+flaky)
//! - Behavior match rate (baseline and instrumented agree on outcome)
//! - Same-config detection variance
//! - Evasion score std dev per config

use crate::helpers::config_fingerprint;
use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::HashMap;

pub struct Stability;

impl EvalMetric for Stability {
    fn metric_id(&self) -> &str {
        "oracle.stability"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Flaky rate
        let flaky = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::Flaky)
            .count();
        let trustworthy_plus_flaky = dataset
            .rounds
            .iter()
            .filter(|r| {
                r.differential_category.is_trustworthy()
                    || r.differential_category == DifferentialCategory::Flaky
            })
            .count();

        let flaky_rate = if trustworthy_plus_flaky > 0 {
            flaky as f64 / trustworthy_plus_flaky as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "oracle.stability.flaky_rate".to_string(),
            axis: "oracle".to_string(),
            category: "stability".to_string(),
            label: "Flaky rate (flaky / trustworthy+flaky)".to_string(),
            value: flaky_rate,
            details: json!({
                "flaky": flaky,
                "trustworthy_plus_flaky": trustworthy_plus_flaky,
            }),
            n,
        });

        // 2. Behavior match rate
        let behavior_matched = dataset.rounds.iter().filter(|r| r.behavior_match).count();
        let behavior_match_rate = behavior_matched as f64 / n as f64;

        results.push(MetricResult {
            metric_id: "oracle.stability.behavior_match_rate".to_string(),
            axis: "oracle".to_string(),
            category: "stability".to_string(),
            label: "Behavior match rate (runs where baseline and instrumented agree)".to_string(),
            value: behavior_match_rate,
            details: json!({
                "matched": behavior_matched,
                "total": n,
            }),
            n,
        });

        // 3. Same-config detection variance
        let mut config_outcomes: HashMap<String, Vec<bool>> = HashMap::new();
        for round in &dataset.rounds {
            if !round.differential_category.is_trustworthy() {
                continue;
            }
            let fp = config_fingerprint(&round.modules, &round.mutations);
            config_outcomes.entry(fp).or_default().push(round.detected);
        }

        // Configs seen >1 time with inconsistent outcomes
        let multi_run_configs: Vec<(&String, &Vec<bool>)> = config_outcomes
            .iter()
            .filter(|(_, outcomes)| outcomes.len() > 1)
            .collect();

        let inconsistent = multi_run_configs
            .iter()
            .filter(|(_, outcomes)| {
                let first = outcomes[0];
                outcomes.iter().any(|o| *o != first)
            })
            .count();

        let config_consistency = if !multi_run_configs.is_empty() {
            1.0 - (inconsistent as f64 / multi_run_configs.len() as f64)
        } else {
            1.0 // No repeated configs → trivially consistent
        };

        results.push(MetricResult {
            metric_id: "oracle.stability.config_consistency".to_string(),
            axis: "oracle".to_string(),
            category: "stability".to_string(),
            label: "Config detection consistency (1 - inconsistent/multi-run configs)".to_string(),
            value: config_consistency,
            details: json!({
                "multi_run_configs": multi_run_configs.len(),
                "inconsistent": inconsistent,
            }),
            n,
        });

        // 4. Evasion score std dev per config
        let mut config_scores: HashMap<String, Vec<f64>> = HashMap::new();
        for round in &dataset.rounds {
            let fp = config_fingerprint(&round.modules, &round.mutations);
            config_scores
                .entry(fp)
                .or_default()
                .push(round.evasion_score);
        }

        let stddevs: Vec<f64> = config_scores
            .values()
            .filter(|scores| scores.len() > 1)
            .map(|scores| {
                let mean = scores.iter().sum::<f64>() / scores.len() as f64;
                let var =
                    scores.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / scores.len() as f64;
                var.sqrt()
            })
            .collect();

        let mean_stddev = if !stddevs.is_empty() {
            stddevs.iter().sum::<f64>() / stddevs.len() as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "oracle.stability.score_variance".to_string(),
            axis: "oracle".to_string(),
            category: "stability".to_string(),
            label: "Mean evasion score std dev per config (lower = more stable)".to_string(),
            value: mean_stddev,
            details: json!({
                "configs_with_repeats": stddevs.len(),
                "stddevs": stddevs,
            }),
            n,
        });

        Ok(results)
    }
}
