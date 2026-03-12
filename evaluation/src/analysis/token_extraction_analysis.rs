//! I7: Token Extraction Analysis
//!
//! Evaluates the token extractor for category coverage, yield, timing, and determinism.
//!
//! **Claim:** Extractor produces deterministic tokens; covers 9+ categories; bounded latency.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;
use std::collections::HashMap;

pub struct TokenExtractionAnalysis;

impl InfraMetric for TokenExtractionAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i7.token_extraction"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.token_extraction {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I7.1: Category coverage
        let total_categories = 9usize; // module, mutation, api, api_arg, seq2, image, etw, etw_event, checkpoint
        let mut all_categories: HashMap<String, usize> = HashMap::new();
        let mut max_active = 0usize;

        for r in results {
            max_active = max_active.max(r.categories_active);
            for (cat, &count) in &r.category_counts {
                *all_categories.entry(cat.clone()).or_default() += count;
            }
        }

        let active_categories = all_categories.len();
        let coverage = active_categories as f64 / total_categories as f64;

        let category_table: Vec<serde_json::Value> = {
            let mut entries: Vec<_> = all_categories.iter().collect();
            entries.sort_by(|a, b| b.1.cmp(a.1));
            entries
                .iter()
                .map(|(cat, count)| {
                    json!({
                        "category": cat,
                        "total_tokens": count,
                    })
                })
                .collect()
        };

        metrics.push(MetricResult {
            metric_id: "infra.i7.token_extraction.category_coverage".to_string(),
            axis: "infrastructure".to_string(),
            category: "token_extraction".to_string(),
            label: "Active token categories / total (9)".to_string(),
            value: coverage,
            details: json!({
                "active_categories": active_categories,
                "total_categories": total_categories,
                "max_active_single_run": max_active,
                "category_table": category_table,
            }),
            n,
        });

        // I7.2: Tokens per doc
        let yields: Vec<f64> = results
            .iter()
            .map(|r| {
                if r.input_doc_count > 0 {
                    r.output_token_count as f64 / r.input_doc_count as f64
                } else {
                    0.0
                }
            })
            .collect();
        let mean_yield = yields.iter().sum::<f64>() / yields.len() as f64;

        let yield_by_run: Vec<serde_json::Value> = results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                json!({
                    "run": i,
                    "input_docs": r.input_doc_count,
                    "output_tokens": r.output_token_count,
                    "tokens_per_doc": yields[i],
                    "category_counts": r.category_counts,
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i7.token_extraction.tokens_per_doc".to_string(),
            axis: "infrastructure".to_string(),
            category: "token_extraction".to_string(),
            label: "Mean tokens per input document".to_string(),
            value: mean_yield,
            details: json!({
                "mean_yield": mean_yield,
                "by_run": yield_by_run,
            }),
            n,
        });

        // I7.3: Latency distribution
        let times: Vec<f64> = results.iter().map(|r| r.extraction_time_us).collect();
        let mean_time = times.iter().sum::<f64>() / times.len() as f64;
        let max_time = times.iter().cloned().fold(f64::MIN, f64::max);

        // Percentiles
        let mut sorted_times = times.clone();
        sorted_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = percentile(&sorted_times, 0.50);
        let p95 = percentile(&sorted_times, 0.95);
        let p99 = percentile(&sorted_times, 0.99);

        metrics.push(MetricResult {
            metric_id: "infra.i7.token_extraction.latency_distribution".to_string(),
            axis: "infrastructure".to_string(),
            category: "token_extraction".to_string(),
            label: "Extraction latency distribution (µs)".to_string(),
            value: mean_time,
            details: json!({
                "mean_us": mean_time,
                "max_us": max_time,
                "p50_us": p50,
                "p95_us": p95,
                "p99_us": p99,
                "all_times_us": times,
            }),
            n,
        });

        // I7.4: Determinism
        let deterministic_count = results.iter().filter(|r| r.deterministic).count();
        let determinism = deterministic_count as f64 / n as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i7.token_extraction.determinism".to_string(),
            axis: "infrastructure".to_string(),
            category: "token_extraction".to_string(),
            label: "Fraction of runs producing deterministic output".to_string(),
            value: determinism,
            details: json!({
                "deterministic": deterministic_count,
                "total": n,
            }),
            n,
        });

        Ok(metrics)
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (p * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
