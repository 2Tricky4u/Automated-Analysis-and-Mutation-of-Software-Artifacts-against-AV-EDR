mod common;

use build::mutator::{MutationSpec, Mutator};
use std::collections::HashMap;

// ── NOP insertion tests ─────────────────────────────────────────────────────

#[test]
fn test_nop_insert_density_100() {
    let ir = common::ir_multi_block(); // 4 block labels: entry, positive, negative, done
    let mutation = MutationSpec {
        id: "llvm.nop_insert".to_string(),
        params: [("density".to_string(), "1.0".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, applied) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert!(applied.contains(&"llvm.nop_insert".to_string()));
    let nop_count = output_str
        .matches(r#"call void asm sideeffect "nop""#)
        .count();
    // All 4 blocks should get NOPs at density=1.0
    assert_eq!(nop_count, 4, "Expected 4 NOPs, got {}", nop_count);
}

#[test]
fn test_nop_insert_density_0() {
    let ir = common::ir_multi_block();
    let mutation = MutationSpec {
        id: "llvm.nop_insert".to_string(),
        params: [("density".to_string(), "0.0".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, _) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert_eq!(
        output_str
            .matches(r#"call void asm sideeffect "nop""#)
            .count(),
        0,
        "No NOPs should be inserted at density=0.0"
    );
}

#[test]
fn test_nop_insert_default_density() {
    let ir = common::ir_simple_function();
    let mutation = MutationSpec {
        id: "llvm.nop_insert".to_string(),
        params: HashMap::new(), // defaults to 0.3
    };

    let (output, applied) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    // Should not panic, output should be valid UTF-8
    let _output_str = String::from_utf8(output).unwrap();
    assert!(applied.contains(&"llvm.nop_insert".to_string()));
}

#[test]
fn test_nop_insert_preserves_structure() {
    let ir = common::ir_multi_block();
    let mutation = MutationSpec {
        id: "llvm.nop_insert".to_string(),
        params: [("density".to_string(), "1.0".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, _) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    // Every original line must still be present
    for line in ir.lines() {
        assert!(
            output_str.contains(line),
            "Original line missing: {:?}",
            line
        );
    }
}

#[test]
fn test_nop_insert_no_blocks() {
    let ir = common::ir_no_blocks(); // declarations only, no block labels
    let mutation = MutationSpec {
        id: "llvm.nop_insert".to_string(),
        params: [("density".to_string(), "1.0".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, _) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert_eq!(
        output_str
            .matches(r#"call void asm sideeffect "nop""#)
            .count(),
        0,
        "No NOPs for IR without block labels"
    );
}

// ── String XOR tests ────────────────────────────────────────────────────────

#[test]
fn test_string_xor_basic() {
    let source = r#"char *msg = "Hello";"#;
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(), // default key 0xAA
    };

    let (output, applied) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert!(applied.contains(&"ast.string_xor".to_string()));
    assert!(output_str.contains("xor_str_0"), "Missing variable name");
    assert!(output_str.contains("^=0xAA"), "Missing XOR decode op");
    assert!(
        output_str.contains("({"),
        "Missing statement expression open"
    );
    assert!(
        output_str.contains("})"),
        "Missing statement expression close"
    );
}

#[test]
fn test_string_xor_multiple_keys() {
    let source = r#"char *msg = "Test";"#;
    let keys: &[(&str, &str)] = &[
        ("0x00", "0x00"),
        ("0xFF", "0xFF"),
        ("0x42", "0x42"),
        ("0xAA", "0xAA"),
    ];

    for &(key_param, expected_hex) in keys {
        let mutation = MutationSpec {
            id: "ast.string_xor".to_string(),
            params: [("xor_key".to_string(), key_param.to_string())]
                .into_iter()
                .collect(),
        };
        let (output, _) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains(&format!("^={}", expected_hex)),
            "Key {} not reflected in output",
            key_param
        );
    }
}

#[test]
fn test_string_xor_preserves_pragmas() {
    let source = common::c_source_pragma_and_strings();
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };

    let (output, _) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    // #pragma string should remain as-is
    assert!(
        output_str.contains(r#""user32""#),
        "Pragma string should not be XOR-encoded"
    );
    // Regular string SHOULD be encoded
    assert!(
        output_str.contains("xor_str_"),
        "Regular string should be XOR-encoded"
    );
}

#[test]
fn test_string_xor_empty_string() {
    let source = r#"char *s = "";"#;
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };

    // Should not panic
    let (output, applied) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
    let _output_str = String::from_utf8(output).unwrap();
    assert!(applied.contains(&"ast.string_xor".to_string()));
}

#[test]
fn test_string_xor_unterminated() {
    let source = r#"char *s = "unterminated"#;
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };

    let (output, _) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
    // Unterminated string → original returned unchanged
    assert_eq!(output, source.as_bytes());
}

#[test]
fn test_string_xor_counter_increments() {
    let source = r#"
int main() {
    char *a = "first";
    char *b = "second";
    char *c = "third";
}
"#;
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };

    let (output, _) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("xor_str_0"), "Missing xor_str_0");
    assert!(output_str.contains("xor_str_1"), "Missing xor_str_1");
    assert!(output_str.contains("xor_str_2"), "Missing xor_str_2");
}

// ── General mutation engine tests ───────────────────────────────────────────

#[test]
fn test_no_mutations_passthrough() {
    let source = "int main() { return 0; }";
    let (output, applied) = Mutator::apply(source.as_bytes(), &[]).unwrap();

    assert_eq!(output, source.as_bytes(), "Output should equal input");
    assert!(applied.is_empty(), "Applied list should be empty");
}

#[test]
fn test_unknown_mutation_skipped() {
    let source = "int x = 42;";
    let mutation = MutationSpec {
        id: "unknown.mutation".to_string(),
        params: HashMap::new(),
    };

    let (output, applied) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();

    assert_eq!(output, source.as_bytes());
    assert!(applied.is_empty());
}

#[test]
fn test_multiple_mutations_chained() {
    // AST mutations (string_xor) run in phase 1, then IR mutations (nop_insert)
    // run in phase 2. Both should be applied.
    let c_source = "int main() {\nentry:\n  char *s = \"hi\";\n  return 0;\n}\n";
    let mutations = vec![
        MutationSpec {
            id: "ast.string_xor".to_string(),
            params: HashMap::new(),
        },
        MutationSpec {
            id: "llvm.nop_insert".to_string(),
            params: [("density".to_string(), "1.0".to_string())]
                .into_iter()
                .collect(),
        },
    ];

    let (output, applied) = Mutator::apply(c_source.as_bytes(), &mutations).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert_eq!(applied.len(), 2, "Both mutations should be applied");
    assert!(applied.contains(&"ast.string_xor".to_string()));
    assert!(applied.contains(&"llvm.nop_insert".to_string()));
    // string_xor should have transformed the string
    assert!(output_str.contains("xor_str_"));
    // nop_insert should have inserted a NOP after "entry:" (asm sideeffect
    // survives even though the "nop" string literal gets XOR-encoded)
    assert!(output_str.contains("asm sideeffect"));
}

#[test]
fn test_mutation_spec_parse() {
    let spec = MutationSpec {
        id: "llvm.nop_insert".to_string(),
        params: HashMap::new(),
    };
    assert_eq!(spec.parse(), ("llvm", "nop_insert"));

    let spec2 = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };
    assert_eq!(spec2.parse(), ("ast", "string_xor"));

    // Invalid format (no dot)
    let spec3 = MutationSpec {
        id: "invalid".to_string(),
        params: HashMap::new(),
    };
    assert_eq!(spec3.parse(), ("unknown", ""));
}

// ── Edge cases: invalid params ──────────────────────────────────────────────

#[test]
fn test_string_xor_invalid_key_uses_default() {
    let source = r#"char *msg = "Hi";"#;
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: [("xor_key".to_string(), "not_a_number".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, applied) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    // Invalid key should fall back to default 0xAA
    assert!(applied.contains(&"ast.string_xor".to_string()));
    assert!(
        output_str.contains("^=0xAA"),
        "Invalid key should fall back to default 0xAA"
    );
}

#[test]
fn test_nop_insert_invalid_density_uses_default() {
    let ir = common::ir_multi_block();
    let mutation = MutationSpec {
        id: "llvm.nop_insert".to_string(),
        params: [("density".to_string(), "abc".to_string())]
            .into_iter()
            .collect(),
    };

    // Should not panic — invalid density falls back to default 0.3
    let (output, applied) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let _output_str = String::from_utf8(output).unwrap();
    assert!(applied.contains(&"llvm.nop_insert".to_string()));
}

// ── Edge cases: idempotency and ordering ────────────────────────────────────

#[test]
fn test_string_xor_applied_twice() {
    // Applying string_xor twice should transform any NEW string literals
    // produced by the first pass (there should be none, since XOR produces
    // statement expressions not string literals)
    let source = r#"char *msg = "Test";"#;
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };

    let (first_output, _) =
        Mutator::apply(source.as_bytes(), std::slice::from_ref(&mutation)).unwrap();
    let (second_output, _) = Mutator::apply(&first_output, &[mutation]).unwrap();

    let first_str = String::from_utf8(first_output.clone()).unwrap();
    let second_str = String::from_utf8(second_output).unwrap();

    // First pass should produce xor_str_0
    assert!(first_str.contains("xor_str_0"));
    // Second pass should not add more xor_str_ (no string literals left to transform)
    // The encoded hex values inside ({...}) are not string literals
    assert_eq!(
        first_str, second_str,
        "Second string_xor pass should be a no-op (no new string literals)"
    );
}

#[test]
fn test_ast_before_ir_ordering() {
    // With 2-way routing, AST mutations (including string_xor) always run
    // in phase 1 and IR mutations in phase 2, regardless of input order.
    let source = "entry:\n  char *s = \"hello\";\n";

    let xor = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };
    let nop = MutationSpec {
        id: "llvm.nop_insert".to_string(),
        params: [("density".to_string(), "1.0".to_string())]
            .into_iter()
            .collect(),
    };

    // Both orderings should produce identical output (AST always runs before IR)
    let (out_a, applied_a) =
        Mutator::apply(source.as_bytes(), &[xor.clone(), nop.clone()]).unwrap();
    let (out_b, applied_b) = Mutator::apply(source.as_bytes(), &[nop, xor]).unwrap();

    assert_eq!(applied_a.len(), 2);
    assert_eq!(applied_b.len(), 2);

    let a_str = String::from_utf8(out_a).unwrap();
    let b_str = String::from_utf8(out_b).unwrap();

    assert_eq!(
        a_str, b_str,
        "Input order should not affect output (AST always runs before IR)"
    );

    assert!(a_str.contains("xor_str_"), "Strings should be XOR-encoded");
    assert!(
        a_str.contains("asm sideeffect"),
        "NOP asm should be present"
    );
}

// ── Edge cases: string content ──────────────────────────────────────────────

#[test]
fn test_string_xor_with_escape_sequences() {
    let source = r#"char *msg = "Hello\nWorld\t!";"#;
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };

    // Should not panic on escape sequences
    let (output, applied) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(applied.contains(&"ast.string_xor".to_string()));
    assert!(output_str.contains("xor_str_0"));
}

// ── Opaque predicate integration tests ───────────────────────────────────

#[test]
fn test_opaque_predicate_density_1() {
    let ir = common::ir_multi_block(); // 2 unconditional branches
    let mutation = MutationSpec {
        id: "llvm.opaque_predicate".to_string(),
        params: [("density".to_string(), "1.0".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, applied) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert!(applied.contains(&"llvm.opaque_predicate".to_string()));
    // Default mode is "robust" — uses inline asm, not trivial `icmp eq i32 0, 0`
    let asm_count = output_str.matches("asm sideeffect \"xor $0, $0\"").count();
    assert_eq!(
        asm_count, 2,
        "Expected 2 robust opaque predicates, got {}",
        asm_count
    );
}

#[test]
fn test_opaque_predicate_density_0() {
    let ir = common::ir_multi_block();
    let mutation = MutationSpec {
        id: "llvm.opaque_predicate".to_string(),
        params: [("density".to_string(), "0.0".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, _) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert_eq!(output_str.matches("icmp eq i32 0, 0").count(), 0);
}

#[test]
fn test_opaque_predicate_no_unconditional() {
    // IR with only conditional branches
    let ir = r#"define void @f() {
entry:
  %cmp = icmp eq i32 1, 1
  br i1 %cmp, label %a, label %b
a:
  ret void
b:
  ret void
}
"#;
    let mutation = MutationSpec {
        id: "llvm.opaque_predicate".to_string(),
        params: [("density".to_string(), "1.0".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, _) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert_eq!(
        output_str.matches("icmp eq i32 0, 0").count(),
        0,
        "No unconditional branches → no opaque predicates"
    );
}

#[test]
fn test_opaque_predicate_default_density() {
    let ir = common::ir_multi_block();
    let mutation = MutationSpec {
        id: "llvm.opaque_predicate".to_string(),
        params: HashMap::new(), // defaults to 0.3
    };

    let (output, applied) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let _output_str = String::from_utf8(output).unwrap();
    assert!(applied.contains(&"llvm.opaque_predicate".to_string()));
}

// ── Junk block integration tests ─────────────────────────────────────────

#[test]
fn test_junk_block_default_count() {
    let ir = common::ir_multi_block();
    let mutation = MutationSpec {
        id: "llvm.junk_block".to_string(),
        params: HashMap::new(), // defaults to count=2
    };

    let (output, applied) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert!(applied.contains(&"llvm.junk_block".to_string()));
    assert_eq!(
        output_str.matches("unreachable").count(),
        2,
        "Default count=2"
    );
}

#[test]
fn test_junk_block_custom_count() {
    let ir = common::ir_multi_block();
    let mutation = MutationSpec {
        id: "llvm.junk_block".to_string(),
        params: [("count".to_string(), "4".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, _) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert_eq!(output_str.matches("unreachable").count(), 4);
}

#[test]
fn test_junk_block_no_functions() {
    let ir = common::ir_no_blocks(); // declarations only
    let mutation = MutationSpec {
        id: "llvm.junk_block".to_string(),
        params: [("count".to_string(), "5".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, _) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert_eq!(
        output_str.matches("unreachable").count(),
        0,
        "Declarations only → no junk blocks"
    );
}

#[test]
fn test_junk_block_preserves_structure() {
    let ir = common::ir_multi_block();
    let mutation = MutationSpec {
        id: "llvm.junk_block".to_string(),
        params: [("count".to_string(), "3".to_string())]
            .into_iter()
            .collect(),
    };

    let (output, _) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    for line in ir.lines() {
        assert!(
            output_str.contains(line),
            "Original line missing: {:?}",
            line
        );
    }
}

// ── Combined IR mutations integration test ───────────────────────────────

#[test]
fn test_all_three_ir_mutations_combined() {
    let ir = common::ir_multi_block();
    let mutations = vec![
        MutationSpec {
            id: "llvm.nop_insert".to_string(),
            params: [("density".to_string(), "1.0".to_string())]
                .into_iter()
                .collect(),
        },
        MutationSpec {
            id: "llvm.opaque_predicate".to_string(),
            params: [("density".to_string(), "1.0".to_string())]
                .into_iter()
                .collect(),
        },
        MutationSpec {
            id: "llvm.junk_block".to_string(),
            params: [("count".to_string(), "2".to_string())]
                .into_iter()
                .collect(),
        },
    ];

    let (output, applied) = Mutator::apply(ir.as_bytes(), &mutations).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    assert_eq!(applied.len(), 3, "All 3 IR mutations should apply");
    assert!(
        output_str.contains("asm sideeffect \"nop\""),
        "NOPs present"
    );
    assert!(
        output_str.contains("asm sideeffect \"xor $0, $0\""),
        "Robust opaque predicates present"
    );
    assert!(output_str.contains("unreachable"), "Junk blocks present");
}

// ── String content edge cases (unchanged) ────────────────────────────────

#[test]
fn test_string_xor_with_include_directive() {
    // #include strings should be skipped (similar to #pragma).
    // Need >100 chars of non-string content between #include and the regular
    // string so the 100-char recent_chars buffer flushes the #include context.
    let source = r#"#include "myheader.h"
int setup_subsystem(void);
int initialize_module(void);
int verify_preconditions(void);
int run_preflight_checks_and_validate_all_configuration(void);
int main() { char *s = "test"; return 0; }
"#;
    let mutation = MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    };

    let (output, _) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    // #include string should NOT be transformed
    assert!(
        output_str.contains(r#""myheader.h""#),
        "#include string should be preserved"
    );
    // Regular string SHOULD be transformed
    assert!(
        output_str.contains("xor_str_"),
        "Regular string should be XOR-encoded"
    );
}
