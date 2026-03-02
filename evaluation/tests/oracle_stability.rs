#![cfg(feature = "oracle")]
mod common;

use evaluation::EvalMetric;
use evaluation::oracle::stability::Stability;

#[test]
fn stability_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = Stability;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    // Flaky rate in [0,1]
    let flaky = results
        .iter()
        .find(|r| r.metric_id == "oracle.stability.flaky_rate")
        .unwrap();
    assert!(flaky.value >= 0.0 && flaky.value <= 1.0);

    // Behavior match rate in [0,1]
    let behavior = results
        .iter()
        .find(|r| r.metric_id == "oracle.stability.behavior_match_rate")
        .unwrap();
    assert!(behavior.value >= 0.0 && behavior.value <= 1.0);
}

#[test]
fn stability_all_evasion_high() {
    let dataset = common::all_evasion_dataset();
    let metric = Stability;
    let results = metric.evaluate(&dataset).unwrap();

    let flaky = results
        .iter()
        .find(|r| r.metric_id == "oracle.stability.flaky_rate")
        .unwrap();
    assert!(
        flaky.value == 0.0,
        "All-evasion should have 0% flaky rate, got {}",
        flaky.value
    );

    let behavior = results
        .iter()
        .find(|r| r.metric_id == "oracle.stability.behavior_match_rate")
        .unwrap();
    assert!(
        behavior.value == 1.0,
        "All-evasion should have 100% behavior match, got {}",
        behavior.value
    );
}
