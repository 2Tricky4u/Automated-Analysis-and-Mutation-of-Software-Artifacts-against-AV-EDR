//! Oracle.Soundness — measures detection scope and blind spots.
//!
//! Sub-metrics:
//! - Static vs dynamic detection ratio
//! - Verdict distribution
//! - Always-detected configs (blind spots — configs that always get caught)
//! - Evasion-enabling module combos (configs that achieve evasion)

use crate::helpers::config_fingerprint;
use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::HashMap;

pub struct Soundness;

impl EvalMetric for Soundness {
    fn metric_id(&self) -> &str {
        "oracle.soundness"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Static vs dynamic detection ratio
        let static_count = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::StaticDetection)
            .count();
        let dynamic_count = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::RealDetection)
            .count();

        let static_ratio = if static_count + dynamic_count > 0 {
            static_count as f64 / (static_count + dynamic_count) as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "oracle.soundness.static_ratio".to_string(),
            axis: "oracle".to_string(),
            category: "soundness".to_string(),
            label: "Static detection ratio (static / all detections)".to_string(),
            value: static_ratio,
            details: json!({
                "static": static_count,
                "dynamic": dynamic_count,
            }),
            n,
        });

        // 2. Verdict distribution
        let mut verdict_dist: HashMap<String, usize> = HashMap::new();
        for round in &dataset.rounds {
            *verdict_dist
                .entry(round.detection_verdict.clone())
                .or_default() += 1;
        }

        let evasion_count = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::Evasion)
            .count();
        let evasion_rate = evasion_count as f64 / n as f64;

        results.push(MetricResult {
            metric_id: "oracle.soundness.evasion_rate".to_string(),
            axis: "oracle".to_string(),
            category: "soundness".to_string(),
            label: "Overall evasion rate".to_string(),
            value: evasion_rate,
            details: json!({
                "verdict_distribution": verdict_dist,
                "evasion_count": evasion_count,
                "total": n,
            }),
            n,
        });

        // 3. Always-detected configs (blind spots)
        let mut config_outcomes: HashMap<String, (usize, usize)> = HashMap::new(); // (detected, total)
        for round in &dataset.rounds {
            // Only count trustworthy rounds
            if !round.differential_category.is_trustworthy() {
                continue;
            }
            let fp = config_fingerprint(&round.modules, &round.mutations);
            let entry = config_outcomes.entry(fp).or_insert((0, 0));
            entry.1 += 1;
            if round.differential_category.is_detected() {
                entry.0 += 1;
            }
        }

        let always_detected: Vec<(&String, &(usize, usize))> = config_outcomes
            .iter()
            .filter(|(_, (det, total))| *det == *total && *total >= 2)
            .collect();

        let blind_spot_ratio = if !config_outcomes.is_empty() {
            always_detected.len() as f64 / config_outcomes.len() as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "oracle.soundness.blind_spot_ratio".to_string(),
            axis: "oracle".to_string(),
            category: "soundness".to_string(),
            label: "Blind spot ratio (always-detected configs / total configs, min 2 runs)"
                .to_string(),
            value: blind_spot_ratio,
            details: json!({
                "always_detected_count": always_detected.len(),
                "total_configs": config_outcomes.len(),
            }),
            n,
        });

        // 4. Evasion-enabling module combos
        let evasion_configs: Vec<&String> = config_outcomes
            .iter()
            .filter(|(_, (det, total))| *det == 0 && *total >= 1)
            .map(|(fp, _)| fp)
            .collect();

        let evasion_config_ratio = if !config_outcomes.is_empty() {
            evasion_configs.len() as f64 / config_outcomes.len() as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "oracle.soundness.evasion_config_ratio".to_string(),
            axis: "oracle".to_string(),
            category: "soundness".to_string(),
            label: "Evasion config ratio (never-detected configs / total)".to_string(),
            value: evasion_config_ratio,
            details: json!({
                "evasion_configs": evasion_configs.len(),
                "total_configs": config_outcomes.len(),
            }),
            n,
        });

        Ok(results)
    }
}
