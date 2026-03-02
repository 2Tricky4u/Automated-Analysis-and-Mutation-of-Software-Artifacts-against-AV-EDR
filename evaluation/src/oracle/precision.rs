//! Oracle.Precision — measures false positive/negative proxy rates.
//!
//! Sub-metrics:
//! - FP proxy: InstrumentationArtifact rate (trace-caused false detections)
//! - FN proxy: Flaky rate (detections that don't reproduce)
//! - Trustworthy ratio: fraction of rounds that are RealDetection or Evasion
//! - Dryrun resolution rate: fraction of rounds with dryrun that resolved Ambiguous

use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;

pub struct Precision;

impl EvalMetric for Precision {
    fn metric_id(&self) -> &str {
        "oracle.precision"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // Count each category
        let mut counts = std::collections::HashMap::new();
        for round in &dataset.rounds {
            *counts
                .entry(format!("{:?}", round.differential_category))
                .or_insert(0usize) += 1;
        }

        let instr_artifact = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::InstrumentationArtifact)
            .count();
        let flaky = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category == DifferentialCategory::Flaky)
            .count();
        let trustworthy = dataset
            .rounds
            .iter()
            .filter(|r| r.differential_category.is_trustworthy())
            .count();

        // 1. FP proxy rate
        let fp_rate = instr_artifact as f64 / n as f64;
        results.push(MetricResult {
            metric_id: "oracle.precision.fp_proxy_rate".to_string(),
            axis: "oracle".to_string(),
            category: "precision".to_string(),
            label: "FP proxy rate (InstrumentationArtifact / total)".to_string(),
            value: fp_rate,
            details: json!({
                "instrumentation_artifact_count": instr_artifact,
                "total": n,
            }),
            n,
        });

        // 2. FN proxy rate
        let fn_rate = flaky as f64 / n as f64;
        results.push(MetricResult {
            metric_id: "oracle.precision.fn_proxy_rate".to_string(),
            axis: "oracle".to_string(),
            category: "precision".to_string(),
            label: "FN proxy rate (Flaky / total)".to_string(),
            value: fn_rate,
            details: json!({
                "flaky_count": flaky,
                "total": n,
            }),
            n,
        });

        // 3. Trustworthy ratio
        let trustworthy_ratio = trustworthy as f64 / n as f64;
        results.push(MetricResult {
            metric_id: "oracle.precision.trustworthy_ratio".to_string(),
            axis: "oracle".to_string(),
            category: "precision".to_string(),
            label: "Trustworthy ratio (RealDetection+Evasion+StaticDetection / total)".to_string(),
            value: trustworthy_ratio,
            details: json!({
                "trustworthy": trustworthy,
                "total": n,
                "category_counts": counts,
            }),
            n,
        });

        // 4. Dryrun resolution rate
        let has_dryrun = dataset.rounds.iter().filter(|r| r.has_dryrun).count();
        let dryrun_resolved = dataset
            .rounds
            .iter()
            .filter(|r| {
                r.has_dryrun
                    && matches!(
                        r.differential_category,
                        DifferentialCategory::MutationFailed | DifferentialCategory::PayloadFailed
                    )
            })
            .count();

        let dryrun_rate = if has_dryrun > 0 {
            dryrun_resolved as f64 / has_dryrun as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "oracle.precision.dryrun_resolution_rate".to_string(),
            axis: "oracle".to_string(),
            category: "precision".to_string(),
            label: "Dryrun resolution rate (resolved MutationFailed/PayloadFailed via dryrun)"
                .to_string(),
            value: dryrun_rate,
            details: json!({
                "has_dryrun": has_dryrun,
                "resolved": dryrun_resolved,
            }),
            n,
        });

        Ok(results)
    }
}
