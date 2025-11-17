/// Minimal mutation engine for AST and LLVM IR transformations
///
/// Implements CLAUDE.md Section 3: Fuzzer & Mutation Engine (minimal viable subset)
///
/// Supported mutations:
/// - llvm.nop_insert: Insert NOP instructions in LLVM IR (control-flow jitter)
/// - ast.string_xor: XOR-encode string literals in C source (constant encoding)
use anyhow::{Context, Result};
use std::collections::HashMap;
use tracing::{info, warn};

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
    pub fn apply(input: &[u8], mutations: &[MutationSpec]) -> Result<(Vec<u8>, Vec<String>)> {
        if mutations.is_empty() {
            return Ok((input.to_vec(), vec![]));
        }

        let mut code = input.to_vec();
        let mut applied = Vec::new();

        for mutation in mutations {
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
                    // Don't fail, just skip unknown mutations
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
            .and_then(|v| v.parse().ok())
            .unwrap_or(0xAA);

        info!("String XOR key: 0x{:02X}", xor_key);

        // Simple regex-like approach for demonstration
        // This is NOT production-ready (doesn't handle escape sequences, multiline strings, etc.)
        let mut output = String::new();
        let mut chars = source_text.chars().peekable();
        let mut in_string = false;
        let mut string_buf = String::new();
        let mut counter = 0;

        while let Some(ch) = chars.next() {
            if ch == '"' && !in_string {
                // Start of string literal
                in_string = true;
                string_buf.clear();
            } else if ch == '"' && in_string {
                // End of string literal - transform it
                in_string = false;

                // Generate XOR-encoded array
                let encoded: Vec<String> = string_buf
                    .bytes()
                    .map(|b| format!("0x{:02X}", b ^ xor_key))
                    .collect();

                // Append null terminator
                let var_name = format!("xor_str_{}", counter);
                counter += 1;

                // Replace inline string with XOR decode logic
                // NOTE: This is a MINIMAL proof-of-concept. Real implementation would:
                // - Track variable scope
                // - Generate proper C code
                // - Handle all string literal contexts
                output.push_str(&format!(
                    "/*XOR*/{{char {}[]={{{}}}; for(int i=0;i<{};i++){}[i]^=0x{:02X}; /*use {}*/}}",
                    var_name,
                    encoded.join(","),
                    encoded.len(),
                    var_name,
                    xor_key,
                    var_name
                ));
            } else if in_string {
                // Inside string literal - buffer it
                string_buf.push(ch);
            } else {
                // Outside string literal - pass through
                output.push(ch);
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
}
