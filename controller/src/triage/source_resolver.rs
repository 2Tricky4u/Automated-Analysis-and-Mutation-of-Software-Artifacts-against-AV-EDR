//! Source line resolver — maps trace line numbers to assembled C source code.
//!
//! Trace events contain `{"line": 189, "func": "carrier", ...}`. This module
//! resolves `line:189` into the actual C code at that line, plus the enclosing
//! function name (heuristic-based).

use std::collections::HashSet;

/// Per-function coverage statistics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FunctionCoverage {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub executed_lines: usize,
    pub percent: f64,
}

/// Aggregate coverage result for an assembled source.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CoverageResult {
    pub total_lines: usize,
    pub total_executable: usize,
    pub executed_lines: usize,
    pub coverage_percent: f64,
    pub cutoff_line: Option<usize>,
    pub cutoff_func: Option<String>,
    pub functions: Vec<FunctionCoverage>,
}

/// A resolved source line with context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLine {
    /// 1-based line number.
    pub line: usize,
    /// Trimmed source code at that line.
    pub code: String,
    /// Enclosing function name (heuristic).
    pub func: Option<String>,
}

/// Pre-parsed source for efficient line lookups.
pub struct SourceMap {
    lines: Vec<String>,
    func_ranges: Vec<FuncRange>,
}

struct FuncRange {
    name: String,
    start_line: usize, // 1-based
    end_line: usize,   // 1-based
}

impl SourceMap {
    /// Build from assembled source text.
    ///
    /// Parses all lines and detects C function boundaries using a brace-depth
    /// heuristic: scans for `type name(` patterns and tracks `{` / `}` depth.
    pub fn new(source: &str) -> Self {
        let lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();
        let func_ranges = detect_functions(&lines);
        Self { lines, func_ranges }
    }

    /// Resolve a single 1-based line number to code + function context.
    pub fn resolve(&self, line: usize) -> Option<ResolvedLine> {
        if line == 0 || line > self.lines.len() {
            return None;
        }
        let code = self.lines[line - 1].trim().to_string();
        let func = self.function_at(line);
        Some(ResolvedLine { line, code, func })
    }

    /// Batch-resolve multiple line numbers. Skips invalid lines.
    #[allow(dead_code)]
    pub fn resolve_many(&self, lines: &[usize]) -> Vec<ResolvedLine> {
        lines.iter().filter_map(|&l| self.resolve(l)).collect()
    }

    /// Total line count.
    #[allow(dead_code)]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Compute coverage statistics from a set of executed line numbers (1-based).
    pub fn compute_coverage(&self, executed: &HashSet<usize>) -> CoverageResult {
        let total_lines = self.lines.len();

        // Cutoff: highest executed line number
        let cutoff_line = executed.iter().copied().max();
        let cutoff_func = cutoff_line.and_then(|l| self.function_at(l));

        // Per-function coverage (skip signature line — only body lines are instrumented)
        let functions: Vec<FunctionCoverage> = self
            .func_ranges
            .iter()
            .map(|fr| {
                let mut func_total = 0usize;
                let mut func_executed = 0usize;
                // Skip signature line (start_line) — it's the return type / name, not executable
                for ln in (fr.start_line + 1)..=fr.end_line {
                    if ln <= self.lines.len() && is_executable_line(&self.lines[ln - 1]) {
                        func_total += 1;
                        if executed.contains(&ln) {
                            func_executed += 1;
                        }
                    }
                }
                let percent = if func_total > 0 {
                    (func_executed as f64 / func_total as f64) * 100.0
                } else {
                    0.0
                };
                FunctionCoverage {
                    name: fr.name.clone(),
                    start_line: fr.start_line,
                    end_line: fr.end_line,
                    total_lines: func_total,
                    executed_lines: func_executed,
                    percent,
                }
            })
            .collect();

        // Global coverage = sum of per-function stats (only function body lines
        // are instrumented, so counting lines outside functions inflates the denominator)
        let total_executable: usize = functions.iter().map(|f| f.total_lines).sum();
        let executed_count: usize = functions.iter().map(|f| f.executed_lines).sum();
        let coverage_percent = if total_executable > 0 {
            (executed_count as f64 / total_executable as f64) * 100.0
        } else {
            0.0
        };

        CoverageResult {
            total_lines,
            total_executable,
            executed_lines: executed_count,
            coverage_percent,
            cutoff_line,
            cutoff_func,
            functions,
        }
    }

    /// Find the enclosing function for a 1-based line number.
    fn function_at(&self, line: usize) -> Option<String> {
        self.func_ranges
            .iter()
            .find(|r| line >= r.start_line && line <= r.end_line)
            .map(|r| r.name.clone())
    }
}

/// Classify a source line as "executable" (non-trivial).
///
/// Returns false for blank lines, preprocessor directives, lone braces,
/// and comment-only lines.
fn is_executable_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('#') {
        return false;
    }
    if trimmed == "{" || trimmed == "}" {
        return false;
    }
    if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
        return false;
    }
    true
}

/// Convenience: resolve one line without building a full SourceMap.
/// Returns the raw (untrimmed) source line.
#[allow(dead_code)]
pub fn resolve_line(source: &str, line: usize) -> Option<&str> {
    if line == 0 {
        return None;
    }
    source.lines().nth(line - 1)
}

/// Detect C function boundaries by scanning for signatures and tracking braces.
///
/// Heuristic: a line matching `<tokens> <name>(<...>)` followed (possibly after
/// more lines) by `{` starts a function. The function ends when brace depth
/// returns to 0.
fn detect_functions(lines: &[String]) -> Vec<FuncRange> {
    let mut ranges = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        // Skip preprocessor, blank, comments
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
        {
            i += 1;
            continue;
        }

        // Try to detect a function signature: `type name(params)` possibly with `{`
        if let Some(name) = try_parse_func_signature(trimmed) {
            // Find the opening brace (may be on same line or next lines)
            let mut brace_line = i;
            let mut found_open = trimmed.contains('{');

            if !found_open {
                // Look ahead a few lines for the opening brace
                for (j, line) in lines.iter().enumerate().skip(i + 1).take(3) {
                    if line.contains('{') {
                        brace_line = j;
                        found_open = true;
                        break;
                    }
                }
            }

            if found_open {
                let start_line = i + 1; // 1-based
                // Track brace depth from the opening brace line
                let mut depth = 0i32;
                let mut end_line = brace_line + 1; // 1-based, default

                for (k, line) in lines.iter().enumerate().skip(brace_line) {
                    for ch in line.chars() {
                        match ch {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    end_line = k + 1; // 1-based
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    if depth == 0 && k >= brace_line {
                        // Check if we actually opened (depth went positive then back to 0)
                        end_line = k + 1;
                        break;
                    }
                }

                ranges.push(FuncRange {
                    name,
                    start_line,
                    end_line,
                });

                i = end_line; // skip past the function body (0-based index)
                continue;
            }
        }

        i += 1;
    }

    ranges
}

/// Try to extract a function name from a C function signature line.
///
/// Matches patterns like:
/// - `int main(int argc, char** argv)`
/// - `void carrier()`
/// - `static DWORD WINAPI thread_func(LPVOID param)`
/// - `BOOL CALLBACK enum_cb(HWND hwnd, LPARAM lp) {`
///
/// Does NOT match: typedefs, declarations without body, struct/enum.
fn try_parse_func_signature(line: &str) -> Option<String> {
    let line = line.trim();

    // Skip obvious non-functions
    if line.starts_with("typedef")
        || line.starts_with("struct ")
        || line.starts_with("enum ")
        || line.starts_with("union ")
        || line.ends_with(';')
    {
        return None;
    }

    // Must contain '(' for a function signature
    let paren_pos = line.find('(')?;

    // The token immediately before '(' is the function name.
    // Walk backwards from paren_pos to find the identifier.
    let before_paren = &line[..paren_pos];
    let name = before_paren
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .next_back()?;

    // Must have at least one token before the name (the return type)
    let name_start = before_paren.rfind(name)?;
    let before_name = before_paren[..name_start].trim();
    if before_name.is_empty() {
        return None;
    }

    // Reject if "name" is a keyword or macro-like all-caps that's likely not a function
    let keywords = [
        "if", "else", "for", "while", "switch", "return", "sizeof", "typeof", "defined",
    ];
    if keywords.contains(&name) {
        return None;
    }

    Some(name.to_string())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SOURCE: &str = r#"#include <stdio.h>
#include <windows.h>

// Anti-emulation module
void antiemulation() {
    for (int i = 0; i < 100; i++) {
        Sleep(1);
    }
}

// Carrier function
int carrier(unsigned char* buf, size_t len) {
    void* mem = VirtualAlloc(NULL, len, MEM_COMMIT, PAGE_READWRITE);
    if (!mem) return -1;
    memcpy(mem, buf, len);
    DWORD old;
    VirtualProtect(mem, len, PAGE_EXECUTE_READ, &old);
    ((void(*)())mem)();
    return 0;
}

int main(int argc, char** argv) {
    antiemulation();
    unsigned char payload[] = { 0x90, 0x90 };
    return carrier(payload, sizeof(payload));
}"#;

    #[test]
    fn resolve_line_basic() {
        let source = "line one\nline two\nline three";
        assert_eq!(resolve_line(source, 1), Some("line one"));
        assert_eq!(resolve_line(source, 2), Some("line two"));
        assert_eq!(resolve_line(source, 3), Some("line three"));
    }

    #[test]
    fn resolve_line_out_of_bounds() {
        let source = "line one\nline two";
        assert_eq!(resolve_line(source, 0), None);
        assert_eq!(resolve_line(source, 3), None);
        assert_eq!(resolve_line(source, 100), None);
    }

    #[test]
    fn resolve_line_empty_source() {
        assert_eq!(resolve_line("", 1), None);
    }

    #[test]
    fn source_map_line_count() {
        let map = SourceMap::new(SAMPLE_SOURCE);
        assert_eq!(map.line_count(), SAMPLE_SOURCE.lines().count());
    }

    #[test]
    fn source_map_resolve_valid_lines() {
        let map = SourceMap::new(SAMPLE_SOURCE);

        // Line 1 = #include <stdio.h>
        let resolved = map.resolve(1).unwrap();
        assert_eq!(resolved.line, 1);
        assert_eq!(resolved.code, "#include <stdio.h>");
        assert_eq!(resolved.func, None); // outside any function

        // Line 6: inside antiemulation()
        let resolved = map.resolve(6).unwrap();
        assert_eq!(resolved.line, 6);
        assert!(resolved.code.contains("for"));
        assert_eq!(resolved.func, Some("antiemulation".to_string()));

        // Line 13: inside carrier()
        let resolved = map.resolve(13).unwrap();
        assert!(resolved.code.contains("VirtualAlloc"));
        assert_eq!(resolved.func, Some("carrier".to_string()));
    }

    #[test]
    fn source_map_resolve_out_of_bounds() {
        let map = SourceMap::new(SAMPLE_SOURCE);
        assert!(map.resolve(0).is_none());
        assert!(map.resolve(9999).is_none());
    }

    #[test]
    fn source_map_resolve_many() {
        let map = SourceMap::new(SAMPLE_SOURCE);
        let results = map.resolve_many(&[1, 6, 9999, 13, 0]);
        // Should skip 9999 and 0
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].line, 1);
        assert_eq!(results[1].line, 6);
        assert_eq!(results[2].line, 13);
    }

    #[test]
    fn function_detection_multiple_functions() {
        let map = SourceMap::new(SAMPLE_SOURCE);

        // Should detect antiemulation, carrier, and main
        // Verify via resolve
        let line5 = map.resolve(5).unwrap(); // void antiemulation() {
        assert_eq!(line5.func, Some("antiemulation".to_string()));

        let line12 = map.resolve(12).unwrap(); // inside carrier (first line of body or sig)
        // Line 12 = "int carrier(...) {"
        assert_eq!(line12.func, Some("carrier".to_string()));

        let line24 = map.resolve(24).unwrap(); // inside main
        assert_eq!(line24.func, Some("main".to_string()));
    }

    #[test]
    fn function_detection_nested_braces() {
        let source = r#"void foo() {
    if (x) {
        if (y) {
            do_stuff();
        }
    }
}

void bar() {
    return;
}"#;
        let map = SourceMap::new(source);

        // Line 4 (do_stuff) should be inside foo
        let resolved = map.resolve(4).unwrap();
        assert_eq!(resolved.func, Some("foo".to_string()));

        // Line 7 (closing brace of foo) should still be inside foo
        let resolved = map.resolve(7).unwrap();
        assert_eq!(resolved.func, Some("foo".to_string()));

        // Line 10 (return) should be inside bar
        let resolved = map.resolve(10).unwrap();
        assert_eq!(resolved.func, Some("bar".to_string()));
    }

    #[test]
    fn compute_coverage_basic() {
        let map = SourceMap::new(SAMPLE_SOURCE);
        // Execute some lines inside antiemulation (5-9) and main (22-25)
        let executed: HashSet<usize> = [5, 6, 7, 8, 9, 22, 23, 24, 25].iter().copied().collect();
        let cov = map.compute_coverage(&executed);

        assert!(cov.total_lines > 0);
        assert!(cov.total_executable > 0);
        assert!(cov.executed_lines > 0);
        assert!(cov.coverage_percent > 0.0);
        assert!(cov.coverage_percent <= 100.0);
        assert_eq!(cov.cutoff_line, Some(25));
        assert_eq!(cov.cutoff_func, Some("main".to_string()));

        // Should have 3 functions detected
        assert_eq!(cov.functions.len(), 3);

        // antiemulation should have coverage
        let ae = cov
            .functions
            .iter()
            .find(|f| f.name == "antiemulation")
            .unwrap();
        assert!(ae.executed_lines > 0);
        assert!(ae.percent > 0.0);

        // carrier should have 0 coverage (no lines executed there)
        let carr = cov.functions.iter().find(|f| f.name == "carrier").unwrap();
        assert_eq!(carr.executed_lines, 0);
        assert_eq!(carr.percent, 0.0);
    }

    #[test]
    fn compute_coverage_empty_executed() {
        let map = SourceMap::new(SAMPLE_SOURCE);
        let executed: HashSet<usize> = HashSet::new();
        let cov = map.compute_coverage(&executed);

        assert_eq!(cov.executed_lines, 0);
        assert_eq!(cov.coverage_percent, 0.0);
        assert_eq!(cov.cutoff_line, None);
        assert_eq!(cov.cutoff_func, None);
    }

    #[test]
    fn is_executable_line_cases() {
        assert!(!is_executable_line(""));
        assert!(!is_executable_line("   "));
        assert!(!is_executable_line("#include <stdio.h>"));
        assert!(!is_executable_line("  {"));
        assert!(!is_executable_line("}"));
        assert!(!is_executable_line("// comment"));
        assert!(!is_executable_line("/* block comment */"));
        assert!(!is_executable_line(" * middle of block comment"));
        assert!(is_executable_line("int x = 5;"));
        assert!(is_executable_line("  return 0;"));
    }

    #[test]
    fn try_parse_func_signature_cases() {
        assert_eq!(
            try_parse_func_signature("int main(int argc, char** argv)"),
            Some("main".to_string())
        );
        assert_eq!(
            try_parse_func_signature("void carrier()"),
            Some("carrier".to_string())
        );
        assert_eq!(
            try_parse_func_signature("static DWORD WINAPI thread_func(LPVOID p) {"),
            Some("thread_func".to_string())
        );
        // Should not match
        assert_eq!(
            try_parse_func_signature("typedef void (*callback_t)(int);"),
            None
        );
        assert_eq!(try_parse_func_signature("#include <stdio.h>"), None);
        assert_eq!(try_parse_func_signature("if (x > 0) {"), None);
        assert_eq!(
            try_parse_func_signature("for (int i = 0; i < n; i++) {"),
            None
        );
    }
}
