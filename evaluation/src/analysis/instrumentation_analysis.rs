//! I6: Instrumentation Overhead Analysis
//!
//! Evaluates weak-symbol linkage overhead for baseline vs instrumented builds.
//!
//! **Claim:** Weak-symbol linkage produces zero overhead for baseline;
//! instrumented has bounded overhead.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct InstrumentationAnalysis;

impl InfraMetric for InstrumentationAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i6.instrumentation"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.instrumentation {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I6.1: Size overhead ratio
        let ratios: Vec<f64> = results.iter().map(|r| r.size_ratio).collect();
        let mean_ratio = ratios.iter().sum::<f64>() / ratios.len() as f64;

        let per_carrier: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "carrier": r.carrier,
                    "baseline_size": r.baseline_size,
                    "instrumented_size": r.instrumented_size,
                    "size_ratio": r.size_ratio,
                    "overhead_bytes": r.instrumented_size as i64 - r.baseline_size as i64,
                    "overhead_percent": (r.size_ratio - 1.0) * 100.0,
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i6.instrumentation.size_overhead".to_string(),
            axis: "infrastructure".to_string(),
            category: "instrumentation".to_string(),
            label: "Mean instrumented/baseline size ratio".to_string(),
            value: mean_ratio,
            details: json!({
                "mean_ratio": mean_ratio,
                "per_carrier": per_carrier,
            }),
            n,
        });

        // I6.2: .text section overhead (approximated by total size ratio)
        let max_ratio = ratios.iter().cloned().fold(f64::MIN, f64::max);
        let min_ratio = ratios.iter().cloned().fold(f64::MAX, f64::min);

        metrics.push(MetricResult {
            metric_id: "infra.i6.instrumentation.text_overhead".to_string(),
            axis: "infrastructure".to_string(),
            category: "instrumentation".to_string(),
            label: "Size ratio range across carriers".to_string(),
            value: max_ratio - min_ratio,
            details: json!({
                "min_ratio": min_ratio,
                "max_ratio": max_ratio,
                "range": max_ratio - min_ratio,
            }),
            n,
        });

        // I6.3: Build time overhead
        let time_ratios: Vec<f64> = results
            .iter()
            .map(|r| r.build_time_instrumented_ms / r.build_time_baseline_ms.max(0.001))
            .collect();
        let mean_time_ratio = time_ratios.iter().sum::<f64>() / time_ratios.len() as f64;

        let build_times: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "carrier": r.carrier,
                    "baseline_ms": r.build_time_baseline_ms,
                    "instrumented_ms": r.build_time_instrumented_ms,
                    "time_ratio": r.build_time_instrumented_ms / r.build_time_baseline_ms.max(0.001),
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i6.instrumentation.build_time_overhead".to_string(),
            axis: "infrastructure".to_string(),
            category: "instrumentation".to_string(),
            label: "Mean instrumented/baseline build time ratio".to_string(),
            value: mean_time_ratio,
            details: json!({
                "mean_time_ratio": mean_time_ratio,
                "per_carrier": build_times,
            }),
            n,
        });

        Ok(metrics)
    }
}
