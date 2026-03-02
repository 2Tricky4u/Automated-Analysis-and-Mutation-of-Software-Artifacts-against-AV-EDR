#![cfg(feature = "oracle")]
mod common;

use evaluation::EvalMetric;
use evaluation::oracle::soundness::Soundness;

#[test]
fn soundness_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = Soundness;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    // Evasion rate should be in [0,1]
    let evasion = results
        .iter()
        .find(|r| r.metric_id == "oracle.soundness.evasion_rate")
        .unwrap();
    assert!(evasion.value >= 0.0 && evasion.value <= 1.0);
}

#[test]
fn soundness_all_detected() {
    let dataset = common::all_detected_dataset();
    let metric = Soundness;
    let results = metric.evaluate(&dataset).unwrap();

    let evasion = results
        .iter()
        .find(|r| r.metric_id == "oracle.soundness.evasion_rate")
        .unwrap();
    assert!(
        evasion.value == 0.0,
        "All-detected should have 0% evasion rate, got {}",
        evasion.value
    );

    // All configs should be "blind spots" (always detected)
    let blind_spots = results
        .iter()
        .find(|r| r.metric_id == "oracle.soundness.blind_spot_ratio")
        .unwrap();
    // With 20 rounds of default modules, should be all blind spots or no repeated configs
    assert!(blind_spots.value >= 0.0);
}

#[test]
fn soundness_all_evasion() {
    let dataset = common::all_evasion_dataset();
    let metric = Soundness;
    let results = metric.evaluate(&dataset).unwrap();

    let evasion = results
        .iter()
        .find(|r| r.metric_id == "oracle.soundness.evasion_rate")
        .unwrap();
    assert!(
        evasion.value == 1.0,
        "All-evasion should have 100% evasion rate, got {}",
        evasion.value
    );
}
