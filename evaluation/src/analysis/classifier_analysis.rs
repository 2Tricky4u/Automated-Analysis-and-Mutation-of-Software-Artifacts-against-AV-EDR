//! B2: Classifier Decision Boundary Analysis (offline component)
//!
//! Cross-tabulates existing round data to produce confusion matrix
//! (verdict → differential category), exit code distribution, and
//! verdict reliability metrics.
//!
//! The exhaustive branch-coverage test lives in `evaluation/tests/classifier_coverage.rs`.
//!
//! **RQ:** How reliable is the local verdict as a proxy for ground-truth detection?
//!
//! **Output:** Confusion matrix data; Sankey diagram data; verdict distribution

use crate::{DifferentialCategory, EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::HashMap;

pub struct ClassifierAnalysis;

impl EvalMetric for ClassifierAnalysis {
    fn metric_id(&self) -> &str {
        "component.b2.classifier_analysis"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Confusion matrix: detection_verdict → differential_category
        let mut confusion: HashMap<(String, String), usize> = HashMap::new();
        let mut verdict_counts: HashMap<String, usize> = HashMap::new();
        let mut category_counts: HashMap<String, usize> = HashMap::new();

        for round in &dataset.rounds {
            let verdict = round.detection_verdict.clone();
            let category = format!("{:?}", round.differential_category);

            *confusion
                .entry((verdict.clone(), category.clone()))
                .or_default() += 1;
            *verdict_counts.entry(verdict).or_default() += 1;
            *category_counts.entry(category).or_default() += 1;
        }

        // Build confusion matrix as nested map
        let verdicts: Vec<String> = {
            let mut v: Vec<String> = verdict_counts.keys().cloned().collect();
            v.sort();
            v
        };
        let categories: Vec<String> = {
            let mut c: Vec<String> = category_counts.keys().cloned().collect();
            c.sort();
            c
        };

        let mut matrix: Vec<Vec<usize>> = Vec::new();
        for verdict in &verdicts {
            let row: Vec<usize> = categories
                .iter()
                .map(|cat| *confusion.get(&(verdict.clone(), cat.clone())).unwrap_or(&0))
                .collect();
            matrix.push(row);
        }

        results.push(MetricResult {
            metric_id: "component.b2.classifier_analysis.confusion_matrix".to_string(),
            axis: "component".to_string(),
            category: "execution_engine".to_string(),
            label: "Confusion matrix (verdict → differential category)".to_string(),
            value: verdicts.len() as f64,
            details: json!({
                "verdicts": verdicts,
                "categories": categories,
                "matrix": matrix,
                "verdict_counts": verdict_counts,
                "category_counts": category_counts,
            }),
            n,
        });

        // 2. Verdict agreement rate
        // "Agreement" = verdict and differential category are consistent:
        //   evasion verdict → Evasion category
        //   detected verdict → RealDetection or StaticDetection
        let mut agreed = 0usize;
        for round in &dataset.rounds {
            let verdict_says_detected = round.detection_verdict == "detected";
            let category_says_detected = round.differential_category.is_detected();
            let verdict_says_evasion = round.detection_verdict == "evasion";
            let category_says_evasion =
                round.differential_category == DifferentialCategory::Evasion;

            if (verdict_says_detected && category_says_detected)
                || (verdict_says_evasion && category_says_evasion)
            {
                agreed += 1;
            }
        }

        let agreement_rate = agreed as f64 / n as f64;

        results.push(MetricResult {
            metric_id: "component.b2.classifier_analysis.verdict_agreement".to_string(),
            axis: "component".to_string(),
            category: "execution_engine".to_string(),
            label: "Verdict ↔ differential category agreement rate".to_string(),
            value: agreement_rate,
            details: json!({
                "agreed": agreed,
                "total": n,
                "interpretation": if agreement_rate > 0.8 {
                    "High agreement: local verdict is a reliable proxy"
                } else if agreement_rate > 0.5 {
                    "Moderate: differential protocol adds significant value"
                } else {
                    "Low: local verdict alone is unreliable"
                },
            }),
            n,
        });

        // 3. Sankey diagram data: verdict → category flows
        let mut flows: Vec<serde_json::Value> = Vec::new();
        for ((verdict, category), &count) in &confusion {
            if count > 0 {
                flows.push(json!({
                    "source": format!("verdict:{}", verdict),
                    "target": format!("category:{}", category),
                    "value": count,
                }));
            }
        }
        flows.sort_by(|a, b| {
            let va = a["value"].as_u64().unwrap_or(0);
            let vb = b["value"].as_u64().unwrap_or(0);
            vb.cmp(&va)
        });

        results.push(MetricResult {
            metric_id: "component.b2.classifier_analysis.sankey_flows".to_string(),
            axis: "component".to_string(),
            category: "execution_engine".to_string(),
            label: "Verdict → category Sankey diagram flows".to_string(),
            value: flows.len() as f64,
            details: json!({
                "flows": flows,
            }),
            n,
        });

        // 4. Per-category evasion score distribution
        let mut scores_by_cat: HashMap<String, Vec<f64>> = HashMap::new();
        for round in &dataset.rounds {
            let cat = format!("{:?}", round.differential_category);
            scores_by_cat
                .entry(cat)
                .or_default()
                .push(round.evasion_score);
        }

        let mut score_summary: Vec<serde_json::Value> = Vec::new();
        for (cat, scores) in &scores_by_cat {
            let mean = scores.iter().sum::<f64>() / scores.len() as f64;
            let mut sorted = scores.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let median = sorted[sorted.len() / 2];
            let min = sorted[0];
            let max = sorted[sorted.len() - 1];

            score_summary.push(json!({
                "category": cat,
                "n": scores.len(),
                "mean": mean,
                "median": median,
                "min": min,
                "max": max,
            }));
        }

        results.push(MetricResult {
            metric_id: "component.b2.classifier_analysis.score_by_category".to_string(),
            axis: "component".to_string(),
            category: "execution_engine".to_string(),
            label: "Evasion score distribution by differential category".to_string(),
            value: scores_by_cat.len() as f64,
            details: json!({
                "per_category": score_summary,
            }),
            n,
        });

        Ok(results)
    }
}
