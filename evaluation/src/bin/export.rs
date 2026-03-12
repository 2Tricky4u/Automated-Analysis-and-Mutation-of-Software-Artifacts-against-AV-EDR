//! Generate an EvalDataset JSON — from synthetic scenarios or live ElasticSearch data.
//!
//! Usage:
//!   cargo run -p evaluation --bin eval-export -- [OPTIONS]
//!
//! Options:
//!   --source <MODE>     Data source: "synthetic" (default) or "es"
//!   --scenario <NAME>   Scenario: random, improvement, plateau, detected, evasion (default: random)
//!   --rounds <N>        Number of rounds for synthetic mode (default: 30)
//!   --seed <N>          RNG seed for random scenario (default: 42)
//!   --output <PATH>     Output JSON path (default: eval_dataset.json)
//!   --enriched          Include synthetic telemetry tokens in the token matrix
//!   --es-url <URL>      ElasticSearch endpoint (default: http://localhost:9200)
//!   --job-id <ID>       Job ID to export from ES (required when --source es)

use evaluation::fixtures::round_factory::RoundSequenceBuilder;
use evaluation::fixtures::token_factory::{build_enriched_token_matrix, build_token_matrix};
use evaluation::{EvalDataset, ModuleSelectionSpec, SelectionRecord};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let source = get_arg(&args, "--source").unwrap_or("synthetic".to_string());
    let output = get_arg(&args, "--output").unwrap_or("eval_dataset.json".to_string());

    let dataset = match source.as_str() {
        "es" => export_from_es(&args)?,
        _ => export_synthetic(&args)?,
    };

    evaluation::fixtures::loader::save_dataset(&dataset, Path::new(&output))?;

    let file_size = std::fs::metadata(&output)?.len();

    match source.as_str() {
        "es" => {
            let job_id = &dataset.job_id;
            let n_rounds = dataset.rounds.len();
            let n_tokens = dataset.token_matrices.len();
            let n_selections = dataset.selections.len();

            eprintln!(
                "Exported {} rounds from ES job {} to {} ({:.1} KB)",
                n_rounds,
                job_id,
                output,
                file_size as f64 / 1024.0
            );
            eprintln!("  rounds:           {}", n_rounds);
            eprintln!(
                "  token_matrices:   {} (from tokens-* index, full telemetry tokens included)",
                n_tokens
            );
            eprintln!(
                "  selections:       {} (stub — rationale/avoid/seek not persisted to ES)",
                n_selections
            );
            eprintln!("  telemetry_tokens: None");

            if n_tokens != n_rounds {
                eprintln!(
                    "  WARNING: token_matrices count ({}) differs from rounds count ({}) — \
                     indicates incomplete token extraction",
                    n_tokens, n_rounds
                );
            }
        }
        _ => {
            let scenario = get_arg(&args, "--scenario").unwrap_or("random".to_string());
            eprintln!(
                "Exported {} rounds ({} scenario) to {} ({:.1} KB)",
                dataset.rounds.len(),
                scenario,
                output,
                file_size as f64 / 1024.0
            );
        }
    }

    Ok(())
}

/// Export from ElasticSearch using async reqwest.
fn export_from_es(args: &[String]) -> anyhow::Result<EvalDataset> {
    let es_url = get_arg(args, "--es-url").unwrap_or("http://localhost:9200".to_string());
    let job_id = get_arg(args, "--job-id")
        .ok_or_else(|| anyhow::anyhow!("--job-id is required when --source es"))?;

    let rt = tokio::runtime::Runtime::new()?;
    let dataset = rt.block_on(evaluation::es_export::fetch_eval_dataset(&es_url, &job_id))?;

    Ok(dataset)
}

/// Export synthetic data from scenario builders.
fn export_synthetic(args: &[String]) -> anyhow::Result<EvalDataset> {
    let scenario = get_arg(args, "--scenario").unwrap_or("random".to_string());
    let n: usize = get_arg(args, "--rounds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let seed: u32 = get_arg(args, "--seed")
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);
    let enriched = args.iter().any(|a| a == "--enriched");

    let mut builder = RoundSequenceBuilder::new();
    match scenario.as_str() {
        "improvement" => {
            builder.improvement_scenario(n);
        }
        "plateau" => {
            builder.plateau_scenario(n);
        }
        "detected" => {
            builder.all_detected(n);
        }
        "evasion" => {
            builder.all_evasion(n);
        }
        _ => {
            builder.random_rounds(n, seed);
        }
    };
    let rounds = builder.build();

    let token_matrices = if enriched {
        build_enriched_token_matrix(&rounds)
    } else {
        build_token_matrix(&rounds)
    };

    let selections = build_sample_selections(&rounds);

    Ok(EvalDataset {
        job_id: format!("eval-{}-{}", scenario, n),
        rounds,
        selections,
        token_matrices,
        telemetry_tokens: None,
    })
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn build_sample_selections(rounds: &[evaluation::RoundSummary]) -> Vec<SelectionRecord> {
    rounds
        .iter()
        .map(|r| {
            let rationale = if r.round_number % 3 == 0 {
                "Exploit best config from prior rounds".to_string()
            } else if r.round_number % 3 == 1 {
                "Explore: epsilon-random new variant".to_string()
            } else {
                "Repeat top performer with mutation variation".to_string()
            };

            SelectionRecord {
                round_number: r.round_number,
                rationale,
                modules: ModuleSelectionSpec::default(),
                mutations: r.mutations.clone(),
                avoid_tokens: if r.round_number > 5 {
                    vec!["api_arg:NtProtectVirtualMemory:protect=R-X".to_string()]
                } else {
                    vec![]
                },
                seek_tokens: if r.round_number > 5 {
                    vec!["module:carrier=peb_walk".to_string()]
                } else {
                    vec![]
                },
            }
        })
        .collect()
}
