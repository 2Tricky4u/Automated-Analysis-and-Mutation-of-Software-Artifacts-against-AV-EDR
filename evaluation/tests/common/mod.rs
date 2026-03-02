//! Shared test fixtures for evaluation integration tests.

use evaluation::fixtures::round_factory::RoundSequenceBuilder;
use evaluation::fixtures::token_factory::{build_enriched_token_matrix, build_token_matrix};
use evaluation::{EvalDataset, ModuleSelectionSpec, SelectionRecord};

/// Build a standard test dataset with mixed outcomes.
pub fn mixed_dataset() -> EvalDataset {
    let mut b = RoundSequenceBuilder::new();
    b.random_rounds(30, 42);
    let rounds = b.build();
    let token_matrices = build_enriched_token_matrix(&rounds);

    EvalDataset {
        job_id: "test-mixed".to_string(),
        rounds,
        selections: vec![],
        token_matrices,
        telemetry_tokens: None,
    }
}

/// Build a dataset with improvement trajectory.
pub fn improvement_dataset() -> EvalDataset {
    let mut b = RoundSequenceBuilder::new();
    b.improvement_scenario(30);
    let rounds = b.build();
    let token_matrices = build_enriched_token_matrix(&rounds);

    EvalDataset {
        job_id: "test-improvement".to_string(),
        rounds,
        selections: sample_selections(30),
        token_matrices,
        telemetry_tokens: None,
    }
}

/// Build a dataset with plateau behavior.
pub fn plateau_dataset() -> EvalDataset {
    let mut b = RoundSequenceBuilder::new();
    b.plateau_scenario(30);
    let rounds = b.build();
    let token_matrices = build_token_matrix(&rounds);

    EvalDataset {
        job_id: "test-plateau".to_string(),
        rounds,
        selections: vec![],
        token_matrices,
        telemetry_tokens: None,
    }
}

/// Build a worst-case dataset (all detected).
pub fn all_detected_dataset() -> EvalDataset {
    let mut b = RoundSequenceBuilder::new();
    b.all_detected(20);
    let rounds = b.build();
    let token_matrices = build_token_matrix(&rounds);

    EvalDataset {
        job_id: "test-all-detected".to_string(),
        rounds,
        selections: vec![],
        token_matrices,
        telemetry_tokens: None,
    }
}

/// Build a best-case dataset (all evasion).
pub fn all_evasion_dataset() -> EvalDataset {
    let mut b = RoundSequenceBuilder::new();
    b.all_evasion(20);
    let rounds = b.build();
    let token_matrices = build_token_matrix(&rounds);

    EvalDataset {
        job_id: "test-all-evasion".to_string(),
        rounds,
        selections: vec![],
        token_matrices,
        telemetry_tokens: None,
    }
}

fn sample_selections(n: usize) -> Vec<SelectionRecord> {
    (1..=n as u32)
        .map(|i| {
            let rationale = if i % 3 == 0 {
                "Exploit best config from prior rounds".to_string()
            } else if i % 3 == 1 {
                "Explore: epsilon-random new variant".to_string()
            } else {
                "Repeat top performer with mutation variation".to_string()
            };

            SelectionRecord {
                round_number: i,
                rationale,
                modules: ModuleSelectionSpec::default(),
                mutations: vec!["ast.fill_pattern".to_string()],
                avoid_tokens: if i > 5 {
                    vec!["api_arg:NtProtectVirtualMemory:protect=R-X".to_string()]
                } else {
                    vec![]
                },
                seek_tokens: if i > 5 {
                    vec!["module:carrier=peb_walk".to_string()]
                } else {
                    vec![]
                },
            }
        })
        .collect()
}
