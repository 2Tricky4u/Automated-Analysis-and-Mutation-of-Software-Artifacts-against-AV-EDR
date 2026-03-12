//! I3: IR Mutation Analysis
//!
//! Evaluates 3 IR mutations for insertion effectiveness, O2 survival, and determinism.
//!
//! **Claim:** 3 IR mutations alter structure; robust opaques survive -O2;
//! output is deterministic from same seed.

use crate::{InfraEvalDataset, InfraMetric, IrMutationResult, MetricResult};
use serde_json::json;

pub struct IrMutationAnalysis;

impl InfraMetric for IrMutationAnalysis {
    fn metric_id(&self) -> &str {
        "infra.i3.ir_mutation"
    }

    fn evaluate(&self, dataset: &InfraEvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let results = match &dataset.ir_mutation {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(vec![]),
        };

        let n = results.len();
        let mut metrics = Vec::new();

        // I3.1: Insertion effectiveness (insertions per input line)
        let mut by_mutation: Vec<serde_json::Value> = Vec::new();
        let mut total_insertions = 0usize;
        let mut total_input_lines = 0usize;

        for r in results {
            let effectiveness = if r.input_lines > 0 {
                r.insertions as f64 / r.input_lines as f64
            } else {
                0.0
            };
            total_insertions += r.insertions;
            total_input_lines += r.input_lines;

            by_mutation.push(json!({
                "mutation_id": r.mutation_id,
                "density": r.density,
                "input_lines": r.input_lines,
                "output_lines": r.output_lines,
                "insertions": r.insertions,
                "effectiveness": effectiveness,
                "line_bloat": r.output_lines as f64 / r.input_lines.max(1) as f64,
            }));
        }

        let overall_effectiveness = if total_input_lines > 0 {
            total_insertions as f64 / total_input_lines as f64
        } else {
            0.0
        };

        metrics.push(MetricResult {
            metric_id: "infra.i3.ir_mutation.insertion_effectiveness".to_string(),
            axis: "infrastructure".to_string(),
            category: "ir_mutation".to_string(),
            label: "Insertions per input line (by mutation)".to_string(),
            value: overall_effectiveness,
            details: json!({
                "by_mutation": by_mutation,
                "total_insertions": total_insertions,
                "total_input_lines": total_input_lines,
            }),
            n,
        });

        // I3.2: O2 survival
        let tested: Vec<&IrMutationResult> =
            results.iter().filter(|r| r.survives_o2.is_some()).collect();
        let survived = tested
            .iter()
            .filter(|r| r.survives_o2 == Some(true))
            .count();
        let survival_rate = if !tested.is_empty() {
            survived as f64 / tested.len() as f64
        } else {
            0.0
        };

        let survival_table: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "mutation_id": r.mutation_id,
                    "survives_o2": r.survives_o2,
                    "tested": r.survives_o2.is_some(),
                })
            })
            .collect();

        metrics.push(MetricResult {
            metric_id: "infra.i3.ir_mutation.o2_survival".to_string(),
            axis: "infrastructure".to_string(),
            category: "ir_mutation".to_string(),
            label: "Fraction surviving -O2 optimization".to_string(),
            value: survival_rate,
            details: json!({
                "survived": survived,
                "tested": tested.len(),
                "survival_table": survival_table,
            }),
            n,
        });

        // I3.3: Determinism
        let deterministic_count = results.iter().filter(|r| r.deterministic).count();
        let determinism = deterministic_count as f64 / n as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i3.ir_mutation.determinism".to_string(),
            axis: "infrastructure".to_string(),
            category: "ir_mutation".to_string(),
            label: "Fraction producing deterministic output from same seed".to_string(),
            value: determinism,
            details: json!({
                "deterministic": deterministic_count,
                "total": n,
            }),
            n,
        });

        // I3.4: Line bloat
        let bloat_ratios: Vec<f64> = results
            .iter()
            .map(|r| r.output_lines as f64 / r.input_lines.max(1) as f64)
            .collect();
        let mean_bloat = bloat_ratios.iter().sum::<f64>() / bloat_ratios.len() as f64;

        metrics.push(MetricResult {
            metric_id: "infra.i3.ir_mutation.line_bloat".to_string(),
            axis: "infrastructure".to_string(),
            category: "ir_mutation".to_string(),
            label: "Mean output/input line ratio".to_string(),
            value: mean_bloat,
            details: json!({
                "mean_bloat": mean_bloat,
                "bloat_ratios": bloat_ratios,
            }),
            n,
        });

        Ok(metrics)
    }
}
