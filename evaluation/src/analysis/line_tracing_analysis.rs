//! I14: Line Tracing Overhead Analysis
//!
//! Evaluates the performance and correctness of AST-level line trace injection.
//!
//! **Claim:** Line trace injection scales linearly with source size and
//! preserves parse validity.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct LineTracingAnalysis;

impl InfraMetric for LineTracingAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i14.line_tracing"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.line_tracing {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let mut metrics = Vec::new();

        // I14.1: Mean throughput (chars/µs)
        let mean_throughput =
            results.iter().map(|r| r.chars_per_us).sum::<f64>() / results.len() as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i14.line_tracing.throughput".to_string(),
            axis: "infrastructure".to_string(),
            category: "line_tracing".to_string(),
            label: "Mean source throughput (chars/µs)".to_string(),
            value: mean_throughput,
            details: json!({
                "per_source": results.iter().map(|r| {
                    json!({
                        "source": r.source_label,
                        "input_lines": r.input_lines,
                        "chars_per_us": r.chars_per_us,
                        "mean_time_us": r.mean_transform_time_us,
                        "stddev_us": r.stddev_transform_time_us,
                    })
                }).collect::<Vec<_>>(),
            }),
            n: results.len(),
        });

        // I14.2: Injection density (trace calls per source line)
        let densities: Vec<f64> = results
            .iter()
            .filter(|r| r.input_lines > 0)
            .map(|r| r.trace_calls_injected as f64 / r.input_lines as f64)
            .collect();

        let mean_density = if densities.is_empty() {
            0.0
        } else {
            densities.iter().sum::<f64>() / densities.len() as f64
        };

        metrics.push(MetricResult {
            metric_id: "infra.i14.line_tracing.injection_density".to_string(),
            axis: "infrastructure".to_string(),
            category: "line_tracing".to_string(),
            label: "Mean trace calls per source line".to_string(),
            value: mean_density,
            details: json!({
                "per_source": results.iter().map(|r| {
                    json!({
                        "source": r.source_label,
                        "input_lines": r.input_lines,
                        "trace_calls": r.trace_calls_injected,
                        "deferred_calls": r.deferred_trace_calls,
                        "density": if r.input_lines > 0 {
                            r.trace_calls_injected as f64 / r.input_lines as f64
                        } else { 0.0 },
                    })
                }).collect::<Vec<_>>(),
            }),
            n: densities.len(),
        });

        // I14.3: Scaling coefficient (µs/line) via linear regression
        let points: Vec<(f64, f64)> = results
            .iter()
            .map(|r| (r.input_lines as f64, r.mean_transform_time_us))
            .collect();

        let (slope, r_squared) = if points.len() >= 2 {
            linear_regression(&points)
        } else {
            (0.0, 0.0)
        };

        metrics.push(MetricResult {
            metric_id: "infra.i14.line_tracing.scaling".to_string(),
            axis: "infrastructure".to_string(),
            category: "line_tracing".to_string(),
            label: "Latency scaling coefficient (µs/line)".to_string(),
            value: slope,
            details: json!({
                "slope_us_per_line": slope,
                "r_squared": r_squared,
                "data_points": points.iter().map(|(x, y)| {
                    json!({"input_lines": x, "mean_time_us": y})
                }).collect::<Vec<_>>(),
            }),
            n: points.len(),
        });

        // I14.4: Validity — fraction of outputs that parse as valid C
        let valid_count = results.iter().filter(|r| r.output_valid).count();
        let validity = valid_count as f64 / results.len() as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i14.line_tracing.validity".to_string(),
            axis: "infrastructure".to_string(),
            category: "line_tracing".to_string(),
            label: "Fraction of outputs that parse as valid C".to_string(),
            value: validity,
            details: json!({
                "valid_count": valid_count,
                "total": results.len(),
                "invalid_sources": results.iter()
                    .filter(|r| !r.output_valid)
                    .map(|r| r.source_label.clone())
                    .collect::<Vec<_>>(),
            }),
            n: results.len(),
        });

        Ok(metrics)
    }
}

/// Simple linear regression returning (slope, r_squared).
fn linear_regression(points: &[(f64, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    let sum_x: f64 = points.iter().map(|(x, _)| x).sum();
    let sum_y: f64 = points.iter().map(|(_, y)| y).sum();
    let sum_xy: f64 = points.iter().map(|(x, y)| x * y).sum();
    let sum_xx: f64 = points.iter().map(|(x, _)| x * x).sum();

    let denom = n * sum_xx - sum_x * sum_x;
    if denom.abs() < f64::EPSILON {
        return (0.0, 0.0);
    }

    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;

    // R-squared
    let mean_y = sum_y / n;
    let ss_tot: f64 = points.iter().map(|(_, y)| (y - mean_y).powi(2)).sum();
    let ss_res: f64 = points
        .iter()
        .map(|(x, y)| (y - (slope * x + intercept)).powi(2))
        .sum();

    let r_squared = if ss_tot > f64::EPSILON {
        1.0 - ss_res / ss_tot
    } else {
        0.0
    };

    (slope, r_squared)
}
