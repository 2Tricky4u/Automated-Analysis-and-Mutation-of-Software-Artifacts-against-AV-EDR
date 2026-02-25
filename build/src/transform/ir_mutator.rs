//! LLVM IR-level mutations (text-based, no LLVM library dependency)
//!
//! Semantic-preserving transformations applied to `.ll` text:
//! - `insert_nops`              — inline asm NOP after block labels
//! - `insert_opaque_predicates` — unconditional br → always-true conditional
//! - `insert_junk_blocks`       — dead unreachable blocks before function `}`

use anyhow::Result;
use tracing::info;

use crate::mutator::MutationSpec;

/// Text-based LLVM IR mutator.
///
/// All transforms operate on `.ll` text (no LLVM C-API needed).
/// Uses a deterministic LCG for density-based insertion.
pub struct IrMutator {
    rng_state: u32,
}

impl IrMutator {
    pub fn new() -> Result<Self> {
        Ok(Self { rng_state: 1234 })
    }

    pub fn with_seed(seed: u32) -> Self {
        Self { rng_state: seed }
    }

    /// Apply a batch of `llvm.*` mutations to IR text.
    ///
    /// Returns `(mutated_ir, applied_ids)`.
    pub fn apply(
        &mut self,
        ir_text: &str,
        mutations: &[&MutationSpec],
    ) -> Result<(String, Vec<String>)> {
        let mut text = ir_text.to_string();
        let mut applied = Vec::new();

        for spec in mutations {
            let (_cat, name) = spec.parse();
            match name {
                "nop_insert" => {
                    text = self.insert_nops(&text, spec)?;
                    applied.push(spec.id.clone());
                }
                "opaque_predicate" => {
                    text = self.insert_opaque_predicates(&text, spec)?;
                    applied.push(spec.id.clone());
                }
                "junk_block" => {
                    text = self.insert_junk_blocks(&text, spec)?;
                    applied.push(spec.id.clone());
                }
                _ => {
                    tracing::warn!("IrMutator: unknown mutation: {}", spec.id);
                }
            }
        }

        Ok((text, applied))
    }

    // ── Handlers ─────────────────────────────────────────────────────────────

    /// Insert `call void asm sideeffect "nop", ""()` after basic block labels.
    ///
    /// Params: `density` (0.0–1.0, default 0.3)
    fn insert_nops(&mut self, ir_text: &str, spec: &MutationSpec) -> Result<String> {
        let density: f32 = spec
            .params
            .get("density")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.3);

        info!("NOP insertion density: {}", density);

        let mut output = String::new();

        for line in ir_text.lines() {
            output.push_str(line);
            output.push('\n');

            // Basic block label: non-indented line ending with ':'
            if line.ends_with(':') && !line.starts_with(' ') && !line.starts_with('\t') {
                let rand_val = self.next_rand();
                if rand_val < density {
                    output.push_str("  call void asm sideeffect \"nop\", \"\"()\n");
                }
            }
        }

        Ok(output)
    }

    /// Convert unconditional `br label %target` to always-true conditional.
    ///
    /// Before: `  br label %done`
    /// After:  `  %__op_N = icmp eq i32 0, 0`
    ///         `  br i1 %__op_N, label %done, label %done`
    ///
    /// Skips conditional branches (`br i1 ...`). Both branch targets are
    /// identical so semantics are preserved. Phi nodes stay valid because
    /// predecessor block labels don't change.
    ///
    /// Params: `density` (0.0–1.0, default 0.3)
    fn insert_opaque_predicates(&mut self, ir_text: &str, spec: &MutationSpec) -> Result<String> {
        let density: f32 = spec
            .params
            .get("density")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.3);

        info!("Opaque predicate density: {}", density);

        let mut output = String::new();
        let mut counter = 0u32;

        for line in ir_text.lines() {
            let trimmed = line.trim();

            // Match unconditional branch: `br label %<target>[, !metadata ...]`
            if let Some(rest) = trimmed.strip_prefix("br label ") {
                let rand_val = self.next_rand();
                if rand_val < density {
                    // Separate label from trailing metadata (e.g. ", !llvm.loop !6")
                    let (label, metadata) = if let Some(comma_pos) = rest.find(", !") {
                        (rest[..comma_pos].trim(), &rest[comma_pos..])
                    } else {
                        (rest.trim(), "")
                    };
                    let indent = &line[..line.len() - trimmed.len()];
                    output.push_str(&format!("{}%__op_{} = icmp eq i32 0, 0\n", indent, counter));
                    output.push_str(&format!(
                        "{}br i1 %__op_{}, label {}, label {}{}\n",
                        indent, counter, label, label, metadata
                    ));
                    counter += 1;
                    continue;
                }
            }

            output.push_str(line);
            output.push('\n');
        }

        Ok(output)
    }

    /// Insert dead unreachable blocks before each function's closing `}`.
    ///
    /// ```llvm
    /// __junk_0:
    ///   %__junk_val_0 = add i32 42, <random>
    ///   unreachable
    /// ```
    ///
    /// Only fires inside `define ... { ... }` (skips `declare`).
    ///
    /// Params: `count` (default 2)
    fn insert_junk_blocks(&mut self, ir_text: &str, spec: &MutationSpec) -> Result<String> {
        let count: u32 = spec
            .params
            .get("count")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);

        info!("Junk block count: {}", count);

        let mut output = String::new();
        let mut in_function = false;
        let mut junk_counter = 0u32;

        for line in ir_text.lines() {
            let trimmed = line.trim();

            // Track function scope
            if trimmed.starts_with("define ") {
                in_function = true;
            }

            // Closing brace of a function — insert junk blocks before it
            if in_function && trimmed == "}" {
                for _ in 0..count {
                    let rand_operand = (self.next_rand() * 10000.0) as u32;
                    output.push_str(&format!("__junk_{}:\n", junk_counter));
                    output.push_str(&format!(
                        "  %__junk_val_{} = add i32 42, {}\n",
                        junk_counter, rand_operand
                    ));
                    output.push_str("  unreachable\n");
                    junk_counter += 1;
                }
                in_function = false;
            }

            output.push_str(line);
            output.push('\n');
        }

        Ok(output)
    }

    // ── RNG ──────────────────────────────────────────────────────────────────

    /// Deterministic LCG: glibc constants, output in [0, 1).
    fn next_rand(&mut self) -> f32 {
        self.rng_state = self.rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        (self.rng_state >> 16) as f32 / 65536.0
    }
}

impl Default for IrMutator {
    fn default() -> Self {
        Self::new().expect("IrMutator::new() is infallible")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spec(id: &str, params: &[(&str, &str)]) -> MutationSpec {
        MutationSpec {
            id: id.to_string(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn ir_with_blocks() -> &'static str {
        r#"define i32 @compute(i32 %x) {
entry:
  %cmp = icmp sgt i32 %x, 0
  br i1 %cmp, label %positive, label %negative

positive:
  %r1 = add i32 %x, 1
  br label %done

negative:
  %r2 = sub i32 0, %x
  br label %done

done:
  %result = phi i32 [ %r1, %positive ], [ %r2, %negative ]
  ret i32 %result
}
"#
    }

    fn ir_no_blocks() -> &'static str {
        "declare i32 @printf(i8*, ...)\ndeclare void @exit(i32)\n"
    }

    // ── insert_nops ──────────────────────────────────────────────────────────

    #[test]
    fn nop_density_1_inserts_at_every_block() {
        let spec = make_spec("llvm.nop_insert", &[("density", "1.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, applied) = m.apply(ir_with_blocks(), &[&spec]).unwrap();
        assert!(applied.contains(&"llvm.nop_insert".to_string()));

        let nop_count = out.matches(r#"call void asm sideeffect "nop""#).count();
        assert_eq!(nop_count, 4, "4 blocks should each get a NOP");
    }

    #[test]
    fn nop_density_0_inserts_none() {
        let spec = make_spec("llvm.nop_insert", &[("density", "0.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        assert_eq!(out.matches(r#"call void asm sideeffect "nop""#).count(), 0);
    }

    #[test]
    fn nop_preserves_all_original_lines() {
        let ir = ir_with_blocks();
        let spec = make_spec("llvm.nop_insert", &[("density", "1.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir, &[&spec]).unwrap();

        for line in ir.lines() {
            assert!(out.contains(line), "Missing original line: {:?}", line);
        }
    }

    #[test]
    fn nop_no_blocks_inserts_nothing() {
        let spec = make_spec("llvm.nop_insert", &[("density", "1.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_no_blocks(), &[&spec]).unwrap();

        assert_eq!(out.matches(r#"call void asm sideeffect "nop""#).count(), 0);
    }

    // ── insert_opaque_predicates ─────────────────────────────────────────────

    #[test]
    fn opaque_density_1_converts_all_unconditional() {
        let spec = make_spec("llvm.opaque_predicate", &[("density", "1.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, applied) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        assert!(applied.contains(&"llvm.opaque_predicate".to_string()));
        // ir_with_blocks has 2 unconditional branches (positive→done, negative→done)
        let pred_count = out.matches("icmp eq i32 0, 0").count();
        assert_eq!(pred_count, 2, "Expected 2 opaque predicates");
    }

    #[test]
    fn opaque_density_0_converts_none() {
        let spec = make_spec("llvm.opaque_predicate", &[("density", "0.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        assert_eq!(out.matches("icmp eq i32 0, 0").count(), 0);
    }

    #[test]
    fn opaque_preserves_conditional_branches() {
        let spec = make_spec("llvm.opaque_predicate", &[("density", "1.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        // The original conditional branch must survive
        assert!(
            out.contains("br i1 %cmp, label %positive, label %negative"),
            "Conditional branch must not be transformed"
        );
    }

    #[test]
    fn opaque_no_unconditional_branches() {
        let ir = r#"define void @f() {
entry:
  %cmp = icmp eq i32 1, 1
  br i1 %cmp, label %a, label %b
a:
  ret void
b:
  ret void
}
"#;
        let spec = make_spec("llvm.opaque_predicate", &[("density", "1.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir, &[&spec]).unwrap();

        assert_eq!(out.matches("icmp eq i32 0, 0").count(), 0);
    }

    #[test]
    fn opaque_preserves_branch_metadata() {
        // Branches with metadata like !llvm.loop must be handled correctly
        let ir_with_meta = r#"define void @f() {
entry:
  br label %loop

loop:
  br label %exit, !llvm.loop !6

exit:
  ret void
}
"#;
        let spec = make_spec("llvm.opaque_predicate", &[("density", "1.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_with_meta, &[&spec]).unwrap();

        // Should produce valid IR: metadata after both labels
        assert!(
            out.contains("label %exit, label %exit, !llvm.loop !6"),
            "Metadata should appear once at the end, got:\n{}",
            out
        );
        // Should NOT duplicate metadata per label
        assert!(
            !out.contains("label %exit, !llvm.loop !6, label %exit, !llvm.loop !6"),
            "Metadata must not be duplicated"
        );
    }

    #[test]
    fn opaque_phi_semantics_preserved() {
        // Phi references predecessor labels; opaque predicates don't rename blocks
        let spec = make_spec("llvm.opaque_predicate", &[("density", "1.0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        assert!(
            out.contains("phi i32 [ %r1, %positive ], [ %r2, %negative ]"),
            "Phi node must be preserved"
        );
    }

    // ── insert_junk_blocks ───────────────────────────────────────────────────

    #[test]
    fn junk_default_count_inserts_2() {
        let spec = make_spec("llvm.junk_block", &[]);
        let mut m = IrMutator::new().unwrap();
        let (out, applied) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        assert!(applied.contains(&"llvm.junk_block".to_string()));
        let junk_count = out.matches("unreachable").count();
        assert_eq!(junk_count, 2, "Default count=2 should insert 2 junk blocks");
    }

    #[test]
    fn junk_custom_count() {
        let spec = make_spec("llvm.junk_block", &[("count", "5")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        assert_eq!(out.matches("unreachable").count(), 5);
    }

    #[test]
    fn junk_count_0_inserts_none() {
        let spec = make_spec("llvm.junk_block", &[("count", "0")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        assert_eq!(out.matches("unreachable").count(), 0);
    }

    #[test]
    fn junk_preserves_original() {
        let ir = ir_with_blocks();
        let spec = make_spec("llvm.junk_block", &[("count", "3")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir, &[&spec]).unwrap();

        for line in ir.lines() {
            assert!(out.contains(line), "Missing original line: {:?}", line);
        }
    }

    #[test]
    fn junk_no_functions() {
        let spec = make_spec("llvm.junk_block", &[("count", "5")]);
        let mut m = IrMutator::new().unwrap();
        let (out, _) = m.apply(ir_no_blocks(), &[&spec]).unwrap();

        assert_eq!(out.matches("unreachable").count(), 0);
    }

    // ── Combined / edge cases ────────────────────────────────────────────────

    #[test]
    fn all_three_together() {
        let specs = [
            make_spec("llvm.nop_insert", &[("density", "1.0")]),
            make_spec("llvm.opaque_predicate", &[("density", "1.0")]),
            make_spec("llvm.junk_block", &[("count", "3")]),
        ];
        let refs: Vec<&MutationSpec> = specs.iter().collect();
        let mut m = IrMutator::new().unwrap();
        let (out, applied) = m.apply(ir_with_blocks(), &refs).unwrap();

        assert_eq!(applied.len(), 3);
        assert!(out.contains("asm sideeffect"));
        assert!(out.contains("icmp eq i32 0, 0"));
        assert!(out.contains("unreachable"));
    }

    #[test]
    fn unknown_mutation_skipped() {
        let spec = make_spec("llvm.unknown_thing", &[]);
        let mut m = IrMutator::new().unwrap();
        let (out, applied) = m.apply(ir_with_blocks(), &[&spec]).unwrap();

        assert!(applied.is_empty());
        assert_eq!(out, ir_with_blocks());
    }

    #[test]
    fn empty_mutations_passthrough() {
        let mut m = IrMutator::new().unwrap();
        let (out, applied) = m.apply(ir_with_blocks(), &[]).unwrap();

        assert!(applied.is_empty());
        assert_eq!(out, ir_with_blocks());
    }
}
