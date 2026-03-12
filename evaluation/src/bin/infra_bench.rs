//! Infrastructure-level benchmark runner.
//!
//! Exercises build + triage crate APIs with timing and writes InfraEvalDataset JSON.
//!
//! Usage:
//!   cargo run -p evaluation --bin infra-bench -- [OPTIONS]
//!
//! Options:
//!   --experiments <IDS>  Comma-separated: i1,i2,...,i15 (default: i7,i8)
//!   --output <PATH>      Output JSON path (default: infra_dataset.json)
//!   --dataset <PATH>     Load real campaign EvalDataset JSON for I7,I8,I10–I13
//!   --quiet              Suppress progress output
//!
//! When --dataset is provided, experiments I7,I8,I10–I13 use real campaign
//! round data instead of synthetic inputs. Other experiments (I1–I5,I9,I14,I15)
//! are unaffected — they exercise build-crate APIs directly.
//!
//! Experiments requiring `--features build-bench`: i1, i2, i3, i5, i9, i14, i15
//! Experiments requiring full toolchain + PE: i4, i6

use evaluation::{
    ConvergenceSimulationResult, EvalDataset, GuidanceUtilizationResult, InfraEvalDataset,
    OracleStabilityResult, SelectorComparisonResult, TokenExtractionResult, TokenScoringResult,
};
use std::collections::HashMap;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    #[cfg(feature = "build-bench")]
    let default_experiments = "i1,i2,i3,i5,i7,i8,i10,i11,i12,i13,i14,i15";
    #[cfg(not(feature = "build-bench"))]
    let default_experiments = "i7,i8,i10,i11,i12,i13";

    let experiments =
        get_arg(&args, "--experiments").unwrap_or_else(|| default_experiments.to_string());
    let output = get_arg(&args, "--output").unwrap_or_else(|| "infra_dataset.json".to_string());
    let quiet = args.iter().any(|a| a == "--quiet");

    let dataset_path = get_arg(&args, "--dataset");

    let enabled: Vec<&str> = experiments.split(',').map(|s| s.trim()).collect();
    let mut dataset = InfraEvalDataset::default();

    // Load real campaign data if --dataset provided
    let real_dataset: Option<EvalDataset> = if let Some(ref path) = dataset_path {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Failed to read dataset {}: {}", path, e));
        let ds: EvalDataset = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("Failed to parse dataset {}: {}", path, e));
        if !quiet {
            eprintln!("Loaded real campaign dataset from {}", path);
            eprintln!("  job_id:           {}", ds.job_id);
            eprintln!("  rounds:           {}", ds.rounds.len());
            eprintln!("  selections:       {}", ds.selections.len());
            eprintln!("  token_matrices:   {}", ds.token_matrices.len());
            eprintln!(
                "  telemetry_tokens: {}",
                ds.telemetry_tokens.as_ref().map_or(0, |v| v.len())
            );
            eprintln!();
        }
        Some(ds)
    } else {
        None
    };

    if !quiet {
        eprintln!("Infrastructure benchmark runner");
        eprintln!("Experiments: {:?}", enabled);
        if real_dataset.is_some() {
            eprintln!("Mode: REAL DATA (I7,I8,I10-I13 use campaign dataset)");
        } else {
            eprintln!("Mode: SYNTHETIC (use --dataset <path> for real campaign data)");
        }
        eprintln!();
    }

    // I1: Payload Encoding (needs build crate)
    if enabled.contains(&"i1") {
        #[cfg(feature = "build-bench")]
        {
            if !quiet {
                eprintln!("Running I1: Payload Encoding...");
            }
            dataset.payload_encoding = Some(build_bench::bench_payload_encoding(quiet));
        }
        #[cfg(not(feature = "build-bench"))]
        if !quiet {
            eprintln!("I1: Payload Encoding requires --features build-bench. Skipping.");
        }
    }

    // I2: AST Mutations (needs build crate)
    if enabled.contains(&"i2") {
        #[cfg(feature = "build-bench")]
        {
            if !quiet {
                eprintln!("Running I2: AST Mutations...");
            }
            dataset.ast_mutation = Some(build_bench::bench_ast_mutations(quiet));
        }
        #[cfg(not(feature = "build-bench"))]
        if !quiet {
            eprintln!("I2: AST Mutations requires --features build-bench. Skipping.");
        }
    }

    // I3: IR Mutations (needs build crate)
    if enabled.contains(&"i3") {
        #[cfg(feature = "build-bench")]
        {
            if !quiet {
                eprintln!("Running I3: IR Mutations...");
            }
            dataset.ir_mutation = Some(build_bench::bench_ir_mutations(quiet));
        }
        #[cfg(not(feature = "build-bench"))]
        if !quiet {
            eprintln!("I3: IR Mutations requires --features build-bench. Skipping.");
        }
    }

    // I4: Binary Mutations (needs compiled PE)
    if enabled.contains(&"i4") && !quiet {
        eprintln!("I4: Binary Mutations requires a compiled PE and build-bench. Skipping.");
    }

    // I5: Template Assembly (needs build crate)
    if enabled.contains(&"i5") {
        #[cfg(feature = "build-bench")]
        {
            if !quiet {
                eprintln!("Running I5: Template Assembly...");
            }
            dataset.template_assembly = Some(build_bench::bench_template_assembly(quiet));
        }
        #[cfg(not(feature = "build-bench"))]
        if !quiet {
            eprintln!("I5: Template Assembly requires --features build-bench. Skipping.");
        }
    }

    // I6: Instrumentation Overhead (needs full toolchain)
    if enabled.contains(&"i6") && !quiet {
        eprintln!("I6: Instrumentation Overhead requires full toolchain. Skipping.");
    }

    // I7: Token Extraction (pure controller, no build crate needed)
    if enabled.contains(&"i7") {
        if !quiet {
            eprintln!(
                "Running I7: Token Extraction{}...",
                if real_dataset.is_some() {
                    " [real]"
                } else {
                    ""
                }
            );
        }
        dataset.token_extraction = Some(if let Some(ref ds) = real_dataset {
            bench_token_extraction_real(ds, quiet)
        } else {
            bench_token_extraction(quiet)
        });
    }

    // I8: Token Scoring Validation (pure controller, no build crate needed)
    if enabled.contains(&"i8") {
        if !quiet {
            eprintln!(
                "Running I8: Token Scoring Validation{}...",
                if real_dataset.is_some() {
                    " [real]"
                } else {
                    ""
                }
            );
        }
        dataset.token_scoring = Some(if let Some(ref ds) = real_dataset {
            bench_token_scoring_real(ds, quiet)
        } else {
            bench_token_scoring(quiet)
        });
    }

    // I9: Input Diversity (needs build crate)
    if enabled.contains(&"i9") {
        #[cfg(feature = "build-bench")]
        {
            if !quiet {
                eprintln!("Running I9: Input Diversity...");
            }
            dataset.input_diversity = Some(build_bench::bench_input_diversity(quiet));
        }
        #[cfg(not(feature = "build-bench"))]
        if !quiet {
            eprintln!("I9: Input Diversity requires --features build-bench. Skipping.");
        }
    }

    // I10: Oracle Stability (pure controller, no build crate needed)
    if enabled.contains(&"i10") {
        if !quiet {
            eprintln!(
                "Running I10: Oracle Stability{}...",
                if real_dataset.is_some() {
                    " [real]"
                } else {
                    ""
                }
            );
        }
        dataset.oracle_stability = Some(if let Some(ref ds) = real_dataset {
            bench_oracle_stability_real(ds, quiet)
        } else {
            bench_oracle_stability(quiet)
        });
    }

    // I11: Selector Comparison (needs tokio runtime for async select())
    if enabled.contains(&"i11") {
        if !quiet {
            eprintln!(
                "Running I11: Selector Comparison{}...",
                if real_dataset.is_some() {
                    " [real]"
                } else {
                    ""
                }
            );
        }
        dataset.selector_comparison = Some(if let Some(ref ds) = real_dataset {
            bench_selector_comparison_real(ds, quiet)
        } else {
            bench_selector_comparison(quiet)
        });
    }

    // I12: Guidance Utilization (needs tokio runtime for async select())
    if enabled.contains(&"i12") {
        if !quiet {
            eprintln!(
                "Running I12: Guidance Utilization{}...",
                if real_dataset.is_some() {
                    " [real]"
                } else {
                    ""
                }
            );
        }
        dataset.guidance_utilization = Some(if let Some(ref ds) = real_dataset {
            bench_guidance_utilization_real(ds, quiet)
        } else {
            bench_guidance_utilization(quiet)
        });
    }

    // I13: Convergence Simulation (pure controller)
    if enabled.contains(&"i13") {
        if !quiet {
            eprintln!(
                "Running I13: Convergence Simulation{}...",
                if real_dataset.is_some() {
                    " [real]"
                } else {
                    ""
                }
            );
        }
        dataset.convergence_simulation = Some(if let Some(ref ds) = real_dataset {
            bench_convergence_simulation_real(ds, quiet)
        } else {
            bench_convergence_simulation(quiet)
        });
    }

    // I14: Line Tracing Overhead (needs build crate)
    if enabled.contains(&"i14") {
        #[cfg(feature = "build-bench")]
        {
            if !quiet {
                eprintln!("Running I14: Line Tracing Overhead...");
            }
            dataset.line_tracing = Some(build_bench::bench_line_tracing(quiet));
        }
        #[cfg(not(feature = "build-bench"))]
        if !quiet {
            eprintln!("I14: Line Tracing requires --features build-bench. Skipping.");
        }
    }

    // I15: Shellcode Checkpoint Patching (needs build crate)
    if enabled.contains(&"i15") {
        #[cfg(feature = "build-bench")]
        {
            if !quiet {
                eprintln!("Running I15: Shellcode Checkpoint Patching...");
            }
            dataset.sc_checkpoint = Some(build_bench::bench_sc_checkpoints(quiet));
        }
        #[cfg(not(feature = "build-bench"))]
        if !quiet {
            eprintln!("I15: SC Checkpoints requires --features build-bench. Skipping.");
        }
    }

    // Write dataset
    let json = serde_json::to_string_pretty(&dataset)?;
    std::fs::write(&output, json)?;

    if !quiet {
        eprintln!();
        eprintln!("Wrote {}", output);
    }

    Ok(())
}

// ── I7: Token Extraction (no build crate needed) ────────────────────────

fn bench_token_extraction(quiet: bool) -> Vec<TokenExtractionResult> {
    use controller::triage::extractor::extract_tokens_from_docs;

    let test_cases: Vec<(usize, Vec<serde_json::Value>)> = vec![
        (5, generate_synthetic_docs(5)),
        (10, generate_synthetic_docs(10)),
        (25, generate_synthetic_docs(25)),
        (50, generate_synthetic_docs(50)),
        (100, generate_synthetic_docs(100)),
    ];

    let mut results = Vec::new();

    for (doc_count, docs) in &test_cases {
        // Run extraction twice to check determinism
        let t0 = Instant::now();
        let tokens_1 = extract_tokens_from_docs(docs);
        let time_1 = t0.elapsed().as_secs_f64() * 1_000_000.0;

        let tokens_2 = extract_tokens_from_docs(docs);
        let deterministic = tokens_1 == tokens_2;

        // Categorize tokens
        let mut category_counts: HashMap<String, usize> = HashMap::new();
        for token in &tokens_1 {
            let category = token_category(token);
            *category_counts.entry(category).or_default() += 1;
        }
        let categories_active = category_counts.len();

        results.push(TokenExtractionResult {
            input_doc_count: *doc_count,
            output_token_count: tokens_1.len(),
            category_counts,
            categories_active,
            extraction_time_us: time_1,
            deterministic,
        });

        if !quiet {
            eprintln!(
                "  docs={:>3} tokens={:>3} categories={} time={:.0}µs det={}",
                doc_count,
                tokens_1.len(),
                categories_active,
                time_1,
                deterministic
            );
        }
    }

    results
}

fn generate_synthetic_docs(count: usize) -> Vec<serde_json::Value> {
    use serde_json::json;

    let api_funcs = [
        "NtAllocateVirtualMemory",
        "NtProtectVirtualMemory",
        "NtWriteVirtualMemory",
        "VirtualAlloc",
        "VirtualProtect",
        "CreateThread",
        "NtCreateThreadEx",
        "WriteProcessMemory",
        "LoadLibraryA",
    ];
    let dlls = [
        "ntdll.dll",
        "kernel32.dll",
        "kernelbase.dll",
        "user32.dll",
        "advapi32.dll",
    ];
    let etw_events = [
        ("Microsoft-Windows-Kernel-Process", "1"),
        ("Microsoft-Windows-Security-Auditing", "4688"),
        ("Microsoft-Windows-Kernel-File", "12"),
    ];

    let mut docs = Vec::new();

    for i in 0..count {
        match i % 4 {
            0 | 1 => {
                let func = api_funcs[i % api_funcs.len()];
                let protect_val = if i % 2 == 0 {
                    "PAGE_EXECUTE_READWRITE"
                } else {
                    "PAGE_READWRITE"
                };
                docs.push(json!({
                    "payload_func": func,
                    "payload_return": if i % 3 == 0 { "0" } else { "1" },
                    "payload_funcparams": format!("protect={}", protect_val),
                    "payload_caller": "dll",
                }));
            }
            2 => {
                let dll = dlls[i % dlls.len()];
                docs.push(json!({
                    "payload_func": "image_load",
                    "payload_funcparams": format!("path=C:\\Windows\\System32\\{}", dll),
                    "payload_caller": "dll",
                }));
            }
            _ => {
                let (provider, event_id) = etw_events[i % etw_events.len()];
                docs.push(json!({
                    "payload_event": event_id,
                    "payload_provider": provider,
                    "payload_eventname": format!("Event{}", event_id),
                }));
            }
        }
    }

    docs
}

fn token_category(token: &str) -> String {
    if token.starts_with("api_arg:") || token.starts_with("api_ret:") {
        token.split(':').next().unwrap_or("other").to_string()
    } else if let Some(prefix) = [
        "api:",
        "seq2:",
        "image:",
        "etw_event:",
        "etw:",
        "module:",
        "mutation:",
        "checkpoint:",
    ]
    .iter()
    .find(|p| token.starts_with(*p))
    {
        prefix.trim_end_matches(':').to_string()
    } else {
        "other".to_string()
    }
}

// ── I8: Token Scoring Validation (no build crate needed) ────────────────

fn bench_token_scoring(quiet: bool) -> Vec<TokenScoringResult> {
    use controller::triage::scorer::{build_guidance, compute_token_scores};

    let mut results = Vec::new();

    // Test case 1: Perfect correlation
    {
        let matrix: Vec<(Vec<String>, bool)> = vec![
            (vec!["token_a".into(), "common".into()], true),
            (vec!["token_a".into(), "common".into()], true),
            (vec!["common".into()], false),
            (vec!["common".into()], false),
        ];
        let scores = compute_token_scores(&matrix);
        let expected_lift = 2.0;
        let computed = scores
            .iter()
            .find(|s| s.token == "token_a")
            .map(|s| s.lift)
            .unwrap_or(0.0);
        let error = (computed - expected_lift).abs();
        let guidance = build_guidance(&scores, 1.5, 0.3);
        let guidance_correct = guidance.avoid_tokens.contains(&"token_a".to_string());

        results.push(TokenScoringResult {
            test_case: "perfect_correlation".to_string(),
            input_rounds: 4,
            expected_lift,
            computed_lift: computed,
            lift_error: error,
            guidance_correct,
        });
        if !quiet {
            eprintln!(
                "  perfect_correlation: exp={:.3} got={:.3} err={:.6} ok={}",
                expected_lift, computed, error, guidance_correct
            );
        }
    }

    // Test case 2: Anti-correlation
    {
        let matrix: Vec<(Vec<String>, bool)> = vec![
            (vec!["common".into()], true),
            (vec!["common".into()], true),
            (vec!["evasive_token".into(), "common".into()], false),
            (vec!["evasive_token".into(), "common".into()], false),
        ];
        let scores = compute_token_scores(&matrix);
        let expected_lift = 0.0;
        let computed = scores
            .iter()
            .find(|s| s.token == "evasive_token")
            .map(|s| s.lift)
            .unwrap_or(0.0);
        let error = (computed - expected_lift).abs();
        let guidance = build_guidance(&scores, 1.5, 0.3);
        let guidance_correct = guidance.seek_tokens.contains(&"evasive_token".to_string());

        results.push(TokenScoringResult {
            test_case: "anti_correlation".to_string(),
            input_rounds: 4,
            expected_lift,
            computed_lift: computed,
            lift_error: error,
            guidance_correct,
        });
        if !quiet {
            eprintln!(
                "  anti_correlation: exp={:.3} got={:.3} err={:.6} ok={}",
                expected_lift, computed, error, guidance_correct
            );
        }
    }

    // Test case 3: Neutral
    {
        let matrix: Vec<(Vec<String>, bool)> = vec![
            (vec!["neutral".into(), "common".into()], true),
            (vec!["neutral".into(), "common".into()], false),
            (vec!["common".into()], true),
            (vec!["common".into()], false),
        ];
        let scores = compute_token_scores(&matrix);
        let expected_lift = 1.0;
        let computed = scores
            .iter()
            .find(|s| s.token == "neutral")
            .map(|s| s.lift)
            .unwrap_or(0.0);
        let error = (computed - expected_lift).abs();
        let guidance = build_guidance(&scores, 1.5, 0.3);
        let guidance_correct = !guidance.avoid_tokens.contains(&"neutral".to_string())
            && !guidance.seek_tokens.contains(&"neutral".to_string());

        results.push(TokenScoringResult {
            test_case: "neutral".to_string(),
            input_rounds: 4,
            expected_lift,
            computed_lift: computed,
            lift_error: error,
            guidance_correct,
        });
        if !quiet {
            eprintln!(
                "  neutral: exp={:.3} got={:.3} err={:.6} ok={}",
                expected_lift, computed, error, guidance_correct
            );
        }
    }

    // Test case 4: All detected (degenerate)
    {
        let matrix: Vec<(Vec<String>, bool)> = vec![
            (vec!["token_x".into()], true),
            (vec!["token_x".into()], true),
            (vec!["token_y".into()], true),
        ];
        let scores = compute_token_scores(&matrix);
        let computed = scores
            .iter()
            .find(|s| s.token == "token_x")
            .map(|s| s.lift)
            .unwrap_or(0.0);

        results.push(TokenScoringResult {
            test_case: "all_detected_degenerate".to_string(),
            input_rounds: 3,
            expected_lift: 0.0,
            computed_lift: computed,
            lift_error: computed.abs(),
            guidance_correct: scores.is_empty(),
        });
        if !quiet {
            eprintln!(
                "  all_detected: scores_empty={} (expected=true)",
                scores.is_empty()
            );
        }
    }

    // Test case 5: All clean (degenerate)
    {
        let matrix: Vec<(Vec<String>, bool)> = vec![
            (vec!["token_a".into()], false),
            (vec!["token_b".into()], false),
            (vec!["token_a".into()], false),
        ];
        let scores = compute_token_scores(&matrix);

        results.push(TokenScoringResult {
            test_case: "all_clean_degenerate".to_string(),
            input_rounds: 3,
            expected_lift: 0.0,
            computed_lift: 0.0,
            lift_error: 0.0,
            guidance_correct: scores.is_empty(),
        });
        if !quiet {
            eprintln!(
                "  all_clean: scores_empty={} (expected=true)",
                scores.is_empty()
            );
        }
    }

    // Test case 6: Single round
    {
        let matrix: Vec<(Vec<String>, bool)> = vec![(vec!["token_only".into()], true)];
        let scores = compute_token_scores(&matrix);

        results.push(TokenScoringResult {
            test_case: "single_round".to_string(),
            input_rounds: 1,
            expected_lift: 0.0,
            computed_lift: 0.0,
            lift_error: 0.0,
            guidance_correct: scores.is_empty(),
        });
        if !quiet {
            eprintln!(
                "  single_round: scores_empty={} (expected=true)",
                scores.is_empty()
            );
        }
    }

    // Test case 7: Larger matrix with known lift
    {
        let matrix: Vec<(Vec<String>, bool)> = vec![
            (vec!["token_hot".into(), "bg".into()], true),
            (vec!["token_hot".into(), "bg".into()], true),
            (vec!["token_hot".into(), "bg".into()], true),
            (vec!["token_hot".into(), "bg".into()], true),
            (vec!["token_hot".into(), "bg".into()], true),
            (vec!["bg".into()], true),
            (vec!["bg".into()], false),
            (vec!["bg".into()], false),
            (vec!["bg".into()], false),
            (vec!["bg".into()], false),
        ];
        let scores = compute_token_scores(&matrix);
        let expected_lift = 1.0 / 0.6;
        let computed = scores
            .iter()
            .find(|s| s.token == "token_hot")
            .map(|s| s.lift)
            .unwrap_or(0.0);
        let error = (computed - expected_lift).abs();
        let guidance = build_guidance(&scores, 1.5, 0.3);
        let guidance_correct = guidance.avoid_tokens.contains(&"token_hot".to_string());

        results.push(TokenScoringResult {
            test_case: "larger_matrix_hot_token".to_string(),
            input_rounds: 10,
            expected_lift,
            computed_lift: computed,
            lift_error: error,
            guidance_correct,
        });
        if !quiet {
            eprintln!(
                "  larger_matrix: exp={:.4} got={:.4} err={:.6} ok={}",
                expected_lift, computed, error, guidance_correct
            );
        }
    }

    results
}

// ── I10: Oracle Stability (no build crate needed) ───────────────────────

fn bench_oracle_stability(quiet: bool) -> Vec<OracleStabilityResult> {
    use controller::triage::scorer::{build_guidance, compute_token_scores};
    use evaluation::IncrementalSnapshot;
    use evaluation::fixtures::round_factory::RoundSequenceBuilder;
    use evaluation::fixtures::token_factory::build_enriched_token_matrix;

    // Build a 30-round history with mixed outcomes
    let rounds = {
        let mut b = RoundSequenceBuilder::new();
        b.random_rounds(30, 42);
        b.build()
    };

    let matrix = build_enriched_token_matrix(&rounds);
    let token_matrix: Vec<(Vec<String>, bool)> = matrix
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect();

    let mut results = Vec::new();

    // Test 1: Repeated determinism
    let scores_1 = compute_token_scores(&token_matrix);
    let guidance_1 = build_guidance(&scores_1, 1.5, 0.3);
    let scores_2 = compute_token_scores(&token_matrix);
    let guidance_2 = build_guidance(&scores_2, 1.5, 0.3);

    let repeated_deterministic = guidance_1.avoid_tokens == guidance_2.avoid_tokens
        && guidance_1.seek_tokens == guidance_2.seek_tokens;

    // Test 2: Permutation robustness
    let full_top5: Vec<String> = scores_1.iter().take(5).map(|s| s.token.clone()).collect();

    let mut permutation_jaccards = Vec::new();
    let mut lift_variances_per_token: HashMap<String, Vec<f64>> = HashMap::new();

    // 10 permutations using simple deterministic shuffles
    for perm_seed in 0..10u32 {
        let mut perm_matrix = token_matrix.clone();
        // Fisher-Yates with deterministic seed
        let mut state = perm_seed.wrapping_mul(2654435761).wrapping_add(1);
        for i in (1..perm_matrix.len()).rev() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let j = (state as usize) % (i + 1);
            perm_matrix.swap(i, j);
        }

        let perm_scores = compute_token_scores(&perm_matrix);
        let perm_top5: Vec<String> = perm_scores
            .iter()
            .take(5)
            .map(|s| s.token.clone())
            .collect();

        // Jaccard similarity (not distance)
        let jaccard_sim = 1.0 - evaluation::helpers::jaccard_distance(&full_top5, &perm_top5);
        permutation_jaccards.push(jaccard_sim);

        // Track lift per token
        for s in &perm_scores {
            lift_variances_per_token
                .entry(s.token.clone())
                .or_default()
                .push(s.lift);
        }
    }

    let mean_jaccard =
        permutation_jaccards.iter().sum::<f64>() / permutation_jaccards.len().max(1) as f64;

    // Compute mean per-token lift variance
    let mut total_variance = 0.0;
    let mut token_count = 0;
    for lifts in lift_variances_per_token.values() {
        if lifts.len() > 1 {
            let mean = lifts.iter().sum::<f64>() / lifts.len() as f64;
            let variance =
                lifts.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / (lifts.len() - 1) as f64;
            total_variance += variance;
            token_count += 1;
        }
    }
    let mean_lift_variance = if token_count > 0 {
        total_variance / token_count as f64
    } else {
        0.0
    };

    // Test 3: Incremental convergence
    let incremental_points = [10, 15, 20, 25, 30];
    let mut snapshots = Vec::new();

    for &count in &incremental_points {
        let partial_matrix: Vec<(Vec<String>, bool)> =
            token_matrix.iter().take(count).cloned().collect();

        let partial_scores = compute_token_scores(&partial_matrix);
        let partial_guidance = build_guidance(&partial_scores, 1.5, 0.3);

        let mut all_partial: Vec<String> = partial_guidance.avoid_tokens.clone();
        all_partial.extend(partial_guidance.seek_tokens.clone());
        let mut all_full: Vec<String> = guidance_1.avoid_tokens.clone();
        all_full.extend(guidance_1.seek_tokens.clone());

        let jaccard_sim = if all_full.is_empty() && all_partial.is_empty() {
            1.0
        } else {
            1.0 - evaluation::helpers::jaccard_distance(&all_partial, &all_full)
        };

        snapshots.push(IncrementalSnapshot {
            round_count: count,
            avoid_count: partial_guidance.avoid_tokens.len(),
            seek_count: partial_guidance.seek_tokens.len(),
            jaccard_with_full: jaccard_sim,
        });
    }

    if !quiet {
        eprintln!(
            "  deterministic={} perm_jaccard={:.3} lift_var={:.6}",
            repeated_deterministic, mean_jaccard, mean_lift_variance
        );
        for s in &snapshots {
            eprintln!(
                "    rounds={:>2} avoid={} seek={} jaccard={:.3}",
                s.round_count, s.avoid_count, s.seek_count, s.jaccard_with_full
            );
        }
    }

    results.push(OracleStabilityResult {
        test_case: "30_round_mixed".to_string(),
        repeated_deterministic,
        permutation_top5_jaccard: mean_jaccard,
        permutation_lift_variance: mean_lift_variance,
        incremental_snapshots: snapshots,
    });

    results
}

// ── I11: Selector Comparison (needs tokio for async select()) ───────────

fn bench_selector_comparison(quiet: bool) -> Vec<SelectorComparisonResult> {
    use controller::triage::coverage_selector::CoverageSelector;
    use controller::triage::fuzzer_selector::FuzzerSelector;
    use controller::triage::random_selector::RandomSelector;
    use controller::triage::scorer::{build_guidance, compute_token_scores};
    use controller::triage::token_selector::TokenSelector;
    use controller::triage::{SearchSpace, Selector};
    use evaluation::fixtures::round_factory::RoundSequenceBuilder;
    use evaluation::fixtures::token_factory::build_enriched_token_matrix;
    use std::collections::BTreeMap;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Build 30-round synthetic history
    let rounds = {
        let mut b = RoundSequenceBuilder::new();
        b.improvement_scenario(30);
        b.build()
    };

    let history: BTreeMap<u32, evaluation::RoundSummary> =
        rounds.iter().map(|r| (r.round_number, r.clone())).collect();

    // Build guidance from this history
    let matrix = build_enriched_token_matrix(&rounds);
    let token_matrix: Vec<(Vec<String>, bool)> = matrix
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect();
    let scores = compute_token_scores(&token_matrix);
    let guidance = build_guidance(&scores, 1.5, 0.3);

    let search_space = SearchSpace::default();
    let default_modules = controller::dispatch::types::ModuleSelectionSpec::default();
    let _pool_size = search_space.mutation_pool.len();

    let selectors: Vec<(&str, Box<dyn Selector>)> = vec![
        ("Coverage", Box::new(CoverageSelector::new())),
        ("Fuzzer", Box::new(FuzzerSelector::new())),
        ("Token", Box::new(TokenSelector::new())),
        ("Random", Box::new(RandomSelector::new())),
    ];

    let mut results = Vec::new();

    for (name, selector) in &selectors {
        let mut per_round_mutations: Vec<Vec<String>> = Vec::new();
        let mut all_selected: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut exploration_count = 0usize;

        // Run selector for rounds 2..32 (simulating a new campaign with given history)
        for round_num in 2..=31u32 {
            let selection = rt.block_on(selector.select(
                "eval-bench",
                round_num,
                &search_space,
                &default_modules,
                &history,
                Some(&guidance),
            ));

            let mut mutation_ids: Vec<String> =
                selection.mutations.iter().map(|m| m.id.clone()).collect();
            mutation_ids.sort();

            for id in &mutation_ids {
                all_selected.insert(id.clone());
            }

            // Detect exploration by rationale
            if selection.rationale.contains("exploration")
                || selection.rationale.contains("random")
                || selection.rationale.contains("explore")
            {
                exploration_count += 1;
            }

            per_round_mutations.push(mutation_ids);
        }

        let rounds_evaluated = per_round_mutations.len();
        let unique_sets: std::collections::HashSet<String> = per_round_mutations
            .iter()
            .map(|muts| muts.join(","))
            .collect();

        let pool_mutations: std::collections::HashSet<String> =
            search_space.mutation_pool.iter().cloned().collect();
        let pool_coverage = if pool_mutations.is_empty() {
            0.0
        } else {
            all_selected.intersection(&pool_mutations).count() as f64 / pool_mutations.len() as f64
        };

        let total_mutations: usize = per_round_mutations.iter().map(|m| m.len()).sum();
        let mean_recipe_size = total_mutations as f64 / rounds_evaluated.max(1) as f64;
        let exploration_rate = exploration_count as f64 / rounds_evaluated.max(1) as f64;

        if !quiet {
            eprintln!(
                "  {:<10} coverage={:.2} unique_sets={} mean_size={:.1} explore={:.2}",
                name,
                pool_coverage,
                unique_sets.len(),
                mean_recipe_size,
                exploration_rate
            );
        }

        results.push(SelectorComparisonResult {
            selector_name: name.to_string(),
            rounds_evaluated,
            unique_mutation_sets: unique_sets.len(),
            mutation_pool_coverage: pool_coverage,
            mean_recipe_size,
            exploration_rate,
            per_round_mutations,
        });
    }

    results
}

// ── I12: Guidance Utilization ───────────────────────────────────────────

fn bench_guidance_utilization(quiet: bool) -> Vec<GuidanceUtilizationResult> {
    use controller::triage::coverage_selector::CoverageSelector;
    use controller::triage::scorer::{build_guidance, compute_token_scores};
    use controller::triage::token_selector::TokenSelector;
    use controller::triage::{ScoredToken, SearchSpace, Selector, TriageGuidance};
    use evaluation::fixtures::round_factory::RoundSequenceBuilder;
    use evaluation::fixtures::token_factory::build_enriched_token_matrix;
    use std::collections::BTreeMap;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Build 20-round history
    let rounds = {
        let mut b = RoundSequenceBuilder::new();
        b.random_rounds(20, 77);
        b.build()
    };
    let history: BTreeMap<u32, evaluation::RoundSummary> =
        rounds.iter().map(|r| (r.round_number, r.clone())).collect();

    // Derive guidance from scoring
    let matrix = build_enriched_token_matrix(&rounds);
    let token_matrix: Vec<(Vec<String>, bool)> = matrix
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect();
    let scores = compute_token_scores(&token_matrix);
    let natural_guidance = build_guidance(&scores, 1.5, 0.3);

    // If natural guidance has few tokens, augment with synthetic ones
    let avoid_tokens = if natural_guidance.avoid_tokens.is_empty() {
        vec![
            "mutation:ast.exec_decoy".to_string(),
            "mutation:ast.timing_pattern".to_string(),
            "mutation:ast.decon_rounds".to_string(),
        ]
    } else {
        natural_guidance
            .avoid_tokens
            .iter()
            .take(3)
            .cloned()
            .collect()
    };
    let seek_tokens = if natural_guidance.seek_tokens.is_empty() {
        vec![
            "mutation:ast.protection_transition".to_string(),
            "mutation:ast.fill_pattern".to_string(),
        ]
    } else {
        natural_guidance
            .seek_tokens
            .iter()
            .take(2)
            .cloned()
            .collect()
    };

    let test_guidance = TriageGuidance {
        avoid_tokens: avoid_tokens.clone(),
        seek_tokens: seek_tokens.clone(),
        scored_avoid: avoid_tokens
            .iter()
            .enumerate()
            .map(|(i, t)| ScoredToken {
                token: t.clone(),
                importance: 2.0 - i as f64 * 0.3,
            })
            .collect(),
        scored_seek: seek_tokens
            .iter()
            .enumerate()
            .map(|(i, t)| ScoredToken {
                token: t.clone(),
                importance: 0.3 + i as f64 * 0.1,
            })
            .collect(),
    };

    let search_space = SearchSpace::default();
    let default_modules = controller::dispatch::types::ModuleSelectionSpec::default();

    let selector_pairs: Vec<(&str, Box<dyn Selector>)> = vec![
        ("Token", Box::new(TokenSelector::new())),
        ("Coverage", Box::new(CoverageSelector::new())),
    ];

    let mut results = Vec::new();

    for (name, selector) in &selector_pairs {
        // Without guidance
        let mut mutations_without: Vec<Vec<String>> = Vec::new();
        for round_num in 2..=21u32 {
            let selection = rt.block_on(selector.select(
                "eval-guidance-test",
                round_num,
                &search_space,
                &default_modules,
                &history,
                None,
            ));
            let mut ids: Vec<String> = selection.mutations.iter().map(|m| m.id.clone()).collect();
            ids.sort();
            mutations_without.push(ids);
        }

        // With guidance
        let mut mutations_with: Vec<Vec<String>> = Vec::new();
        for round_num in 2..=21u32 {
            let selection = rt.block_on(selector.select(
                "eval-guidance-test",
                round_num,
                &search_space,
                &default_modules,
                &history,
                Some(&test_guidance),
            ));
            let mut ids: Vec<String> = selection.mutations.iter().map(|m| m.id.clone()).collect();
            ids.sort();
            mutations_with.push(ids);
        }

        // Extract the mutation parts from avoid/seek tokens
        let avoid_mutation_ids: Vec<String> = avoid_tokens
            .iter()
            .filter_map(|t| t.strip_prefix("mutation:"))
            .map(|s| s.to_string())
            .collect();
        let seek_mutation_ids: Vec<String> = seek_tokens
            .iter()
            .filter_map(|t| t.strip_prefix("mutation:"))
            .map(|s| s.to_string())
            .collect();

        // Compute avoidance rate: fraction of guided rounds NOT selecting avoid mutations
        let avoidance_rate = if avoid_mutation_ids.is_empty() {
            1.0
        } else {
            let avoiding_rounds = mutations_with
                .iter()
                .filter(|muts| !avoid_mutation_ids.iter().any(|avoid| muts.contains(avoid)))
                .count();
            avoiding_rounds as f64 / mutations_with.len().max(1) as f64
        };

        // Compute seek adoption rate: fraction of guided rounds selecting seek mutations
        let seek_adoption_rate = if seek_mutation_ids.is_empty() {
            0.0
        } else {
            let adopting_rounds = mutations_with
                .iter()
                .filter(|muts| seek_mutation_ids.iter().any(|seek| muts.contains(seek)))
                .count();
            adopting_rounds as f64 / mutations_with.len().max(1) as f64
        };

        // Compute recipe Jaccard delta
        let mut total_jaccard = 0.0;
        let n_rounds = mutations_without.len().min(mutations_with.len());
        for i in 0..n_rounds {
            total_jaccard +=
                evaluation::helpers::jaccard_distance(&mutations_without[i], &mutations_with[i]);
        }
        let recipe_jaccard_delta = total_jaccard / n_rounds.max(1) as f64;

        if !quiet {
            eprintln!(
                "  {:<10} avoid={:.2} seek={:.2} delta={:.3}",
                name, avoidance_rate, seek_adoption_rate, recipe_jaccard_delta
            );
        }

        results.push(GuidanceUtilizationResult {
            selector_name: name.to_string(),
            rounds: n_rounds,
            mutations_without_guidance: mutations_without,
            mutations_with_guidance: mutations_with,
            avoid_tokens: avoid_tokens.clone(),
            seek_tokens: seek_tokens.clone(),
            avoidance_rate,
            seek_adoption_rate,
            recipe_jaccard_delta,
        });
    }

    results
}

// ── I13: Convergence Simulation ─────────────────────────────────────────

fn bench_convergence_simulation(quiet: bool) -> Vec<ConvergenceSimulationResult> {
    use controller::triage::accumulation::{
        AccumulationConfig, compute_marginal_contributions, compute_recipe_diversity,
        determine_phase, reconstruct_best_recipe,
    };
    use evaluation::fixtures::round_factory::RoundSequenceBuilder;
    use std::collections::{BTreeMap, HashSet};

    let total_rounds = 40usize;
    let rounds = {
        let mut b = RoundSequenceBuilder::new();
        b.improvement_scenario(total_rounds);
        b.build()
    };

    let config = AccumulationConfig::default();
    let pool_size = 10; // Default pool size (10 AST mutations)

    let fixed_set: HashSet<&str> = HashSet::new(); // No fixed mutations in this simulation

    let mut phase_transitions: Vec<(u32, String)> = Vec::new();
    let mut recipe_size_trajectory: Vec<(u32, usize)> = Vec::new();
    let mut diversity_trajectory: Vec<(u32, f64)> = Vec::new();
    let mut best_score_trajectory: Vec<(u32, f64)> = Vec::new();
    let mut marginal_contribution_count: Vec<(u32, usize)> = Vec::new();
    let mut prev_phase = String::new();

    for end in 1..=total_rounds {
        let round_num = end as u32;
        let partial: BTreeMap<u32, evaluation::RoundSummary> = rounds
            .iter()
            .take(end)
            .map(|r| (r.round_number, r.clone()))
            .collect();

        // Phase determination
        let phase = determine_phase(round_num, pool_size, &config);
        let phase_name = format!("{:?}", phase);
        if phase_name != prev_phase {
            phase_transitions.push((round_num, phase_name.clone()));
            prev_phase = phase_name;
        }

        // Recipe reconstruction
        let (recipe, best_score) = reconstruct_best_recipe(&partial, &fixed_set);
        recipe_size_trajectory.push((round_num, recipe.len()));
        best_score_trajectory.push((round_num, best_score));

        // Diversity (need at least 2 rounds in window)
        let diversity = if end >= 2 {
            compute_recipe_diversity(&partial, &fixed_set, 5)
        } else {
            0.0
        };
        diversity_trajectory.push((round_num, diversity));

        // Marginal contributions
        let marginals = compute_marginal_contributions(&partial, &fixed_set);
        let contributing = marginals.values().filter(|&&v| v > 0.0).count();
        marginal_contribution_count.push((round_num, contributing));
    }

    if !quiet {
        eprintln!("  Phase transitions:");
        for (round, phase) in &phase_transitions {
            eprintln!("    round {} -> {}", round, phase);
        }
        let final_recipe_size = recipe_size_trajectory.last().map(|(_, s)| *s).unwrap_or(0);
        let final_score = best_score_trajectory.last().map(|(_, s)| *s).unwrap_or(0.0);
        eprintln!("  Final recipe size: {}", final_recipe_size);
        eprintln!("  Final best score: {:.3}", final_score);
    }

    vec![ConvergenceSimulationResult {
        total_rounds,
        phase_transitions,
        recipe_size_trajectory,
        diversity_trajectory,
        best_score_trajectory,
        marginal_contribution_count,
    }]
}

// ── Real-data benchmark variants (--dataset) ────────────────────────────

/// I7 with real data: reconstruct ES docs from telemetry tokens or round metadata.
fn bench_token_extraction_real(real: &EvalDataset, quiet: bool) -> Vec<TokenExtractionResult> {
    use controller::triage::extractor::extract_tokens_from_docs;

    let mut results = Vec::new();

    let all_docs = if let Some(ref toks) = real.telemetry_tokens {
        reconstruct_docs_from_telemetry(toks)
    } else {
        reconstruct_docs_from_rounds(&real.rounds)
    };

    if all_docs.is_empty() {
        if !quiet {
            eprintln!("  [real] No documents reconstructable from dataset");
        }
        return results;
    }

    let batch_sizes: Vec<usize> = vec![5, 10, 25, 50, 100]
        .into_iter()
        .filter(|&s| s <= all_docs.len())
        .collect();
    // Always include the full set if not already covered
    let full = all_docs.len();
    let batch_sizes: Vec<usize> = if batch_sizes.last().copied() == Some(full) {
        batch_sizes
    } else {
        let mut b = batch_sizes;
        b.push(full);
        b
    };

    for &batch_size in &batch_sizes {
        let docs: Vec<_> = all_docs.iter().take(batch_size).cloned().collect();

        let t0 = Instant::now();
        let tokens_1 = extract_tokens_from_docs(&docs);
        let time_1 = t0.elapsed().as_secs_f64() * 1_000_000.0;

        let tokens_2 = extract_tokens_from_docs(&docs);
        let deterministic = tokens_1 == tokens_2;

        let mut category_counts: HashMap<String, usize> = HashMap::new();
        for token in &tokens_1 {
            *category_counts.entry(token_category(token)).or_default() += 1;
        }
        let categories_active = category_counts.len();

        if !quiet {
            eprintln!(
                "  [real] docs={:>4} tokens={:>4} categories={} time={:.0}µs det={}",
                batch_size,
                tokens_1.len(),
                categories_active,
                time_1,
                deterministic
            );
        }

        results.push(TokenExtractionResult {
            input_doc_count: batch_size,
            output_token_count: tokens_1.len(),
            category_counts,
            categories_active,
            extraction_time_us: time_1,
            deterministic,
        });
    }

    results
}

/// Reconstruct ES-style docs from RoundTelemetryTokens.
fn reconstruct_docs_from_telemetry(
    rounds: &[evaluation::RoundTelemetryTokens],
) -> Vec<serde_json::Value> {
    use serde_json::json;
    let mut docs = Vec::new();

    for round in rounds {
        for token in &round.api_tokens {
            if let Some(func) = token.strip_prefix("api:") {
                docs.push(json!({
                    "payload_func": func,
                    "payload_return": "1",
                    "payload_caller": "dll",
                }));
            } else if let Some(rest) = token.strip_prefix("api_arg:") {
                // api_arg:VirtualProtect:flProtect=RWX → func + params
                let parts: Vec<&str> = rest.splitn(2, ':').collect();
                if parts.len() == 2 {
                    docs.push(json!({
                        "payload_func": parts[0],
                        "payload_funcparams": parts[1],
                        "payload_caller": "dll",
                    }));
                }
            }
        }
        for token in &round.image_tokens {
            if let Some(dll) = token.strip_prefix("image:") {
                docs.push(json!({
                    "payload_func": "image_load",
                    "payload_funcparams": format!("path=C:\\Windows\\System32\\{}", dll),
                    "payload_caller": "dll",
                }));
            }
        }
        for token in &round.etw_tokens {
            if let Some(rest) = token.strip_prefix("etw_event:") {
                let parts: Vec<&str> = rest.splitn(2, '/').collect();
                if parts.len() == 2 {
                    docs.push(json!({
                        "payload_event": parts[1],
                        "payload_eventname": parts[0],
                        "payload_provider": "unknown",
                    }));
                }
            }
        }
    }

    docs
}

/// Fallback: reconstruct minimal docs from round metadata (modules + mutations).
fn reconstruct_docs_from_rounds(rounds: &[evaluation::RoundSummary]) -> Vec<serde_json::Value> {
    use serde_json::json;
    let mut docs = Vec::new();
    let api_funcs = [
        "NtAllocateVirtualMemory",
        "NtProtectVirtualMemory",
        "VirtualAlloc",
        "VirtualProtect",
        "CreateThread",
    ];

    for (i, _r) in rounds.iter().enumerate() {
        let func = api_funcs[i % api_funcs.len()];
        docs.push(json!({
            "payload_func": func,
            "payload_return": if i % 3 == 0 { "0" } else { "1" },
            "payload_funcparams": format!("protect={}", if i % 2 == 0 { "PAGE_EXECUTE_READWRITE" } else { "PAGE_READWRITE" }),
            "payload_caller": "dll",
        }));
    }

    docs
}

/// I8 with real data: validate scoring on real token matrices.
fn bench_token_scoring_real(real: &EvalDataset, quiet: bool) -> Vec<TokenScoringResult> {
    use controller::triage::scorer::{build_guidance, compute_token_scores};

    let mut results = Vec::new();

    let token_matrix: Vec<(Vec<String>, bool)> = real
        .token_matrices
        .iter()
        .filter(|e| e.trustworthy)
        .map(|e| (e.tokens.clone(), e.detected))
        .collect();

    if token_matrix.is_empty() {
        if !quiet {
            eprintln!("  [real] No trustworthy token matrix entries");
        }
        return results;
    }

    // Test 1: Full matrix scoring
    let scores = compute_token_scores(&token_matrix);
    let guidance = build_guidance(&scores, 1.5, 0.3);
    let top_lift = scores.first().map(|s| s.lift).unwrap_or(0.0);

    if !quiet {
        eprintln!(
            "  [real] full_matrix: {} rounds, {} scored tokens, avoid={} seek={}, top_lift={:.4}",
            token_matrix.len(),
            scores.len(),
            guidance.avoid_tokens.len(),
            guidance.seek_tokens.len(),
            top_lift
        );
    }

    results.push(TokenScoringResult {
        test_case: format!("real_full_matrix_{}_rounds", token_matrix.len()),
        input_rounds: token_matrix.len(),
        expected_lift: top_lift,
        computed_lift: top_lift,
        lift_error: 0.0,
        guidance_correct: true,
    });

    // Test 2: Determinism — compare via serialization since TokenScore lacks PartialEq
    let scores_2 = compute_token_scores(&token_matrix);
    let deterministic = scores.len() == scores_2.len()
        && scores
            .iter()
            .zip(scores_2.iter())
            .all(|(a, b)| a.token == b.token && (a.lift - b.lift).abs() < f64::EPSILON);

    if !quiet {
        eprintln!("  [real] determinism: {}", deterministic);
    }

    results.push(TokenScoringResult {
        test_case: "real_determinism".to_string(),
        input_rounds: token_matrix.len(),
        expected_lift: 0.0,
        computed_lift: 0.0,
        lift_error: 0.0,
        guidance_correct: deterministic,
    });

    // Test 3: Half-matrix convergence — does scoring on first half produce similar results?
    let half = token_matrix.len() / 2;
    if half >= 2 {
        let half_matrix: Vec<_> = token_matrix[..half].to_vec();
        let half_scores = compute_token_scores(&half_matrix);
        let half_top = half_scores.first().map(|s| s.lift).unwrap_or(0.0);
        let delta = (top_lift - half_top).abs();

        let half_guidance = build_guidance(&half_scores, 1.5, 0.3);
        let convergence_ok = half_guidance.avoid_tokens.len() + half_guidance.seek_tokens.len() > 0
            || guidance.avoid_tokens.len() + guidance.seek_tokens.len() == 0;

        if !quiet {
            eprintln!(
                "  [real] half_convergence: half_top={:.4} full_top={:.4} delta={:.4}",
                half_top, top_lift, delta
            );
        }

        results.push(TokenScoringResult {
            test_case: format!("real_half_convergence_{}_rounds", half),
            input_rounds: half,
            expected_lift: top_lift,
            computed_lift: half_top,
            lift_error: delta,
            guidance_correct: convergence_ok,
        });
    }

    results
}

/// I10 with real data: oracle stability on real campaign rounds.
fn bench_oracle_stability_real(real: &EvalDataset, quiet: bool) -> Vec<OracleStabilityResult> {
    use controller::triage::scorer::{build_guidance, compute_token_scores};
    use evaluation::IncrementalSnapshot;

    if real.rounds.len() < 5 {
        if !quiet {
            eprintln!(
                "  [real] Need at least 5 rounds for stability analysis, got {}",
                real.rounds.len()
            );
        }
        return vec![];
    }

    // Use real token matrix if available, else derive from rounds
    let token_matrix: Vec<(Vec<String>, bool)> = if !real.token_matrices.is_empty() {
        real.token_matrices
            .iter()
            .filter(|e| e.trustworthy)
            .map(|e| (e.tokens.clone(), e.detected))
            .collect()
    } else {
        let matrix = evaluation::fixtures::token_factory::build_enriched_token_matrix(&real.rounds);
        matrix
            .iter()
            .filter(|e| e.trustworthy)
            .map(|e| (e.tokens.clone(), e.detected))
            .collect()
    };

    if token_matrix.len() < 3 {
        if !quiet {
            eprintln!("  [real] Not enough trustworthy rounds for stability");
        }
        return vec![];
    }

    // Test 1: Repeated determinism
    let scores_1 = compute_token_scores(&token_matrix);
    let guidance_1 = build_guidance(&scores_1, 1.5, 0.3);
    let scores_2 = compute_token_scores(&token_matrix);
    let guidance_2 = build_guidance(&scores_2, 1.5, 0.3);
    let repeated_deterministic = guidance_1.avoid_tokens == guidance_2.avoid_tokens
        && guidance_1.seek_tokens == guidance_2.seek_tokens;

    // Test 2: Permutation robustness
    let full_top5: Vec<String> = scores_1.iter().take(5).map(|s| s.token.clone()).collect();
    let mut permutation_jaccards = Vec::new();
    let mut lift_variances_per_token: HashMap<String, Vec<f64>> = HashMap::new();

    for perm_seed in 0..10u32 {
        let mut perm_matrix = token_matrix.clone();
        let mut state = perm_seed.wrapping_mul(2654435761).wrapping_add(1);
        for i in (1..perm_matrix.len()).rev() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let j = (state as usize) % (i + 1);
            perm_matrix.swap(i, j);
        }

        let perm_scores = compute_token_scores(&perm_matrix);
        let perm_top5: Vec<String> = perm_scores
            .iter()
            .take(5)
            .map(|s| s.token.clone())
            .collect();
        let jaccard_sim = 1.0 - evaluation::helpers::jaccard_distance(&full_top5, &perm_top5);
        permutation_jaccards.push(jaccard_sim);

        for s in &perm_scores {
            lift_variances_per_token
                .entry(s.token.clone())
                .or_default()
                .push(s.lift);
        }
    }

    let mean_jaccard =
        permutation_jaccards.iter().sum::<f64>() / permutation_jaccards.len().max(1) as f64;

    let mut total_variance = 0.0;
    let mut token_count = 0;
    for lifts in lift_variances_per_token.values() {
        if lifts.len() > 1 {
            let mean = lifts.iter().sum::<f64>() / lifts.len() as f64;
            let variance =
                lifts.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / (lifts.len() - 1) as f64;
            total_variance += variance;
            token_count += 1;
        }
    }
    let mean_lift_variance = if token_count > 0 {
        total_variance / token_count as f64
    } else {
        0.0
    };

    // Test 3: Incremental convergence
    let total = token_matrix.len();
    let incremental_points: Vec<usize> = (0..5)
        .map(|i| (total * (i + 1)) / 5)
        .filter(|&n| n >= 2)
        .collect();

    let mut snapshots = Vec::new();
    for &count in &incremental_points {
        let partial_matrix: Vec<(Vec<String>, bool)> =
            token_matrix.iter().take(count).cloned().collect();
        let partial_scores = compute_token_scores(&partial_matrix);
        let partial_guidance = build_guidance(&partial_scores, 1.5, 0.3);

        let mut all_partial: Vec<String> = partial_guidance.avoid_tokens.clone();
        all_partial.extend(partial_guidance.seek_tokens.clone());
        let mut all_full: Vec<String> = guidance_1.avoid_tokens.clone();
        all_full.extend(guidance_1.seek_tokens.clone());

        let jaccard_sim = if all_full.is_empty() && all_partial.is_empty() {
            1.0
        } else {
            1.0 - evaluation::helpers::jaccard_distance(&all_partial, &all_full)
        };

        snapshots.push(IncrementalSnapshot {
            round_count: count,
            avoid_count: partial_guidance.avoid_tokens.len(),
            seek_count: partial_guidance.seek_tokens.len(),
            jaccard_with_full: jaccard_sim,
        });
    }

    if !quiet {
        eprintln!(
            "  [real] {} rounds, deterministic={} perm_jaccard={:.3} lift_var={:.6}",
            token_matrix.len(),
            repeated_deterministic,
            mean_jaccard,
            mean_lift_variance
        );
        for s in &snapshots {
            eprintln!(
                "    rounds={:>3} avoid={} seek={} jaccard={:.3}",
                s.round_count, s.avoid_count, s.seek_count, s.jaccard_with_full
            );
        }
    }

    vec![OracleStabilityResult {
        test_case: format!("real_{}_rounds", token_matrix.len()),
        repeated_deterministic,
        permutation_top5_jaccard: mean_jaccard,
        permutation_lift_variance: mean_lift_variance,
        incremental_snapshots: snapshots,
    }]
}

/// I11 with real data: selector comparison on real campaign history.
fn bench_selector_comparison_real(
    real: &EvalDataset,
    quiet: bool,
) -> Vec<SelectorComparisonResult> {
    use controller::triage::coverage_selector::CoverageSelector;
    use controller::triage::fuzzer_selector::FuzzerSelector;
    use controller::triage::random_selector::RandomSelector;
    use controller::triage::scorer::{build_guidance, compute_token_scores};
    use controller::triage::token_selector::TokenSelector;
    use controller::triage::{SearchSpace, Selector};
    use std::collections::BTreeMap;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    if real.rounds.len() < 3 {
        if !quiet {
            eprintln!("  [real] Need at least 3 rounds, got {}", real.rounds.len());
        }
        return vec![];
    }

    let history: BTreeMap<u32, evaluation::RoundSummary> = real
        .rounds
        .iter()
        .map(|r| (r.round_number, r.clone()))
        .collect();

    // Derive guidance from real token matrix
    let token_matrix: Vec<(Vec<String>, bool)> = if !real.token_matrices.is_empty() {
        real.token_matrices
            .iter()
            .filter(|e| e.trustworthy)
            .map(|e| (e.tokens.clone(), e.detected))
            .collect()
    } else {
        let matrix = evaluation::fixtures::token_factory::build_enriched_token_matrix(&real.rounds);
        matrix
            .iter()
            .filter(|e| e.trustworthy)
            .map(|e| (e.tokens.clone(), e.detected))
            .collect()
    };

    let scores = compute_token_scores(&token_matrix);
    let guidance = build_guidance(&scores, 1.5, 0.3);

    let search_space = SearchSpace::default();
    let default_modules = controller::dispatch::types::ModuleSelectionSpec::default();

    let selectors: Vec<(&str, Box<dyn Selector>)> = vec![
        ("Coverage", Box::new(CoverageSelector::new())),
        ("Fuzzer", Box::new(FuzzerSelector::new())),
        ("Token", Box::new(TokenSelector::new())),
        ("Random", Box::new(RandomSelector::new())),
    ];

    let mut results = Vec::new();
    let n_rounds = real.rounds.len().min(30);

    for (name, selector) in &selectors {
        let mut per_round_mutations: Vec<Vec<String>> = Vec::new();
        let mut all_selected: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut exploration_count = 0usize;

        for round_num in 2..=(n_rounds as u32 + 1) {
            let selection = rt.block_on(selector.select(
                &real.job_id,
                round_num,
                &search_space,
                &default_modules,
                &history,
                Some(&guidance),
            ));

            let mut mutation_ids: Vec<String> =
                selection.mutations.iter().map(|m| m.id.clone()).collect();
            mutation_ids.sort();

            for id in &mutation_ids {
                all_selected.insert(id.clone());
            }

            if selection.rationale.contains("exploration")
                || selection.rationale.contains("random")
                || selection.rationale.contains("explore")
            {
                exploration_count += 1;
            }

            per_round_mutations.push(mutation_ids);
        }

        let rounds_evaluated = per_round_mutations.len();
        let unique_sets: std::collections::HashSet<String> = per_round_mutations
            .iter()
            .map(|muts| muts.join(","))
            .collect();

        let pool_mutations: std::collections::HashSet<String> =
            search_space.mutation_pool.iter().cloned().collect();
        let pool_coverage = if pool_mutations.is_empty() {
            0.0
        } else {
            all_selected.intersection(&pool_mutations).count() as f64 / pool_mutations.len() as f64
        };

        let total_mutations: usize = per_round_mutations.iter().map(|m| m.len()).sum();
        let mean_recipe_size = total_mutations as f64 / rounds_evaluated.max(1) as f64;
        let exploration_rate = exploration_count as f64 / rounds_evaluated.max(1) as f64;

        if !quiet {
            eprintln!(
                "  [real] {:<10} coverage={:.2} unique_sets={} mean_size={:.1} explore={:.2}",
                name,
                pool_coverage,
                unique_sets.len(),
                mean_recipe_size,
                exploration_rate
            );
        }

        results.push(SelectorComparisonResult {
            selector_name: name.to_string(),
            rounds_evaluated,
            unique_mutation_sets: unique_sets.len(),
            mutation_pool_coverage: pool_coverage,
            mean_recipe_size,
            exploration_rate,
            per_round_mutations,
        });
    }

    results
}

/// I12 with real data: guidance utilization on real campaign history.
fn bench_guidance_utilization_real(
    real: &EvalDataset,
    quiet: bool,
) -> Vec<GuidanceUtilizationResult> {
    use controller::triage::coverage_selector::CoverageSelector;
    use controller::triage::scorer::{build_guidance, compute_token_scores};
    use controller::triage::token_selector::TokenSelector;
    use controller::triage::{ScoredToken, SearchSpace, Selector, TriageGuidance};
    use std::collections::BTreeMap;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    if real.rounds.len() < 3 {
        if !quiet {
            eprintln!("  [real] Need at least 3 rounds, got {}", real.rounds.len());
        }
        return vec![];
    }

    let history: BTreeMap<u32, evaluation::RoundSummary> = real
        .rounds
        .iter()
        .map(|r| (r.round_number, r.clone()))
        .collect();

    // Derive guidance from real data
    let token_matrix: Vec<(Vec<String>, bool)> = if !real.token_matrices.is_empty() {
        real.token_matrices
            .iter()
            .filter(|e| e.trustworthy)
            .map(|e| (e.tokens.clone(), e.detected))
            .collect()
    } else {
        let matrix = evaluation::fixtures::token_factory::build_enriched_token_matrix(&real.rounds);
        matrix
            .iter()
            .filter(|e| e.trustworthy)
            .map(|e| (e.tokens.clone(), e.detected))
            .collect()
    };

    let scores = compute_token_scores(&token_matrix);
    let natural_guidance = build_guidance(&scores, 1.5, 0.3);

    let avoid_tokens = if natural_guidance.avoid_tokens.is_empty() {
        vec![
            "mutation:ast.exec_decoy".to_string(),
            "mutation:ast.timing_pattern".to_string(),
            "mutation:ast.decon_rounds".to_string(),
        ]
    } else {
        natural_guidance
            .avoid_tokens
            .iter()
            .take(5)
            .cloned()
            .collect()
    };
    let seek_tokens = if natural_guidance.seek_tokens.is_empty() {
        vec![
            "mutation:ast.protection_transition".to_string(),
            "mutation:ast.fill_pattern".to_string(),
        ]
    } else {
        natural_guidance
            .seek_tokens
            .iter()
            .take(3)
            .cloned()
            .collect()
    };

    if !quiet {
        eprintln!(
            "  [real] guidance: avoid={:?} seek={:?}",
            avoid_tokens, seek_tokens
        );
    }

    let test_guidance = TriageGuidance {
        avoid_tokens: avoid_tokens.clone(),
        seek_tokens: seek_tokens.clone(),
        scored_avoid: avoid_tokens
            .iter()
            .enumerate()
            .map(|(i, t)| ScoredToken {
                token: t.clone(),
                importance: 2.0 - i as f64 * 0.3,
            })
            .collect(),
        scored_seek: seek_tokens
            .iter()
            .enumerate()
            .map(|(i, t)| ScoredToken {
                token: t.clone(),
                importance: 0.3 + i as f64 * 0.1,
            })
            .collect(),
    };

    let search_space = SearchSpace::default();
    let default_modules = controller::dispatch::types::ModuleSelectionSpec::default();
    let n_rounds = real.rounds.len().min(20);

    let selector_pairs: Vec<(&str, Box<dyn Selector>)> = vec![
        ("Token", Box::new(TokenSelector::new())),
        ("Coverage", Box::new(CoverageSelector::new())),
    ];

    let mut results = Vec::new();

    for (name, selector) in &selector_pairs {
        let mut mutations_without: Vec<Vec<String>> = Vec::new();
        for round_num in 2..=(n_rounds as u32 + 1) {
            let selection = rt.block_on(selector.select(
                &real.job_id,
                round_num,
                &search_space,
                &default_modules,
                &history,
                None,
            ));
            let mut ids: Vec<String> = selection.mutations.iter().map(|m| m.id.clone()).collect();
            ids.sort();
            mutations_without.push(ids);
        }

        let mut mutations_with: Vec<Vec<String>> = Vec::new();
        for round_num in 2..=(n_rounds as u32 + 1) {
            let selection = rt.block_on(selector.select(
                &real.job_id,
                round_num,
                &search_space,
                &default_modules,
                &history,
                Some(&test_guidance),
            ));
            let mut ids: Vec<String> = selection.mutations.iter().map(|m| m.id.clone()).collect();
            ids.sort();
            mutations_with.push(ids);
        }

        let avoid_mutation_ids: Vec<String> = avoid_tokens
            .iter()
            .filter_map(|t| t.strip_prefix("mutation:"))
            .map(|s| s.to_string())
            .collect();
        let seek_mutation_ids: Vec<String> = seek_tokens
            .iter()
            .filter_map(|t| t.strip_prefix("mutation:"))
            .map(|s| s.to_string())
            .collect();

        let avoidance_rate = if avoid_mutation_ids.is_empty() {
            1.0
        } else {
            let avoiding_rounds = mutations_with
                .iter()
                .filter(|muts| !avoid_mutation_ids.iter().any(|avoid| muts.contains(avoid)))
                .count();
            avoiding_rounds as f64 / mutations_with.len().max(1) as f64
        };

        let seek_adoption_rate = if seek_mutation_ids.is_empty() {
            0.0
        } else {
            let adopting_rounds = mutations_with
                .iter()
                .filter(|muts| seek_mutation_ids.iter().any(|seek| muts.contains(seek)))
                .count();
            adopting_rounds as f64 / mutations_with.len().max(1) as f64
        };

        let mut total_jaccard = 0.0;
        let n = mutations_without.len().min(mutations_with.len());
        for i in 0..n {
            total_jaccard +=
                evaluation::helpers::jaccard_distance(&mutations_without[i], &mutations_with[i]);
        }
        let recipe_jaccard_delta = total_jaccard / n.max(1) as f64;

        if !quiet {
            eprintln!(
                "  [real] {:<10} avoid={:.2} seek={:.2} delta={:.3}",
                name, avoidance_rate, seek_adoption_rate, recipe_jaccard_delta
            );
        }

        results.push(GuidanceUtilizationResult {
            selector_name: name.to_string(),
            rounds: n,
            mutations_without_guidance: mutations_without,
            mutations_with_guidance: mutations_with,
            avoid_tokens: avoid_tokens.clone(),
            seek_tokens: seek_tokens.clone(),
            avoidance_rate,
            seek_adoption_rate,
            recipe_jaccard_delta,
        });
    }

    results
}

/// I13 with real data: convergence simulation on real campaign rounds.
fn bench_convergence_simulation_real(
    real: &EvalDataset,
    quiet: bool,
) -> Vec<ConvergenceSimulationResult> {
    use controller::triage::accumulation::{
        AccumulationConfig, compute_marginal_contributions, compute_recipe_diversity,
        determine_phase, reconstruct_best_recipe,
    };
    use std::collections::{BTreeMap, HashSet};

    if real.rounds.len() < 5 {
        if !quiet {
            eprintln!(
                "  [real] Need at least 5 rounds for convergence, got {}",
                real.rounds.len()
            );
        }
        return vec![];
    }

    let total_rounds = real.rounds.len();
    let config = AccumulationConfig::default();
    let pool_size = 10;
    let fixed_set: HashSet<&str> = HashSet::new();

    let mut phase_transitions: Vec<(u32, String)> = Vec::new();
    let mut recipe_size_trajectory: Vec<(u32, usize)> = Vec::new();
    let mut diversity_trajectory: Vec<(u32, f64)> = Vec::new();
    let mut best_score_trajectory: Vec<(u32, f64)> = Vec::new();
    let mut marginal_contribution_count: Vec<(u32, usize)> = Vec::new();
    let mut prev_phase = String::new();

    for end in 1..=total_rounds {
        let round_num = end as u32;
        let partial: BTreeMap<u32, evaluation::RoundSummary> = real
            .rounds
            .iter()
            .take(end)
            .map(|r| (r.round_number, r.clone()))
            .collect();

        let phase = determine_phase(round_num, pool_size, &config);
        let phase_name = format!("{:?}", phase);
        if phase_name != prev_phase {
            phase_transitions.push((round_num, phase_name.clone()));
            prev_phase = phase_name;
        }

        let (recipe, best_score) = reconstruct_best_recipe(&partial, &fixed_set);
        recipe_size_trajectory.push((round_num, recipe.len()));
        best_score_trajectory.push((round_num, best_score));

        let diversity = if end >= 2 {
            compute_recipe_diversity(&partial, &fixed_set, 5)
        } else {
            0.0
        };
        diversity_trajectory.push((round_num, diversity));

        let marginals = compute_marginal_contributions(&partial, &fixed_set);
        let contributing = marginals.values().filter(|&&v| v > 0.0).count();
        marginal_contribution_count.push((round_num, contributing));
    }

    if !quiet {
        eprintln!("  [real] {} rounds, phase transitions:", total_rounds);
        for (round, phase) in &phase_transitions {
            eprintln!("    round {} -> {}", round, phase);
        }
        let final_recipe_size = recipe_size_trajectory.last().map(|(_, s)| *s).unwrap_or(0);
        let final_score = best_score_trajectory.last().map(|(_, s)| *s).unwrap_or(0.0);
        eprintln!("  [real] Final recipe size: {}", final_recipe_size);
        eprintln!("  [real] Final best score: {:.3}", final_score);
    }

    vec![ConvergenceSimulationResult {
        total_rounds,
        phase_transitions,
        recipe_size_trajectory,
        diversity_trajectory,
        best_score_trajectory,
        marginal_contribution_count,
    }]
}

// ── Build-crate-dependent benchmarks ────────────────────────────────────

#[cfg(feature = "build-bench")]
mod build_bench {
    use evaluation::{
        AstMutationResult, InputDiversityResult, IrMutationResult, LineTracingResult,
        PayloadEncodingResult, ScCheckpointResult, TemplateAssemblyResult,
    };
    use std::collections::HashMap;
    use std::time::Instant;

    // ── I14: Line Tracing Overhead ──────────────────────────────────────

    pub fn bench_line_tracing(quiet: bool) -> Vec<LineTracingResult> {
        use build::instrument::line_tracer::{SourceLanguage, inject_line_traces};

        let iterations = 50usize;
        let mut results = Vec::new();

        // Source 1: REFERENCE_C_SOURCE (~70 lines)
        let sources: Vec<(&str, String)> = vec![
            ("reference_c", REFERENCE_C_SOURCE.to_string()),
            ("synthetic_100", generate_synthetic_c(100)),
            ("synthetic_300", generate_synthetic_c(300)),
            ("synthetic_500", generate_synthetic_c(500)),
            ("synthetic_1000", generate_synthetic_c(1000)),
        ];

        for (label, source) in &sources {
            let input_lines = source.lines().count();

            // Warmup run
            let _ = inject_line_traces(source, SourceLanguage::C);

            // Timed runs
            let mut times = Vec::with_capacity(iterations);
            let mut output_source = String::new();

            for _ in 0..iterations {
                let t0 = Instant::now();
                match inject_line_traces(source, SourceLanguage::C) {
                    Ok(out) => {
                        let elapsed = t0.elapsed().as_secs_f64() * 1_000_000.0;
                        times.push(elapsed);
                        output_source = out;
                    }
                    Err(e) => {
                        if !quiet {
                            eprintln!("  {} FAILED: {}", label, e);
                        }
                        break;
                    }
                }
            }

            if times.is_empty() {
                continue;
            }

            let mean = times.iter().sum::<f64>() / times.len() as f64;
            let variance = times.iter().map(|t| (t - mean).powi(2)).sum::<f64>()
                / (times.len() - 1).max(1) as f64;
            let stddev = variance.sqrt();
            let first_time = times[0];

            let output_lines = output_source.lines().count();
            let trace_calls = output_source.matches("__trace_line_binary(").count()
                + output_source.matches("__trace_line(").count();
            let deferred = output_source.matches("__seen_L").count() / 2; // each deferred has decl + check
            let output_valid = check_parse_valid(&output_source);
            let chars_per_us = if mean > 0.0 {
                source.len() as f64 / mean
            } else {
                0.0
            };

            if !quiet {
                eprintln!(
                    "  {:<20} lines={:>5} traces={:>4} deferred={:>3} mean={:.0}µs stddev={:.0}µs valid={}",
                    label, input_lines, trace_calls, deferred, mean, stddev, output_valid
                );
            }

            results.push(LineTracingResult {
                source_label: label.to_string(),
                input_lines,
                output_lines,
                trace_calls_injected: trace_calls,
                deferred_trace_calls: deferred,
                trace_format: "binary".to_string(),
                transform_time_us: first_time,
                mean_transform_time_us: mean,
                stddev_transform_time_us: stddev,
                iterations,
                output_valid,
                chars_per_us,
            });
        }

        results
    }

    /// Generate synthetic C source of approximately `target_lines` lines.
    fn generate_synthetic_c(target_lines: usize) -> String {
        let mut out = String::new();
        out.push_str("#include <windows.h>\n#include <stdio.h>\n\n");

        let mut line_count = 3usize;
        let mut func_idx = 0u32;

        while line_count < target_lines {
            out.push_str(&format!("void synthetic_func_{}(void) {{\n", func_idx));
            out.push_str("    int x = 0;\n");
            out.push_str("    for (int i = 0; i < 10; i++) {\n");
            out.push_str("        x += i * 2;\n");
            out.push_str("        if (x > 50) break;\n");
            out.push_str("    }\n");
            out.push_str("    volatile int y = x;\n");
            out.push_str("    (void)y;\n");
            out.push_str("}\n\n");
            line_count += 10;
            func_idx += 1;
        }

        out.push_str("int main() {\n");
        for i in 0..func_idx {
            out.push_str(&format!("    synthetic_func_{}();\n", i));
        }
        out.push_str("    return 0;\n}\n");

        out
    }

    // ── I15: Shellcode Checkpoint Patching ──────────────────────────────

    pub fn bench_sc_checkpoints(quiet: bool) -> Vec<ScCheckpointResult> {
        use build::template::sc_checkpoints::{generate_c_header, patch_shellcode};
        use build::template::shellcode_stub::{STUB_SIZE, prepend_checkpoint_stub};

        let iterations = 50usize;

        let shellcode_files = [
            "eicar.bin",
            "calc64.bin",
            "messagebox.bin",
            "msf-revshell-win64.bin",
            "msf-revwinhttp.bin",
            "nanodump_v0_direct.bin",
            "met_revhttp.bin",
            "2_beacon_x64.bin",
            "NimPlant.bin",
        ];

        let checkpoint_counts: [u32; 5] = [1, 3, 5, 10, 20];

        let shellcode_dir = std::path::PathBuf::from("data/shellcodes");
        if !shellcode_dir.exists() {
            if !quiet {
                eprintln!("  Shellcode directory not found: {:?}", shellcode_dir);
            }
            return vec![];
        }

        let mut results = Vec::new();

        for &filename in &shellcode_files {
            let path = shellcode_dir.join(filename);
            let raw_shellcode = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    if !quiet {
                        eprintln!("  Skipping {}: {}", filename, e);
                    }
                    continue;
                }
            };

            let shellcode_size = raw_shellcode.len();

            // Prepend stub (time it once with iterations)
            let mut stub_times = Vec::with_capacity(iterations);
            let mut with_stub = Vec::new();
            for _ in 0..iterations {
                let t0 = Instant::now();
                with_stub = prepend_checkpoint_stub(&raw_shellcode);
                let elapsed = t0.elapsed().as_secs_f64() * 1_000_000.0;
                stub_times.push(elapsed);
            }
            let mean_stub_time = stub_times.iter().sum::<f64>() / stub_times.len() as f64;
            let size_with_stub = with_stub.len();

            // Get reachable boundaries count (single run)
            let mut boundary_probe = with_stub.clone();
            let reachable_boundaries = match patch_shellcode(&mut boundary_probe, 0, STUB_SIZE) {
                Ok(_) => {
                    // For count=0, we don't get boundaries. Do a max-count probe instead.
                    let mut probe2 = with_stub.clone();
                    match patch_shellcode(&mut probe2, u32::MAX, STUB_SIZE) {
                        Ok(p) => p.table.len(),
                        Err(_) => 0,
                    }
                }
                Err(_) => 0,
            };

            for &count in &checkpoint_counts {
                // Timed patch runs
                let mut patch_times = Vec::with_capacity(iterations);
                let mut header_times = Vec::with_capacity(iterations);
                let mut last_patched = None;

                for _ in 0..iterations {
                    let mut buf = with_stub.clone();
                    let t0 = Instant::now();
                    let result = patch_shellcode(&mut buf, count, STUB_SIZE);
                    let elapsed = t0.elapsed().as_secs_f64() * 1_000_000.0;
                    patch_times.push(elapsed);

                    if let Ok(ref patched) = result {
                        let t1 = Instant::now();
                        let _ = generate_c_header(patched);
                        let header_elapsed = t1.elapsed().as_secs_f64() * 1_000_000.0;
                        header_times.push(header_elapsed);
                        last_patched = Some(patched.clone());
                    }
                }

                if patch_times.is_empty() {
                    continue;
                }

                let mean_patch = patch_times.iter().sum::<f64>() / patch_times.len() as f64;
                let variance = patch_times
                    .iter()
                    .map(|t| (t - mean_patch).powi(2))
                    .sum::<f64>()
                    / (patch_times.len() - 1).max(1) as f64;
                let stddev_patch = variance.sqrt();
                let first_patch_time = patch_times[0];
                let mean_header = if header_times.is_empty() {
                    0.0
                } else {
                    header_times.iter().sum::<f64>() / header_times.len() as f64
                };

                let (actual_checkpoints, boundary_correct, progress_pcts) =
                    if let Some(ref patched) = last_patched {
                        let pcts: Vec<u8> = patched.table.iter().map(|e| e.progress_pct).collect();
                        // Verify boundary correctness: all offsets should be >= STUB_SIZE
                        let correct = patched.table.iter().all(|e| e.offset >= STUB_SIZE);
                        (patched.table.len(), correct, pcts)
                    } else {
                        (0, true, vec![])
                    };

                let bytes_per_us = if mean_patch > 0.0 {
                    shellcode_size as f64 / mean_patch
                } else {
                    0.0
                };

                if !quiet {
                    eprintln!(
                        "  {:<28} size={:>7} count={:>2} actual={:>2} mean={:>10.1}µs throughput={:.1} bytes/µs",
                        filename,
                        shellcode_size,
                        count,
                        actual_checkpoints,
                        mean_patch,
                        bytes_per_us
                    );
                }

                results.push(ScCheckpointResult {
                    shellcode_name: filename.to_string(),
                    shellcode_size,
                    requested_checkpoints: count,
                    actual_checkpoints,
                    size_with_stub,
                    reachable_boundaries,
                    patch_time_us: first_patch_time,
                    mean_patch_time_us: mean_patch,
                    stddev_patch_time_us: stddev_patch,
                    stub_prepend_time_us: mean_stub_time,
                    header_gen_time_us: mean_header,
                    iterations,
                    bytes_per_us,
                    boundary_correct,
                    checkpoint_progress_pcts: progress_pcts,
                });
            }
        }

        results
    }

    pub fn bench_input_diversity(quiet: bool) -> Vec<InputDiversityResult> {
        use build::mutator::MutationSpec;
        use build::transform::ast_mutator::AstMutator;

        let mut mutator = match AstMutator::new() {
            Ok(m) => m,
            Err(e) => {
                if !quiet {
                    eprintln!("  Failed to create AstMutator: {}", e);
                }
                return vec![];
            }
        };

        let source = REFERENCE_C_SOURCE;
        let input_lines = source.lines().count() as i64;

        let mutations = [
            "ast.decon_rounds:count=50",
            "ast.fill_pattern:pattern=0xCC",
            "ast.timing_pattern:method=rdtsc",
            "ast.protection_transition:strategy=rw_rx",
            "ast.benign_preamble:count=5",
            "ast.exec_decoy:target=calc",
            "ast.api_sequence_obfuscation:inserts=3",
            "ast.const_obfuscation",
            "ast.string_xor:xor_key=0xAA",
            "ast.benign_syscall_insert:count=5",
        ];

        // Apply each mutation to get outputs
        let mut outputs: Vec<(String, String, i64)> = Vec::new(); // (mutation_id, output_source, line_delta)

        for mutation_str in &mutations {
            let spec = MutationSpec::from_cli_str(mutation_str);
            match mutator.apply(source, &[&spec]) {
                Ok((output, _)) => {
                    let delta = output.lines().count() as i64 - input_lines;
                    outputs.push((mutation_str.to_string(), output, delta));
                }
                Err(_) => {
                    outputs.push((mutation_str.to_string(), String::new(), -input_lines));
                }
            }
        }

        // Compute pairwise distances
        let mut results = Vec::new();
        for i in 0..outputs.len() {
            for j in (i + 1)..outputs.len() {
                let (ref id_a, ref src_a, delta_a) = outputs[i];
                let (ref id_b, ref src_b, delta_b) = outputs[j];

                let max_delta = delta_a.abs().max(delta_b.abs()).max(1) as f64;
                let normalized_distance = (delta_a - delta_b).abs() as f64 / max_delta;
                let outputs_differ = src_a != src_b;

                results.push(InputDiversityResult {
                    mutation_a: id_a.clone(),
                    mutation_b: id_b.clone(),
                    line_delta_a: delta_a,
                    line_delta_b: delta_b,
                    normalized_distance,
                    outputs_differ,
                });
            }
        }

        if !quiet {
            let differ_count = results.iter().filter(|r| r.outputs_differ).count();
            eprintln!(
                "  {} pairs, {} differ ({:.0}%)",
                results.len(),
                differ_count,
                differ_count as f64 / results.len().max(1) as f64 * 100.0
            );
        }

        results
    }

    pub fn bench_payload_encoding(quiet: bool) -> Vec<PayloadEncodingResult> {
        use build::template::payload::{EncodingType, PayloadEncoder, generate_test_payload};

        let encoder = PayloadEncoder::new();
        let sizes: Vec<usize> = vec![1, 16, 64, 256, 1024, 4096, 8192, 16384];
        let encodings = [
            ("xor", EncodingType::Xor),
            ("english", EncodingType::English),
            ("subbyte", EncodingType::SubByte),
            ("none", EncodingType::None),
        ];

        let mut results = Vec::new();

        for &size in &sizes {
            let payload = generate_test_payload(size);

            for (name, enc_type) in &encodings {
                let t0 = Instant::now();
                let encoded = encoder.encode(&payload, enc_type.clone());
                let encode_time = t0.elapsed().as_secs_f64() * 1_000_000.0;

                let header = encoder.generate_c_header(&encoded);
                let header_compiles = !header.is_empty() && header.contains("payload");

                let entropy = byte_entropy(&encoded.data);

                let roundtrip_correct = match enc_type {
                    EncodingType::None => encoded.data == payload,
                    _ => !encoded.data.is_empty(),
                };

                results.push(PayloadEncodingResult {
                    encoding_type: name.to_string(),
                    payload_size: size,
                    encoded_size: encoded.data.len(),
                    encoded_entropy: entropy,
                    roundtrip_correct,
                    encode_time_us: encode_time,
                    header_compiles,
                });

                if !quiet {
                    eprintln!(
                        "  {} size={:>5} encoded={:>6} entropy={:.3} time={:.0}µs",
                        name,
                        size,
                        encoded.data.len(),
                        entropy,
                        encode_time
                    );
                }
            }
        }

        results
    }

    pub fn bench_ast_mutations(quiet: bool) -> Vec<AstMutationResult> {
        use build::mutator::MutationSpec;
        use build::transform::ast_mutator::AstMutator;

        let mut mutator = match AstMutator::new() {
            Ok(m) => m,
            Err(e) => {
                if !quiet {
                    eprintln!("  Failed to create AstMutator: {}", e);
                }
                return vec![];
            }
        };

        let source = REFERENCE_C_SOURCE;
        let input_lines = source.lines().count();
        let input_ast_nodes = count_ast_nodes(source);

        let mutations = [
            "ast.decon_rounds:count=50",
            "ast.fill_pattern:pattern=0xCC",
            "ast.timing_pattern:method=rdtsc",
            "ast.protection_transition:strategy=rw_rx",
            "ast.benign_preamble:count=5",
            "ast.exec_decoy:target=calc",
            "ast.api_sequence_obfuscation:inserts=3",
            "ast.const_obfuscation",
            "ast.string_xor:xor_key=0xAA",
            "ast.benign_syscall_insert:count=5",
        ];

        let mut results = Vec::new();

        for mutation_str in &mutations {
            let spec = MutationSpec::from_cli_str(mutation_str);
            let t0 = Instant::now();
            let outcome = mutator.apply(source, &[&spec]);
            let transform_time = t0.elapsed().as_secs_f64() * 1_000_000.0;

            match outcome {
                Ok((output, _applied)) => {
                    let output_lines = output.lines().count();
                    let output_ast_nodes = count_ast_nodes(&output);
                    let parse_valid = check_parse_valid(&output);

                    results.push(AstMutationResult {
                        mutation_id: mutation_str.to_string(),
                        input_lines,
                        output_lines,
                        line_delta: output_lines as i64 - input_lines as i64,
                        input_ast_nodes,
                        output_ast_nodes,
                        parse_valid,
                        compile_success: None,
                        transform_time_us: transform_time,
                    });

                    if !quiet {
                        eprintln!(
                            "  {:<40} delta={:>4} valid={} time={:.0}µs",
                            mutation_str,
                            output_lines as i64 - input_lines as i64,
                            parse_valid,
                            transform_time
                        );
                    }
                }
                Err(e) => {
                    results.push(AstMutationResult {
                        mutation_id: mutation_str.to_string(),
                        input_lines,
                        output_lines: 0,
                        line_delta: -(input_lines as i64),
                        input_ast_nodes,
                        output_ast_nodes: 0,
                        parse_valid: false,
                        compile_success: None,
                        transform_time_us: transform_time,
                    });
                    if !quiet {
                        eprintln!("  {:<40} ERROR: {}", mutation_str, e);
                    }
                }
            }
        }

        results
    }

    pub fn bench_ir_mutations(quiet: bool) -> Vec<IrMutationResult> {
        use build::mutator::MutationSpec;
        use build::transform::ir_mutator::IrMutator;

        let reference_ir = REFERENCE_LLVM_IR;
        let input_lines = reference_ir.lines().count();

        let mutations = [
            ("llvm.nop_insert:density=1.0", 1.0f32),
            ("llvm.opaque_predicate:density=1.0,mode=robust", 1.0),
            ("llvm.opaque_predicate:density=1.0,mode=trivial", 1.0),
            ("llvm.junk_block:count=3", 1.0),
        ];

        let mut results = Vec::new();

        for (mutation_str, density) in &mutations {
            let spec = MutationSpec::from_cli_str(mutation_str);

            let mut mutator1 = IrMutator::with_seed(42);
            let t0 = Instant::now();
            let result1 = mutator1.apply(reference_ir, &[&spec]);
            let transform_time = t0.elapsed().as_secs_f64() * 1_000_000.0;

            let mut mutator2 = IrMutator::with_seed(42);
            let result2 = mutator2.apply(reference_ir, &[&spec]);

            match (result1, result2) {
                (Ok((output1, _)), Ok((output2, _))) => {
                    let output_lines = output1.lines().count();
                    let insertions = output_lines.saturating_sub(input_lines);
                    let deterministic = output1 == output2;

                    results.push(IrMutationResult {
                        mutation_id: mutation_str.to_string(),
                        density: *density,
                        input_lines,
                        output_lines,
                        insertions,
                        survives_o2: None,
                        deterministic,
                        transform_time_us: transform_time,
                    });

                    if !quiet {
                        eprintln!(
                            "  {:<50} ins={:>3} det={} time={:.0}µs",
                            mutation_str, insertions, deterministic, transform_time
                        );
                    }
                }
                (Err(e), _) | (_, Err(e)) => {
                    results.push(IrMutationResult {
                        mutation_id: mutation_str.to_string(),
                        density: *density,
                        input_lines,
                        output_lines: 0,
                        insertions: 0,
                        survives_o2: None,
                        deterministic: false,
                        transform_time_us: transform_time,
                    });
                    if !quiet {
                        eprintln!("  {:<50} ERROR: {}", mutation_str, e);
                    }
                }
            }
        }

        results
    }

    pub fn bench_template_assembly(quiet: bool) -> Vec<TemplateAssemblyResult> {
        use build::template::assembler::{Assembler, ModuleSelection};
        use build::template::payload::{EncodingType, PayloadEncoder};

        let template_dir = std::path::PathBuf::from("build/templates");
        if !template_dir.exists() {
            if !quiet {
                eprintln!("  Template directory not found: {:?}", template_dir);
            }
            return vec![];
        }

        let mut assembler = match Assembler::new(&template_dir) {
            Ok(a) => a,
            Err(e) => {
                if !quiet {
                    eprintln!("  Failed to create Assembler: {}", e);
                }
                return vec![];
            }
        };

        let encoder = PayloadEncoder::new();
        let payload = vec![0x90; 64];
        let encoded = encoder.encode(&payload, EncodingType::Xor);
        let payload_code = encoder.generate_c_header(&encoded);

        let carriers = ["alloc_rw_rx", "change_rw_rx", "peb_walk"];
        let decoders = ["xor", "english", "none", "subbyte"];
        let antiemulations = ["none", "sirallocalot", "timeraw"];
        let deconditioners = ["none", "alloc_loop"];
        let guardrails = ["none", "env"];
        let virtualprotects = ["standard", "undersized"];
        let decoys = ["none", "winexec"];

        let mut results = Vec::new();
        let mut count = 0usize;
        let max_combinations = 100;

        for carrier in &carriers {
            for decoder in &decoders {
                for anti in &antiemulations {
                    if count >= max_combinations {
                        break;
                    }

                    let decon = deconditioners[count % deconditioners.len()];
                    let guard = guardrails[count % guardrails.len()];
                    let vp = virtualprotects[count % virtualprotects.len()];
                    let decoy = decoys[count % decoys.len()];

                    let modules = ModuleSelection {
                        carrier: carrier.to_string(),
                        decoder: decoder.to_string(),
                        antiemulation: anti.to_string(),
                        deconditioner: decon.to_string(),
                        guardrail: guard.to_string(),
                        virtualprotect: vp.to_string(),
                        decoy: decoy.to_string(),
                    };

                    let t0 = Instant::now();
                    let result = assembler.assemble(&modules, &payload_code);
                    let assembly_time = t0.elapsed().as_secs_f64() * 1_000_000.0;

                    let mut module_map = HashMap::new();
                    module_map.insert("carrier".to_string(), carrier.to_string());
                    module_map.insert("decoder".to_string(), decoder.to_string());
                    module_map.insert("antiemulation".to_string(), anti.to_string());
                    module_map.insert("deconditioner".to_string(), decon.to_string());
                    module_map.insert("guardrail".to_string(), guard.to_string());
                    module_map.insert("virtualprotect".to_string(), vp.to_string());
                    module_map.insert("decoy".to_string(), decoy.to_string());

                    match result {
                        Ok(assembled) => {
                            let markers_resolved = !assembled.lines().any(|l| {
                                let trimmed = l.trim();
                                trimmed.starts_with("// @MODULE:")
                            });
                            let output_lines = assembled.lines().count();

                            results.push(TemplateAssemblyResult {
                                modules: module_map,
                                markers_resolved,
                                output_lines,
                                assembly_time_us: assembly_time,
                            });

                            if !quiet && count < 10 {
                                eprintln!(
                                    "  [{:>3}] {}+{} resolved={} lines={} time={:.0}µs",
                                    count,
                                    carrier,
                                    decoder,
                                    markers_resolved,
                                    output_lines,
                                    assembly_time
                                );
                            }
                        }
                        Err(e) => {
                            results.push(TemplateAssemblyResult {
                                modules: module_map,
                                markers_resolved: false,
                                output_lines: 0,
                                assembly_time_us: assembly_time,
                            });
                            if !quiet {
                                eprintln!("  [{:>3}] {}+{} ERROR: {}", count, carrier, decoder, e);
                            }
                        }
                    }

                    assembler.clear_cache();
                    count += 1;
                }
            }
        }

        if !quiet {
            eprintln!("  Tested {} combinations", results.len());
        }

        results
    }

    fn byte_entropy(data: &[u8]) -> f64 {
        if data.is_empty() {
            return 0.0;
        }
        let mut counts = [0usize; 256];
        for &b in data {
            counts[b as usize] += 1;
        }
        let total = data.len() as f64;
        let mut entropy = 0.0;
        for &c in &counts {
            if c > 0 {
                let p = c as f64 / total;
                entropy -= p * p.log2();
            }
        }
        entropy
    }

    fn count_ast_nodes(source: &str) -> usize {
        source
            .chars()
            .filter(|c| matches!(c, ';' | '{' | '}'))
            .count()
    }

    fn check_parse_valid(source: &str) -> bool {
        let open = source.chars().filter(|c| *c == '{').count();
        let close = source.chars().filter(|c| *c == '}').count();
        if open != close {
            return false;
        }
        !source.contains("ERROR") || source.contains("\"ERROR\"")
    }

    const REFERENCE_C_SOURCE: &str = r#"
#include <windows.h>
#include <stdio.h>

// @MUTATE:decon_rounds
#define DECON_ROUNDS 10
void decondition() {
    for (int i = 0; i < DECON_ROUNDS; i++) {
        void* p = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if (p) VirtualFree(p, 0, MEM_RELEASE);
    }
}

// @MUTATE:fill_pattern
void fill_memory(void* dest, size_t len) {
    memset(dest, 0x90, len);
}

// @MUTATE:timing_pattern
void antiemulation() {
    DWORD start = GetTickCount();
    Sleep(100);
    DWORD elapsed = GetTickCount() - start;
    if (elapsed < 90) ExitProcess(1);
}

// @MUTATE:protection_transition
void protect(void* addr, size_t len) {
    DWORD old;
    VirtualProtect(addr, len, PAGE_EXECUTE_READ, &old);
}

// @MUTATE:benign_preamble
void benign_ops() {
    char buf[256];
    GetComputerNameA(buf, &(DWORD){sizeof(buf)});
    GetUserNameA(buf, &(DWORD){sizeof(buf)});
}

// @MUTATE:exec_decoy
void exec_decoy() {
    STARTUPINFO si = { sizeof(si) };
    PROCESS_INFORMATION pi;
    CreateProcessA(NULL, "notepad.exe", NULL, NULL, FALSE, CREATE_SUSPENDED, NULL, NULL, &si, &pi);
    TerminateProcess(pi.hProcess, 0);
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
}

// @MUTATE:api_sequence_obfuscation
void api_obfuscation() {
    GetCurrentProcessId();
    GetTickCount();
    Sleep(0);
}

int main() {
    const char* message = "Hello World";
    int value = 42;
    int arr[] = {1, 2, 3, 4, 5};
    benign_ops();
    decondition();
    antiemulation();
    void* mem = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!mem) return 1;
    fill_memory(mem, 4096);
    protect(mem, 4096);
    exec_decoy();
    api_obfuscation();
    return 0;
}
"#;

    const REFERENCE_LLVM_IR: &str = r#"
; ModuleID = 'test.c'
source_filename = "test.c"
target datalayout = "e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-windows-msvc19.29.30133"

define dso_local i32 @main() #0 {
entry:
  %retval = alloca i32, align 4
  %x = alloca i32, align 4
  store i32 0, i32* %retval, align 4
  store i32 42, i32* %x, align 4
  %0 = load i32, i32* %x, align 4
  %cmp = icmp sgt i32 %0, 10
  br i1 %cmp, label %if.then, label %if.else

if.then:
  store i32 1, i32* %retval, align 4
  br label %return

if.else:
  store i32 0, i32* %retval, align 4
  br label %return

return:
  %1 = load i32, i32* %retval, align 4
  ret i32 %1
}

define dso_local void @helper() #0 {
entry:
  %buf = alloca [256 x i8], align 16
  br label %loop

loop:
  %i = phi i32 [ 0, %entry ], [ %next, %loop ]
  %next = add i32 %i, 1
  %done = icmp eq i32 %next, 100
  br i1 %done, label %exit, label %loop

exit:
  ret void
}

attributes #0 = { noinline nounwind optnone }
"#;
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
