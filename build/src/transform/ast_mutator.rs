//! AST-level mutations using tree-sitter for structural code transforms.
//!
//! Generic over all module types — matches MutationSpec IDs to @MUTATE markers:
//!   MutationSpec { id: "ast.<marker_name>", params } → all @MUTATE:<marker_name> locations
//!
//! Supported mutations (research-informed deconditioning):
//! - `ast.decon_rounds`          — iteration count (EDR cutoff threshold)
//! - `ast.fill_pattern`          — benign data content (entropy / memory scan)
//! - `ast.exec_decoy`            — execute from allocated memory (#1 EDR signal)
//! - `ast.timing_pattern`        — inter-operation delays (temporal tokens)
//! - `ast.protection_transition` — memory protection pattern (rule match)
//!
//! Adding a new mutation:
//!   1. Add a `// @MUTATE:<name>` marker in the C template
//!   2. Add a match arm in `apply_at_marker()`
//!   3. Implement the handler method

use anyhow::{Context, Result};
use std::collections::HashMap;
use tracing::{debug, info, warn};
use tree_sitter::Parser;

use crate::mutator::MutationSpec;
use crate::template::assembler::{MutationMarker, extract_mutation_markers};

/// AST-level mutator backed by tree-sitter.
pub struct AstMutator {
    parser: Parser,
}

impl AstMutator {
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        let language = tree_sitter_c::LANGUAGE;
        parser
            .set_language(&language.into())
            .context("Failed to set tree-sitter C language")?;
        Ok(Self { parser })
    }

    /// Apply AST mutations to source code.
    ///
    /// Returns the mutated source and a list of applied mutation IDs.
    pub fn apply(
        &mut self,
        source: &str,
        mutations: &[&MutationSpec],
    ) -> Result<(String, Vec<String>)> {
        let mut result = source.to_string();
        let mut applied = Vec::new();

        for mutation in mutations {
            let (_category, name) = mutation.parse();

            let markers = extract_mutation_markers(&result);
            let matching: Vec<_> = markers
                .iter()
                .filter(|m| m.name == name || m.name.starts_with(&format!("{}(", name)))
                .collect();

            if matching.is_empty() {
                warn!("No @MUTATE:{} markers found in source", name);
                continue;
            }

            // Parse current source with tree-sitter (for handlers that need the AST)
            let _tree = self.parser.parse(&result, None);

            // Apply bottom-up (reverse line order) to preserve line numbers
            let count = matching.len();
            for marker in matching.into_iter().rev() {
                result = self.apply_at_marker(&result, marker, name, &mutation.params)?;
            }

            applied.push(mutation.id.clone());
            info!("Applied ast.{} at {} location(s)", name, count);
        }

        Ok((result, applied))
    }

    fn apply_at_marker(
        &self,
        source: &str,
        marker: &MutationMarker,
        name: &str,
        params: &HashMap<String, String>,
    ) -> Result<String> {
        match name {
            "decon_rounds" => self.apply_decon_rounds(source, marker, params),
            "fill_pattern" => self.apply_fill_pattern(source, marker, params),
            "exec_decoy" => self.apply_exec_decoy(source, marker, params),
            "timing_pattern" => self.apply_timing_pattern(source, marker, params),
            "protection_transition" => self.apply_protection_transition(source, marker, params),
            _ => {
                debug!("Unimplemented AST mutation: {} (skipping)", name);
                Ok(source.to_string())
            }
        }
    }

    // ── decon_rounds ──────────────────────────────────────────────────────

    fn apply_decon_rounds(
        &self,
        source: &str,
        marker: &MutationMarker,
        params: &HashMap<String, String>,
    ) -> Result<String> {
        let count: u32 = params
            .get("count")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        let method = params.get("method").map(|s| s.as_str()).unwrap_or("fixed");

        let lines: Vec<&str> = source.lines().collect();
        let marker_idx = marker.line - 1; // 1-indexed → 0-indexed

        // Find the for-loop line after the marker
        let for_idx = (marker_idx + 1..lines.len()).find(|&i| lines[i].trim().starts_with("for"));

        let Some(for_idx) = for_idx else {
            warn!(
                "decon_rounds: no for-loop found after marker at line {}",
                marker.line
            );
            return Ok(source.to_string());
        };

        let indent: String = lines[for_idx]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

        match method {
            "fixed" => {
                out[for_idx] = out[for_idx].replace("DECON_ROUNDS", &count.to_string());
            }
            "runtime" => {
                let half = count / 2;
                let init = format!(
                    "{}int __decon_n = (GetTickCount() % {}) + {};",
                    indent, count, half
                );
                out[for_idx] = out[for_idx].replace("DECON_ROUNDS", "__decon_n");
                out.insert(for_idx, init);
            }
            _ => {
                warn!("decon_rounds: unknown method '{}', skipping", method);
            }
        }

        Ok(out.join("\n"))
    }

    // ── fill_pattern ──────────────────────────────────────────────────────

    fn apply_fill_pattern(
        &self,
        source: &str,
        marker: &MutationMarker,
        params: &HashMap<String, String>,
    ) -> Result<String> {
        let pattern = params.get("pattern").map(|s| s.as_str()).unwrap_or("xor");

        if pattern == "xor" {
            return Ok(source.to_string()); // default, no change
        }

        let lines: Vec<&str> = source.lines().collect();
        let marker_idx = marker.line - 1;

        // Find the fill loop start
        let fill_start =
            (marker_idx + 1..lines.len()).find(|&i| lines[i].trim().starts_with("for"));

        let Some(fill_start) = fill_start else {
            warn!(
                "fill_pattern: no for-loop found after marker at line {}",
                marker.line
            );
            return Ok(source.to_string());
        };

        let fill_end = find_block_end(&lines, fill_start);
        let indent: String = lines[fill_start]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();

        let replacement = match pattern {
            "nop_sled" => format!(
                "{indent}for (int k = 0; k < PAYLOAD_LEN - 1; k++) {{\n\
                 {indent}    buf[k] = (char)0x90;\n\
                 {indent}}}\n\
                 {indent}buf[PAYLOAD_LEN - 1] = (char)0xC3;"
            ),
            "random" => format!(
                "{indent}{{ unsigned int __rs = (unsigned int)(i * 2654435761u + 0xDEAD);\n\
                 {indent}  for (int k = 0; k < PAYLOAD_LEN; k++) {{\n\
                 {indent}    __rs ^= __rs << 13; __rs ^= __rs >> 17; __rs ^= __rs << 5;\n\
                 {indent}    buf[k] = (char)(__rs & 0xFF);\n\
                 {indent}  }}\n\
                 {indent}}}"
            ),
            "zero" => format!("{indent}memset(buf, 0, PAYLOAD_LEN);"),
            _ => {
                warn!("fill_pattern: unknown pattern '{}', skipping", pattern);
                return Ok(source.to_string());
            }
        };

        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        out.splice(
            fill_start..=fill_end,
            replacement.lines().map(|l| l.to_string()),
        );

        Ok(out.join("\n"))
    }

    // ── exec_decoy ────────────────────────────────────────────────────────

    fn apply_exec_decoy(
        &self,
        source: &str,
        marker: &MutationMarker,
        params: &HashMap<String, String>,
    ) -> Result<String> {
        let method = params.get("method").map(|s| s.as_str()).unwrap_or("none");

        if method == "none" {
            return Ok(source.to_string());
        }

        let lines: Vec<&str> = source.lines().collect();
        let marker_idx = marker.line - 1;

        // Determine indentation from the next code line
        let next_code = (marker_idx + 1..lines.len())
            .find(|&i| !lines[i].trim().is_empty() && !lines[i].trim().starts_with("//"));
        let indent = next_code
            .map(|i| {
                lines[i]
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .collect::<String>()
            })
            .unwrap_or_else(|| "        ".to_string());

        let exec_code = match method {
            "direct" => format!("{indent}((void(*)())buf)();"),
            "thread" => format!(
                "{indent}{{ HANDLE __ht = CreateThread(NULL, 0, (LPTHREAD_START_ROUTINE)buf, NULL, 0, NULL);\n\
                 {indent}  if (__ht) {{ WaitForSingleObject(__ht, 5000); CloseHandle(__ht); }}\n\
                 {indent}}}"
            ),
            _ => {
                warn!("exec_decoy: unknown method '{}', skipping", method);
                return Ok(source.to_string());
            }
        };

        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        // Insert after the marker line
        for (offset, line) in exec_code.lines().enumerate() {
            out.insert(marker_idx + 1 + offset, line.to_string());
        }

        Ok(out.join("\n"))
    }

    // ── timing_pattern ────────────────────────────────────────────────────

    fn apply_timing_pattern(
        &self,
        source: &str,
        marker: &MutationMarker,
        params: &HashMap<String, String>,
    ) -> Result<String> {
        let min_ms: u32 = params
            .get("min_ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let max_ms: u32 = params
            .get("max_ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);
        let range = max_ms.saturating_sub(min_ms).max(1);

        let lines: Vec<&str> = source.lines().collect();
        let marker_idx = marker.line - 1;

        let target_idx = (marker_idx + 1..lines.len()).find(|&i| {
            let t = lines[i].trim();
            !t.is_empty() && !t.starts_with("//")
        });

        let Some(target_idx) = target_idx else {
            warn!(
                "timing_pattern: no target statement after marker at line {}",
                marker.line
            );
            return Ok(source.to_string());
        };

        let indent: String = lines[target_idx]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let sleep_line = format!("{indent}Sleep({min_ms} + (GetTickCount() % {range}));");

        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        out.insert(target_idx, sleep_line);

        Ok(out.join("\n"))
    }

    // ── protection_transition ─────────────────────────────────────────────

    fn apply_protection_transition(
        &self,
        source: &str,
        marker: &MutationMarker,
        params: &HashMap<String, String>,
    ) -> Result<String> {
        let pattern = params.get("pattern").map(|s| s.as_str()).unwrap_or("rw_rx");

        if pattern == "rw_rx" {
            return Ok(source.to_string()); // default, no change
        }

        let lines: Vec<&str> = source.lines().collect();
        let marker_idx = marker.line - 1;

        // Find VirtualProtect line after marker
        let vp_idx = (marker_idx + 1..lines.len()).find(|&i| lines[i].contains("VirtualProtect"));

        let Some(vp_idx) = vp_idx else {
            warn!(
                "protection_transition: no VirtualProtect found after marker at line {}",
                marker.line
            );
            return Ok(source.to_string());
        };

        let indent: String = lines[vp_idx]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

        match pattern {
            "rw_rwx" => {
                out[vp_idx] = out[vp_idx].replace("p_RX", "p_RWX");
            }
            "rw_r_rx" => {
                // Insert intermediate R protection before the RX step
                let staged = format!("{indent}VirtualProtect(buf, PAYLOAD_LEN, 0x02, &old_prot);");
                out.insert(vp_idx, staged);
            }
            _ => {
                warn!(
                    "protection_transition: unknown pattern '{}', skipping",
                    pattern
                );
            }
        }

        Ok(out.join("\n"))
    }
}

impl Default for AstMutator {
    fn default() -> Self {
        Self::new().expect("Failed to initialize AstMutator")
    }
}

/// Find the end of a code block starting at `start_line`.
///
/// Tracks brace nesting to find the matching closing `}`.
/// Falls back to same line if no braces are found.
fn find_block_end(lines: &[&str], start_line: usize) -> usize {
    let mut depth: i32 = 0;
    let mut found_open = false;

    for i in start_line..lines.len() {
        for ch in lines[i].chars() {
            if ch == '{' {
                depth += 1;
                found_open = true;
            } else if ch == '}' {
                depth -= 1;
                if found_open && depth == 0 {
                    return i;
                }
            }
        }
    }

    // Fallback: if no block found, return start line
    start_line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The basic.c template content used across all tests.
    fn basic_template() -> &'static str {
        r#"#include "../header/definitions.h"

#ifndef DECON_ROUNDS
#define DECON_ROUNDS 20
#endif

void deconditioner() {
    DWORD old_prot;

    // @MUTATE:decon_rounds
    for (int i = 0; i < DECON_ROUNDS; i++) {

        char *buf = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
        if (!buf) continue;

        // @MUTATE:fill_pattern
        for (int k = 0; k < PAYLOAD_LEN; k++) {
            buf[k] = (char)(k ^ (i + 0x41));
        }

        // @MUTATE:timing_pattern
        // @MUTATE:protection_transition
        VirtualProtect(buf, PAYLOAD_LEN, p_RX, &old_prot);

        // @MUTATE:exec_decoy

        // @MUTATE:timing_pattern
        VirtualFree(buf, 0, 0x8000);
    }
}"#
    }

    fn make_spec(id: &str, params: &[(&str, &str)]) -> MutationSpec {
        MutationSpec {
            id: id.to_string(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn test_decon_rounds_fixed() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.decon_rounds", &[("count", "50"), ("method", "fixed")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.decon_rounds".to_string()));
        assert!(out.contains("i < 50"));
        assert!(!out.contains("DECON_ROUNDS;"));
    }

    #[test]
    fn test_decon_rounds_runtime() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec(
            "ast.decon_rounds",
            &[("count", "50"), ("method", "runtime")],
        );
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.decon_rounds".to_string()));
        assert!(out.contains("GetTickCount()"));
        assert!(out.contains("__decon_n"));
        assert!(out.contains("i < __decon_n"));
    }

    #[test]
    fn test_fill_pattern_nop_sled() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.fill_pattern", &[("pattern", "nop_sled")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.fill_pattern".to_string()));
        assert!(out.contains("0x90"));
        assert!(out.contains("0xC3"));
        // Original XOR fill should be gone
        assert!(!out.contains("k ^ (i + 0x41)"));
    }

    #[test]
    fn test_fill_pattern_random() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.fill_pattern", &[("pattern", "random")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.fill_pattern".to_string()));
        assert!(out.contains("2654435761u"));
        assert!(out.contains("__rs ^="));
        assert!(!out.contains("k ^ (i + 0x41)"));
    }

    #[test]
    fn test_fill_pattern_zero() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.fill_pattern", &[("pattern", "zero")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.fill_pattern".to_string()));
        assert!(out.contains("memset(buf, 0, PAYLOAD_LEN)"));
        assert!(!out.contains("k ^ (i + 0x41)"));
    }

    #[test]
    fn test_fill_pattern_xor_noop() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.fill_pattern", &[("pattern", "xor")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.fill_pattern".to_string()));
        // Default "xor" should leave source unchanged
        assert!(out.contains("k ^ (i + 0x41)"));
    }

    #[test]
    fn test_exec_decoy_direct() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.exec_decoy", &[("method", "direct")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.exec_decoy".to_string()));
        assert!(out.contains("((void(*)())buf)()"));
    }

    #[test]
    fn test_exec_decoy_thread() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.exec_decoy", &[("method", "thread")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.exec_decoy".to_string()));
        assert!(out.contains("CreateThread"));
        assert!(out.contains("WaitForSingleObject"));
        assert!(out.contains("CloseHandle"));
    }

    #[test]
    fn test_exec_decoy_none_noop() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.exec_decoy", &[("method", "none")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.exec_decoy".to_string()));
        // "none" should not add any execution code
        assert!(!out.contains("((void(*)())buf)()"));
        assert!(!out.contains("CreateThread"));
    }

    #[test]
    fn test_timing_pattern() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.timing_pattern", &[("min_ms", "50"), ("max_ms", "200")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.timing_pattern".to_string()));
        assert!(out.contains("Sleep(50 + (GetTickCount() % 150))"));
    }

    #[test]
    fn test_timing_pattern_defaults() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.timing_pattern", &[]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.timing_pattern".to_string()));
        // Default: min=10, max=100, range=90
        assert!(out.contains("Sleep(10 + (GetTickCount() % 90))"));
    }

    #[test]
    fn test_timing_pattern_two_markers() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.timing_pattern", &[("min_ms", "10"), ("max_ms", "100")]);
        let (out, _applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        // basic.c has two @MUTATE:timing_pattern markers
        let sleep_count = out.matches("Sleep(").count();
        assert_eq!(
            sleep_count, 2,
            "Expected 2 Sleep() calls, got {}",
            sleep_count
        );
    }

    #[test]
    fn test_protection_rw_rwx() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.protection_transition", &[("pattern", "rw_rwx")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.protection_transition".to_string()));
        assert!(out.contains("p_RWX"));
    }

    #[test]
    fn test_protection_staged() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.protection_transition", &[("pattern", "rw_r_rx")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.protection_transition".to_string()));
        // Should have the original VirtualProtect plus an intermediate one
        let vp_count = out.matches("VirtualProtect").count();
        assert!(
            vp_count >= 2,
            "Expected at least 2 VirtualProtect calls, got {}",
            vp_count
        );
        assert!(out.contains("0x02")); // PAGE_READONLY
    }

    #[test]
    fn test_protection_default_noop() {
        let mut ast = AstMutator::new().unwrap();
        let spec = make_spec("ast.protection_transition", &[("pattern", "rw_rx")]);
        let (out, applied) = ast.apply(basic_template(), &[&spec]).unwrap();

        assert!(applied.contains(&"ast.protection_transition".to_string()));
        // Default should not change anything
        assert!(out.contains("p_RX"));
        assert!(!out.contains("p_RWX"));
    }

    #[test]
    fn test_combined_mutations() {
        let mut ast = AstMutator::new().unwrap();

        let specs = vec![
            make_spec("ast.decon_rounds", &[("count", "50"), ("method", "fixed")]),
            make_spec("ast.fill_pattern", &[("pattern", "nop_sled")]),
            make_spec("ast.exec_decoy", &[("method", "direct")]),
        ];
        let refs: Vec<&MutationSpec> = specs.iter().collect();

        let (out, applied) = ast.apply(basic_template(), &refs).unwrap();

        assert_eq!(applied.len(), 3);
        assert!(out.contains("i < 50"));
        assert!(out.contains("0x90"));
        assert!(out.contains("((void(*)())buf)()"));
    }

    #[test]
    fn test_no_markers_graceful() {
        let mut ast = AstMutator::new().unwrap();
        let source = "int main() { return 0; }";
        let spec = make_spec("ast.decon_rounds", &[("count", "50")]);

        let (out, applied) = ast.apply(source, &[&spec]).unwrap();

        // No markers → no mutations applied, source unchanged
        assert!(applied.is_empty());
        assert_eq!(out, source);
    }

    #[test]
    fn test_unknown_mutation_skipped() {
        let mut ast = AstMutator::new().unwrap();
        let source = "// @MUTATE:unknown_thing\nint x = 42;";
        let spec = make_spec("ast.unknown_thing", &[]);

        let (out, applied) = ast.apply(source, &[&spec]).unwrap();

        // Unknown mutation is applied (returns source unchanged) but still recorded
        assert!(applied.contains(&"ast.unknown_thing".to_string()));
        assert!(out.contains("int x = 42;"));
    }

    #[test]
    fn test_find_block_end() {
        let lines = vec![
            "        for (int k = 0; k < N; k++) {",
            "            buf[k] = 0;",
            "        }",
            "        next_statement();",
        ];
        assert_eq!(find_block_end(&lines, 0), 2);
    }

    #[test]
    fn test_find_block_end_nested() {
        let lines = vec![
            "        for (int k = 0; k < N; k++) {",
            "            if (k > 5) {",
            "                buf[k] = 1;",
            "            }",
            "        }",
            "        next();",
        ];
        assert_eq!(find_block_end(&lines, 0), 4);
    }
}
