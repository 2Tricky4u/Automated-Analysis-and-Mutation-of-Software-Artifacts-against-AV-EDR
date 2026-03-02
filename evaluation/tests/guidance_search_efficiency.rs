#![cfg(feature = "guidance")]
mod common;

use evaluation::EvalMetric;
use evaluation::guidance::search_efficiency::SearchEfficiency;

#[test]
fn search_efficiency_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = SearchEfficiency;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    let epr = results
        .iter()
        .find(|r| r.metric_id == "guidance.search_efficiency.evasions_per_round")
        .unwrap();
    assert!(epr.value >= 0.0 && epr.value <= 1.0);
}

#[test]
fn search_efficiency_all_evasion() {
    let dataset = common::all_evasion_dataset();
    let metric = SearchEfficiency;
    let results = metric.evaluate(&dataset).unwrap();

    let epr = results
        .iter()
        .find(|r| r.metric_id == "guidance.search_efficiency.evasions_per_round")
        .unwrap();
    assert!(
        epr.value == 1.0,
        "All-evasion should have 100% evasion rate, got {}",
        epr.value
    );

    let ttfe = results
        .iter()
        .find(|r| r.metric_id == "guidance.search_efficiency.time_to_first_evasion")
        .unwrap();
    assert!(
        ttfe.value == 1.0,
        "All-evasion should find first evasion at round 1, got {}",
        ttfe.value
    );
}

#[test]
fn search_efficiency_all_detected() {
    let dataset = common::all_detected_dataset();
    let metric = SearchEfficiency;
    let results = metric.evaluate(&dataset).unwrap();

    let epr = results
        .iter()
        .find(|r| r.metric_id == "guidance.search_efficiency.evasions_per_round")
        .unwrap();
    assert!(
        epr.value == 0.0,
        "All-detected should have 0% evasion rate, got {}",
        epr.value
    );
}
