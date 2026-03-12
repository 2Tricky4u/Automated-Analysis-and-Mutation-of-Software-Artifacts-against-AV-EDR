//! Component-level and infrastructure-level academic evaluation experiments.
//!
//! These experiments produce tables and figures suitable for a thesis evaluation chapter.
//! All operate on [`EvalDataset`] or [`InfraEvalDataset`] loaded from JSON — no production
//! code is modified.
//!
//! ## Component Experiments (C1–C5, B2–B3)
//!
//! | ID  | Module                    | Section   | Description                              |
//! |-----|---------------------------|-----------|------------------------------------------|
//! | C1  | `token_sensitivity`       | Triage    | Token scoring sensitivity analysis       |
//! | C3  | `token_coverage`          | Triage    | Token extraction coverage                |
//! | C4  | `scoring_convergence`     | Triage    | Scoring convergence over rounds          |
//! | C5  | `counterfactual`          | Triage    | Counterfactual validation (Fisher test)  |
//! | B3  | `telemetry_completeness`  | Execution | Telemetry collection completeness        |
//! | B2  | `classifier_analysis`     | Execution | Classifier decision boundary analysis    |
//!
//! ## Infrastructure Experiments (I1–I15)
//!
//! | ID  | Module                            | Section         | Description                          |
//! |-----|-----------------------------------|-----------------|--------------------------------------|
//! | I1  | `payload_encoding`                | Build           | Payload encoding analysis            |
//! | I2  | `ast_mutation_analysis`           | Build           | AST mutation impact analysis         |
//! | I3  | `ir_mutation_analysis`            | Build           | IR mutation analysis                 |
//! | I4  | `binary_mutation_analysis`        | Build           | Binary mutation analysis             |
//! | I5  | `template_assembly_analysis`      | Build           | Template assembly analysis           |
//! | I6  | `instrumentation_analysis`        | Build           | Instrumentation overhead analysis    |
//! | I7  | `token_extraction_analysis`       | Triage          | Token extraction analysis            |
//! | I8  | `token_scoring_validation`        | Triage          | Token scoring validation             |
//! | I9  | `input_diversity_analysis`        | Input           | Input diversity (pairwise distance)  |
//! | I10 | `oracle_stability_analysis`       | Oracle          | Oracle stability (permutation test)  |
//! | I11 | `selector_comparison_analysis`    | Guidance        | Selector baseline comparison         |
//! | I12 | `guidance_utilization_analysis`   | Guidance        | Guidance utilization measurement     |
//! | I13 | `convergence_simulation_analysis` | Guidance        | Convergence simulation               |
//! | I14 | `line_tracing_analysis`           | Instrumentation | Line tracing overhead                |
//! | I15 | `sc_checkpoint_analysis`          | Instrumentation | Shellcode checkpoint patching        |

// Component-level modules
pub mod classifier_analysis;
pub mod counterfactual;
pub mod scoring_convergence;
pub mod telemetry_completeness;
pub mod token_coverage;
pub mod token_sensitivity;

// Infrastructure-level modules
pub mod ast_mutation_analysis;
pub mod binary_mutation_analysis;
pub mod convergence_simulation_analysis;
pub mod guidance_utilization_analysis;
pub mod input_diversity_analysis;
pub mod instrumentation_analysis;
pub mod ir_mutation_analysis;
pub mod line_tracing_analysis;
pub mod oracle_stability_analysis;
pub mod payload_encoding;
pub mod sc_checkpoint_analysis;
pub mod selector_comparison_analysis;
pub mod template_assembly_analysis;
pub mod token_extraction_analysis;
pub mod token_scoring_validation;

use crate::{EvalMetric, InfraMetric};

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

/// Returns all infrastructure-level analysis metrics (I1–I15).
pub fn all_infra_analysis_metrics() -> Vec<Box<dyn InfraMetric>> {
    vec![
        Box::new(payload_encoding::PayloadEncoding),
        Box::new(ast_mutation_analysis::AstMutationAnalysis),
        Box::new(ir_mutation_analysis::IrMutationAnalysis),
        Box::new(binary_mutation_analysis::BinaryMutationAnalysis),
        Box::new(template_assembly_analysis::TemplateAssemblyAnalysis),
        Box::new(instrumentation_analysis::InstrumentationAnalysis),
        Box::new(token_extraction_analysis::TokenExtractionAnalysis),
        Box::new(token_scoring_validation::TokenScoringValidation),
        Box::new(input_diversity_analysis::InputDiversityAnalysis),
        Box::new(oracle_stability_analysis::OracleStabilityAnalysis),
        Box::new(selector_comparison_analysis::SelectorComparisonAnalysis),
        Box::new(guidance_utilization_analysis::GuidanceUtilizationAnalysis),
        Box::new(convergence_simulation_analysis::ConvergenceSimulationAnalysis),
        Box::new(line_tracing_analysis::LineTracingAnalysis),
        Box::new(sc_checkpoint_analysis::ScCheckpointAnalysis),
    ]
}
