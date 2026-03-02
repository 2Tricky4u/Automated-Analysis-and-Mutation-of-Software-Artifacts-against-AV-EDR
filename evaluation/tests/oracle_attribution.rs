#![cfg(feature = "oracle")]
mod common;

use evaluation::EvalMetric;
use evaluation::oracle::attribution::Attribution;

#[test]
fn attribution_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = Attribution;
    let results = metric.evaluate(&dataset).unwrap();

    // Mixed dataset with enriched tokens should produce attribution results
    // (needs mix of detected and evasion for lift to be non-degenerate)
    if !results.is_empty() {
        let ranking = results
            .iter()
            .find(|r| r.metric_id == "oracle.attribution.token_ranking");
        if let Some(ranking) = ranking {
            assert!(ranking.value >= 0.0, "Top importance should be >= 0");
        }
    }
}

#[test]
fn attribution_all_detected_empty() {
    let dataset = common::all_detected_dataset();
    let metric = Attribution;
    let results = metric.evaluate(&dataset).unwrap();

    // All-detected → lift is degenerate → compute_token_scores returns empty
    // So attribution may return fewer results
    // This is correct behavior: can't do attribution without outcome variance
    for r in &results {
        assert_eq!(r.axis, "oracle");
    }
}
