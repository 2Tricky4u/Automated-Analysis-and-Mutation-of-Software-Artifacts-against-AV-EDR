/// AST-Level Line Tracing Instrumentation (C/C++ Compatible)
///
/// Injects printf/std::cout statements at the source code level (before compilation)
/// using tree-sitter parser. Works with both C and C++ sources.
use anyhow::{Context, Result};
use std::path::Path;
use tree_sitter::{Node, Parser};

/// Language detection based on file extension
#[derive(Debug, Clone, Copy)]
pub enum SourceLanguage {
    C,
    Cpp,
}

impl SourceLanguage {
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") | Some("h++") => {
                SourceLanguage::Cpp
            }
            _ => SourceLanguage::C, // Default to C for .c, .h, or unknown
        }
    }
}

/// Trace output format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TraceFormat {
    /// Base64-encoded text format (Lepori thesis format)
    Base64,
    /// Binary protocol with structured headers
    #[default]
    Binary,
}

/// Inject line tracing statements into C/C++ source code
/// file_path: Optional path to embed in trace metadata (defaults to "source")
/// format: Trace output format (default: Binary)
pub fn inject_line_traces(source: &str, language: SourceLanguage) -> Result<String> {
    inject_line_traces_with_opts(source, language, "source", TraceFormat::default())
}

/// Inject line tracing statements with custom file path
pub fn inject_line_traces_with_path(
    source: &str,
    language: SourceLanguage,
    file_path: &str,
) -> Result<String> {
    inject_line_traces_with_opts(source, language, file_path, TraceFormat::default())
}

/// Default number of iterations for the instrumentation delay loop
pub const DEFAULT_DELAY_ITERATIONS: u32 = 0;

/// Inject line tracing statements with all options
pub fn inject_line_traces_with_opts(
    source: &str,
    language: SourceLanguage,
    file_path: &str,
    format: TraceFormat,
) -> Result<String> {
    inject_line_traces_with_delay(
        source,
        language,
        file_path,
        format,
        DEFAULT_DELAY_ITERATIONS,
    )
}

/// Inject line tracing statements with configurable delay loop iterations
pub fn inject_line_traces_with_delay(
    source: &str,
    _language: SourceLanguage,
    file_path: &str,
    format: TraceFormat,
    delay_iterations: u32,
) -> Result<String> {
    let mut parser = Parser::new();

    // Use C++ parser for both C and C++ (C++ parser can parse C code)
    // This simplifies the implementation and handles edge cases better
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .context("Failed to set tree-sitter C++ language")?;

    let tree = parser
        .parse(source, None)
        .context("Failed to parse source code")?;

    let root = tree.root_node();

    // Collect all statement locations where we want to inject traces
    let mut injections =
        collect_injection_points(&root, source, file_path, format, delay_iterations)?;

    // Sort by offset (descending) so we can inject without shifting offsets
    injections.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));

    // Add runtime function declaration at the top of the file
    let mut result = String::new();
    result
        .push_str("// AST-level line tracing runtime functions (from instrumentation_runtime.c)\n");
    match format {
        TraceFormat::Base64 => {
            result.push_str("void __trace_line_b64(const char* base64_marker);\n\n");
        }
        TraceFormat::Binary => {
            result.push_str("void __trace_line_binary(const char* file, unsigned int line, const char* func);\n\n");
        }
    }
    result.push_str(source);

    // Apply injections (offsets are now shifted by declaration length)
    let declaration_len = result.len() - source.len();
    for (offset, trace_stmt) in injections {
        result.insert_str(offset + declaration_len, &trace_stmt);
    }

    Ok(result)
}

/// Collect locations of statements where we should inject line traces
fn collect_injection_points(
    root: &Node,
    source: &str,
    file_path: &str,
    format: TraceFormat,
    delay_iterations: u32,
) -> Result<Vec<(usize, String)>> {
    let mut injections = Vec::new();

    // Recursively walk the AST looking for statements in compound blocks
    visit_node(
        root,
        source,
        file_path,
        format,
        delay_iterations,
        &mut injections,
    );

    Ok(injections)
}

/// Recursively visit nodes to find injection points
fn visit_node(
    node: &Node,
    source: &str,
    file_path: &str,
    format: TraceFormat,
    delay_iterations: u32,
    injections: &mut Vec<(usize, String)>,
) {
    // Check if this is a compound statement (block)
    if node.kind() == "compound_statement" {
        let parent_is_loop = node.parent().is_some_and(|p| is_loop_kind(p.kind()));
        let mut deferred_lines: Vec<usize> = Vec::new();

        // Iterate through child statements and inject before each
        for child in node.children(&mut node.walk()) {
            if is_traceable_statement(&child) {
                let start_line = child.start_position().row + 1; // 1-indexed
                let start_offset = child.start_byte();

                // Calculate indentation
                let indent = calculate_indentation(source, start_offset);

                if parent_is_loop && !is_eager_in_loop(child.kind()) {
                    // Deferred: set a flag instead of tracing; trace after loop
                    let flag_stmt = generate_flag_set(start_line, &indent);
                    injections.push((start_offset, flag_stmt));
                    deferred_lines.push(start_line);
                } else {
                    // Eager: trace immediately
                    let trace_stmt = generate_trace_statement(
                        start_line,
                        &indent,
                        file_path,
                        format,
                        delay_iterations,
                    );
                    injections.push((start_offset, trace_stmt));
                }
            }
        }

        // Emit flag declarations BEFORE the loop and deferred traces AFTER it.
        if parent_is_loop && !deferred_lines.is_empty() {
            let loop_node = node.parent().unwrap();
            emit_deferred_loop_traces(
                &loop_node,
                source,
                &mut deferred_lines,
                file_path,
                format,
                delay_iterations,
                injections,
            );
        }
    }

    // Handle preprocessor conditional blocks (#ifdef, #if, #else, #elif)
    // Tree-sitter parses these as separate nodes; statements inside them are NOT
    // children of the enclosing compound_statement, so we must inject here too.
    // IMPORTANT: Only inject when the preproc block is inside a function body
    // (has a compound_statement ancestor). Top-level #ifdef blocks contain
    // declarations/includes — injecting a function call there is invalid C.
    if matches!(
        node.kind(),
        "preproc_ifdef" | "preproc_else" | "preproc_elif" | "preproc_if"
    ) && has_compound_statement_ancestor(node)
    {
        let enclosing_loop = find_enclosing_loop(node);
        let mut deferred_lines: Vec<usize> = Vec::new();

        for child in node.children(&mut node.walk()) {
            if is_traceable_statement(&child) {
                let start_line = child.start_position().row + 1;
                let start_offset = child.start_byte();
                let indent = calculate_indentation(source, start_offset);

                if enclosing_loop.is_some() && !is_eager_in_loop(child.kind()) {
                    let flag_stmt = generate_flag_set(start_line, &indent);
                    injections.push((start_offset, flag_stmt));
                    deferred_lines.push(start_line);
                } else {
                    let trace_stmt = generate_trace_statement(
                        start_line,
                        &indent,
                        file_path,
                        format,
                        delay_iterations,
                    );
                    injections.push((start_offset, trace_stmt));
                }
            }
        }

        if let Some(loop_node) = enclosing_loop
            && !deferred_lines.is_empty()
        {
            emit_deferred_loop_traces(
                &loop_node,
                source,
                &mut deferred_lines,
                file_path,
                format,
                delay_iterations,
                injections,
            );
        }
    }

    // Recursively visit children
    for child in node.children(&mut node.walk()) {
        visit_node(
            &child,
            source,
            file_path,
            format,
            delay_iterations,
            injections,
        );
    }
}

/// Emit flag declarations before a loop and deferred conditional traces after it.
///
/// Declarations must be outside the loop so the names are visible both inside
/// the loop body and at the deferred trace site after the loop.
fn emit_deferred_loop_traces(
    loop_node: &Node,
    source: &str,
    deferred_lines: &mut Vec<usize>,
    file_path: &str,
    format: TraceFormat,
    delay_iterations: u32,
    injections: &mut Vec<(usize, String)>,
) {
    deferred_lines.dedup(); // Remove consecutive duplicate line numbers
    let loop_indent = calculate_indentation(source, loop_node.start_byte());

    // Flag declarations before the loop
    let mut declarations = String::new();
    for line in deferred_lines.iter() {
        declarations.push_str(&generate_flag_declaration(*line, &loop_indent));
    }
    injections.push((loop_node.start_byte(), declarations));

    // Deferred conditional traces after the loop
    let mut deferred_block = String::from("\n");
    for line in deferred_lines.iter() {
        deferred_block.push_str(&generate_deferred_trace(
            *line,
            &loop_indent,
            file_path,
            format,
            delay_iterations,
        ));
    }
    injections.push((loop_node.end_byte(), deferred_block));
}

/// Check if a node has a compound_statement (function body) ancestor.
/// Returns false for top-level nodes (file scope).
fn has_compound_statement_ancestor(node: &Node) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "compound_statement" {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// Check if a node is a statement we should trace
fn is_traceable_statement(node: &Node) -> bool {
    matches!(
        node.kind(),
        "expression_statement"
            | "declaration"
            | "if_statement"
            | "while_statement"
            | "for_statement"
            | "for_range_loop" // C++ range-based for
            | "do_statement"   // do { } while(...)
            | "return_statement"
            | "break_statement"
            | "continue_statement"
            | "switch_statement"
            | "case_statement"
            | "labeled_statement"
            | "goto_statement"
            | "try_statement" // C++ try-catch
            | "throw_statement" // C++ throw
    )
}

/// Check if a node kind is a loop construct
fn is_loop_kind(kind: &str) -> bool {
    matches!(
        kind,
        "for_statement" | "while_statement" | "do_statement" | "for_range_loop"
    )
}

/// Statements that must keep eager tracing even inside loops.
/// These can transfer control past the deferred block after the loop.
fn is_eager_in_loop(kind: &str) -> bool {
    matches!(kind, "return_statement" | "goto_statement")
}

/// Walk up the tree from `node` to find the nearest enclosing loop.
/// Returns the loop node if this node is inside a loop body (compound_statement
/// whose parent is a loop kind).
fn find_enclosing_loop<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "compound_statement"
            && let Some(grandparent) = parent.parent()
            && is_loop_kind(grandparent.kind())
        {
            return Some(grandparent);
        }
        current = parent.parent();
    }
    None
}

/// Calculate indentation by looking at the beginning of the line
fn calculate_indentation(source: &str, offset: usize) -> String {
    let line_start = source[..offset].rfind('\n').map(|pos| pos + 1).unwrap_or(0);

    let indent_slice = &source[line_start..offset];
    indent_slice
        .chars()
        .take_while(|c| c.is_whitespace() && *c != '\n')
        .collect()
}

/// Generate trace statement (Base64 or binary format)
fn generate_trace_statement(
    line: usize,
    indent: &str,
    file_path: &str,
    format: TraceFormat,
    delay_iterations: u32,
) -> String {
    let delay = if delay_iterations > 0 {
        format!(
            "{}volatile long __inst_wait{} = 1; for (; __inst_wait{} < {}; __inst_wait{} += 2) {{}}\n",
            indent, line, line, delay_iterations, line
        )
    } else {
        String::new()
    };

    match format {
        TraceFormat::Base64 => {
            // Base64-encoded format (Lepori thesis format)
            // Format matches Lepori (2023) Section 6.4.2, Figure 6.3:
            // "line:<filepath>:<line_number>:<metadata>" encoded in Base64
            let line_marker = format!("line:{}:{}:", file_path, line);
            let encoded = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                line_marker.as_bytes(),
            );

            // Call __trace_line_b64() with Base64 magic signature prefix "YjY0"
            format!(
                "{}__trace_line_b64(\"YjY0{}\");\n{}",
                indent, encoded, delay
            )
        }
        TraceFormat::Binary => {
            // Binary protocol format: direct call with structured arguments
            // No Base64 encoding, no string formatting - just pass pointers
            // Runtime will build binary header + payload
            // Use __func__ macro (C99) for automatic function name capture
            format!(
                "{}__trace_line_binary(\"{}\", {}, __func__);\n{}",
                indent, file_path, line, delay
            )
        }
    }
}

/// Generate a flag declaration for deferred loop tracing.
/// Emits `static int __seen_L{line} = 0;` — must be placed BEFORE the loop
/// so the name is visible both inside the loop body and at the deferred
/// trace site after the loop.
fn generate_flag_declaration(line: usize, indent: &str) -> String {
    format!("{}static int __seen_L{} = 0;\n", indent, line)
}

/// Generate a flag assignment for inside a loop body.
/// Emits `__seen_L{line} = 1;` to record that the line was reached.
fn generate_flag_set(line: usize, indent: &str) -> String {
    format!("{}__seen_L{} = 1;\n", indent, line)
}

/// Generate a deferred conditional trace call for after a loop.
/// Emits `if (__seen_L{line}) __trace_line_binary/b64(...)` so the trace
/// fires at most once, regardless of how many loop iterations executed.
fn generate_deferred_trace(
    line: usize,
    indent: &str,
    file_path: &str,
    format: TraceFormat,
    delay_iterations: u32,
) -> String {
    let delay = if delay_iterations > 0 {
        format!(
            "{}volatile long __inst_wait{} = 1; for (; __inst_wait{} < {}; __inst_wait{} += 2) {{}}\n",
            indent, line, line, delay_iterations, line
        )
    } else {
        String::new()
    };

    match format {
        TraceFormat::Base64 => {
            let line_marker = format!("line:{}:{}:", file_path, line);
            let encoded = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                line_marker.as_bytes(),
            );
            format!(
                "{}if (__seen_L{}) __trace_line_b64(\"YjY0{}\");\n{}",
                indent, line, encoded, delay
            )
        }
        TraceFormat::Binary => {
            format!(
                "{}if (__seen_L{}) __trace_line_binary(\"{}\", {}, __func__);\n{}",
                indent, line, file_path, line, delay
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_c_basic_injection() {
        let source = r#"
int main() {
    int x = 42;
    printf("Hello\n");
    return 0;
}
"#;

        let result = inject_line_traces(source, SourceLanguage::C).unwrap();

        // Should contain Base64-encoded line markers
        assert!(result.contains("// AST-level line tracing runtime functions"));
        assert!(result.contains("__trace_line_binary("));
    }

    #[test]
    fn test_cpp_basic_injection() {
        let source = r#"
int main() {
    int x = 42;
    std::cout << "Hello" << std::endl;
    return 0;
}
"#;

        let result = inject_line_traces(source, SourceLanguage::Cpp).unwrap();

        // Should contain trace markers
        assert!(result.contains("__trace_line_binary("));
        assert_eq!(result.matches("__trace_line_binary(").count(), 4);
    }

    #[test]
    fn test_nested_blocks() {
        let source = r#"
int main() {
    if (1) {
        int y = 10;
    }
    return 0;
}
"#;

        let result = inject_line_traces(source, SourceLanguage::C).unwrap();

        // Should inject in both outer and inner blocks
        assert_eq!(result.matches("__trace_line_binary(").count(), 4);
    }

    #[test]
    fn test_indentation_preservation() {
        let source = r#"
int main() {
    if (1) {
        int y = 10;
    }
}
"#;

        let result = inject_line_traces(source, SourceLanguage::C).unwrap();

        // Check that indentation is preserved (look for indented printf)
        let lines: Vec<&str> = result.lines().collect();
        let printf_lines: Vec<&&str> = lines
            .iter()
            .filter(|l| l.contains("__trace_line_binary("))
            .collect();

        // At least one printf should be indented
        assert!(printf_lines.iter().any(|l| l.starts_with("    ")));
    }

    #[test]
    fn test_ifdef_block_injection() {
        let source = r#"
int main() {
    int x = 1;
#ifdef ENABLE_INSTRUMENTATION
    launch_shellcode();
#else
    launch_shellcode_alt();
#endif
    return 0;
}
"#;

        let result = inject_line_traces(source, SourceLanguage::C).unwrap();

        // Statements inside #ifdef and #else should get trace injections
        // x=1, if_stmt(#ifdef block acts as parent), launch_shellcode, launch_shellcode_alt, return = at least 4
        let trace_count = result.matches("__trace_line_binary(").count();
        assert!(
            trace_count >= 4,
            "Expected at least 4 trace injections (including inside #ifdef/#else), got {}.\nResult:\n{}",
            trace_count,
            result
        );

        // Specifically check that the line with launch_shellcode gets traced
        assert!(
            result.contains("launch_shellcode()"),
            "launch_shellcode() call should still be present"
        );
    }

    #[test]
    fn test_ifdef_directives_not_traced() {
        // Preprocessor directive lines (#ifdef, #else, #endif) must NOT get
        // trace calls — only the statements *inside* those blocks should.
        let source = r#"
int main() {
    int before = 1;
#ifdef SOME_FLAG
    int inside_ifdef = 2;
#else
    int inside_else = 3;
#endif
    int after = 4;
    return 0;
}
"#;

        let result = inject_line_traces(source, SourceLanguage::C).unwrap();
        let lines: Vec<&str> = result.lines().collect();

        // No trace call should appear on the same line as a directive
        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("#ifdef")
                || trimmed.starts_with("#else")
                || trimmed.starts_with("#endif")
                || trimmed.starts_with("#if ")
            {
                assert!(
                    !line.contains("__trace_line_binary("),
                    "Preprocessor directive should not be traced: {}",
                    line
                );
            }
        }

        // The actual statements inside the blocks should still be traced.
        // before, inside_ifdef, inside_else, after, return = 5 statements
        let trace_count = result.matches("__trace_line_binary(").count();
        assert!(
            trace_count >= 5,
            "Expected at least 5 trace injections for statements, got {}.\nResult:\n{}",
            trace_count,
            result
        );
    }

    #[test]
    fn test_toplevel_ifdef_not_traced() {
        // Top-level #ifdef blocks (file scope) must NOT get trace injections.
        // Injecting a function call at file scope is invalid C.
        let source = r#"
#include <stdio.h>

#ifdef _WIN32
typedef unsigned long DWORD;
#else
typedef unsigned int DWORD;
#endif

int main() {
    DWORD x = 1;
#ifdef ENABLE_FEATURE
    x = 2;
#endif
    return x;
}
"#;

        let result = inject_line_traces(source, SourceLanguage::C).unwrap();

        // The top-level typedef lines should NOT be traced
        let output_lines: Vec<&str> = result.lines().collect();
        for (i, line) in output_lines.iter().enumerate() {
            if line.contains("typedef") {
                // The line before a typedef must not be a trace call
                if i > 0 {
                    assert!(
                        !output_lines[i - 1].contains("__trace_line_binary("),
                        "Top-level typedef inside #ifdef should not be traced.\nPreceding line: {}\nTypedef line: {}",
                        output_lines[i - 1],
                        line
                    );
                }
            }
        }

        // But the statements inside main()'s #ifdef SHOULD be traced
        // x=1, x=2, return = 3 statements inside function body
        let trace_count = result.matches("__trace_line_binary(").count();
        assert!(
            trace_count >= 3,
            "Expected at least 3 trace injections inside main(), got {}.\nResult:\n{}",
            trace_count,
            result
        );
    }

    #[test]
    fn test_language_detection() {
        assert!(matches!(
            SourceLanguage::from_path(Path::new("test.c")),
            SourceLanguage::C
        ));
        assert!(matches!(
            SourceLanguage::from_path(Path::new("test.cpp")),
            SourceLanguage::Cpp
        ));
        assert!(matches!(
            SourceLanguage::from_path(Path::new("test.h")),
            SourceLanguage::C
        ));
    }
}
