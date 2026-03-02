//! Input.Expressiveness — measures module/mutation coverage and unique configurations.
//!
//! Sub-metrics:
//! - Module variant coverage: fraction of available variants actually used per category
//! - Mutation coverage: fraction of mutation pool actually applied
//! - Unique config count: distinct (modules+mutations) fingerprints
//! - Category reachability: how many module categories were varied

use crate::helpers::config_fingerprint;
use crate::{EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::{HashMap, HashSet};

pub struct Expressiveness;

/// Known module variants per category (mirrors build templates).
const KNOWN_VARIANTS: &[(&str, &[&str])] = &[
    ("carrier", &["alloc_rw_rx", "change_rw_rx", "peb_walk"]),
    ("decoder", &["xor", "english"]),
    ("antiemulation", &["none", "sirallocalot", "timeraw"]),
    ("deconditioner", &["none", "alloc_loop"]),
    ("guardrail", &["none", "env"]),
    ("virtualprotect", &["standard", "undersized"]),
    ("decoy", &["none", "calc", "winexec"]),
];

/// Known AST mutations.
const KNOWN_MUTATIONS: &[&str] = &[
    "ast.decon_rounds",
    "ast.fill_pattern",
    "ast.exec_decoy",
    "ast.timing_pattern",
    "ast.protection_transition",
];

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

impl EvalMetric for Expressiveness {
    fn metric_id(&self) -> &str {
        "input.expressiveness"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        let n = dataset.rounds.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // 1. Module variant coverage per category
        let mut category_coverage: HashMap<&str, HashSet<String>> = HashMap::new();
        for (cat, _) in KNOWN_VARIANTS {
            category_coverage.insert(cat, HashSet::new());
        }

        for round in &dataset.rounds {
            for (cat, _) in KNOWN_VARIANTS {
                let val = get_module_value(&round.modules, cat);
                category_coverage.get_mut(cat).unwrap().insert(val);
            }
        }

        let mut per_category = serde_json::Map::new();
        let mut total_used = 0usize;
        let mut total_available = 0usize;

        for (cat, known) in KNOWN_VARIANTS {
            let used = category_coverage[cat].len();
            let available = known.len();
            total_used += used;
            total_available += available;
            per_category.insert(
                cat.to_string(),
                json!({
                    "used": used,
                    "available": available,
                    "coverage": used as f64 / available as f64,
                    "variants_seen": category_coverage[cat].iter().collect::<Vec<_>>(),
                }),
            );
        }

        let module_coverage = total_used as f64 / total_available as f64;
        results.push(MetricResult {
            metric_id: "input.expressiveness.module_coverage".to_string(),
            axis: "input".to_string(),
            category: "expressiveness".to_string(),
            label: "Module variant coverage (used/total across all categories)".to_string(),
            value: module_coverage,
            details: json!({ "per_category": per_category }),
            n,
        });

        // 2. Mutation coverage
        let used_mutations: HashSet<&str> = dataset
            .rounds
            .iter()
            .flat_map(|r| r.mutations.iter().map(|m: &String| m.as_str()))
            .collect();

        let known_set: HashSet<&str> = KNOWN_MUTATIONS.iter().copied().collect();
        let covered = used_mutations.intersection(&known_set).count();
        let mutation_coverage = if KNOWN_MUTATIONS.is_empty() {
            0.0
        } else {
            covered as f64 / KNOWN_MUTATIONS.len() as f64
        };

        results.push(MetricResult {
            metric_id: "input.expressiveness.mutation_coverage".to_string(),
            axis: "input".to_string(),
            category: "expressiveness".to_string(),
            label: "Mutation pool coverage (used/available)".to_string(),
            value: mutation_coverage,
            details: json!({
                "used": used_mutations.iter().collect::<Vec<_>>(),
                "known": KNOWN_MUTATIONS,
                "covered": covered,
            }),
            n,
        });

        // 3. Unique config count
        let unique_configs: HashSet<String> = dataset
            .rounds
            .iter()
            .map(|r| config_fingerprint(&r.modules, &r.mutations))
            .collect();

        let config_uniqueness = unique_configs.len() as f64 / n as f64;
        results.push(MetricResult {
            metric_id: "input.expressiveness.unique_configs".to_string(),
            axis: "input".to_string(),
            category: "expressiveness".to_string(),
            label: "Unique configuration ratio (distinct configs / total rounds)".to_string(),
            value: config_uniqueness,
            details: json!({
                "unique_count": unique_configs.len(),
                "total_rounds": n,
            }),
            n,
        });

        // 4. Category reachability: how many categories had >1 variant used
        let varied_categories = category_coverage
            .iter()
            .filter(|(_, v)| v.len() > 1)
            .count();
        let reachability = varied_categories as f64 / KNOWN_VARIANTS.len() as f64;

        results.push(MetricResult {
            metric_id: "input.expressiveness.category_reachability".to_string(),
            axis: "input".to_string(),
            category: "expressiveness".to_string(),
            label: "Category reachability (categories with >1 variant used)".to_string(),
            value: reachability,
            details: json!({
                "varied": varied_categories,
                "total": KNOWN_VARIANTS.len(),
            }),
            n,
        });

        Ok(results)
    }
}
