//! C3: Token Extraction Coverage
//!
//! Measures the breadth and distribution of extracted triage tokens across
//! 9 categories (module, mutation, api, api_arg, seq2, image, etw, etw_event, checkpoint).
//!
//! **RQ:** Which token categories contribute to attribution?
//!
//! **Output:** Stacked bar chart data; 9-row coverage table; token presence heatmap

use crate::{EvalDataset, EvalMetric, MetricResult};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};

pub struct TokenCoverage;

/// Token category prefixes (9 categories from CLAUDE.md §6).
const TOKEN_CATEGORIES: &[(&str, &str)] = &[
    ("module:", "Module"),
    ("mutation:", "Mutation"),
    ("api:", "API call"),
    ("api_arg:", "API argument"),
    ("seq2:", "Sequence (2-gram)"),
    ("image:", "Image load"),
    ("etw:", "ETW provider"),
    ("etw_event:", "ETW event"),
    ("checkpoint:", "Checkpoint"),
];

fn categorize_token(token: &str) -> &'static str {
    for &(prefix, name) in TOKEN_CATEGORIES {
        if token.starts_with(prefix) {
            return name;
        }
    }
    "Unknown"
}

impl EvalMetric for TokenCoverage {
    fn metric_id(&self) -> &str {
        "component.c3.token_coverage"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        if dataset.token_matrices.is_empty() {
            return Ok(vec![]);
        }

        let n = dataset.rounds.len();
        let mut results = Vec::new();

        // Collect all tokens across all rounds
        let mut category_tokens: HashMap<&str, HashSet<String>> = HashMap::new();
        let mut category_occurrences: HashMap<&str, usize> = HashMap::new();
        let mut tokens_per_round: Vec<usize> = Vec::new();
        let mut all_unique_tokens: HashSet<String> = HashSet::new();

        for entry in &dataset.token_matrices {
            tokens_per_round.push(entry.tokens.len());

            for token in &entry.tokens {
                let cat = categorize_token(token);
                category_tokens
                    .entry(cat)
                    .or_default()
                    .insert(token.clone());
                *category_occurrences.entry(cat).or_default() += 1;
                all_unique_tokens.insert(token.clone());
            }
        }

        // 1. Per-category coverage table
        let total_occurrences: usize = category_occurrences.values().sum();
        let mut coverage_table: Vec<serde_json::Value> = Vec::new();

        for &(prefix, name) in TOKEN_CATEGORIES {
            let unique = category_tokens.get(name).map_or(0, |s| s.len());
            let occurrences = *category_occurrences.get(name).unwrap_or(&0);
            let proportion = if total_occurrences > 0 {
                occurrences as f64 / total_occurrences as f64
            } else {
                0.0
            };
            let rounds_present = dataset
                .token_matrices
                .iter()
                .filter(|e| e.tokens.iter().any(|t| t.starts_with(prefix)))
                .count();

            coverage_table.push(json!({
                "category": name,
                "prefix": prefix,
                "unique_tokens": unique,
                "total_occurrences": occurrences,
                "proportion": proportion,
                "rounds_present": rounds_present,
                "round_coverage": rounds_present as f64 / dataset.token_matrices.len() as f64,
                "example_tokens": category_tokens.get(name)
                    .map(|s| s.iter().take(5).cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            }));
        }

        // Categories with actual tokens
        let active_categories = TOKEN_CATEGORIES
            .iter()
            .filter(|(_, name)| category_tokens.get(name).is_some_and(|s| !s.is_empty()))
            .count();
        let category_coverage = active_categories as f64 / TOKEN_CATEGORIES.len() as f64;

        results.push(MetricResult {
            metric_id: "component.c3.token_coverage.category_table".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Token category coverage (active categories / 9 total)".to_string(),
            value: category_coverage,
            details: json!({
                "active_categories": active_categories,
                "total_categories": TOKEN_CATEGORIES.len(),
                "coverage_table": coverage_table,
                "total_unique_tokens": all_unique_tokens.len(),
                "total_occurrences": total_occurrences,
            }),
            n,
        });

        // 2. Tokens per round statistics
        let mean_tokens =
            tokens_per_round.iter().sum::<usize>() as f64 / tokens_per_round.len().max(1) as f64;
        let min_tokens = tokens_per_round.iter().copied().min().unwrap_or(0);
        let max_tokens = tokens_per_round.iter().copied().max().unwrap_or(0);
        let mut sorted_counts = tokens_per_round.clone();
        sorted_counts.sort();
        let median_tokens = if sorted_counts.is_empty() {
            0.0
        } else {
            sorted_counts[sorted_counts.len() / 2] as f64
        };

        results.push(MetricResult {
            metric_id: "component.c3.token_coverage.tokens_per_round".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Mean tokens per round".to_string(),
            value: mean_tokens,
            details: json!({
                "mean": mean_tokens,
                "median": median_tokens,
                "min": min_tokens,
                "max": max_tokens,
                "per_round": tokens_per_round,
            }),
            n,
        });

        // 3. Token presence heatmap (tokens × rounds matrix)
        // Use top-20 most frequent tokens for the heatmap
        let mut token_freq: BTreeMap<String, usize> = BTreeMap::new();
        for entry in &dataset.token_matrices {
            for token in &entry.tokens {
                *token_freq.entry(token.clone()).or_default() += 1;
            }
        }

        let mut freq_sorted: Vec<(String, usize)> = token_freq.into_iter().collect();
        freq_sorted.sort_by(|a, b| b.1.cmp(&a.1));
        let top_tokens: Vec<String> = freq_sorted
            .iter()
            .take(20)
            .map(|(t, _)| t.clone())
            .collect();

        let mut heatmap: Vec<Vec<bool>> = Vec::new();
        for entry in &dataset.token_matrices {
            let token_set: HashSet<&str> = entry.tokens.iter().map(|t| t.as_str()).collect();
            let row: Vec<bool> = top_tokens
                .iter()
                .map(|t| token_set.contains(t.as_str()))
                .collect();
            heatmap.push(row);
        }

        results.push(MetricResult {
            metric_id: "component.c3.token_coverage.presence_heatmap".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Token presence heatmap (top-20 tokens × rounds)".to_string(),
            value: all_unique_tokens.len() as f64,
            details: json!({
                "token_labels": top_tokens,
                "heatmap": heatmap,
                "round_numbers": dataset.token_matrices.iter()
                    .map(|e| e.round_number).collect::<Vec<_>>(),
            }),
            n,
        });

        // 4. Category proportion for stacked bar chart
        let mut stacked_data: Vec<serde_json::Value> = Vec::new();
        for entry in &dataset.token_matrices {
            let mut cat_counts: HashMap<&str, usize> = HashMap::new();
            for token in &entry.tokens {
                let cat = categorize_token(token);
                *cat_counts.entry(cat).or_default() += 1;
            }
            stacked_data.push(json!({
                "round": entry.round_number,
                "counts": cat_counts,
                "total": entry.tokens.len(),
            }));
        }

        results.push(MetricResult {
            metric_id: "component.c3.token_coverage.category_proportions".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Per-round token category proportions (for stacked bar)".to_string(),
            value: active_categories as f64,
            details: json!({
                "stacked_data": stacked_data,
                "categories": TOKEN_CATEGORIES.iter()
                    .map(|(_, name)| name).collect::<Vec<_>>(),
            }),
            n,
        });

        Ok(results)
    }
}
