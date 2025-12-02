///! Instrumentation injection at LLVM IR level
///!
///! Injects telemetry collection code:
///! - Basic-block coverage (AFL-style bitmap)
///! - Thread-aware API tracing (WINNIE-style)
///! - Line-level tracing (diagnostic mode, Base64-encoded to named pipe)
///! - Checkpoint markers for key operations
///!
///! Architecture:
///!   1. Parse LLVM IR
///!   2. Insert instrumentation calls at:
///!      - BB entry points → __coverage_bb(bb_id)
///!      - Line boundaries → __trace_line(file, line, func)
///!      - Before API calls → __checkpoint(name, args)
///!   3. Link with runtime library (instrumentation_runtime.obj)
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info};

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
        let abs_output_path = std::fs::canonicalize(output_path).unwrap_or_else(|_| output_path.to_path_buf());
        eprintln!("[INSTRUMENTER] Starting instrumentation: {:?} mode={:?}", abs_ir_path, trace_mode);
        eprintln!("[INSTRUMENTER] Output path: {:?}", abs_output_path);
        info!("Instrumenting LLVM IR: {:?} (mode: {:?})", abs_ir_path, trace_mode);

        // Read IR file
        let ir_content = tokio::fs::read_to_string(ir_path)
            .await
            .context("Failed to read IR file")?;

        // Parse IR and inject instrumentation
        let instrumented_ir = match trace_mode {
            crate::TraceMode::Off => {
                debug!("No instrumentation requested");
                ir_content
            }
            crate::TraceMode::BB => {
                self.inject_bb_coverage(&ir_content)?
            }
            crate::TraceMode::Api => {
                self.inject_api_tracing(&ir_content)?
            }
            crate::TraceMode::ApiPlusBB => {
                let with_bb = self.inject_bb_coverage(&ir_content)?;
                self.inject_api_tracing(&with_bb)?
            }
            crate::TraceMode::Lines => {
                // Line tracing is done at AST/source level (tree-sitter-based)
                // No IR instrumentation needed, just pass through
                ir_content
            }
            crate::TraceMode::LinesAroundBB(bb_id) => {
                // Line tracing is done at AST/source level (tree-sitter-based)
                // TODO: Implement targeted narrowing around specific BB
                info!("LinesAroundBB mode not fully implemented, using full line tracing");
                ir_content
            }
            crate::TraceMode::All => {
                // Line tracing is done at AST/source level (tree-sitter-based)
                // Only inject BB + API at IR level
                let with_bb = self.inject_bb_coverage(&ir_content)?;
                self.inject_api_tracing(&with_bb)?
            }
        };

        // Add runtime function declarations
        let full_ir = self.add_runtime_declarations(&instrumented_ir, trace_mode)?;

        // Write instrumented IR
        tokio::fs::write(output_path, full_ir)
            .await
            .context("Failed to write instrumented IR")?;

        info!(
            "IR-level instrumentation complete: {} BBs, {} lines (AST-level line traces not counted here)",
            self.bb_counter, self.line_counter
        );

        Ok(())
    }

    /// Inject BB coverage instrumentation (AFL-style)
    fn inject_bb_coverage(&mut self, ir: &str) -> Result<String> {
        debug!("Injecting BB coverage instrumentation");

        let mut instrumented = String::new();
        let mut in_function = false;
        let mut bb_id = 0u32;
        let mut injected_count = 0;
        let mut need_entry_marker = false;
        let mut is_in_main = false;
        let mut init_injected = false;

        for line in ir.lines() {
            // Detect function entry
            if line.trim_start().starts_with("define ") {
                in_function = true;
                is_in_main = line.contains(" @main(") || line.contains(" @wmain(");
                // DON'T reset bb_id here - it should be global across all functions

                // ONLY instrument non-library functions (skip linkonce_odr comdat functions)
                let is_library_function = line.contains("linkonce_odr") || line.contains("comdat");
                need_entry_marker = !is_library_function;

                if is_in_main {
                    eprintln!("[INSTRUMENTER] Found main() function - will inject __coverage_init()");
                } else if !is_library_function {
                    debug!("Found function definition: {}", line.trim());
                } else {
                    debug!("Skipping library function: {}", line.trim());
                }
                instrumented.push_str(line);
                instrumented.push('\n');
                continue;
            }

            // Detect function exit
            if in_function && line.trim() == "}" {
                in_function = false;
                debug!("Function end, injected {} BB markers", injected_count);
                injected_count = 0;
                instrumented.push_str(line);
                instrumented.push('\n');
                continue;
            }

            // Inject coverage call at BB entry
            if in_function {
                let trimmed = line.trim();

                // Insert entry BB marker AFTER allocas but before first real instruction
                if need_entry_marker && !trimmed.is_empty() && !trimmed.starts_with(';') && !trimmed.starts_with("!") {
                    // If this line is an alloca, don't inject yet
                    if trimmed.contains("= alloca ") {
                        // Skip injection, just append the line
                        instrumented.push_str(line);
                        instrumented.push('\n');
                        continue;
                    }

                    // We've passed all allocas (or there are none), inject now
                    if self.bb_counter == 0 {
                        eprintln!("[INSTRUMENTER] First BB injection after allocas");
                        eprintln!("  Next instruction: {}", trimmed);
                    }

                    // If this is main() and we haven't injected init yet, do it first
                    if is_in_main && !init_injected {
                        instrumented.push_str("  call void @__coverage_init()\n");
                        init_injected = true;
                        eprintln!("[INSTRUMENTER] Injected __coverage_init() call in main()");
                    }

                    instrumented.push_str(&format!(
                        "  call void @__coverage_bb(i32 {})\n",
                        bb_id
                    ));
                    bb_id += 1;
                    self.bb_counter += 1;
                    injected_count += 1;
                    need_entry_marker = false;
                    debug!("Injected entry BB marker {}", bb_id - 1);
                }

                // BB entry: explicit label definition (e.g., "label_name:" or "entry:" or "10:")
                if trimmed.ends_with(':') && !trimmed.starts_with(';') && !trimmed.starts_with("!") {
                    // Found a basic block label - insert AFTER the label
                    instrumented.push_str(line);
                    instrumented.push('\n');
                    instrumented.push_str(&format!(
                        "  call void @__coverage_bb(i32 {})\n",
                        bb_id
                    ));
                    bb_id += 1;
                    self.bb_counter += 1;
                    injected_count += 1;
                    debug!("Injected BB marker {} at label: {}", bb_id - 1, trimmed);
                    continue; // Skip the normal line append below
                }
            }

            instrumented.push_str(line);
            instrumented.push('\n');
        }

        eprintln!("[INSTRUMENTER] BB coverage injection complete: {} markers injected", self.bb_counter);
        info!("BB coverage injection complete: {} markers injected", self.bb_counter);

        Ok(instrumented)
    }

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
                if line.contains(&format!("call ")) && line.contains(api) {
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
        let needs_bb = matches!(trace_mode,
            crate::TraceMode::BB | crate::TraceMode::ApiPlusBB | crate::TraceMode::All);
        let needs_api = matches!(trace_mode,
            crate::TraceMode::Api | crate::TraceMode::ApiPlusBB |
            crate::TraceMode::Lines | crate::TraceMode::LinesAroundBB(_) | crate::TraceMode::All);

        if !needs_bb && !needs_api {
            // No instrumentation, return IR as-is
            return Ok(ir.to_string());
        }

        if needs_bb {
            declarations.push_str("; BB coverage runtime (external C functions)\n");
            declarations.push_str("declare void @__coverage_bb(i32)\n");
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
            if !found_first_definition &&
               (trimmed.starts_with("define ") ||
                trimmed.starts_with("declare ") ||
                (trimmed.starts_with('@') && !trimmed.is_empty()) ||
                (trimmed.starts_with('%') && trimmed.contains(" = type "))) {
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

    #[tokio::test]
    async fn test_inject_bb_coverage() {
        let mut instrumenter = Instrumenter::new();

        let ir = r#"
define i32 @main() {
entry:
  %retval = alloca i32
  store i32 0, i32* %retval
  br label %loop

loop:
  %i = phi i32 [ 0, %entry ], [ %i.next, %loop ]
  %i.next = add i32 %i, 1
  %cmp = icmp slt i32 %i, 10
  br i1 %cmp, label %loop, label %exit

exit:
  ret i32 0
}
"#;

        let result = instrumenter.inject_bb_coverage(ir).unwrap();

        // Should have 3 BBs: entry, loop, exit
        assert!(result.contains("@__coverage_bb(i32 0)"));
        assert!(result.contains("@__coverage_bb(i32 1)"));
        assert!(result.contains("@__coverage_bb(i32 2)"));
        assert_eq!(instrumenter.bb_counter, 3);
    }
}
