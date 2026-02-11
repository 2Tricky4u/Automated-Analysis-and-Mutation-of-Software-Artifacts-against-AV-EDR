//! Instrumentation injection at LLVM IR level
//!
//! Injects telemetry collection code:
//! - Basic-block coverage (LLVM SanitizerCoverage - accurate CFG-aware detection)
//! - Thread-aware API tracing (WINNIE-style)
//! - Line-level tracing (diagnostic mode, Base64-encoded to named pipe)
//! - Checkpoint markers for key operations
//!
//! Architecture:
//!   1. Use LLVM's `opt` tool with SanitizerCoverage pass for BB coverage
//!   2. Parse LLVM IR for API tracing injection (text-based, selective)
//!   3. Link with runtime library (instrumentation_runtime.obj)
//!
//! BB Coverage uses LLVM SanitizerCoverage (industry standard, used by AFL++/libFuzzer):
//!   - Accurate: Uses proper LLVM BasicBlock API (not text parsing)
//!   - Complete: Detects ALL basic blocks including implicit ones
//!   - Efficient: ~3-5% overhead
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use tracing::debug;

pub struct Instrumenter {
    bb_counter: u32,
    line_counter: u32,
}

impl Instrumenter {
    pub fn new() -> Self {
        Self {
            bb_counter: 0,
            line_counter: 0,
        }
    }

    /// Inject instrumentation into LLVM IR
    pub async fn instrument(
        &mut self,
        ir_path: &Path,
        trace_mode: crate::TraceMode,
        output_path: &Path,
    ) -> Result<()> {
        let abs_ir_path = std::fs::canonicalize(ir_path).unwrap_or_else(|_| ir_path.to_path_buf());
        let abs_output_path =
            std::fs::canonicalize(output_path).unwrap_or_else(|_| output_path.to_path_buf());
        eprintln!(
            "[INSTRUMENTER] Starting instrumentation: {:?} mode={:?}",
            abs_ir_path, trace_mode
        );
        eprintln!("[INSTRUMENTER] Output path: {:?}", abs_output_path);
        debug!(
            "Instrumenting LLVM IR: {:?} (mode: {:?})",
            abs_ir_path, trace_mode
        );

        // Determine what instrumentation we need
        let needs_bb = matches!(
            trace_mode,
            crate::TraceMode::BB | crate::TraceMode::ApiPlusBB | crate::TraceMode::All
        );
        let needs_api = matches!(
            trace_mode,
            crate::TraceMode::Api | crate::TraceMode::ApiPlusBB | crate::TraceMode::All
        );

        // Apply SanitizerCoverage for BB instrumentation (if needed)
        let ir_after_bb = if needs_bb {
            let temp_bb = output_path.with_extension("bb.ll");
            self.inject_bb_coverage_sancov(ir_path, &temp_bb).await?;
            temp_bb
        } else {
            ir_path.to_path_buf()
        };

        // Read IR (either original or post-SanitizerCoverage)
        let ir_content = tokio::fs::read_to_string(&ir_after_bb)
            .await
            .context("Failed to read IR file")?;

        // Apply API tracing (text-based injection) if needed
        let instrumented_ir = if needs_api {
            self.inject_api_tracing(&ir_content)?
        } else {
            ir_content
        };

        // Add runtime function declarations
        let full_ir = self.add_runtime_declarations(&instrumented_ir, trace_mode)?;

        // Write final instrumented IR
        tokio::fs::write(output_path, full_ir)
            .await
            .context("Failed to write instrumented IR")?;

        // Clean up temporary BB IR file
        if needs_bb {
            let _ = tokio::fs::remove_file(&ir_after_bb).await;
        }

        debug!(
            "IR-level instrumentation complete: {} BBs (via SanitizerCoverage), {} API checkpoints",
            self.bb_counter, self.line_counter
        );

        Ok(())
    }

    /// Inject BB coverage using LLVM SanitizerCoverage (industry-standard)
    async fn inject_bb_coverage_sancov(
        &mut self,
        ir_path: &Path,
        output_path: &Path,
    ) -> Result<()> {
        debug!("Applying LLVM SanitizerCoverage pass for BB instrumentation");
        eprintln!("[INSTRUMENTER] Running opt with SanitizerCoverage pass");

        // Use LLVM opt tool with SanitizerCoverage pass
        // Note: Pass name changed in LLVM 13+: "sancov" → "sancov-module"
        // For Windows COFF: use trace-pc (simple callback) instead of trace-pc-guard (requires section support)
        let mut cmd = Command::new("opt");
        cmd.arg("-passes=sancov-module")
            .arg("-sanitizer-coverage-level=3") // BB-level coverage
            .arg("-sanitizer-coverage-trace-pc") // Simple PC callback (works on Windows COFF)
            .arg(ir_path)
            .arg("-S") // Output as text IR
            .arg("-o")
            .arg(output_path);

        eprintln!("[INSTRUMENTER] opt command: {:?}", cmd);

        let output = cmd
            .output()
            .context("Failed to execute opt - ensure LLVM toolchain is in PATH")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("[INSTRUMENTER] opt failed: {}", stderr);
            anyhow::bail!("opt (SanitizerCoverage) failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            debug!("opt stdout: {}", stdout);
        }

        // Count BBs from SanitizerCoverage guards (estimate)
        let ir_content = tokio::fs::read_to_string(output_path)
            .await
            .context("Failed to read instrumented IR")?;

        let call_count = ir_content
            .matches("call void @__sanitizer_cov_trace_pc()")
            .count();
        self.bb_counter = call_count as u32;

        eprintln!(
            "[INSTRUMENTER] SanitizerCoverage injected {} BB callbacks",
            self.bb_counter
        );
        debug!(
            "SanitizerCoverage complete: {} basic blocks instrumented",
            self.bb_counter
        );

        Ok(())
    }

    // REMOVED: inject_bb_coverage_naive() - deprecated text-based BB injection
    // Replaced by inject_bb_coverage_sancov() which uses LLVM SanitizerCoverage
    // Old implementation had ~30% BB detection miss rate (implicit BBs without labels)
    // See BB-COVERAGE-IMPROVEMENT.md for detailed comparison

    /// Inject API tracing instrumentation
    fn inject_api_tracing(&mut self, ir: &str) -> Result<String> {
        debug!("Injecting API tracing instrumentation");

        // API calls to instrument (Windows API subset)
        let target_apis = vec![
            "VirtualAlloc",
            "VirtualProtect",
            "WriteProcessMemory",
            "CreateRemoteThread",
            "LoadLibrary",
            "GetProcAddress",
            "CreateProcess",
            "OpenProcess",
        ];

        let mut instrumented = String::new();
        let mut string_constants = Vec::new();

        for line in ir.lines() {
            // Check if line contains a call to target API and insert checkpoint before it
            for api in &target_apis {
                if line.contains(&"call ".to_string()) && line.contains(api) {
                    // Insert checkpoint before API call
                    let checkpoint_name = format!("api:{}", api);
                    let str_id = self.line_counter;

                    instrumented.push_str(&format!(
                        "  call void @__checkpoint(i8* getelementptr inbounds ([{} x i8], [{} x i8]* @.str.checkpoint.{}, i32 0, i32 0))\n",
                        checkpoint_name.len() + 1,
                        checkpoint_name.len() + 1,
                        str_id
                    ));

                    // Store string constant for later injection
                    string_constants.push((str_id, checkpoint_name));
                    self.line_counter += 1;
                    break;
                }
            }

            instrumented.push_str(line);
            instrumented.push('\n');
        }

        // Now inject string constants at the beginning (after metadata)
        Ok(self.inject_string_constants(&instrumented, &string_constants))
    }

    /// Inject string constant definitions into IR
    fn inject_string_constants(&self, ir: &str, constants: &[(u32, String)]) -> String {
        if constants.is_empty() {
            return ir.to_string();
        }

        let mut result = String::new();
        let mut inserted = false;

        for line in ir.lines() {
            let trimmed = line.trim();

            // Insert string constants before the first function definition
            if !inserted && trimmed.starts_with("define ") {
                // Add all string constants
                for (id, value) in constants {
                    result.push_str(&format!(
                        "@.str.checkpoint.{} = private unnamed_addr constant [{} x i8] c\"{}\\00\"\n",
                        id,
                        value.len() + 1,
                        value
                    ));
                }
                result.push('\n');
                inserted = true;
            }

            result.push_str(line);
            result.push('\n');
        }

        result
    }

    // REMOVED: inject_line_tracing (IR-level)
    // Line tracing is now done at AST/source level using tree-sitter (see ast_line_tracer.rs)

    /// Add runtime function declarations to IR
    fn add_runtime_declarations(&self, ir: &str, trace_mode: crate::TraceMode) -> Result<String> {
        let mut declarations = String::new();

        // Add necessary runtime function declarations based on trace mode
        // Determine what declarations we need
        let needs_bb = matches!(
            trace_mode,
            crate::TraceMode::BB | crate::TraceMode::ApiPlusBB | crate::TraceMode::All
        );
        let needs_api = matches!(
            trace_mode,
            crate::TraceMode::Api
                | crate::TraceMode::ApiPlusBB
                | crate::TraceMode::Lines
                | crate::TraceMode::LinesAroundBB(_)
                | crate::TraceMode::All
        );

        if !needs_bb && !needs_api {
            // No instrumentation, return IR as-is
            return Ok(ir.to_string());
        }

        if needs_bb {
            // Note: SanitizerCoverage pass automatically adds its own declarations
            // (__sanitizer_cov_trace_pc_guard, __sanitizer_cov_trace_pc_guard_init)
            // We only add legacy coverage functions for backward compatibility
            declarations.push_str("; Legacy coverage functions (for backward compatibility)\n");
            declarations.push_str("declare void @__coverage_init()\n");
            declarations.push_str("declare void @__coverage_flush()\n");
            declarations.push('\n');
        }

        if needs_api {
            declarations.push_str("; Checkpoint runtime\n");
            declarations.push_str("declare void @__checkpoint(i8*)\n");
            declarations.push_str("declare void @__trace_line(i32, i8*, i32, i8*)\n");
            declarations.push_str("declare void @__trace_init(i8*)\n");
            declarations.push_str("declare void @__trace_flush()\n");
            declarations.push('\n');
        }

        // Insert declarations after initial metadata (source_filename, target datalayout, target triple)
        // Split IR into lines and find the first non-metadata line
        let mut result = String::new();
        let mut found_first_definition = false;

        for line in ir.lines() {
            let trimmed = line.trim();

            // Insert declarations before the first actual definition (define, declare, @global, or %)
            if !found_first_definition
                && (trimmed.starts_with("define ")
                    || trimmed.starts_with("declare ")
                    || (trimmed.starts_with('@') && !trimmed.is_empty())
                    || (trimmed.starts_with('%') && trimmed.contains(" = type ")))
            {
                // Found first definition, insert declarations here
                result.push_str(&declarations);
                found_first_definition = true;
            }

            result.push_str(line);
            result.push('\n');
        }

        // If we never found a definition (weird IR), just append declarations at end
        if !found_first_definition {
            result.push_str(&declarations);
        }

        Ok(result)
    }
}

impl Default for Instrumenter {
    fn default() -> Self {
        Self::new()
    }
}

/// Base64-encode a string for embedding in LLVM IR string constants
#[cfg(test)]
fn base64_str(s: &str) -> String {
    use base64::{Engine as _, engine::general_purpose};
    general_purpose::STANDARD.encode(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encoding() {
        assert_eq!(base64_str("main"), "bWFpbg==");
        assert_eq!(base64_str("loader.c"), "bG9hZGVyLmM=");
    }

    // NOTE: BB coverage testing now requires LLVM opt tool
    // SanitizerCoverage is applied via external process, not text parsing
    // Integration tests in build/emitter/tests/ validate full pipeline
    //
    // To test manually:
    // 1. Create test.ll with simple function
    // 2. Run: opt -passes=sancov -sanitizer-coverage-level=3 test.ll -o test_instrumented.ll
    // 3. Verify: grep -c "__sanitizer_cov_trace_pc_guard" test_instrumented.ll
    //
    // Old naive test removed - see git history for reference
}
