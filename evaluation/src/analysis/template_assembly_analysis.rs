//! I5: Template Assembly Analysis
//!
//! Evaluates the 7-slot module system for marker resolution across combinations.
//!
//! **Claim:** 7-slot module system resolves all markers across 576 combinations.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct TemplateAssemblyAnalysis;

impl InfraMetric for TemplateAssemblyAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i5.template_assembly"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.template_assembly {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I5.1: Marker resolution rate
        let resolved_count = results.iter().filter(|r| r.markers_resolved).count();
        let resolution_rate = resolved_count as f64 / n as f64;

        let failed: Vec<serde_json::Value> = results
            .iter()
            .filter(|r| !r.markers_resolved)
            .map(|r| json!({ "modules": r.modules }))
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i5.template_assembly.marker_resolution".to_string(),
            axis: "infrastructure".to_string(),
            category: "template_assembly".to_string(),
            label: "Fraction of combinations with all markers resolved".to_string(),
            value: resolution_rate,
            details: json!({
                "resolved": resolved_count,
                "total": n,
                "failed_combinations": failed,
            }),
            n,
        });

        // I5.2: Combination coverage
        // 576 = product of module variant counts
        let total_combinations = 576usize;
        let coverage = n as f64 / total_combinations as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i5.template_assembly.combination_coverage".to_string(),
            axis: "infrastructure".to_string(),
            category: "template_assembly".to_string(),
            label: "Tested combinations / total possible (576)".to_string(),
            value: coverage.min(1.0),
            details: json!({
                "tested": n,
                "total_possible": total_combinations,
                "coverage_percent": coverage * 100.0,
            }),
            n,
        });

        // I5.3: Assembly latency
        let times: Vec<f64> = results.iter().map(|r| r.assembly_time_us).collect();
        let mean_time = times.iter().sum::<f64>() / times.len() as f64;
        let max_time = times.iter().cloned().fold(f64::MIN, f64::max);
        let min_time = times.iter().cloned().fold(f64::MAX, f64::min);

        // Build histogram bins
        let bin_count = 20usize;
        let bin_width = if max_time > min_time {
            (max_time - min_time) / bin_count as f64
        } else {
            1.0
        };
        let mut histogram = vec![0usize; bin_count];
        for &t in &times {
            let bin = ((t - min_time) / bin_width).floor() as usize;
            let bin = bin.min(bin_count - 1);
            histogram[bin] += 1;
        }

        let bin_edges: Vec<f64> = (0..=bin_count)
            .map(|i| min_time + i as f64 * bin_width)
            .collect();

        // Output line distribution
        let line_counts: Vec<usize> = results.iter().map(|r| r.output_lines).collect();
        let mean_lines = line_counts.iter().sum::<usize>() as f64 / line_counts.len() as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i5.template_assembly.latency".to_string(),
            axis: "infrastructure".to_string(),
            category: "template_assembly".to_string(),
            label: "Assembly time distribution".to_string(),
            value: mean_time,
            details: json!({
                "mean_us": mean_time,
                "min_us": min_time,
                "max_us": max_time,
                "histogram": histogram,
                "bin_edges": bin_edges,
                "mean_output_lines": mean_lines,
                "all_times_us": times,
            }),
            n,
        });

        Ok(metrics)
    }
}
