//! C5: Counterfactual Validation of Token Attribution
//!
//! For each token, partitions rounds by token presence, computes the detection rate
//! delta, and runs Fisher's exact test (two-sided) with Bonferroni correction.
//!
//! **RQ:** Are lift scores causally meaningful or confounded?
//!
//! **Output:** Forest plot data (deltas + 95% CI); significance table; volcano plot data

use crate::{EvalDataset, EvalMetric, MetricResult, TokenMatrixEntry};
use serde_json::json;
use std::collections::{HashMap, HashSet};

pub struct CounterfactualValidation;

// ─── Fisher's Exact Test Implementation ────────────────────────────────────

/// Log-factorial using simple summation. Sufficient for n ≤ ~170.
fn log_factorial(n: u64) -> f64 {
    (2..=n).map(|i| (i as f64).ln()).sum()
}

/// Log of binomial coefficient C(n, k).
fn log_choose(n: u64, k: u64) -> f64 {
    if k > n {
        return f64::NEG_INFINITY;
    }
    log_factorial(n) - log_factorial(k) - log_factorial(n - k)
}

/// Log-PMF of hypergeometric distribution.
///
/// P(X = k | N, K, n) = C(K,k) * C(N-K, n-k) / C(N, n)
fn hypergeometric_log_pmf(k: u64, n_draw: u64, big_k: u64, n_total: u64) -> f64 {
    if k > big_k || k > n_draw {
        return f64::NEG_INFINITY;
    }
    if n_draw - k > n_total - big_k {
        return f64::NEG_INFINITY;
    }
    log_choose(big_k, k) + log_choose(n_total - big_k, n_draw - k) - log_choose(n_total, n_draw)
}

/// Two-sided Fisher's exact test for a 2×2 contingency table.
///
/// ```text
///                | Detected | Not Detected |
/// Token Present  |    a     |      b       | a+b
/// Token Absent   |    c     |      d       | c+d
///                | a+c      |     b+d      | n
/// ```
///
/// Returns p-value (two-sided, "small p" method).
fn fisher_exact_two_sided(a: u64, b: u64, c: u64, d: u64) -> f64 {
    let n_total = a + b + c + d;
    let row1 = a + b; // token present total
    let col1 = a + c; // detected total

    // Probability of observed table
    let log_p_obs = hypergeometric_log_pmf(a, row1, col1, n_total);
    let p_obs = log_p_obs.exp();

    // Range of possible k values (# detected among token-present rounds)
    let min_k = (row1 + col1).saturating_sub(n_total);
    let max_k = row1.min(col1);

    // Sum probabilities of all tables with P ≤ P_observed
    let mut p_value = 0.0;
    for k in min_k..=max_k {
        let log_p_k = hypergeometric_log_pmf(k, row1, col1, n_total);
        let p_k = log_p_k.exp();
        if p_k <= p_obs * (1.0 + 1e-7) {
            p_value += p_k;
        }
    }

    p_value.min(1.0)
}

/// 95% confidence interval for a proportion using Wilson score interval.
fn wilson_ci(successes: u64, total: u64) -> (f64, f64) {
    if total == 0 {
        return (0.0, 1.0);
    }
    let n = total as f64;
    let p = successes as f64 / n;
    let z = 1.96; // 95% CI
    let z2 = z * z;

    let denom = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denom;
    let margin = z * ((p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt()) / denom;

    ((center - margin).max(0.0), (center + margin).min(1.0))
}

fn trustworthy_pairs(entries: &[TokenMatrixEntry]) -> Vec<(Vec<String>, bool)> {
    entries
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect()
}

impl EvalMetric for CounterfactualValidation {
    fn metric_id(&self) -> &str {
        "component.c5.counterfactual"
    }

    fn evaluate(&self, dataset: &EvalDataset) -> anyhow::Result<Vec<MetricResult>> {
        if dataset.token_matrices.is_empty() {
            return Ok(vec![]);
        }

        let matrix = trustworthy_pairs(&dataset.token_matrices);
        if matrix.len() < 6 {
            return Ok(vec![]);
        }

        let n = dataset.rounds.len();
        let mut results = Vec::new();

        // Collect all unique tokens
        let all_tokens: HashSet<&str> = matrix
            .iter()
            .flat_map(|(tokens, _)| tokens.iter().map(|t| t.as_str()))
            .collect();

        // For each token, compute counterfactual statistics
        let mut token_stats: Vec<serde_json::Value> = Vec::new();
        let num_tests = all_tokens.len();

        // Base detection rate
        let total_detected = matrix.iter().filter(|(_, d)| *d).count();
        let base_rate = total_detected as f64 / matrix.len() as f64;

        // Per-token contingency table
        let mut token_data: HashMap<&str, (u64, u64, u64, u64)> = HashMap::new();

        for token in &all_tokens {
            let mut a = 0u64; // present AND detected
            let mut b = 0u64; // present AND not detected
            let mut c = 0u64; // absent AND detected
            let mut d = 0u64; // absent AND not detected

            for (tokens, detected) in &matrix {
                let present = tokens.iter().any(|t| t.as_str() == *token);
                match (present, *detected) {
                    (true, true) => a += 1,
                    (true, false) => b += 1,
                    (false, true) => c += 1,
                    (false, false) => d += 1,
                }
            }

            token_data.insert(token, (a, b, c, d));
        }

        for (&token, &(a, b, c, d)) in &token_data {
            let n_with = a + b;
            let n_without = c + d;

            if n_with == 0 || n_without == 0 {
                continue; // Can't compute delta if token always/never present
            }

            let rate_with = a as f64 / n_with as f64;
            let rate_without = c as f64 / n_without as f64;
            let delta = rate_with - rate_without;

            // Fisher's exact test
            let p_value = fisher_exact_two_sided(a, b, c, d);
            let p_bonferroni = (p_value * num_tests as f64).min(1.0);

            // 95% CI for the delta (using individual Wilson CIs)
            let (ci_low_with, ci_high_with) = wilson_ci(a, n_with);
            let (ci_low_without, ci_high_without) = wilson_ci(c, n_without);
            let delta_ci_low = ci_low_with - ci_high_without;
            let delta_ci_high = ci_high_with - ci_low_without;

            // Lift (same formula as scorer)
            let lift = if base_rate > 0.0 {
                rate_with / base_rate
            } else {
                1.0
            };

            // For volcano plot: -log10(p) vs delta
            let neg_log10_p = if p_value > 0.0 {
                -p_value.log10()
            } else {
                16.0 // Cap at 10^-16
            };

            token_stats.push(json!({
                "token": token,
                "n_with": n_with,
                "n_without": n_without,
                "detected_with": a,
                "detected_without": c,
                "rate_with": rate_with,
                "rate_without": rate_without,
                "delta": delta,
                "delta_ci_low": delta_ci_low,
                "delta_ci_high": delta_ci_high,
                "lift": lift,
                "p_value": p_value,
                "p_bonferroni": p_bonferroni,
                "significant_005": p_bonferroni < 0.05,
                "significant_010": p_bonferroni < 0.10,
                "neg_log10_p": neg_log10_p,
            }));
        }

        // Sort by absolute delta (largest effect size first)
        token_stats.sort_by(|a, b| {
            let da = a["delta"].as_f64().unwrap_or(0.0).abs();
            let db = b["delta"].as_f64().unwrap_or(0.0).abs();
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });

        // 1. Forest plot data (all tokens with sufficient observations)
        let significant_count = token_stats
            .iter()
            .filter(|s| s["significant_005"].as_bool().unwrap_or(false))
            .count();
        let marginal_count = token_stats
            .iter()
            .filter(|s| {
                s["significant_010"].as_bool().unwrap_or(false)
                    && !s["significant_005"].as_bool().unwrap_or(false)
            })
            .count();

        results.push(MetricResult {
            metric_id: "component.c5.counterfactual.forest_plot".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Counterfactual validation: significant tokens (Bonferroni α=0.05)".to_string(),
            value: significant_count as f64 / token_stats.len().max(1) as f64,
            details: json!({
                "significant_005": significant_count,
                "marginal_010": marginal_count,
                "total_tested": token_stats.len(),
                "num_tests_bonferroni": num_tests,
                "base_detection_rate": base_rate,
                "token_results": token_stats,
            }),
            n,
        });

        // 2. Summary: top effect sizes
        let top_positive: Vec<&serde_json::Value> = token_stats
            .iter()
            .filter(|s| s["delta"].as_f64().unwrap_or(0.0) > 0.0)
            .take(5)
            .collect();
        let top_negative: Vec<&serde_json::Value> = token_stats
            .iter()
            .filter(|s| s["delta"].as_f64().unwrap_or(0.0) < 0.0)
            .take(5)
            .collect();

        let largest_delta = token_stats
            .first()
            .and_then(|s| s["delta"].as_f64())
            .unwrap_or(0.0)
            .abs();

        results.push(MetricResult {
            metric_id: "component.c5.counterfactual.effect_sizes".to_string(),
            axis: "component".to_string(),
            category: "triage_engine".to_string(),
            label: "Largest absolute detection rate delta".to_string(),
            value: largest_delta,
            details: json!({
                "top_detection_increasing": top_positive,
                "top_detection_decreasing": top_negative,
            }),
            n,
        });

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fisher_exact_extreme() {
        // All detected when present, none when absent
        let p = fisher_exact_two_sided(5, 0, 0, 5);
        assert!(p < 0.05, "Extreme table should be significant: p={}", p);
    }

    #[test]
    fn test_fisher_exact_uniform() {
        // Equal rates
        let p = fisher_exact_two_sided(5, 5, 5, 5);
        assert!(p > 0.5, "Uniform table should not be significant: p={}", p);
    }

    #[test]
    fn test_fisher_exact_one_cell_zero() {
        // Moderate effect
        let p = fisher_exact_two_sided(8, 3, 2, 7);
        assert!(p < 0.1, "Moderate effect: p={}", p);
    }

    #[test]
    fn test_wilson_ci() {
        let (lo, hi) = wilson_ci(5, 10);
        assert!(lo < 0.5);
        assert!(hi > 0.5);
        assert!(lo > 0.0);
        assert!(hi < 1.0);
    }

    #[test]
    fn test_log_choose() {
        let lc = log_choose(10, 3);
        let expected = (120.0f64).ln(); // C(10,3) = 120
        assert!((lc - expected).abs() < 0.001);
    }
}
