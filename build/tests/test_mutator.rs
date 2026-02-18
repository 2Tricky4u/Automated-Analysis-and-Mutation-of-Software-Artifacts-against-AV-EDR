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
    // Use a C source that has both a block-like label (for nop_insert) and a
    // string literal (for string_xor). nop_insert looks for non-indented lines
    // ending with ':', and string_xor looks for quoted strings.
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
    assert!(output_str.contains("xor_str_0"));
    // nop_insert should have inserted a NOP after "entry:"
    assert!(output_str.contains(r#"call void asm sideeffect "nop""#));
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
fn test_mutation_order_matters_when_cross_producing_content() {
    // nop_insert inserts `call void asm sideeffect "nop", ""()` which contains
    // string literals. If string_xor runs after nop_insert, it transforms those
    // strings too. This test documents that mutation order CAN matter.
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

    // xor-first: only "hello" is XOR'd, then NOP is inserted cleanly
    let (out_xf, applied_xf) =
        Mutator::apply(source.as_bytes(), &[xor.clone(), nop.clone()]).unwrap();
    // nop-first: NOP is inserted (with "nop" and "" strings), then XOR transforms all strings
    let (out_nf, applied_nf) = Mutator::apply(source.as_bytes(), &[nop, xor]).unwrap();

    // Both orders should successfully apply both mutations
    assert_eq!(applied_xf.len(), 2);
    assert_eq!(applied_nf.len(), 2);

    let xf_str = String::from_utf8(out_xf).unwrap();
    let nf_str = String::from_utf8(out_nf).unwrap();

    // xor-first: "hello" is XOR'd, NOP asm string is clean
    assert!(
        xf_str.contains("xor_str_0"),
        "xor-first should XOR the hello string"
    );
    assert!(
        xf_str.contains(r#"sideeffect "nop""#),
        "xor-first: NOP string should be clean"
    );

    // nop-first: NOP's "nop" and "" strings are ALSO XOR'd by the second pass
    assert!(nf_str.contains("xor_str_"), "nop-first should XOR strings");
    // The NOP asm's string literals get transformed too
    assert!(
        !nf_str.contains(r#"sideeffect "nop""#),
        "nop-first: NOP string should be XOR-transformed by second pass"
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
