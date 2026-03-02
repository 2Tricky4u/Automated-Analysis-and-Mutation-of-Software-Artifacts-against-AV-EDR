#![cfg(feature = "input")]
mod common;

use evaluation::EvalMetric;
use evaluation::input::expressiveness::Expressiveness;

#[test]
fn expressiveness_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = Expressiveness;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    for r in &results {
        assert_eq!(r.axis, "input");
        assert_eq!(r.category, "expressiveness");
        assert!(r.n > 0);
    }

    // Module coverage: with 30 random rounds, should cover most variants
    let module_cov = results
        .iter()
        .find(|r| r.metric_id == "input.expressiveness.module_coverage")
        .unwrap();
    assert!(
        module_cov.value > 0.3,
        "Module coverage should be >30% with 30 random rounds, got {}",
        module_cov.value
    );

    // Unique configs: random rounds should produce diverse configs
    let unique = results
        .iter()
        .find(|r| r.metric_id == "input.expressiveness.unique_configs")
        .unwrap();
    assert!(
        unique.value > 0.5,
        "Config uniqueness should be >50%, got {}",
        unique.value
    );
}

#[test]
fn expressiveness_all_detected() {
    let dataset = common::all_detected_dataset();
    let metric = Expressiveness;
    let results = metric.evaluate(&dataset).unwrap();

    // All-detected with default modules → low reachability
    let reachability = results
        .iter()
        .find(|r| r.metric_id == "input.expressiveness.category_reachability")
        .unwrap();
    assert!(
        reachability.value < 0.5,
        "All-detected (same modules) should have low reachability, got {}",
        reachability.value
    );
}
