//! I2: AST Mutation Analysis
//!
//! Evaluates each AST mutation for source transformation impact and validity.
//!
//! **Claim:** Each AST mutation produces measurable source transformation;
//! output remains valid C.

use crate::{InfraEvalDataset, InfraMetric, MetricResult};
use serde_json::json;

pub struct AstMutationAnalysis;

impl InfraMetric for AstMutationAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i2.ast_mutation"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.ast_mutation {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I2.1: Line impact per mutation
        let mut mutation_impacts: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "mutation_id": r.mutation_id,
                    "input_lines": r.input_lines,
                    "output_lines": r.output_lines,
                    "line_delta": r.line_delta,
                    "input_ast_nodes": r.input_ast_nodes,
                    "output_ast_nodes": r.output_ast_nodes,
                    "node_delta": r.output_ast_nodes as i64 - r.input_ast_nodes as i64,
                })
            })
            .collect();

        mutation_impacts.sort_by(|a, b| {
            let da = a["line_delta"].as_i64().unwrap_or(0).abs();
            let db = b["line_delta"].as_i64().unwrap_or(0).abs();
            db.cmp(&da)
        });

        let max_delta = results
            .iter()
            .map(|r| r.line_delta.unsigned_abs())
            .max()
            .unwrap_or(0);
        let mean_delta = results.iter().map(|r| r.line_delta.abs()).sum::<i64>() as f64 / n as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i2.ast_mutation.line_impact".to_string(),
            axis: "infrastructure".to_string(),
            category: "ast_mutation".to_string(),
            label: "Per-mutation line delta table".to_string(),
            value: mean_delta,
            details: json!({
                "mutations": mutation_impacts,
                "max_abs_delta": max_delta,
                "mean_abs_delta": mean_delta,
            }),
            n,
        });

        // I2.2: Parse validity
        let valid_count = results.iter().filter(|r| r.parse_valid).count();
        let validity = valid_count as f64 / n as f64;

        let invalid: Vec<&str> = results
            .iter()
            .filter(|r| !r.parse_valid)
            .map(|r| r.mutation_id.as_str())
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i2.ast_mutation.parse_validity".to_string(),
            axis: "infrastructure".to_string(),
            category: "ast_mutation".to_string(),
            label: "Fraction of mutations producing valid C".to_string(),
            value: validity,
            details: json!({
                "valid": valid_count,
                "total": n,
                "invalid_mutations": invalid,
            }),
            n,
        });

        // I2.3: Transform latency
        let times: Vec<f64> = results.iter().map(|r| r.transform_time_us).collect();
        let mean_time = times.iter().sum::<f64>() / times.len() as f64;

        let latency_by_mutation: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "mutation_id": r.mutation_id,
                    "time_us": r.transform_time_us,
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i2.ast_mutation.transform_latency".to_string(),
            axis: "infrastructure".to_string(),
            category: "ast_mutation".to_string(),
            label: "Mean AST transform latency (µs)".to_string(),
            value: mean_time,
            details: json!({
                "mean_us": mean_time,
                "by_mutation": latency_by_mutation,
            }),
            n,
        });

        Ok(metrics)
    }
}
