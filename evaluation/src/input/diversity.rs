//! Input.Diversity — measures how different generated artifacts are from each other.
//!
//! Sub-metrics:
//! - Mean pairwise Jaccard distance of mutation sets
//! - Shannon entropy of module distributions
//! - Cumulative unique configs over time
//! - Seq2 token uniqueness (if telemetry available)

use crate::helpers::{config_fingerprint, mean_pairwise_jaccard, normalized_entropy};
use crate::{EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::{HashMap, HashSet};

pub struct Diversity;

fn get_module_value(modules: &crate::ModuleSelectionSpec, category: &str) -> String {
    match category {
        "carrier" => modules.carrier.clone(),
        "decoder" => modules.decoder.clone(),
        "antiemulation" => modules.antiemulation.clone(),
        "deconditioner" => modules.deconditioner.clone(),
        "guardrail" => modules.guardrail.clone(),
        "virtualprotect" => modules.virtualprotect.clone(),
        "decoy" => modules.decoy.clone(),
        _ => String::new(),
    }
}

impl EvalMetric for Diversity {
    fn metric_id(&self) -> &str {
        "input.diversity"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Mean pairwise Jaccard distance of mutation sets
        let mutation_sets: Vec<Vec<String>> =
            dataset.rounds.iter().map(|r| r.mutations.clone()).collect();

        let jaccard = mean_pairwise_jaccard(&mutation_sets);
        results.push(MetricResult {
            metric_id: "input.diversity.mutation_jaccard".to_string(),
            axis: "input".to_string(),
            category: "diversity".to_string(),
            label: "Mean pairwise Jaccard distance of mutation sets".to_string(),
            value: jaccard,
            details: json!({}),
            n,
        });

        // 2. Shannon entropy of module distributions per category
        let categories = [
            "carrier",
            "decoder",
            "antiemulation",
            "deconditioner",
            "guardrail",
            "virtualprotect",
            "decoy",
        ];

        let mut per_category_entropy = serde_json::Map::new();
        let mut total_entropy = 0.0;

        for cat in &categories {
            let mut dist: HashMap<String, usize> = HashMap::new();
            for round in &dataset.rounds {
                let val = get_module_value(&round.modules, cat);
                *dist.entry(val).or_default() += 1;
            }
            let counts: Vec<usize> = dist.values().copied().collect();
            let ent = normalized_entropy(&counts);
            total_entropy += ent;
            per_category_entropy.insert(
                cat.to_string(),
                json!({
                    "entropy": ent,
                    "distribution": dist,
                }),
            );
        }

        let avg_entropy = total_entropy / categories.len() as f64;
        results.push(MetricResult {
            metric_id: "input.diversity.module_entropy".to_string(),
            axis: "input".to_string(),
            category: "diversity".to_string(),
            label: "Mean normalized Shannon entropy of module distributions".to_string(),
            value: avg_entropy,
            details: json!({ "per_category": per_category_entropy }),
            n,
        });

        // 3. Cumulative unique configs over time
        let mut seen = HashSet::new();
        let mut cumulative: Vec<usize> = Vec::with_capacity(n);
        for round in &dataset.rounds {
            let fp = config_fingerprint(&round.modules, &round.mutations);
            seen.insert(fp);
            cumulative.push(seen.len());
        }

        // Discovery rate: fraction of rounds that introduced a new config
        let new_configs = if n > 0 {
            cumulative.windows(2).filter(|w| w[1] > w[0]).count() as f64 / (n - 1).max(1) as f64
        } else {
            0.0
        };

        results.push(MetricResult {
            metric_id: "input.diversity.config_discovery_rate".to_string(),
            axis: "input".to_string(),
            category: "diversity".to_string(),
            label: "Config discovery rate (fraction of rounds introducing new configs)".to_string(),
            value: new_configs,
            details: json!({
                "cumulative_unique": cumulative,
                "total_unique": seen.len(),
            }),
            n,
        });

        // 4. Seq2 token uniqueness (from token matrices if available)
        if !dataset.token_matrices.is_empty() {
            let all_seq2: HashSet<&str> = dataset
                .token_matrices
                .iter()
                .flat_map(|tm| {
                    tm.tokens
                        .iter()
                        .filter(|t| t.starts_with("seq2:"))
                        .map(|t| t.as_str())
                })
                .collect();

            let total_seq2_occurrences: usize = dataset
                .token_matrices
                .iter()
                .flat_map(|tm| tm.tokens.iter().filter(|t| t.starts_with("seq2:")))
                .count();

            let seq2_uniqueness = if total_seq2_occurrences > 0 {
                all_seq2.len() as f64 / total_seq2_occurrences as f64
            } else {
                0.0
            };

            results.push(MetricResult {
                metric_id: "input.diversity.seq2_uniqueness".to_string(),
                axis: "input".to_string(),
                category: "diversity".to_string(),
                label: "Seq2 token uniqueness (unique seq2 / total seq2 occurrences)".to_string(),
                value: seq2_uniqueness,
                details: json!({
                    "unique_seq2": all_seq2.len(),
                    "total_occurrences": total_seq2_occurrences,
                }),
                n,
            });
        }

        Ok(results)
    }
}
