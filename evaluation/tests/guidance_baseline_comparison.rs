#![cfg(feature = "guidance")]
mod common;

use evaluation::EvalMetric;
use evaluation::guidance::baseline_comparison::BaselineComparison;

#[test]
fn baseline_comparison_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = BaselineComparison;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    for r in &results {
        assert_eq!(r.axis, "guidance");
        assert_eq!(r.category, "baseline_comparison");
    }

    // Evasion rate delta should exist
    let delta = results
        .iter()
        .find(|r| r.metric_id == "guidance.baseline_comparison.evasion_rate_delta")
        .unwrap();
    // Delta is guided - random, could be positive or negative
    assert!(
        delta.value >= -1.0 && delta.value <= 1.0,
        "Delta should be in [-1,1], got {}",
        delta.value
    );
}

#[test]
fn baseline_comparison_improvement() {
    let dataset = common::improvement_dataset();
    let metric = BaselineComparison;
    let results = metric.evaluate(&dataset).unwrap();

    // Improvement scenario should have token guidance usage since selections have avoid/seek
    let usage = results
        .iter()
        .find(|r| r.metric_id == "guidance.baseline_comparison.token_guidance_usage");
    if let Some(usage) = usage {
        assert!(
            usage.value > 0.0,
            "Improvement dataset with selections should have token guidance usage"
        );
    }
}
