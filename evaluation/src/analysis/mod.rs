//! Component-level academic evaluation experiments.
//!
//! These experiments produce tables and figures suitable for a thesis evaluation chapter.
//! All operate on [`EvalDataset`] loaded from JSON — no production code is modified.
//!
//! ## Experiments
//!
//! | ID  | Module                    | Section   | Description                              |
//! |-----|---------------------------|-----------|------------------------------------------|
//! | C1  | `token_sensitivity`       | Triage    | Token scoring sensitivity analysis       |
//! | C3  | `token_coverage`          | Triage    | Token extraction coverage                |
//! | C4  | `scoring_convergence`     | Triage    | Scoring convergence over rounds          |
//! | C5  | `counterfactual`          | Triage    | Counterfactual validation (Fisher test)  |
//! | B3  | `telemetry_completeness`  | Execution | Telemetry collection completeness        |
//! | B2  | `classifier_analysis`     | Execution | Classifier decision boundary analysis    |

pub mod classifier_analysis;
pub mod counterfactual;
pub mod scoring_convergence;
pub mod telemetry_completeness;
pub mod token_coverage;
pub mod token_sensitivity;

use crate::EvalMetric;

/// Returns all component-level analysis metrics.
pub fn all_analysis_metrics() -> Vec<Box<dyn EvalMetric>> {
    vec![
        Box::new(token_sensitivity::TokenSensitivity),
        Box::new(token_coverage::TokenCoverage),
        Box::new(scoring_convergence::ScoringConvergence),
        Box::new(counterfactual::CounterfactualValidation),
        Box::new(telemetry_completeness::TelemetryCompleteness),
        Box::new(classifier_analysis::ClassifierAnalysis),
    ]
}
