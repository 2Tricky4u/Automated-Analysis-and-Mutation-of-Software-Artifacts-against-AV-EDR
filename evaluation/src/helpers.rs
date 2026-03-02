//! Shared math utilities for evaluation metrics.

use std::collections::HashSet;

/// Shannon entropy of a distribution (in bits).
///
/// Takes a slice of counts, normalizes to probabilities, and computes H.
/// Returns 0.0 for empty input or all-zero counts.
pub fn shannon_entropy(counts: &[usize]) -> f64 {
    let total: usize = counts.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let total_f = total as f64;
    let mut h = 0.0;
    for &c in counts {
        if c > 0 {
            let p = c as f64 / total_f;
            h -= p * p.log2();
        }
    }
    h
}

/// Normalized Shannon entropy (0.0–1.0).
///
/// Divides by log2(n_categories). Returns 0.0 if n_categories <= 1.
pub fn normalized_entropy(counts: &[usize]) -> f64 {
    let n = counts.len();
    if n <= 1 {
        return 0.0;
    }
    let max_entropy = (n as f64).log2();
    if max_entropy == 0.0 {
        return 0.0;
    }
    shannon_entropy(counts) / max_entropy
}

/// Jaccard distance between two string sets: 1 - |A ∩ B| / |A ∪ B|.
///
/// Returns 1.0 if both sets are empty (maximally different by convention for evaluation).
pub fn jaccard_distance(a: &[String], b: &[String]) -> f64 {
    let set_a: HashSet<&str> = a.iter().map(|s| s.as_str()).collect();
    let set_b: HashSet<&str> = b.iter().map(|s| s.as_str()).collect();

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        return 1.0;
    }

    1.0 - (intersection as f64 / union as f64)
}

/// Mean pairwise Jaccard distance across a list of string sets.
///
/// Returns 0.0 for fewer than 2 sets.
pub fn mean_pairwise_jaccard(sets: &[Vec<String>]) -> f64 {
    let n = sets.len();
    if n < 2 {
        return 0.0;
    }

    let mut total = 0.0;
    let mut count = 0usize;

    for i in 0..n {
        for j in (i + 1)..n {
            total += jaccard_distance(&sets[i], &sets[j]);
            count += 1;
        }
    }

    total / count as f64
}

/// Pearson correlation coefficient between two equal-length f64 slices.
///
/// Returns 0.0 if fewer than 2 observations or zero variance.
pub fn pearson_correlation(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len().min(y.len());
    if n < 2 {
        return 0.0;
    }

    let mean_x = x[..n].iter().sum::<f64>() / n as f64;
    let mean_y = y[..n].iter().sum::<f64>() / n as f64;

    let mut cov = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;

    for i in 0..n {
        let dx = x[i] - mean_x;
        let dy = y[i] - mean_y;
        cov += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }

    let denom = (var_x * var_y).sqrt();
    if denom == 0.0 {
        return 0.0;
    }

    cov / denom
}

/// Configuration fingerprint: sorted concatenation of module values + mutation IDs.
///
/// Two rounds with the same fingerprint had identical configurations.
pub fn config_fingerprint(
    modules: &controller::dispatch::types::ModuleSelectionSpec,
    mutations: &[String],
) -> String {
    let mut parts = vec![
        format!("c={}", modules.carrier),
        format!("d={}", modules.decoder),
        format!("a={}", modules.antiemulation),
        format!("dc={}", modules.deconditioner),
        format!("g={}", modules.guardrail),
        format!("v={}", modules.virtualprotect),
        format!("dy={}", modules.decoy),
    ];
    let mut sorted_mutations: Vec<&str> = mutations.iter().map(|s| s.as_str()).collect();
    sorted_mutations.sort();
    for m in sorted_mutations {
        parts.push(format!("m={}", m));
    }
    parts.join("|")
}

/// Rolling window average over a series.
pub fn rolling_mean(values: &[f64], window: usize) -> Vec<f64> {
    if window == 0 || values.is_empty() {
        return Vec::new();
    }
    values
        .windows(window)
        .map(|w| w.iter().sum::<f64>() / w.len() as f64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shannon_entropy_uniform() {
        // 4 equally-likely outcomes → 2 bits
        let counts = [10, 10, 10, 10];
        assert!((shannon_entropy(&counts) - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_shannon_entropy_degenerate() {
        // All in one bin → 0 bits
        let counts = [100, 0, 0, 0];
        assert!((shannon_entropy(&counts)).abs() < 0.001);
    }

    #[test]
    fn test_normalized_entropy() {
        let uniform = [10, 10, 10, 10];
        assert!((normalized_entropy(&uniform) - 1.0).abs() < 0.001);

        let degenerate = [100, 0, 0, 0];
        assert!((normalized_entropy(&degenerate)).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_distance_identical() {
        let a = vec!["x".to_string(), "y".to_string()];
        let b = vec!["x".to_string(), "y".to_string()];
        assert!((jaccard_distance(&a, &b)).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_distance_disjoint() {
        let a = vec!["x".to_string()];
        let b = vec!["y".to_string()];
        assert!((jaccard_distance(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_pearson_perfect_positive() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 4.0, 6.0, 8.0, 10.0];
        assert!((pearson_correlation(&x, &y) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_pearson_no_correlation() {
        // Symmetric around mean: x ascending, y symmetric → zero correlation
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 4.0, 3.0, 4.0, 2.0];
        assert!(
            pearson_correlation(&x, &y).abs() < 0.01,
            "Got {}",
            pearson_correlation(&x, &y)
        );
    }

    #[test]
    fn test_rolling_mean() {
        let vals = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let rm = rolling_mean(&vals, 3);
        assert_eq!(rm.len(), 3);
        assert!((rm[0] - 2.0).abs() < 0.001);
        assert!((rm[1] - 3.0).abs() < 0.001);
        assert!((rm[2] - 4.0).abs() < 0.001);
    }
}
