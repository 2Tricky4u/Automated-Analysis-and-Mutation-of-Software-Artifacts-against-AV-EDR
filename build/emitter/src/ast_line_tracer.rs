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

/// Inject line tracing statements into C/C++ source code
/// file_path: Optional path to embed in trace metadata (defaults to "source")
pub fn inject_line_traces(source: &str, language: SourceLanguage) -> Result<String> {
    inject_line_traces_with_path(source, language, "source")
}

/// Inject line tracing statements with custom file path
pub fn inject_line_traces_with_path(
    source: &str,
    language: SourceLanguage,
    file_path: &str,
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
    let mut injections = collect_injection_points(&root, source, language, file_path)?;

    // Sort by offset (descending) so we can inject without shifting offsets
    injections.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));

    // Add runtime function declaration at the top of the file
    let mut result = String::new();
    result.push_str("// AST-level line tracing runtime function (from instrumentation_runtime.c)\n");
    result.push_str("void __trace_line_b64(const char* base64_marker);\n\n");
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
) -> Result<Vec<(usize, String)>> {
    let mut injections = Vec::new();

    // Recursively walk the AST looking for statements in compound blocks
    visit_node(root, source, language, file_path, &mut injections);

    Ok(injections)
}

/// Recursively visit nodes to find injection points
fn visit_node(
    node: &Node,
    source: &str,
    language: SourceLanguage,
    file_path: &str,
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
                let trace_stmt = generate_trace_statement(start_line, &indent, language, file_path);

                injections.push((start_offset, trace_stmt));
            }
        }
    }

    // Recursively visit children
    for child in node.children(&mut node.walk()) {
        visit_node(&child, source, language, file_path, injections);
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
    let line_start = source[..offset]
        .rfind('\n')
        .map(|pos| pos + 1)
        .unwrap_or(0);

    let indent_slice = &source[line_start..offset];
    indent_slice
        .chars()
        .take_while(|c| c.is_whitespace() && *c != '\n')
        .collect()
}

/// Generate trace statement with Base64-encoded metadata (Lepori thesis format)
/// Format: "line:<filepath>:<line_number>:<metadata>" encoded in Base64
/// Writes to trace pipe via __trace_line_b64() runtime function
fn generate_trace_statement(
    line: usize,
    indent: &str,
    _language: SourceLanguage,
    file_path: &str,
) -> String {
    // Format matches Lepori (2023) Section 6.4.2, Figure 6.3:
    // "line:<filepath>:<line_number>:<metadata>"
    // For now, metadata is empty (could add function name, AST node type, etc.)
    let line_marker = format!("line:{}:{}:", file_path, line);
    let encoded =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, line_marker.as_bytes());

    // Call __trace_line_b64() with Base64 magic signature prefix "YjY0" (matches thesis Figure 6.3)
    // This writes to trace pipe/file, which worker will collect after execution
    // Also add delay mechanism (volatile loop to prevent optimization)
    // Delay allows EDR to kill process before next line executes, making truncation point clear
    format!(
        "{}__trace_line_b64(\"YjY0{}\");\n{}volatile long __inst_wait{} = 1; for (; __inst_wait{} < 10000; __inst_wait{} += 2) {{}}\n",
        indent, encoded, indent, line, line, line
    )
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
        assert!(result.contains("[TRACE:"));
        assert!(result.contains("printf(\"[TRACE:"));
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
        assert!(result.contains("[TRACE:"));
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
        assert!(result.matches("[TRACE:").count() >= 2);
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
        let printf_lines: Vec<&&str> = lines.iter().filter(|l| l.contains("printf")).collect();

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
