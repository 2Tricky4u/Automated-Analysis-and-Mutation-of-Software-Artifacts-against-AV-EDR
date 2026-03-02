#![cfg(feature = "oracle")]
mod common;

use evaluation::EvalMetric;
use evaluation::oracle::precision::Precision;

#[test]
fn precision_mixed_dataset() {
    let dataset = common::mixed_dataset();
    let metric = Precision;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    for r in &results {
        assert_eq!(r.axis, "oracle");
        assert_eq!(r.category, "precision");
    }

    // Trustworthy ratio should be non-trivial
    let trustworthy = results
        .iter()
        .find(|r| r.metric_id == "oracle.precision.trustworthy_ratio")
        .unwrap();
    assert!(
        trustworthy.value > 0.0 && trustworthy.value <= 1.0,
        "Trustworthy ratio should be in (0,1], got {}",
        trustworthy.value
    );

    // FP and FN rates should be in [0,1]
    let fp = results
        .iter()
        .find(|r| r.metric_id == "oracle.precision.fp_proxy_rate")
        .unwrap();
    assert!(fp.value >= 0.0 && fp.value <= 1.0);

    let fn_rate = results
        .iter()
        .find(|r| r.metric_id == "oracle.precision.fn_proxy_rate")
        .unwrap();
    assert!(fn_rate.value >= 0.0 && fn_rate.value <= 1.0);
}

#[test]
fn precision_all_evasion() {
    let dataset = common::all_evasion_dataset();
    let metric = Precision;
    let results = metric.evaluate(&dataset).unwrap();

    let fp = results
        .iter()
        .find(|r| r.metric_id == "oracle.precision.fp_proxy_rate")
        .unwrap();
    assert!(
        fp.value == 0.0,
        "All-evasion should have 0% FP proxy rate, got {}",
        fp.value
    );

    let trustworthy = results
        .iter()
        .find(|r| r.metric_id == "oracle.precision.trustworthy_ratio")
        .unwrap();
    assert!(
        trustworthy.value == 1.0,
        "All-evasion should be 100% trustworthy, got {}",
        trustworthy.value
    );
}
