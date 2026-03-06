//! Token scoring — lift, confidence, importance.
//!
//! Pure computation, no IO. Takes a matrix of (tokens, detected) per round
//! and computes per-token lift scores for avoid/seek classification.
//!
//! Lift(T) = P(detected | T) / P(detected)
//! Confidence(T) = min(1.0, n_total / 5.0)
//! Importance(T) = lift * confidence

use crate::triage::TriageGuidance;
use std::collections::HashMap;

/// Per-token statistics from the round matrix.
///
/// Computed by [`compute_token_scores`] from a matrix of (tokens, detected)
/// tuples. Each token that appears in at least one round gets a score entry.
/// Downstream, [`build_guidance`] classifies tokens into avoid/seek based on
/// their lift and confidence.
#[derive(Debug, Clone)]
pub struct TokenScore {
    /// Normalized triage token string (e.g. `"api:VirtualProtect"`, `"module:carrier=alloc_rw_rx"`).
    pub token: String,
    /// Lift = P(detected | token) / P(detected). Values > 1.0 indicate
    /// positive correlation with detection; values < 1.0 indicate evasion
    /// correlation.
    pub lift: f64,
    /// Evidence-based confidence in [0.0, 1.0], calculated as
    /// `min(1.0, n_total / 5.0)`. Reaches 1.0 once the token has been
    /// observed in at least 5 rounds.
    pub confidence: f64,
    /// Composite importance score: `lift * confidence`. Used for ranking
    /// tokens before avoid/seek classification.
    pub importance: f64,
    /// Number of rounds where this token appeared **and** detection occurred.
    #[allow(dead_code)]
    pub n_detected: u32,
    /// Total number of rounds in which this token appeared.
    #[allow(dead_code)]
    pub n_total: u32,
}

/// Compute lift/confidence/importance for each token across all rounds.
///
/// `round_tokens` is a list of (tokens, detected) tuples — one per trustworthy round.
/// Returns scores sorted by importance descending.
pub fn compute_token_scores(round_tokens: &[(Vec<String>, bool)]) -> Vec<TokenScore> {
    if round_tokens.is_empty() {
        return Vec::new();
    }

    let total_rounds = round_tokens.len() as f64;
    let total_detected = round_tokens.iter().filter(|(_, d)| *d).count() as f64;

    // Base detection rate
    let p_detected = total_detected / total_rounds;

    // Guard: if all or none detected, lift is degenerate — return empty
    if p_detected == 0.0 || p_detected == 1.0 {
        return Vec::new();
    }

    // Count per-token occurrences and co-occurrence with detection
    let mut token_stats: HashMap<String, (u32, u32)> = HashMap::new(); // (n_total, n_detected)

    for (tokens, detected) in round_tokens {
        for token in tokens {
            let entry = token_stats.entry(token.clone()).or_insert((0, 0));
            entry.0 += 1;
            if *detected {
                entry.1 += 1;
            }
        }
    }

    let mut scores: Vec<TokenScore> = token_stats
        .into_iter()
        .map(|(token, (n_total, n_detected))| {
            let p_detected_given_t = n_detected as f64 / n_total as f64;
            let lift = p_detected_given_t / p_detected;
            let confidence = (n_total as f64 / 5.0).min(1.0);
            let importance = lift * confidence;

            TokenScore {
                token,
                lift,
                confidence,
                importance,
                n_detected,
                n_total,
            }
        })
        .collect();

    scores.sort_by(|a, b| {
        b.importance
            .partial_cmp(&a.importance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    scores
}

/// Build avoid/seek guidance from scored tokens.
///
/// - `avoid`: tokens where lift > `lift_threshold` AND confidence > `min_confidence`
/// - `seek`: tokens where lift < `1/lift_threshold` AND confidence > `min_confidence`
pub fn build_guidance(
    scores: &[TokenScore],
    lift_threshold: f64,
    min_confidence: f64,
) -> TriageGuidance {
    let inverse_threshold = 1.0 / lift_threshold;

    let mut avoid_tokens: Vec<String> = scores
        .iter()
        .filter(|s| s.lift > lift_threshold && s.confidence > min_confidence)
        .map(|s| s.token.clone())
        .collect();

    let mut seek_tokens: Vec<String> = scores
        .iter()
        .filter(|s| s.lift < inverse_threshold && s.confidence > min_confidence)
        .map(|s| s.token.clone())
        .collect();

    // Already sorted by importance via scores ordering, but let's be explicit
    // (avoid is sorted descending by importance, seek ascending by lift = most evasion-correlated first)
    avoid_tokens.truncate(50); // Cap to keep guidance manageable
    seek_tokens.truncate(50);

    TriageGuidance {
        avoid_tokens,
        seek_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lift_high_for_detection_correlated_token() {
        // Token A appears only in detected rounds
        let rounds = vec![
            (vec!["A".to_string(), "B".to_string()], true),
            (vec!["A".to_string(), "B".to_string()], true),
            (vec!["B".to_string()], false),
            (vec!["B".to_string()], false),
        ];

        let scores = compute_token_scores(&rounds);

        let a_score = scores.iter().find(|s| s.token == "A").unwrap();
        // P(detected) = 2/4 = 0.5, P(detected|A) = 2/2 = 1.0, lift = 2.0
        assert!((a_score.lift - 2.0).abs() < 0.01, "A lift should be 2.0");
        assert_eq!(a_score.n_detected, 2);
        assert_eq!(a_score.n_total, 2);
    }

    #[test]
    fn test_seek_token_only_in_evasion() {
        // Token C appears only in evasion (not detected) rounds
        let rounds = vec![
            (vec!["X".to_string()], true),
            (vec!["X".to_string()], true),
            (vec!["C".to_string(), "X".to_string()], false),
            (vec!["C".to_string(), "X".to_string()], false),
        ];

        let scores = compute_token_scores(&rounds);

        let c_score = scores.iter().find(|s| s.token == "C").unwrap();
        // P(detected) = 0.5, P(detected|C) = 0/2 = 0.0, lift = 0.0
        assert!(c_score.lift < 0.01, "C lift should be ~0.0");

        let guidance = build_guidance(&scores, 1.5, 0.3);
        assert!(
            guidance.seek_tokens.contains(&"C".to_string()),
            "C should be in seek tokens"
        );
    }

    #[test]
    fn test_confidence_scales_with_observations() {
        let rounds = vec![
            (vec!["A".to_string()], true),
            (vec!["B".to_string()], false),
        ];

        let scores = compute_token_scores(&rounds);
        let a_score = scores.iter().find(|s| s.token == "A").unwrap();
        // n_total = 1, confidence = min(1.0, 1/5) = 0.2
        assert!(
            (a_score.confidence - 0.2).abs() < 0.01,
            "Confidence for 1 obs should be 0.2, got {}",
            a_score.confidence
        );

        // 5+ observations
        let rounds5 = vec![
            (vec!["A".to_string()], true),
            (vec!["A".to_string()], true),
            (vec!["A".to_string()], true),
            (vec!["A".to_string()], true),
            (vec!["A".to_string()], true),
            (vec!["B".to_string()], false),
        ];
        let scores5 = compute_token_scores(&rounds5);
        let a5 = scores5.iter().find(|s| s.token == "A").unwrap();
        assert!(
            (a5.confidence - 1.0).abs() < 0.01,
            "Confidence for 5+ obs should be 1.0, got {}",
            a5.confidence
        );
    }

    #[test]
    fn test_build_guidance_thresholds() {
        let rounds = vec![
            (vec!["bad".to_string(), "neutral".to_string()], true),
            (vec!["bad".to_string(), "neutral".to_string()], true),
            (vec!["bad".to_string(), "neutral".to_string()], true),
            (vec!["good".to_string(), "neutral".to_string()], false),
            (vec!["good".to_string(), "neutral".to_string()], false),
            (vec!["good".to_string(), "neutral".to_string()], false),
        ];

        let scores = compute_token_scores(&rounds);
        let guidance = build_guidance(&scores, 1.5, 0.3);

        // "bad" lift = (3/3)/(3/6) = 2.0 > 1.5 → avoid
        assert!(
            guidance.avoid_tokens.contains(&"bad".to_string()),
            "bad should be avoided"
        );
        // "good" lift = (0/3)/(0.5) = 0.0 < 1/1.5=0.67 → seek
        assert!(
            guidance.seek_tokens.contains(&"good".to_string()),
            "good should be sought"
        );
        // "neutral" lift = (3/6)/(0.5) = 1.0 — neither avoid nor seek
        assert!(
            !guidance.avoid_tokens.contains(&"neutral".to_string()),
            "neutral should not be avoided"
        );
        assert!(
            !guidance.seek_tokens.contains(&"neutral".to_string()),
            "neutral should not be sought"
        );
    }

    #[test]
    fn test_empty_rounds() {
        let scores = compute_token_scores(&[]);
        assert!(scores.is_empty());
    }

    #[test]
    fn test_all_detected_returns_empty() {
        // All detected → p_detected = 1.0 → lift is degenerate
        let rounds = vec![(vec!["A".to_string()], true), (vec!["A".to_string()], true)];
        let scores = compute_token_scores(&rounds);
        assert!(scores.is_empty(), "All-detected should return empty scores");
    }
}
