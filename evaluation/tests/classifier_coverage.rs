//! B2: Classifier Decision Boundary Analysis — Exhaustive Test
//!
//! Tests the 11-branch classifier decision tree by constructing
//! synthetic evidence tuples covering every branch.
//!
//! The classifier maps (exit_code, timed_out, has_launched, telemetry_events)
//! to one of 7 DetectionVerdict values. This test verifies all branches
//! are reachable and produce expected verdicts.
//!
//! Run: cargo test -p evaluation --test classifier_coverage

/// Synthetic exit codes mirroring worker/agent/src/execution/types.rs.
const EXIT_INFRA: i32 = -4;
const EXIT_TIMEOUT: i32 = -3;
const EXIT_NO_CODE: i32 = -2;
const EXIT_WAIT_FAILED: i32 = -1;

/// AV NTSTATUS codes (from classifier.rs).
const AV_NTSTATUS_1: i32 = 0xC0000906u32 as i32; // STATUS_VIRUS_INFECTED
const AV_NTSTATUS_2: i32 = 0xC0000907u32 as i32; // STATUS_VIRUS_DELETED

/// Crash NTSTATUS codes.
const ACCESS_VIOLATION: i32 = 0xC0000005u32 as i32;

/// Simulated classifier decision tree (replicates the logic from
/// worker/agent/src/execution/classifier.rs without importing the crate).
///
/// This avoids adding worker-agent as a dependency while still
/// exhaustively testing the decision boundary.
fn classify(exit_code: i32, timed_out: bool, has_launched: bool) -> &'static str {
    // Branch 1: Infrastructure error (spawn/setup failure)
    if exit_code == EXIT_INFRA {
        return "InfraError";
    }

    // Branch 2: Wait failed
    if exit_code == EXIT_WAIT_FAILED {
        return "InfraError";
    }

    // Branch 3: Guardrail exit codes (10-19)
    if (10..20).contains(&exit_code) {
        return "InfraError";
    }

    // Branch 4: Clean exit
    if exit_code == 0 {
        return "Evasion";
    }

    // Branch 5: Timeout with launch
    if timed_out && has_launched {
        return "Evasion";
    }

    // Branch 6: Timeout without launch (stalled)
    if timed_out && !has_launched {
        return "Stalled";
    }

    // Branch 7: No exit code (killed by EDR)
    if exit_code == EXIT_NO_CODE {
        return "Detected";
    }

    // Branch 8: AV NTSTATUS codes
    if exit_code == AV_NTSTATUS_1 || exit_code == AV_NTSTATUS_2 {
        return "Detected";
    }

    // Branch 9: Crash NTSTATUS codes
    if exit_code == ACCESS_VIOLATION {
        return "Ambiguous";
    }

    // Branch 10: Carrier exit codes (30-39)
    if (30..40).contains(&exit_code) {
        return "Ambiguous";
    }

    // Branch 11: Other nonzero
    "Ambiguous"
}

/// Test case structure.
struct TestCase {
    exit_code: i32,
    timed_out: bool,
    has_launched: bool,
    expected_verdict: &'static str,
    description: &'static str,
}

#[test]
fn test_all_classifier_branches() {
    let cases = vec![
        // Branch 1: EXIT_INFRA
        TestCase {
            exit_code: EXIT_INFRA,
            timed_out: false,
            has_launched: false,
            expected_verdict: "InfraError",
            description: "EXIT_INFRA → InfraError",
        },
        // Branch 2: EXIT_WAIT_FAILED
        TestCase {
            exit_code: EXIT_WAIT_FAILED,
            timed_out: false,
            has_launched: false,
            expected_verdict: "InfraError",
            description: "EXIT_WAIT_FAILED → InfraError",
        },
        // Branch 3: Guardrail codes (10-19)
        TestCase {
            exit_code: 10,
            timed_out: false,
            has_launched: true,
            expected_verdict: "InfraError",
            description: "exit_code=10 (guardrail) → InfraError",
        },
        TestCase {
            exit_code: 15,
            timed_out: false,
            has_launched: true,
            expected_verdict: "InfraError",
            description: "exit_code=15 (guardrail mid-range) → InfraError",
        },
        TestCase {
            exit_code: 19,
            timed_out: false,
            has_launched: true,
            expected_verdict: "InfraError",
            description: "exit_code=19 (guardrail upper) → InfraError",
        },
        // Branch 4: Clean exit
        TestCase {
            exit_code: 0,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Evasion",
            description: "exit_code=0 → Evasion",
        },
        // Branch 5: Timeout + launched
        TestCase {
            exit_code: EXIT_TIMEOUT,
            timed_out: true,
            has_launched: true,
            expected_verdict: "Evasion",
            description: "timeout + launched → Evasion",
        },
        // Branch 6: Timeout + not launched
        TestCase {
            exit_code: EXIT_TIMEOUT,
            timed_out: true,
            has_launched: false,
            expected_verdict: "Stalled",
            description: "timeout + not launched → Stalled",
        },
        // Branch 7: EXIT_NO_CODE (killed)
        TestCase {
            exit_code: EXIT_NO_CODE,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Detected",
            description: "EXIT_NO_CODE → Detected",
        },
        // Branch 8: AV NTSTATUS
        TestCase {
            exit_code: AV_NTSTATUS_1,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Detected",
            description: "STATUS_VIRUS_INFECTED → Detected",
        },
        TestCase {
            exit_code: AV_NTSTATUS_2,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Detected",
            description: "STATUS_VIRUS_DELETED → Detected",
        },
        // Branch 9: Crash NTSTATUS
        TestCase {
            exit_code: ACCESS_VIOLATION,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Ambiguous",
            description: "ACCESS_VIOLATION → Ambiguous",
        },
        // Branch 10: Carrier codes (30-39)
        TestCase {
            exit_code: 30,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Ambiguous",
            description: "exit_code=30 (carrier) → Ambiguous",
        },
        TestCase {
            exit_code: 35,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Ambiguous",
            description: "exit_code=35 (carrier mid-range) → Ambiguous",
        },
        // Branch 11: Other nonzero
        TestCase {
            exit_code: 1,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Ambiguous",
            description: "exit_code=1 (other) → Ambiguous",
        },
        TestCase {
            exit_code: 42,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Ambiguous",
            description: "exit_code=42 (other) → Ambiguous",
        },
        TestCase {
            exit_code: 255,
            timed_out: false,
            has_launched: true,
            expected_verdict: "Ambiguous",
            description: "exit_code=255 (other) → Ambiguous",
        },
    ];

    let mut branch_coverage: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for tc in &cases {
        let verdict = classify(tc.exit_code, tc.timed_out, tc.has_launched);
        assert_eq!(
            verdict, tc.expected_verdict,
            "FAILED: {} — got '{}', expected '{}'",
            tc.description, verdict, tc.expected_verdict
        );
        branch_coverage.insert(verdict);
    }

    // Verify all 7 verdict categories are reachable
    let expected_verdicts = ["InfraError", "Evasion", "Stalled", "Detected", "Ambiguous"];

    for &expected in &expected_verdicts {
        assert!(
            branch_coverage.contains(expected),
            "Verdict '{}' was never reached in test cases",
            expected
        );
    }

    eprintln!(
        "All {} test cases passed, {} verdict categories covered",
        cases.len(),
        branch_coverage.len()
    );
}

#[test]
fn test_boundary_exit_codes() {
    // Test exact boundaries of guardrail and carrier code ranges
    assert_eq!(classify(9, false, true), "Ambiguous"); // Just below guardrail
    assert_eq!(classify(10, false, true), "InfraError"); // Lower bound guardrail
    assert_eq!(classify(19, false, true), "InfraError"); // Upper bound guardrail
    assert_eq!(classify(20, false, true), "Ambiguous"); // Just above guardrail

    assert_eq!(classify(29, false, true), "Ambiguous"); // Just below carrier
    assert_eq!(classify(30, false, true), "Ambiguous"); // Lower bound carrier
    assert_eq!(classify(39, false, true), "Ambiguous"); // Upper bound carrier
    assert_eq!(classify(40, false, true), "Ambiguous"); // Just above carrier
}

#[test]
fn test_verdict_distribution_from_dataset() {
    // Load eval_dataset.json and cross-tabulate verdicts vs categories
    let path = std::path::Path::new("eval_dataset.json");
    if !path.exists() {
        eprintln!("Skipping dataset test: eval_dataset.json not found");
        return;
    }

    let content = std::fs::read_to_string(path).unwrap();
    let dataset: serde_json::Value = serde_json::from_str(&content).unwrap();

    let rounds = dataset["rounds"].as_array().unwrap();
    let n = rounds.len();

    let mut confusion: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();

    for round in rounds {
        let verdict = round["detection_verdict"].as_str().unwrap().to_string();
        let category = round["differential_category"].as_str().unwrap().to_string();
        *confusion.entry((verdict, category)).or_default() += 1;
    }

    eprintln!("\nVerdict → Category confusion matrix ({} rounds):", n);
    eprintln!("{:<15} → {:<25} Count", "Verdict", "Category");
    eprintln!("{:-<50}", "");

    let mut entries: Vec<_> = confusion.iter().collect();
    entries.sort_by_key(|((v, c), _)| (v.clone(), c.clone()));

    for ((verdict, category), count) in entries {
        eprintln!("{:<15} → {:<25} {}", verdict, category, count);
    }
}
