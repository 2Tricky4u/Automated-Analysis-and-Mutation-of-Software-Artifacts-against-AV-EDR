mod common;

use build::{SourceLanguage, TraceFormat, inject_line_traces, inject_line_traces_with_opts};
use std::path::Path;

#[test]
fn test_injection_count_per_source() {
    // Use `__trace_line_binary("` to count only actual trace calls, excluding
    // the `void __trace_line_binary(const char*...)` declaration in the header.

    // Minimal: `int main() { return 0; }` → 1 statement (return)
    let minimal = inject_line_traces(common::c_source_minimal(), SourceLanguage::C).unwrap();
    assert_eq!(
        minimal.matches("__trace_line_binary(\"").count(),
        1,
        "minimal source: expected 1 trace call"
    );

    // Nested blocks: if { while { decl } } + return → 4 statements
    let nested = inject_line_traces(common::c_source_nested_blocks(), SourceLanguage::C).unwrap();
    assert_eq!(
        nested.matches("__trace_line_binary(\"").count(),
        4,
        "nested source: expected 4 trace calls"
    );

    // Empty body: `void f() {}` → 0 statements
    let empty = inject_line_traces(common::c_source_empty_body(), SourceLanguage::C).unwrap();
    assert_eq!(
        empty.matches("__trace_line_binary(\"").count(),
        0,
        "empty body: expected 0 trace calls"
    );
}

#[test]
fn test_binary_format_default() {
    let source = common::c_source_minimal();
    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    assert!(
        result.contains("__trace_line_binary("),
        "Default format should use __trace_line_binary"
    );
    assert!(
        !result.contains("__trace_line_b64("),
        "Default format should NOT use __trace_line_b64"
    );
}

#[test]
fn test_base64_format_explicit() {
    let source = common::c_source_minimal();
    let result = inject_line_traces_with_opts(
        source,
        SourceLanguage::C,
        "test.c",
        TraceFormat::Base64,
    )
    .unwrap();

    assert!(
        result.contains("__trace_line_b64("),
        "Base64 format should use __trace_line_b64"
    );
    assert!(
        !result.contains("__trace_line_binary("),
        "Base64 format should NOT use __trace_line_binary"
    );
}

#[test]
fn test_file_path_embedded() {
    let source = common::c_source_minimal();
    let custom_path = "my/custom/file.c";
    let result = inject_line_traces_with_opts(
        source,
        SourceLanguage::C,
        custom_path,
        TraceFormat::Binary,
    )
    .unwrap();

    assert!(
        result.contains(custom_path),
        "Custom file path should appear in trace arguments"
    );
}

#[test]
fn test_runtime_declarations_present() {
    let source = common::c_source_minimal();

    // Binary format
    let bin = inject_line_traces_with_opts(source, SourceLanguage::C, "s", TraceFormat::Binary)
        .unwrap();
    assert!(
        bin.contains("void __trace_line_binary("),
        "Missing binary runtime declaration"
    );

    // Base64 format
    let b64 = inject_line_traces_with_opts(source, SourceLanguage::C, "s", TraceFormat::Base64)
        .unwrap();
    assert!(
        b64.contains("void __trace_line_b64("),
        "Missing base64 runtime declaration"
    );
}

#[test]
fn test_indentation_preserved() {
    let source = common::c_source_nested_blocks();
    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    let trace_lines: Vec<&str> = result
        .lines()
        .filter(|l| l.contains("__trace_line_binary("))
        .collect();

    // Inner block (while body) should have more indentation than outer block
    assert!(
        trace_lines.len() >= 2,
        "Expected at least 2 trace lines for indentation check"
    );

    // At least one trace should be indented with 4+ spaces (outer)
    assert!(
        trace_lines.iter().any(|l| l.starts_with("    ")),
        "Expected at least one trace with 4-space indent"
    );
    // At least one should be more deeply indented (inner block)
    assert!(
        trace_lines.iter().any(|l| l.starts_with("        ")),
        "Expected at least one trace with 8-space indent"
    );
}

#[test]
fn test_c_vs_cpp_language() {
    let c_source = common::c_source_minimal();
    let cpp_source = common::c_source_cpp_style();

    let c_result = inject_line_traces(c_source, SourceLanguage::C);
    assert!(c_result.is_ok(), "C parsing should succeed");

    let cpp_result = inject_line_traces(cpp_source, SourceLanguage::Cpp);
    assert!(cpp_result.is_ok(), "C++ parsing should succeed");
}

#[test]
fn test_traceable_statement_types() {
    // Source that exercises: expression_statement, declaration, if, while, for, return
    let source = r#"int main() {
    int x = 0;
    x = x + 1;
    if (x > 0) {
        x = 2;
    }
    while (x > 0) {
        x--;
    }
    for (int i = 0; i < 10; i++) {
        x += i;
    }
    return x;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();
    let count = result.matches("__trace_line_binary(\"").count();

    // declaration(x), expression(x=x+1), if, expression(x=2),
    // while, expression(x--), for, expression(x+=i), return = 9
    assert!(
        count >= 9,
        "Expected at least 9 trace calls for all statement types, got {}",
        count
    );
}

#[test]
fn test_delay_loop_injected() {
    let source = common::c_source_minimal();
    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // Count only actual trace calls (not the declaration) by matching the opening quote
    let trace_count = result.matches("__trace_line_binary(\"").count();
    let delay_count = result.matches("__inst_wait").count();

    // Each trace call should have a corresponding delay loop.
    // The delay loop uses __inst_wait<N> three times per injection.
    assert!(
        delay_count > 0,
        "Expected at least one __inst_wait delay loop"
    );
    assert_eq!(
        delay_count,
        trace_count * 3,
        "__inst_wait count should be 3x trace count (3 references per delay loop)"
    );
}

#[test]
fn test_empty_source() {
    let result = inject_line_traces("", SourceLanguage::C).unwrap();

    // Should succeed with only the declaration header
    assert!(
        result.contains("// AST-level line tracing runtime functions"),
        "Should have declaration header"
    );
    assert_eq!(
        result.matches("__trace_line_binary(\"").count(),
        0,
        "No trace calls for empty source"
    );
}

#[test]
fn test_source_with_only_comments() {
    let source = "// This is a comment\n/* Another comment */\n";
    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // No traceable statements in comments
    assert_eq!(
        result.matches("__trace_line_binary(\"").count(),
        0,
        "No trace calls for comment-only source"
    );
}

#[test]
fn test_language_detection_from_path() {
    assert!(matches!(
        SourceLanguage::from_path(Path::new("file.c")),
        SourceLanguage::C
    ));
    assert!(matches!(
        SourceLanguage::from_path(Path::new("file.cpp")),
        SourceLanguage::Cpp
    ));
    assert!(matches!(
        SourceLanguage::from_path(Path::new("file.cc")),
        SourceLanguage::Cpp
    ));
    assert!(matches!(
        SourceLanguage::from_path(Path::new("file.cxx")),
        SourceLanguage::Cpp
    ));
    assert!(matches!(
        SourceLanguage::from_path(Path::new("file.hpp")),
        SourceLanguage::Cpp
    ));
    assert!(matches!(
        SourceLanguage::from_path(Path::new("file.h")),
        SourceLanguage::C
    ));
    // Unknown extensions default to C
    assert!(matches!(
        SourceLanguage::from_path(Path::new("file.rs")),
        SourceLanguage::C
    ));
}

// ── Preprocessor and complex source tests ───────────────────────────────────

#[test]
fn test_trace_with_preprocessor_directives() {
    let source = r#"
#include <stdio.h>
#ifdef _WIN32
#define MY_CONST 42
#endif

int main() {
    int x = MY_CONST;
    return x;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // Should successfully parse and inject traces
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert!(
        trace_count >= 2,
        "Expected at least 2 traces (declaration + return), got {}",
        trace_count
    );

    // Preprocessor directives should NOT have traces injected into them
    // (they're not statements inside compound_statement)
    assert!(
        result.contains("#include <stdio.h>"),
        "Preprocessor directives should be preserved"
    );
    assert!(
        result.contains("#ifdef _WIN32"),
        "#ifdef should be preserved"
    );
}

#[test]
fn test_trace_with_function_pointers() {
    let source = r#"
typedef void (*func_ptr)(void);

int main() {
    func_ptr fp = (func_ptr)0x12345678;
    fp();
    return 0;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert!(
        trace_count >= 3,
        "Expected traces for declaration, expression, return; got {}",
        trace_count
    );
}

#[test]
fn test_trace_multifunction_source() {
    let source = r#"
void helper() {
    int a = 1;
    int b = 2;
}

int process(int x) {
    if (x > 0) {
        return x;
    }
    return 0;
}

int main() {
    helper();
    int result = process(42);
    return result;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // Should inject traces in all three functions
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert!(
        trace_count >= 8,
        "Expected traces in all 3 functions (8+), got {}",
        trace_count
    );
}

#[test]
fn test_trace_with_switch_statement() {
    let source = r#"
int main() {
    int x = 2;
    switch(x) {
        case 1:
            x = 10;
            break;
        case 2:
            x = 20;
            break;
        default:
            x = 0;
            break;
    }
    return x;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // switch, case, break, and return are all traceable statements
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert!(
        trace_count >= 5,
        "Expected traces for switch/case/break/return, got {}",
        trace_count
    );
}

#[test]
fn test_trace_preserves_code_semantics() {
    // Verify that trace injection doesn't break the structure
    // by checking that all original non-whitespace lines are present
    let source = r#"int main() {
    int x = 42;
    x = x + 1;
    return x;
}"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            assert!(
                result.contains(trimmed),
                "Original code line '{}' should be preserved after trace injection",
                trimmed
            );
        }
    }
}
