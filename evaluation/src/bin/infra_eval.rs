//! Infrastructure-level evaluation runner.
//!
//! Reads InfraEvalDataset JSON, runs I1–I13 analysis modules, and writes
//! JSON + CSV reports suitable for thesis figures.
//!
//! Usage:
//!   cargo run -p evaluation --bin infra-eval -- [OPTIONS]
//!
//! Options:
//!   --input <PATH>       Input InfraEvalDataset JSON (default: infra_dataset.json)
//!   --output <PATH>      Output JSON report path (default: infra_eval_report.json)
//!   --csv <PATH>         Output CSV report path (default: infra_eval_metrics.csv)
//!   --experiment <ID>    Only run specific experiment: i1–i13 (default: all)
//!   --quiet              Suppress output to stderr

use evaluation::report::csv_report::{format_csv, write_csv_report};
use evaluation::report::json_report::write_json_report;
use evaluation::{InfraEvalDataset, InfraMetric, MetricResult};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let input = get_arg(&args, "--input").unwrap_or_else(|| "infra_dataset.json".to_string());
    let json_out =
        get_arg(&args, "--output").unwrap_or_else(|| "infra_eval_report.json".to_string());
    let csv_out = get_arg(&args, "--csv").unwrap_or_else(|| "infra_eval_metrics.csv".to_string());
    let experiment_filter = get_arg(&args, "--experiment");
    let quiet = args.iter().any(|a| a == "--quiet");

    // Load dataset
    let content = std::fs::read_to_string(&input)?;
    let dataset: InfraEvalDataset = serde_json::from_str(&content)?;

    if !quiet {
        eprintln!("Loaded InfraEvalDataset from {}", input);
        eprintln!(
            "  payload_encoding: {}",
            dataset.payload_encoding.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  ast_mutation:      {}",
            dataset.ast_mutation.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  ir_mutation:       {}",
            dataset.ir_mutation.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  binary_mutation:   {}",
            dataset.binary_mutation.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  template_assembly: {}",
            dataset.template_assembly.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  instrumentation:   {}",
            dataset.instrumentation.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  token_extraction:  {}",
            dataset.token_extraction.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  token_scoring:     {}",
            dataset.token_scoring.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  input_diversity:   {}",
            dataset.input_diversity.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  oracle_stability:  {}",
            dataset.oracle_stability.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  selector_cmp:      {}",
            dataset.selector_comparison.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  guidance_util:     {}",
            dataset.guidance_utilization.as_ref().map_or(0, |v| v.len())
        );
        eprintln!(
            "  convergence_sim:   {}",
            dataset
                .convergence_simulation
                .as_ref()
                .map_or(0, |v| v.len())
        );
    }

    // Get all infrastructure metrics
    let all_metrics = evaluation::all_infra_metrics();

    // Filter by experiment ID if requested
    let metrics: Vec<Box<dyn InfraMetric>> = if let Some(ref filter) = experiment_filter {
        let prefix = match filter.as_str() {
            "i1" => "infra.i1",
            "i2" => "infra.i2",
            "i3" => "infra.i3",
            "i4" => "infra.i4",
            "i5" => "infra.i5",
            "i6" => "infra.i6",
            "i7" => "infra.i7",
            "i8" => "infra.i8",
            "i9" => "infra.i9",
            "i10" => "infra.i10",
            "i11" => "infra.i11",
            "i12" => "infra.i12",
            "i13" => "infra.i13",
            other => other,
        };
        all_metrics
            .into_iter()
            .filter(|m| m.metric_id().starts_with(prefix))
            .collect()
    } else {
        all_metrics
    };

    if !quiet {
        eprintln!("\nRunning {} infrastructure experiments...", metrics.len());
    }

    // Run metrics
    let mut results: Vec<MetricResult> = Vec::new();
    for metric in &metrics {
        match metric.evaluate(&dataset) {
            Ok(metric_results) => {
                if !quiet {
                    for r in &metric_results {
                        eprintln!("  {:<60} {:>8.4}  (n={})", r.metric_id, r.value, r.n);
                    }
                }
                results.extend(metric_results);
            }
            Err(e) => {
                if !quiet {
                    eprintln!("  ERROR in {}: {}", metric.metric_id(), e);
                }
            }
        }
    }

    if !quiet {
        eprintln!("\n{:-<70}", "");
        eprintln!(
            "  INFRASTRUCTURE EVALUATION: {} metrics from {} experiments",
            results.len(),
            metrics.len()
        );
        eprintln!("{:-<70}", "");
    }

    // Write JSON report
    write_json_report(&results, Path::new(&json_out))?;
    if !quiet {
        eprintln!("JSON report: {}", json_out);
    }

    // Write CSV report
    write_csv_report(&results, Path::new(&csv_out))?;
    if !quiet {
        eprintln!("CSV report:  {}", csv_out);
    }

    // Print CSV to stdout if quiet mode
    if quiet {
        print!("{}", format_csv(&results));
    }

    Ok(())
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
