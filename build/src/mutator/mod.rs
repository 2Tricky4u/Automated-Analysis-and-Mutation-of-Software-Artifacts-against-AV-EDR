/// Mutation engine for AST and LLVM IR transformations
///
/// Implements CLAUDE.md Section 3: Fuzzer & Mutation Engine
///
/// Supported mutations:
/// - ast.decon_rounds:          Iteration count (tree-sitter)
/// - ast.fill_pattern:          Benign data content (tree-sitter)
/// - ast.exec_decoy:            Execute from allocated memory (tree-sitter)
/// - ast.timing_pattern:        Inter-operation delays (tree-sitter)
/// - ast.protection_transition: Memory protection pattern (tree-sitter)
/// - ast.string_xor:            XOR-encode string literals (legacy)
/// - llvm.nop_insert:           Insert NOP instructions in LLVM IR (legacy)
use anyhow::{Context, Result};
use std::collections::HashMap;
use tracing::{info, warn};

use crate::transform::AstMutator;

/// Mutation specification from proto (edr.common.Mutation)
#[derive(Debug, Clone)]
pub struct MutationSpec {
    pub id: String,
    pub params: HashMap<String, String>,
}

impl MutationSpec {
    /// Parse mutation ID into category and name
    pub fn parse(&self) -> (&str, &str) {
        let parts: Vec<&str> = self.id.split('.').collect();
        if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            warn!("Invalid mutation ID format: {}", self.id);
            ("unknown", "")
        }
    }
}

/// Mutation engine (stateless transformer)
pub struct Mutator;

impl Mutator {
    /// Apply mutations to source code or IR
    ///
    /// # Arguments
    /// * `input` - Original source code (C source or LLVM IR)
    /// * `mutations` - List of mutations to apply
    ///
    /// # Returns
    /// Transformed code and list of successfully applied mutation IDs
    ///
    /// Mutation routing:
    /// - `ast.*` (except `string_xor`) → tree-sitter `AstMutator`
    /// - `ast.string_xor`              → legacy inline transformer
    /// - `llvm.nop_insert`             → legacy IR transformer
    pub fn apply(input: &[u8], mutations: &[MutationSpec]) -> Result<(Vec<u8>, Vec<String>)> {
        if mutations.is_empty() {
            return Ok((input.to_vec(), vec![]));
        }

        let mut code = input.to_vec();
        let mut applied = Vec::new();

        // Partition: tree-sitter AST mutations vs legacy
        let (ts_mutations, legacy): (Vec<_>, Vec<_>) = mutations.iter().partition(|m| {
            let (cat, name) = m.parse();
            cat == "ast" && name != "string_xor"
        });

        // Phase 1: tree-sitter AST mutations (decon_rounds, fill_pattern, etc.)
        if !ts_mutations.is_empty() {
            let source = String::from_utf8(code.clone()).context("C source must be valid UTF-8")?;
            let mut ast = AstMutator::new().context("Failed to init tree-sitter")?;
            let refs: Vec<&MutationSpec> = ts_mutations.iter().copied().collect();
            let (mutated, ast_applied) = ast.apply(&source, &refs)?;
            code = mutated.into_bytes();
            applied.extend(ast_applied);
        }

        // Phase 2: legacy mutations (string_xor, llvm.nop_insert)
        for mutation in &legacy {
            let (category, name) = mutation.parse();

            match (category, name) {
                ("llvm", "nop_insert") => {
                    info!("Applying mutation: llvm.nop_insert");
                    code = Self::insert_llvm_nops(&code, mutation)?;
                    applied.push(mutation.id.clone());
                }
                ("ast", "string_xor") => {
                    info!("Applying mutation: ast.string_xor");
                    code = Self::xor_encode_strings(&code, mutation)?;
                    applied.push(mutation.id.clone());
                }
                _ => {
                    warn!("Unknown mutation: {}", mutation.id);
                }
            }
        }

        Ok((code, applied))
    }

    /// Insert NOP instructions in LLVM IR (control-flow jitter)
    ///
    /// Strategy: Insert `call void asm sideeffect "nop", ""()` after each basic block entry
    /// This jitters timing and makes binary signatures less predictable.
    ///
    /// Parameters:
    /// - `density` (default: 0.3): Probability of inserting NOP at each insertion point
    fn insert_llvm_nops(ir_code: &[u8], spec: &MutationSpec) -> Result<Vec<u8>> {
        let ir_text = String::from_utf8(ir_code.to_vec()).context("LLVM IR must be valid UTF-8")?;

        let density: f32 = spec
            .params
            .get("density")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.3);

        info!("NOP insertion density: {}", density);

        // Find all basic block entries (lines starting with a label, e.g., "entry:")
        let mut output = String::new();
        let mut rng_state = 1234u32; // Simple LCG for deterministic randomness

        for line in ir_text.lines() {
            output.push_str(line);
            output.push('\n');

            // Check if this is a basic block label (ends with ':' and is not indented)
            if line.ends_with(':') && !line.starts_with(' ') && !line.starts_with('\t') {
                // Simple LCG random: next = (a * current + c) mod m
                rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
                let rand_val = (rng_state >> 16) as f32 / 32768.0;

                if rand_val < density {
                    // Insert NOP inline assembly
                    output.push_str("  call void asm sideeffect \"nop\", \"\"()\n");
                }
            }
        }

        Ok(output.into_bytes())
    }

    /// XOR-encode string literals in C source (constant encoding)
    ///
    /// Strategy: Find string literals like "Hello" and replace with XOR-decoded version:
    /// Before: char *str = "Hello";
    /// After:  char str_buf[] = {0x48^0xAA, 0x65^0xAA, ...}; // XOR with key 0xAA
    ///
    /// This is a MINIMAL implementation - just demonstrates the concept.
    /// A full implementation would use tree-sitter or libclang for proper AST parsing.
    ///
    /// Parameters:
    /// - `xor_key` (default: 0xAA): XOR key for encoding
    fn xor_encode_strings(source_code: &[u8], spec: &MutationSpec) -> Result<Vec<u8>> {
        let source_text =
            String::from_utf8(source_code.to_vec()).context("C source must be valid UTF-8")?;

        let xor_key: u8 = spec
            .params
            .get("xor_key")
            .and_then(|v| {
                // Support both "0xAA" and "170" formats
                if v.starts_with("0x") || v.starts_with("0X") {
                    u8::from_str_radix(&v[2..], 16).ok()
                } else {
                    v.parse().ok()
                }
            })
            .unwrap_or(0xAA);

        info!("String XOR key: 0x{:02X}", xor_key);

        // Simple regex-like approach for demonstration
        // This is NOT production-ready (doesn't handle escape sequences, multiline strings, etc.)
        let mut output = String::new();
        let chars = source_text.chars().peekable();
        let mut in_string = false;
        let mut string_buf = String::new();
        let mut counter = 0;

        // Track last 100 chars to detect #pragma context
        let mut recent_chars = String::new();

        for ch in chars {
            if ch == '"' && !in_string {
                // Start of string literal
                in_string = true;
                string_buf.clear();
            } else if ch == '"' && in_string {
                // End of string literal - check if we should transform it
                in_string = false;

                // Skip XOR encoding for compile-time constant contexts:
                // - #pragma directives (require compile-time string literals)
                // - #include directives
                // - Other preprocessor contexts
                let should_skip = recent_chars.contains("#pragma")
                    || recent_chars.contains("#include")
                    || recent_chars.contains("#error")
                    || recent_chars.contains("#warning");

                if should_skip {
                    // Pass through original string literal
                    output.push('"');
                    output.push_str(&string_buf);
                    output.push('"');
                } else {
                    // Generate XOR-encoded array
                    let encoded: Vec<String> = string_buf
                        .bytes()
                        .map(|b| format!("0x{:02X}", b ^ xor_key))
                        .collect();

                    let var_name = format!("xor_str_{}", counter);
                    counter += 1;

                    // Replace inline string with XOR decode logic using GNU C statement expression
                    // Statement expressions ({ ... }) allow us to execute code and return a value
                    // This works in any expression context (printf args, function calls, etc.)
                    output.push_str(&format!(
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
                    ));
                }
            } else if in_string {
                // Inside string literal - buffer it
                string_buf.push(ch);
            } else {
                // Outside string literal - pass through and track context
                output.push(ch);
                recent_chars.push(ch);

                // Keep only last 100 chars for context detection
                if recent_chars.len() > 100 {
                    recent_chars.remove(0);
                }
            }
        }

        if in_string {
            // Unterminated string - return original
            warn!("Unterminated string literal detected, skipping XOR encoding");
            return Ok(source_code.to_vec());
        }

        Ok(output.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nop_insertion() {
        let ir = r#"define i32 @main() {
entry:
  %result = add i32 1, 2
  ret i32 %result
}
"#;

        let mutation = MutationSpec {
            id: "llvm.nop_insert".to_string(),
            params: [("density".to_string(), "1.0".to_string())]
                .into_iter()
                .collect(),
        };

        let (output, applied) = Mutator::apply(ir.as_bytes(), &[mutation]).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(applied.contains(&"llvm.nop_insert".to_string()));
        assert!(output_str.contains("call void asm sideeffect \"nop\""));
    }

    #[test]
    fn test_string_xor_simple() {
        let source = r#"char *msg = "Hello";"#;

        let mutation = MutationSpec {
            id: "ast.string_xor".to_string(),
            params: [("xor_key".to_string(), "0x42".to_string())]
                .into_iter()
                .collect(),
        };

        let (output, applied) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(applied.contains(&"ast.string_xor".to_string()));
        assert!(output_str.contains("xor_str_0"));
        assert!(output_str.contains("^=0x42"));
        // Should use statement expression syntax ({ ... })
        assert!(output_str.contains("({"));
        assert!(output_str.contains("})"));
    }

    #[test]
    fn test_no_mutations() {
        let source = "int main() { return 0; }";
        let (output, applied) = Mutator::apply(source.as_bytes(), &[]).unwrap();

        assert_eq!(output, source.as_bytes());
        assert!(applied.is_empty());
    }

    #[test]
    fn test_unknown_mutation_skipped() {
        let source = "int x = 42;";

        let mutation = MutationSpec {
            id: "unknown.mutation".to_string(),
            params: HashMap::new(),
        };

        let (output, applied) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();

        // Should pass through unchanged
        assert_eq!(output, source.as_bytes());
        assert!(applied.is_empty());
    }

    #[test]
    fn test_ast_decon_rounds_via_mutator() {
        let source = r#"void deconditioner() {
    // @MUTATE:decon_rounds
    for (int i = 0; i < DECON_ROUNDS; i++) {
        do_stuff();
    }
}"#;

        let mutation = MutationSpec {
            id: "ast.decon_rounds".to_string(),
            params: [
                ("count".to_string(), "50".to_string()),
                ("method".to_string(), "fixed".to_string()),
            ]
            .into_iter()
            .collect(),
        };

        let (output, applied) = Mutator::apply(source.as_bytes(), &[mutation]).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(applied.contains(&"ast.decon_rounds".to_string()));
        assert!(output_str.contains("i < 50"));
    }

    #[test]
    fn test_mixed_ast_and_legacy_mutations() {
        // Source with both @MUTATE marker and string literal
        let source = r#"void deconditioner() {
    char *msg = "Hello";
    // @MUTATE:decon_rounds
    for (int i = 0; i < DECON_ROUNDS; i++) {
        do_stuff();
    }
}"#;

        let mutations = vec![
            MutationSpec {
                id: "ast.decon_rounds".to_string(),
                params: [
                    ("count".to_string(), "30".to_string()),
                    ("method".to_string(), "fixed".to_string()),
                ]
                .into_iter()
                .collect(),
            },
            MutationSpec {
                id: "ast.string_xor".to_string(),
                params: [("xor_key".to_string(), "0x42".to_string())]
                    .into_iter()
                    .collect(),
            },
        ];

        let (output, applied) = Mutator::apply(source.as_bytes(), &mutations).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Both mutations should be applied
        assert!(applied.contains(&"ast.decon_rounds".to_string()));
        assert!(applied.contains(&"ast.string_xor".to_string()));
        assert!(output_str.contains("i < 30"));
        assert!(output_str.contains("xor_str_0"));
    }
}
