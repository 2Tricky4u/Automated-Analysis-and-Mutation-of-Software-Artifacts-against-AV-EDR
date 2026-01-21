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

/// Inject line tracing statements with all options
pub fn inject_line_traces_with_opts(
    source: &str,
    language: SourceLanguage,
    file_path: &str,
    format: TraceFormat,
) -> Result<String> {
    let mut parser = Parser::new();

    // Use C++ parser for both C and C++ (C++ parser can parse C code)
    // This simplifies the implementation and handles edge cases better
    parser
        .set_language(&tree_sitter_cpp::language())
        .context("Failed to set tree-sitter C++ language")?;

    let tree = parser
        .parse(source, None)
        .context("Failed to parse source code")?;

    let root = tree.root_node();

    // Collect all statement locations where we want to inject traces
    let mut injections = collect_injection_points(&root, source, language, file_path, format)?;

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
    language: SourceLanguage,
    file_path: &str,
    format: TraceFormat,
) -> Result<Vec<(usize, String)>> {
    let mut injections = Vec::new();

    // Recursively walk the AST looking for statements in compound blocks
    visit_node(root, source, language, file_path, format, &mut injections);

    Ok(injections)
}

/// Recursively visit nodes to find injection points
fn visit_node(
    node: &Node,
    source: &str,
    language: SourceLanguage,
    file_path: &str,
    format: TraceFormat,
    injections: &mut Vec<(usize, String)>,
) {
    // Check if this is a compound statement (block)
    if node.kind() == "compound_statement" {
        // Iterate through child statements and inject before each
        for child in node.children(&mut node.walk()) {
            if is_traceable_statement(&child) {
                let start_line = child.start_position().row + 1; // 1-indexed
                let start_offset = child.start_byte();

                // Calculate indentation
                let indent = calculate_indentation(source, start_offset);

                // Generate trace statement with file path and line number
                let trace_stmt =
                    generate_trace_statement(start_line, &indent, language, file_path, format);

                injections.push((start_offset, trace_stmt));
            }
        }
    }

    // Recursively visit children
    for child in node.children(&mut node.walk()) {
        visit_node(&child, source, language, file_path, format, injections);
    }
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
    _language: SourceLanguage,
    file_path: &str,
    format: TraceFormat,
) -> String {
    let delay = format!(
        "{}volatile long __inst_wait{} = 1; for (; __inst_wait{} < 10000; __inst_wait{} += 2) {{}}\n",
        indent, line, line, line
    );

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
