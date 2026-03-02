//! Load an EvalDataset JSON, run all enabled metrics, output JSON + CSV reports.
//!
//! Usage:
//!   cargo run -p evaluation --features full --bin evaluate -- [OPTIONS]
//!
//! Options:
//!   --input <PATH>       Input EvalDataset JSON (default: eval_dataset.json)
//!   --json <PATH>        Output JSON report path (default: eval_report.json)
//!   --csv <PATH>         Output CSV report path (default: eval_metrics.csv)
//!   --summary <PATH>     Output summary JSON grouped by axis (optional)
//!   --axis <NAME>        Only run metrics for this axis: input, oracle, guidance (default: all)
//!   --quiet              Suppress metric output to stderr

use evaluation::fixtures::loader::load_dataset;
use evaluation::report::csv_report::{format_csv, write_csv_report};
use evaluation::report::json_report::{write_json_report, write_summary_report};
use evaluation::{MetricResult, run_evaluation};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let input = get_arg(&args, "--input").unwrap_or("eval_dataset.json".to_string());
    let json_out = get_arg(&args, "--json").unwrap_or("eval_report.json".to_string());
    let csv_out = get_arg(&args, "--csv").unwrap_or("eval_metrics.csv".to_string());
    let summary_out = get_arg(&args, "--summary");
    let axis_filter = get_arg(&args, "--axis");
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

    // Run evaluation
    let all_results = run_evaluation(&dataset);

    // Filter by axis if requested
    let results: Vec<MetricResult> = if let Some(ref axis) = axis_filter {
        all_results
            .into_iter()
            .filter(|r| r.axis == *axis)
            .collect()
    } else {
        all_results
    };

    // Print to stderr
    if !quiet {
        eprintln!("\n{:-<70}", "");
        eprintln!("  EVALUATION RESULTS ({})", dataset.job_id);
        eprintln!("{:-<70}", "");

        let mut current_axis = String::new();
        for r in &results {
            if r.axis != current_axis {
                current_axis = r.axis.clone();
                eprintln!("\n  [{} axis]", current_axis.to_uppercase());
            }
            eprintln!("    {:<55} {:>8.4}  (n={})", r.metric_id, r.value, r.n);
        }
        eprintln!("\n{:-<70}", "");
        eprintln!("  Total: {} metrics", results.len());
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

    // Write summary if requested
    if let Some(ref path) = summary_out {
        write_summary_report(&results, &dataset.job_id, Path::new(path))?;
        if !quiet {
            eprintln!("Summary:     {}", path);
        }
    }

    // Also print CSV to stdout for piping
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
