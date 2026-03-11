//! Component-level academic evaluation runner.
//!
//! Runs all offline analysis experiments (C1–C5, B2–B3) on an EvalDataset JSON
//! and produces JSON + CSV reports suitable for thesis figures.
//!
//! Usage:
//!   cargo run -p evaluation --features full --bin component-eval -- [OPTIONS]
//!
//! Options:
//!   --input <PATH>       Input EvalDataset JSON (default: eval_dataset.json)
//!   --output <PATH>      Output JSON report path (default: component_eval_report.json)
//!   --csv <PATH>         Output CSV report path (default: component_eval_metrics.csv)
//!   --experiment <ID>    Only run specific experiment: c1, c3, c4, c5, b2, b3 (default: all)
//!   --quiet              Suppress output to stderr

use evaluation::analysis;
use evaluation::fixtures::loader::load_dataset;
use evaluation::report::csv_report::{format_csv, write_csv_report};
use evaluation::report::json_report::write_json_report;
use evaluation::{EvalMetric, MetricResult};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let input = get_arg(&args, "--input").unwrap_or("eval_dataset.json".to_string());
    let json_out = get_arg(&args, "--output").unwrap_or("component_eval_report.json".to_string());
    let csv_out = get_arg(&args, "--csv").unwrap_or("component_eval_metrics.csv".to_string());
    let experiment_filter = get_arg(&args, "--experiment");
    let quiet = args.iter().any(|a| a == "--quiet");

    // Load dataset
    let dataset = load_dataset(Path::new(&input))?;

    if !quiet {
        eprintln!(
            "Loaded dataset: job_id={}, rounds={}, tokens={}, selections={}",
            dataset.job_id,
            dataset.rounds.len(),
            dataset.token_matrices.len(),
            dataset.selections.len(),
        );
    }

    // Get all analysis metrics
    let all_metrics = analysis::all_analysis_metrics();

    // Filter by experiment ID if requested
    let metrics: Vec<Box<dyn EvalMetric>> = if let Some(ref filter) = experiment_filter {
        let prefix = match filter.as_str() {
            "c1" => "component.c1",
            "c3" => "component.c3",
            "c4" => "component.c4",
            "c5" => "component.c5",
            "b2" => "component.b2",
            "b3" => "component.b3",
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
        eprintln!("Running {} component experiments...", metrics.len());
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
            "  COMPONENT EVALUATION: {} metrics from {} experiments",
            results.len(),
            metrics.len()
        );
        eprintln!("{:-<70}", "");
    }

    // Write JSON report (includes full details for plotting)
    write_json_report(&results, Path::new(&json_out))?;
    if !quiet {
        eprintln!("JSON report: {}", json_out);
    }

    // Write CSV report (summary values)
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
