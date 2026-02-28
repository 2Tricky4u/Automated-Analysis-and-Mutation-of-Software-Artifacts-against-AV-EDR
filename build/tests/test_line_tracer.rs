mod common;

use build::{
    SourceLanguage, TraceFormat, inject_line_traces, inject_line_traces_with_delay,
    inject_line_traces_with_opts,
};
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
    let result =
        inject_line_traces_with_opts(source, SourceLanguage::C, "test.c", TraceFormat::Base64)
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
    let result =
        inject_line_traces_with_opts(source, SourceLanguage::C, custom_path, TraceFormat::Binary)
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
    let bin =
        inject_line_traces_with_opts(source, SourceLanguage::C, "s", TraceFormat::Binary).unwrap();
    assert!(
        bin.contains("void __trace_line_binary("),
        "Missing binary runtime declaration"
    );

    // Base64 format
    let b64 =
        inject_line_traces_with_opts(source, SourceLanguage::C, "s", TraceFormat::Base64).unwrap();
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
    let result = inject_line_traces_with_delay(
        source,
        SourceLanguage::C,
        "test.c",
        TraceFormat::default(),
        1000,
    )
    .unwrap();

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

// ── Deferred loop tracing tests ─────────────────────────────────────────────

#[test]
fn test_deferred_for_loop() {
    let source = r#"int main() {
    for (int i = 0; i < 1000; i++) {
        do_work();
        int x = i;
    }
    return 0;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // Loop body statements should use flag-sets, not direct trace calls
    assert!(
        result.contains("__seen_L"),
        "Loop body should use deferred flag-sets"
    );

    // Deferred conditional traces should appear after the loop
    assert!(
        result.contains("if (__seen_L"),
        "Deferred conditional traces should appear after loop"
    );

    // The for statement itself should still get an eager trace
    // for, do_work (deferred), int x (deferred), return = 2 eager + 2 deferred = 4
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert_eq!(
        trace_count, 4,
        "Expected 4 trace calls (2 eager + 2 deferred conditional), got {}.\nResult:\n{}",
        trace_count, result
    );
}

#[test]
fn test_deferred_while_loop() {
    let source = r#"int main() {
    while (1) {
        int x = 0;
        x++;
    }
    return 0;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    assert!(
        result.contains("__seen_L"),
        "While loop body should use deferred flag-sets"
    );
    assert!(
        result.contains("if (__seen_L"),
        "Deferred conditional traces should appear after while loop"
    );

    // while, x=0 (deferred), x++ (deferred), return = 2 eager + 2 deferred = 4
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert_eq!(
        trace_count, 4,
        "Expected 4 trace calls, got {}.\nResult:\n{}",
        trace_count, result
    );
}

#[test]
fn test_deferred_do_while() {
    let source = r#"int main() {
    do {
        int x = 1;
    } while (1);
    return 0;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // do_statement should be traced (bug fix: was missing from is_traceable_statement)
    // and its body should be deferred
    assert!(
        result.contains("__seen_L"),
        "do-while body should use deferred flag-sets"
    );
    assert!(
        result.contains("if (__seen_L"),
        "Deferred conditional trace should appear after do-while"
    );

    // do, x=1 (deferred), return = 2 eager + 1 deferred = 3
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert_eq!(
        trace_count, 3,
        "Expected 3 trace calls, got {}.\nResult:\n{}",
        trace_count, result
    );
}

#[test]
fn test_return_inside_loop_stays_eager() {
    let source = r#"int main() {
    for (int i = 0; i < 10; i++) {
        int x = i;
        if (x > 5) {
            return 1;
        }
    }
    return 0;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // Count eager __trace_line_binary before return statements
    // The return inside the loop should have a direct trace (not deferred)
    let lines: Vec<&str> = result.lines().collect();
    let mut found_eager_return_trace = false;
    for (i, line) in lines.iter().enumerate() {
        if line.contains("return 1;") && i > 0 {
            // The line before return should be an eager trace call (not a flag-set)
            if lines[i - 1].contains("__trace_line_binary(\"") {
                found_eager_return_trace = true;
            }
        }
    }
    assert!(
        found_eager_return_trace,
        "return statement inside loop should have eager trace, not deferred.\nResult:\n{}",
        result
    );

    // x = i should be deferred (flag-set, not eager trace)
    let mut found_deferred_flag = false;
    for (i, line) in lines.iter().enumerate() {
        if line.contains("int x = i;") && i > 0 && lines[i - 1].contains("__seen_L") {
            found_deferred_flag = true;
        }
    }
    assert!(
        found_deferred_flag,
        "declaration inside loop should use deferred flag-set.\nResult:\n{}",
        result
    );
}

#[test]
fn test_nested_loops_deferred() {
    let source = r#"int main() {
    for (int i = 0; i < 10; i++) {
        for (int j = 0; j < 10; j++) {
            int x = i + j;
        }
        int y = i;
    }
    return 0;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // Both loops' bodies should use deferred tracing
    // Inner for's body: x = i + j → deferred at inner loop end
    // Outer for's body: inner for (deferred), y = i (deferred) → at outer loop end
    assert!(
        result.contains("__seen_L"),
        "Nested loop bodies should use deferred flag-sets"
    );

    // Count deferred conditional traces
    let deferred_count = result.matches("if (__seen_L").count();
    assert!(
        deferred_count >= 3,
        "Expected at least 3 deferred traces (inner for, x, y), got {}.\nResult:\n{}",
        deferred_count,
        result
    );

    // outer for, inner for (deferred), x (deferred), y (deferred), return
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert_eq!(
        trace_count, 5,
        "Expected 5 trace calls (2 eager + 3 deferred), got {}.\nResult:\n{}",
        trace_count, result
    );
}

#[test]
fn test_non_loop_compound_unchanged() {
    // if/switch bodies (without an enclosing loop) should stay eager
    let source = r#"int main() {
    if (1) {
        int x = 1;
        int y = 2;
    }
    return 0;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // No deferred tracing — if is not a loop
    assert!(
        !result.contains("__seen_L"),
        "if body (not in a loop) should not use deferred tracing.\nResult:\n{}",
        result
    );

    // if, x=1, y=2, return = 4 eager traces
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert_eq!(
        trace_count, 4,
        "Expected 4 eager trace calls, got {}.\nResult:\n{}",
        trace_count, result
    );
}

#[test]
fn test_empty_loop_body() {
    let source = r#"int main() {
    for (int i = 0; i < 10; i++) {}
    return 0;
}
"#;

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // No deferred flags — empty body has no traceable children
    assert!(
        !result.contains("__seen_L"),
        "Empty loop body should not produce deferred flags.\nResult:\n{}",
        result
    );

    // for, return = 2 eager traces
    let trace_count = result.matches("__trace_line_binary(\"").count();
    assert_eq!(
        trace_count, 2,
        "Expected 2 trace calls (for + return), got {}.\nResult:\n{}",
        trace_count, result
    );
}

#[test]
fn test_deferred_same_line_no_redefinition() {
    // Multiple statements on the same source line inside a loop must produce
    // exactly one flag declaration (`static int __seen_L2 = 0;`) and one
    // deferred trace (`if (__seen_L2) ...`), not duplicates that would cause
    // a "redefinition of '__seen_L2'" compile error.
    let source = "int main() {\n    for (int i = 0; i < 10; i++) { int a = 1; int b = 2; }\n    return 0;\n}\n";

    let result = inject_line_traces(source, SourceLanguage::C).unwrap();

    // Both `int a = 1;` and `int b = 2;` are on line 2 → one unique deferred line
    let decl_count = result.matches("static int __seen_L2 = 0;").count();
    assert_eq!(
        decl_count, 1,
        "Expected exactly 1 flag declaration for line 2, got {} (duplicate would cause compile error).\nResult:\n{}",
        decl_count, result
    );

    let deferred_trace_count = result.matches("if (__seen_L2)").count();
    assert_eq!(
        deferred_trace_count, 1,
        "Expected exactly 1 deferred trace for line 2, got {}.\nResult:\n{}",
        deferred_trace_count, result
    );

    // The flag-set assignments inside the loop body can remain duplicated (harmless)
    let flag_set_count = result.matches("__seen_L2 = 1;").count();
    assert!(
        flag_set_count >= 2,
        "Expected at least 2 flag-set assignments inside loop body, got {}.\nResult:\n{}",
        flag_set_count,
        result
    );
}
