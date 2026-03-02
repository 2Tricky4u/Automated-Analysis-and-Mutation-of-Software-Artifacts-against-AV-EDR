#![cfg(feature = "input")]
mod common;

use evaluation::EvalMetric;
use evaluation::input::validity::Validity;

#[test]
fn validity_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = Validity;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    let rejection = results
        .iter()
        .find(|r| r.metric_id == "input.validity.rejection_rate")
        .unwrap();
    // Mixed dataset has some MutationFailed rounds
    assert!(rejection.value >= 0.0 && rejection.value <= 1.0);

    let exec_rate = results
        .iter()
        .find(|r| r.metric_id == "input.validity.execution_rate")
        .unwrap();
    // Rejection + execution should sum to ~1.0
    assert!(
        (rejection.value + exec_rate.value - 1.0).abs() < 0.001,
        "Rejection + execution should = 1.0"
    );
}

#[test]
fn validity_all_evasion() {
    let dataset = common::all_evasion_dataset();
    let metric = Validity;
    let results = metric.evaluate(&dataset).unwrap();

    let rejection = results
        .iter()
        .find(|r| r.metric_id == "input.validity.rejection_rate")
        .unwrap();
    assert!(
        rejection.value == 0.0,
        "All-evasion should have 0% rejection, got {}",
        rejection.value
    );
}
