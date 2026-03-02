#![cfg(feature = "guidance")]
mod common;

use evaluation::EvalMetric;
use evaluation::guidance::feedback_quality::FeedbackQuality;

#[test]
fn feedback_quality_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = FeedbackQuality;
    let results = metric.evaluate(&dataset).unwrap();

    // Should produce coverage correlation if rounds have coverage_percent
    if !results.is_empty() {
        for r in &results {
            assert_eq!(r.axis, "guidance");
            assert_eq!(r.category, "feedback_quality");
        }
    }
}

#[test]
fn feedback_quality_improvement() {
    let dataset = common::improvement_dataset();
    let metric = FeedbackQuality;
    let results = metric.evaluate(&dataset).unwrap();

    // Improvement scenario should produce correlation results
    let corr = results
        .iter()
        .find(|r| r.metric_id == "guidance.feedback_quality.coverage_correlation");
    if let Some(corr) = corr {
        // Correlation should be in [-1, 1]
        assert!(
            corr.value >= -1.0 && corr.value <= 1.0,
            "Correlation should be in [-1,1], got {}",
            corr.value
        );
    }
}
