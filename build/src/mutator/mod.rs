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
/// - ast.string_xor:            XOR-encode string literals (tree-sitter)
/// - llvm.nop_insert:           Insert NOP instructions in LLVM IR
/// - llvm.opaque_predicate:     Opaque predicates in LLVM IR
/// - llvm.junk_block:           Dead unreachable blocks in LLVM IR
use anyhow::{Context, Result};
use std::collections::HashMap;
use tracing::warn;

use crate::transform::AstMutator;
use crate::transform::IrMutator;

/// Mutation specification from proto (edr.common.Mutation)
#[derive(Debug, Clone)]
pub struct MutationSpec {
    pub id: String,
    pub params: HashMap<String, String>,
}

impl MutationSpec {
    /// Parse "id:key=val,key=val" CLI syntax into a MutationSpec.
    ///
    /// Examples:
    /// - `"ast.string_xor"` → id="ast.string_xor", params={}
    /// - `"ast.decon_rounds:count=50,method=fixed"` → id="ast.decon_rounds", params={count:50, method:fixed}
    pub fn from_cli_str(s: &str) -> Self {
        if let Some((id, params_str)) = s.split_once(':') {
            let params: HashMap<String, String> = params_str
                .split(',')
                .filter_map(|kv| {
                    let (k, v) = kv.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect();
            MutationSpec {
                id: id.to_string(),
                params,
            }
        } else {
            MutationSpec {
                id: s.to_string(),
                params: HashMap::new(),
            }
        }
    }

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
    /// Mutation routing (2-way):
    /// - `ast.*`  → tree-sitter `AstMutator` (marker-based + global string_xor)
    /// - `llvm.*` → `IrMutator`
    pub fn apply(input: &[u8], mutations: &[MutationSpec]) -> Result<(Vec<u8>, Vec<String>)> {
        if mutations.is_empty() {
            return Ok((input.to_vec(), vec![]));
        }

        let mut code = input.to_vec();
        let mut applied = Vec::new();

        let mut ast_mutations = Vec::new();
        let mut ir_mutations = Vec::new();

        for m in mutations.iter() {
            let (cat, _name) = m.parse();
            match cat {
                "ast" => ast_mutations.push(m),
                "llvm" => ir_mutations.push(m),
                "binary" => {} // Handled post-link in builder.rs
                _ => {
                    warn!("Unknown mutation category: {}", m.id);
                }
            }
        }

        // Phase 1: tree-sitter AST mutations (all ast.*, including string_xor)
        if !ast_mutations.is_empty() {
            let source = String::from_utf8(code.clone()).context("C source must be valid UTF-8")?;
            let mut ast = AstMutator::new().context("Failed to init tree-sitter")?;
            let refs: Vec<&MutationSpec> = ast_mutations.into_iter().collect();
            let (mutated, ast_applied) = ast.apply(&source, &refs)?;
            code = mutated.into_bytes();
            applied.extend(ast_applied);
        }

        // Phase 2: LLVM IR mutations (nop_insert, opaque_predicate, junk_block)
        if !ir_mutations.is_empty() {
            let ir_text = String::from_utf8(code.clone()).context("IR text must be valid UTF-8")?;
            let mut ir = IrMutator::new().context("Failed to init IrMutator")?;
            let refs: Vec<&MutationSpec> = ir_mutations.into_iter().collect();
            let (mutated, ir_applied) = ir.apply(&ir_text, &refs)?;
            code = mutated.into_bytes();
            applied.extend(ir_applied);
        }

        Ok((code, applied))
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
    fn test_mixed_ast_mutations() {
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
