#![cfg(feature = "guidance")]
mod common;

use evaluation::EvalMetric;
use evaluation::guidance::convergence::Convergence;

#[test]
fn convergence_improvement_dataset() {
    let dataset = common::improvement_dataset();
    let metric = Convergence;
    let results = metric.evaluate(&dataset).unwrap();

    assert!(!results.is_empty(), "Should produce results");

    for r in &results {
        assert_eq!(r.axis, "guidance");
        assert_eq!(r.category, "convergence");
    }

    // Improvement scenario: second half should have more evasions than first half
    let decay = results
        .iter()
        .find(|r| r.metric_id == "guidance.convergence.decay_ratio")
        .unwrap();
    assert!(
        decay.value >= 1.0,
        "Improvement scenario decay ratio should be >= 1.0, got {}",
        decay.value
    );
}

#[test]
fn convergence_plateau_dataset() {
    let dataset = common::plateau_dataset();
    let metric = Convergence;
    let results = metric.evaluate(&dataset).unwrap();

    // Plateau scenario: should detect plateau early
    let plateau = results
        .iter()
        .find(|r| r.metric_id == "guidance.convergence.plateau_round");
    if let Some(plateau) = plateau {
        assert!(
            plateau.value < 1.0,
            "Plateau should be detected before end, got {}",
            plateau.value
        );
    }
}

#[test]
fn convergence_exploitation_ratio() {
    let dataset = common::improvement_dataset();
    let metric = Convergence;
    let results = metric.evaluate(&dataset).unwrap();

    let exploit = results
        .iter()
        .find(|r| r.metric_id == "guidance.convergence.exploitation_ratio");
    if let Some(exploit) = exploit {
        assert!(
            exploit.value >= 0.0 && exploit.value <= 1.0,
            "Exploitation ratio should be in [0,1], got {}",
            exploit.value
        );
    }
}
