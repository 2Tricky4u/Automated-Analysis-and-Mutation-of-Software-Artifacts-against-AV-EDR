//! Instrumentation injection at LLVM IR level
//!
//! Injects telemetry collection code:
//! - Basic-block coverage (LLVM SanitizerCoverage - accurate CFG-aware detection)
//! - Thread-aware API tracing (WINNIE-style)
//! - Line-level tracing (diagnostic mode, Base64-encoded to named pipe)
//! - Checkpoint markers for key operations
//!
//! Architecture: LLVM SanitizerCoverage handles BB coverage via `opt`, API tracing is
//! injected by parsing LLVM IR text, and the result links against instrumentation_runtime.obj.
//!
//! BB Coverage uses LLVM SanitizerCoverage (industry standard, used by AFL++/libFuzzer):
//!   - Accurate: Uses proper LLVM BasicBlock API (not text parsing)
//!   - Complete: Detects ALL basic blocks including implicit ones
//!   - Efficient: ~3-5% overhead
use anyhow::{Context, Result};
use std::path::Path;
use tracing::debug;

pub struct Instrumenter;

impl Instrumenter {
    pub fn new() -> Self {
        Self
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
        debug!(
            "Starting instrumentation: {:?} mode={:?}, output={:?}",
            abs_ir_path, trace_mode, abs_output_path
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

        // BB instrumentation: Clang now handles SanitizerCoverage natively via
        // -fsanitize-coverage=trace-pc (added in builder.rs). The IR already contains
        // __sanitizer_cov_trace_pc() callbacks. No separate opt pass needed.
        //
        // Previous approach (opt -passes=sancov-module) broke in LLVM 17+ because the
        // pass requires sanitize_coverage function attributes that only Clang adds.
        let mut bb_counter: u32 = 0;

        if needs_bb {
            // Count BB callbacks injected by Clang for logging
            let ir_text = tokio::fs::read_to_string(ir_path)
                .await
                .context("Failed to read IR for BB callback count")?;
            bb_counter = ir_text
                .matches("call void @__sanitizer_cov_trace_pc()")
                .count() as u32;
            debug!("Clang SanitizerCoverage: {} BB callbacks in IR", bb_counter);
            if bb_counter == 0 {
                tracing::warn!(
                    "BB coverage requested but 0 callbacks found in IR. \
                     Check that clang was invoked with -fsanitize-coverage=trace-pc"
                );
            }
        }

        // Read IR (with BB callbacks already present if needs_bb)
        let ir_content = tokio::fs::read_to_string(ir_path)
            .await
            .context("Failed to read IR file")?;

        // Apply API tracing (text-based injection) if needed
        let mut line_counter: u32 = 0;
        let instrumented_ir = if needs_api {
            Self::inject_api_tracing(&ir_content, &mut line_counter)?
        } else {
            ir_content
        };

        // Add runtime function declarations
        let full_ir = Self::add_runtime_declarations(&instrumented_ir, trace_mode)?;

        // Write final instrumented IR
        tokio::fs::write(output_path, full_ir)
            .await
            .context("Failed to write instrumented IR")?;

        debug!(
            "IR-level instrumentation complete: {} BBs (via SanitizerCoverage), {} API checkpoints",
            bb_counter, line_counter
        );

        Ok(())
    }

    /// Inject API tracing instrumentation
    fn inject_api_tracing(ir: &str, line_counter: &mut u32) -> Result<String> {
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
                    let str_id = *line_counter;

                    instrumented.push_str(&format!(
                        "  call void @__checkpoint(i8* getelementptr inbounds ([{} x i8], [{} x i8]* @.str.checkpoint.{}, i32 0, i32 0))\n",
                        checkpoint_name.len() + 1,
                        checkpoint_name.len() + 1,
                        str_id
                    ));

                    // Store string constant for later injection
                    string_constants.push((str_id, checkpoint_name));
                    *line_counter += 1;
                    break;
                }
            }

            instrumented.push_str(line);
            instrumented.push('\n');
        }

        // Now inject string constants at the beginning (after metadata)
        Ok(Self::inject_string_constants(
            &instrumented,
            &string_constants,
        ))
    }

    /// Inject string constant definitions into IR
    fn inject_string_constants(ir: &str, constants: &[(u32, String)]) -> String {
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

    /// Add runtime function declarations to IR
    fn add_runtime_declarations(ir: &str, trace_mode: crate::TraceMode) -> Result<String> {
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
            // SanitizerCoverage pass automatically adds its own declarations
            // (__sanitizer_cov_trace_pc_guard, __sanitizer_cov_trace_pc_guard_init)
            // No additional declarations needed for BB coverage.
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

    // BB coverage testing requires LLVM opt tool (SanitizerCoverage is applied via
    // external process). Integration tests in build/emitter/tests/ validate the full pipeline.
}
