//! I15: Shellcode Checkpoint Patching Analysis
//!
//! Evaluates the performance and correctness of INT3-based shellcode checkpoint
//! insertion across multiple shellcode sizes and checkpoint counts.
//!
//! **Claim:** Checkpoint patching time scales with shellcode size (disassembly
//! dominates), not checkpoint count.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct ScCheckpointAnalysis;

impl InfraMetric for ScCheckpointAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i15.sc_checkpoint"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.sc_checkpoint {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let mut metrics = Vec::new();

        // I15.1: Mean patch throughput (bytes/µs)
        let throughputs: Vec<f64> = results
            .iter()
            .filter(|r| r.bytes_per_us > 0.0)
            .map(|r| r.bytes_per_us)
            .collect();

        let mean_throughput = if throughputs.is_empty() {
            0.0
        } else {
            throughputs.iter().sum::<f64>() / throughputs.len() as f64
        };

        metrics.push(MetricResult {
            metric_id: "infra.i15.sc_checkpoint.throughput_by_size".to_string(),
            axis: "infrastructure".to_string(),
            category: "sc_checkpoint".to_string(),
            label: "Mean patch throughput (bytes/µs)".to_string(),
            value: mean_throughput,
            details: json!({
                "per_file": results.iter().map(|r| {
                    json!({
                        "shellcode": r.shellcode_name,
                        "size": r.shellcode_size,
                        "checkpoints": r.requested_checkpoints,
                        "bytes_per_us": r.bytes_per_us,
                        "mean_patch_us": r.mean_patch_time_us,
                    })
                }).collect::<Vec<_>>(),
            }),
            n: throughputs.len(),
        });

        // I15.2: Scaling by size — linear fit of (shellcode_size, mean_patch_time_us)
        // for a fixed checkpoint count (5)
        let size_points: Vec<(f64, f64)> = results
            .iter()
            .filter(|r| r.requested_checkpoints == 5)
            .map(|r| (r.shellcode_size as f64, r.mean_patch_time_us))
            .collect();

        let (size_slope, size_r2) = if size_points.len() >= 2 {
            linear_regression(&size_points)
        } else {
            (0.0, 0.0)
        };

        metrics.push(MetricResult {
            metric_id: "infra.i15.sc_checkpoint.scaling_by_size".to_string(),
            axis: "infrastructure".to_string(),
            category: "sc_checkpoint".to_string(),
            label: "Patch time scaling with shellcode size (µs/byte, checkpoint_count=5)"
                .to_string(),
            value: size_slope,
            details: json!({
                "slope_us_per_byte": size_slope,
                "r_squared": size_r2,
                "data_points": size_points.iter().map(|(x, y)| {
                    json!({"shellcode_size": x, "mean_patch_us": y})
                }).collect::<Vec<_>>(),
            }),
            n: size_points.len(),
        });

        // I15.3: Scaling by checkpoint count — for the median-size shellcode
        // Find the shellcode name that appears most centrally by size
        let mut unique_files: Vec<(&str, usize)> = Vec::new();
        for r in results {
            if !unique_files
                .iter()
                .any(|(n, _)| *n == r.shellcode_name.as_str())
            {
                unique_files.push((&r.shellcode_name, r.shellcode_size));
            }
        }
        unique_files.sort_by_key(|(_, s)| *s);

        let mid_file = unique_files
            .get(unique_files.len() / 2)
            .map(|(n, _)| *n)
            .unwrap_or("");

        let count_points: Vec<(f64, f64)> = results
            .iter()
            .filter(|r| r.shellcode_name == mid_file)
            .map(|r| (r.requested_checkpoints as f64, r.mean_patch_time_us))
            .collect();

        let (count_slope, count_r2) = if count_points.len() >= 2 {
            linear_regression(&count_points)
        } else {
            (0.0, 0.0)
        };

        metrics.push(MetricResult {
            metric_id: "infra.i15.sc_checkpoint.scaling_by_checkpoints".to_string(),
            axis: "infrastructure".to_string(),
            category: "sc_checkpoint".to_string(),
            label: format!(
                "Patch time scaling with checkpoint count (µs/checkpoint, file={})",
                mid_file
            ),
            value: count_slope,
            details: json!({
                "reference_file": mid_file,
                "slope_us_per_checkpoint": count_slope,
                "r_squared": count_r2,
                "data_points": count_points.iter().map(|(x, y)| {
                    json!({"checkpoint_count": x, "mean_patch_us": y})
                }).collect::<Vec<_>>(),
            }),
            n: count_points.len(),
        });

        // I15.4: Clamping rate — fraction of benchmarks where actual < requested
        let clamped = results
            .iter()
            .filter(|r| (r.actual_checkpoints as u32) < r.requested_checkpoints)
            .count();
        let clamping_rate = clamped as f64 / results.len() as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i15.sc_checkpoint.clamping_rate".to_string(),
            axis: "infrastructure".to_string(),
            category: "sc_checkpoint".to_string(),
            label: "Fraction of cases where actual checkpoints < requested".to_string(),
            value: clamping_rate,
            details: json!({
                "clamped_count": clamped,
                "total": results.len(),
                "clamped_cases": results.iter()
                    .filter(|r| (r.actual_checkpoints as u32) < r.requested_checkpoints)
                    .map(|r| json!({
                        "shellcode": r.shellcode_name,
                        "size": r.shellcode_size,
                        "requested": r.requested_checkpoints,
                        "actual": r.actual_checkpoints,
                        "reachable_boundaries": r.reachable_boundaries,
                    }))
                    .collect::<Vec<_>>(),
            }),
            n: results.len(),
        });

        // I15.5: Boundary correctness
        let correct_count = results.iter().filter(|r| r.boundary_correct).count();
        let correctness = correct_count as f64 / results.len() as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i15.sc_checkpoint.boundary_correctness".to_string(),
            axis: "infrastructure".to_string(),
            category: "sc_checkpoint".to_string(),
            label: "Fraction of benchmarks with correct instruction boundaries".to_string(),
            value: correctness,
            details: json!({
                "correct_count": correct_count,
                "total": results.len(),
                "failures": results.iter()
                    .filter(|r| !r.boundary_correct)
                    .map(|r| json!({
                        "shellcode": r.shellcode_name,
                        "checkpoints": r.requested_checkpoints,
                    }))
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
