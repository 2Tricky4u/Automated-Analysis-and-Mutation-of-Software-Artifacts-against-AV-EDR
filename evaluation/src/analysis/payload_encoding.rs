//! I1: Payload Encoding Analysis
//!
//! Evaluates 4 encoding types across payload sizes for entropy profiles,
//! roundtrip correctness, size expansion, and latency.
//!
//! **Claim:** 4 encodings produce distinct entropy profiles; roundtrip is lossless.

use crate::{InfraEvalDataset, InfraMetric, MetricResult, PayloadEncodingResult};
use serde_json::json;
use std::collections::HashMap;

pub struct PayloadEncoding;

impl InfraMetric for PayloadEncoding {
    fn metric_id(&self) -> &str {
        "infra.i1.payload_encoding"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.payload_encoding {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // Group by encoding type
        let mut by_type: HashMap<&str, Vec<&PayloadEncodingResult>> = HashMap::new();
        for r in results {
            by_type.entry(r.encoding_type.as_str()).or_default().push(r);
        }

        // I1.1: Entropy comparison — max entropy range across encoding types
        let mut type_entropies: Vec<serde_json::Value> = Vec::new();
        let mut min_mean_entropy = f64::MAX;
        let mut max_mean_entropy = f64::MIN;

        for (enc_type, entries) in &by_type {
            let entropies: Vec<f64> = entries.iter().map(|e| e.encoded_entropy).collect();
            let mean = entropies.iter().sum::<f64>() / entropies.len() as f64;
            min_mean_entropy = min_mean_entropy.min(mean);
            max_mean_entropy = max_mean_entropy.max(mean);

            let by_size: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    json!({
                        "payload_size": e.payload_size,
                        "entropy": e.encoded_entropy,
                    })
                })
                .collect();

            type_entropies.push(json!({
                "encoding_type": enc_type,
                "mean_entropy": mean,
                "min_entropy": entropies.iter().cloned().fold(f64::MAX, f64::min),
                "max_entropy": entropies.iter().cloned().fold(f64::MIN, f64::max),
                "by_size": by_size,
            }));
        }

        let entropy_range = if min_mean_entropy <= max_mean_entropy {
            max_mean_entropy - min_mean_entropy
        } else {
            0.0
        };

        metrics.push(MetricResult {
            metric_id: "infra.i1.payload_encoding.entropy_comparison".to_string(),
            axis: "infrastructure".to_string(),
            category: "payload_encoding".to_string(),
            label: "Encoding entropy comparison (max range across types)".to_string(),
            value: entropy_range,
            details: json!({
                "type_entropies": type_entropies,
                "entropy_range": entropy_range,
                "encoding_types": by_type.keys().collect::<Vec<_>>(),
            }),
            n,
        });

        // I1.2: Roundtrip correctness
        let correct_count = results.iter().filter(|r| r.roundtrip_correct).count();
        let correctness = correct_count as f64 / n as f64;

        let incorrect: Vec<serde_json::Value> = results
            .iter()
            .filter(|r| !r.roundtrip_correct)
            .map(|r| {
                json!({
                    "encoding_type": r.encoding_type,
                    "payload_size": r.payload_size,
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i1.payload_encoding.roundtrip_correctness".to_string(),
            axis: "infrastructure".to_string(),
            category: "payload_encoding".to_string(),
            label: "Roundtrip correctness (fraction correct)".to_string(),
            value: correctness,
            details: json!({
                "correct": correct_count,
                "total": n,
                "incorrect_cases": incorrect,
            }),
            n,
        });

        // I1.3: Size expansion ratio
        let mut expansion_by_type: Vec<serde_json::Value> = Vec::new();
        let mut mean_expansion = 0.0;
        let mut count = 0usize;

        for (enc_type, entries) in &by_type {
            let ratios: Vec<f64> = entries
                .iter()
                .map(|e| e.encoded_size as f64 / e.payload_size.max(1) as f64)
                .collect();
            let mean_ratio = ratios.iter().sum::<f64>() / ratios.len() as f64;
            mean_expansion += mean_ratio;
            count += 1;

            expansion_by_type.push(json!({
                "encoding_type": enc_type,
                "mean_expansion": mean_ratio,
                "max_expansion": ratios.iter().cloned().fold(f64::MIN, f64::max),
            }));
        }

        let overall_expansion = if count > 0 {
            mean_expansion / count as f64
        } else {
            1.0
        };

        metrics.push(MetricResult {
            metric_id: "infra.i1.payload_encoding.size_expansion".to_string(),
            axis: "infrastructure".to_string(),
            category: "payload_encoding".to_string(),
            label: "Size expansion ratio (encoded/original)".to_string(),
            value: overall_expansion,
            details: json!({
                "by_type": expansion_by_type,
                "overall_mean": overall_expansion,
            }),
            n,
        });

        // I1.4: Encoding latency
        let times: Vec<f64> = results.iter().map(|r| r.encode_time_us).collect();
        let mean_time = times.iter().sum::<f64>() / times.len() as f64;

        let mut latency_by_type: Vec<serde_json::Value> = Vec::new();
        for (enc_type, entries) in &by_type {
            let type_times: Vec<f64> = entries.iter().map(|e| e.encode_time_us).collect();
            let type_mean = type_times.iter().sum::<f64>() / type_times.len() as f64;
            latency_by_type.push(json!({
                "encoding_type": enc_type,
                "mean_us": type_mean,
                "max_us": type_times.iter().cloned().fold(f64::MIN, f64::max),
            }));
        }

        metrics.push(MetricResult {
            metric_id: "infra.i1.payload_encoding.latency".to_string(),
            axis: "infrastructure".to_string(),
            category: "payload_encoding".to_string(),
            label: "Mean encode latency (µs)".to_string(),
            value: mean_time,
            details: json!({
                "mean_us": mean_time,
                "by_type": latency_by_type,
                "all_times_us": times,
            }),
            n,
        });

        Ok(metrics)
    }
}
