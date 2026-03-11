//! B3: Telemetry Collection Completeness
//!
//! Analyzes coverage_percent, token counts per source, and their correlation
//! with detection verdict.
//!
//! **RQ:** Is telemetry sufficient for triage? Does coverage correlate with verdict?
//!
//! **Output:** Box plot data by verdict; coverage histogram; token count statistics

use crate::helpers::pearson_correlation;
use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::HashMap;

pub struct TelemetryCompleteness;

impl EvalMetric for TelemetryCompleteness {
    fn metric_id(&self) -> &str {
        "component.b3.telemetry_completeness"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Coverage distribution by differential category
        let mut coverage_by_category: HashMap<String, Vec<f64>> = HashMap::new();

        for round in &dataset.rounds {
            if let Some(cov) = round.coverage_percent {
                let cat = format!("{:?}", round.differential_category);
                coverage_by_category.entry(cat).or_default().push(cov);
            }
        }

        let mut box_plot_data: Vec<serde_json::Value> = Vec::new();
        for (cat, values) in &coverage_by_category {
            if values.is_empty() {
                continue;
            }
            let mut sorted = values.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n_vals = sorted.len();
            let mean = sorted.iter().sum::<f64>() / n_vals as f64;
            let median = sorted[n_vals / 2];
            let q1 = sorted[n_vals / 4];
            let q3 = sorted[3 * n_vals / 4];
            let min = sorted[0];
            let max = sorted[n_vals - 1];

            box_plot_data.push(json!({
                "category": cat,
                "n": n_vals,
                "mean": mean,
                "median": median,
                "q1": q1,
                "q3": q3,
                "min": min,
                "max": max,
                "values": sorted,
            }));
        }

        // Overall coverage statistics
        let all_coverage: Vec<f64> = dataset
            .rounds
            .iter()
            .filter_map(|r| r.coverage_percent)
            .collect();
        let overall_mean = if !all_coverage.is_empty() {
            all_coverage.iter().sum::<f64>() / all_coverage.len() as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "component.b3.telemetry_completeness.coverage_by_verdict".to_string(),
            axis: "component".to_string(),
            category: "execution_engine".to_string(),
            label: "Coverage distribution by differential category".to_string(),
            value: overall_mean,
            details: json!({
                "box_plot_data": box_plot_data,
                "overall_mean_coverage": overall_mean,
                "rounds_with_coverage": all_coverage.len(),
                "rounds_without_coverage": n - all_coverage.len(),
            }),
            n,
        });

        // 2. Coverage ↔ detection correlation
        let coverages: Vec<f64> = dataset
            .rounds
            .iter()
            .filter_map(|r| r.coverage_percent.map(|c| (c, r.detected)))
            .map(|(c, _)| c)
            .collect();
        let detected_flags: Vec<f64> = dataset
            .rounds
            .iter()
            .filter_map(|r| {
                r.coverage_percent
                    .map(|_| if r.detected { 1.0 } else { 0.0 })
            })
            .collect();

        let cov_detect_corr = if coverages.len() >= 4 {
            pearson_correlation(&coverages, &detected_flags)
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "component.b3.telemetry_completeness.coverage_detection_corr".to_string(),
            axis: "component".to_string(),
            category: "execution_engine".to_string(),
            label: "Coverage ↔ detection Pearson correlation".to_string(),
            value: cov_detect_corr,
            details: json!({
                "n_pairs": coverages.len(),
                "interpretation": if cov_detect_corr < -0.3 {
                    "Negative: lower coverage correlates with detection (killed before full execution)"
                } else if cov_detect_corr > 0.3 {
                    "Positive: higher coverage correlates with detection (unexpected)"
                } else {
                    "Weak: coverage does not strongly predict detection"
                },
            }),
            n,
        });

        // 3. Coverage histogram (binned)
        let bins = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
        let mut histogram: Vec<usize> = vec![0; bins.len() - 1];
        for &cov in &all_coverage {
            for i in 0..bins.len() - 1 {
                if cov >= bins[i] && cov < bins[i + 1] {
                    histogram[i] += 1;
                    break;
                }
                // Last bin includes 1.0
                if i == bins.len() - 2 && cov >= bins[i] {
                    histogram[i] += 1;
                }
            }
        }

        results.push(MetricResult {
            metric_id: "component.b3.telemetry_completeness.coverage_histogram".to_string(),
            axis: "component".to_string(),
            category: "execution_engine".to_string(),
            label: "Coverage percent histogram".to_string(),
            value: all_coverage.len() as f64,
            details: json!({
                "bin_edges": bins,
                "counts": histogram,
                "all_values": all_coverage,
            }),
            n,
        });

        // 4. Token counts per round from token_matrices
        if !dataset.token_matrices.is_empty() {
            let mut token_counts_by_verdict: HashMap<String, Vec<usize>> = HashMap::new();

            for (entry, round) in dataset.token_matrices.iter().zip(dataset.rounds.iter()) {
                let cat = format!("{:?}", round.differential_category);
                token_counts_by_verdict
                    .entry(cat)
                    .or_default()
                    .push(entry.tokens.len());
            }

            let mut token_count_summary: Vec<serde_json::Value> = Vec::new();
            for (cat, counts) in &token_counts_by_verdict {
                let mean = counts.iter().sum::<usize>() as f64 / counts.len().max(1) as f64;
                let min = counts.iter().copied().min().unwrap_or(0);
                let max = counts.iter().copied().max().unwrap_or(0);

                token_count_summary.push(json!({
                    "category": cat,
                    "n": counts.len(),
                    "mean_tokens": mean,
                    "min_tokens": min,
                    "max_tokens": max,
                }));
            }

            results.push(MetricResult {
                metric_id: "component.b3.telemetry_completeness.token_counts".to_string(),
                axis: "component".to_string(),
                category: "execution_engine".to_string(),
                label: "Token counts by differential category".to_string(),
                value: dataset.token_matrices.len() as f64,
                details: json!({
                    "per_category": token_count_summary,
                }),
                n,
            });
        }

        // 5. Trustworthy fraction over time (sliding window)
        let trustworthy_counts: Vec<f64> = dataset
            .rounds
            .iter()
            .map(|r| {
                if r.differential_category.is_trustworthy() {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();

        let window = 5.min(n);
        if n >= window {
            let windowed: Vec<f64> = trustworthy_counts
                .windows(window)
                .map(|w| w.iter().sum::<f64>() / w.len() as f64)
                .collect();

            let overall_trustworthy = trustworthy_counts.iter().sum::<f64>() / n as f64;

            // Contamination proxy: non-trustworthy fraction
            let contamination_rate = 1.0 - overall_trustworthy;

            results.push(MetricResult {
                metric_id: "component.b3.telemetry_completeness.trustworthy_trajectory"
                    .to_string(),
                axis: "component".to_string(),
                category: "execution_engine".to_string(),
                label: "Trustworthy round fraction (1 - contamination rate)".to_string(),
                value: overall_trustworthy,
                details: json!({
                    "overall_trustworthy_rate": overall_trustworthy,
                    "contamination_rate": contamination_rate,
                    "windowed_trustworthy": windowed,
                    "window_size": window,
                    "category_counts": {
                        "Evasion": dataset.rounds.iter()
                            .filter(|r| r.differential_category == DifferentialCategory::Evasion).count(),
                        "RealDetection": dataset.rounds.iter()
                            .filter(|r| r.differential_category == DifferentialCategory::RealDetection).count(),
                        "InstrumentationArtifact": dataset.rounds.iter()
                            .filter(|r| r.differential_category == DifferentialCategory::InstrumentationArtifact).count(),
                        "Flaky": dataset.rounds.iter()
                            .filter(|r| r.differential_category == DifferentialCategory::Flaky).count(),
                        "MutationFailed": dataset.rounds.iter()
                            .filter(|r| r.differential_category == DifferentialCategory::MutationFailed).count(),
                    },
                }),
                n,
            });
        }

        Ok(results)
    }
}
