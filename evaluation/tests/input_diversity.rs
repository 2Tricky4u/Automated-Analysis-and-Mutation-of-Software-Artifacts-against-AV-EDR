#![cfg(feature = "input")]
mod common;

use evaluation::EvalMetric;
use evaluation::input::diversity::Diversity;

#[test]
fn diversity_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = Diversity;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    // Mutation Jaccard distance
    let jaccard = results
        .iter()
        .find(|r| r.metric_id == "input.diversity.mutation_jaccard")
        .unwrap();
    assert!(
        jaccard.value >= 0.0 && jaccard.value <= 1.0,
        "Jaccard distance should be in [0,1], got {}",
        jaccard.value
    );

    // Module entropy
    let entropy = results
        .iter()
        .find(|r| r.metric_id == "input.diversity.module_entropy")
        .unwrap();
    assert!(
        entropy.value >= 0.0 && entropy.value <= 1.0,
        "Normalized entropy should be in [0,1], got {}",
        entropy.value
    );
}

#[test]
fn diversity_all_detected_low() {
    let dataset = common::all_detected_dataset();
    let metric = Diversity;
    let results = metric.evaluate(&dataset).unwrap();

    // All-detected uses default modules → low entropy
    let entropy = results
        .iter()
        .find(|r| r.metric_id == "input.diversity.module_entropy")
        .unwrap();
    assert!(
        entropy.value < 0.5,
        "All-detected (uniform modules) should have low entropy, got {}",
        entropy.value
    );
}
