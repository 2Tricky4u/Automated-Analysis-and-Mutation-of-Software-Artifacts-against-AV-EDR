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
        info!("Instrumenting LLVM IR: {:?} (mode: {:?})", ir_path, trace_mode);

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
                self.inject_line_tracing(&ir_content, None)?
            }
            crate::TraceMode::LinesAroundBB(bb_id) => {
                self.inject_line_tracing(&ir_content, Some(bb_id))?
            }
            crate::TraceMode::All => {
                let with_bb = self.inject_bb_coverage(&ir_content)?;
                let with_api = self.inject_api_tracing(&with_bb)?;
                self.inject_line_tracing(&with_api, None)?
            }
        };

        // Add runtime function declarations
        let full_ir = self.add_runtime_declarations(&instrumented_ir, trace_mode)?;

        // Write instrumented IR
        tokio::fs::write(output_path, full_ir)
            .await
            .context("Failed to write instrumented IR")?;

        info!("Instrumentation complete: {} BBs, {} lines", self.bb_counter, self.line_counter);

        Ok(())
    }

    /// Inject BB coverage instrumentation (AFL-style)
    fn inject_bb_coverage(&mut self, ir: &str) -> Result<String> {
        debug!("Injecting BB coverage instrumentation");

        let mut instrumented = String::new();
        let mut in_function = false;
        let mut bb_id = 0u32;

        for line in ir.lines() {
            instrumented.push_str(line);
            instrumented.push('\n');

            // Detect function entry
            if line.trim_start().starts_with("define ") {
                in_function = true;
                bb_id = 0;
            }

            // Detect function exit
            if in_function && line.trim() == "}" {
                in_function = false;
            }

            // Inject coverage call at BB entry (lines starting with label or after branch)
            if in_function {
                let trimmed = line.trim();

                // BB entry: label definition (e.g., "label_name:")
                // OR after a terminator instruction (br, ret, switch, etc.)
                if trimmed.ends_with(':') && !trimmed.starts_with(';') {
                    // Found a basic block label
                    instrumented.push_str(&format!(
                        "  call void @__coverage_bb(i32 {})\n",
                        bb_id
                    ));
                    bb_id += 1;
                    self.bb_counter += 1;
                }
            }
        }

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

        for line in ir.lines() {
            // Check if line contains a call to target API and insert checkpoint before it
            for api in &target_apis {
                if line.contains(&format!("call ")) && line.contains(api) {
                    // Insert checkpoint before API call
                    let checkpoint_name = format!("api:{}", api);
                    instrumented.push_str(&format!(
                        "  call void @__checkpoint(i8* getelementptr inbounds ([{}x i8], [{}x i8]* @.str.checkpoint.{}, i32 0, i32 0))\n",
                        checkpoint_name.len() + 1,
                        checkpoint_name.len() + 1,
                        self.line_counter
                    ));
                    self.line_counter += 1;
                    break;
                }
            }

            instrumented.push_str(line);
            instrumented.push('\n');
        }

        Ok(instrumented)
    }

    /// Inject line-level tracing with Base64 encoding to named pipe
    fn inject_line_tracing(&mut self, ir: &str, _around_bb: Option<u32>) -> Result<String> {
        debug!("Injecting line-level tracing instrumentation");

        let mut instrumented = String::new();
        let mut in_function = false;
        let mut current_function = String::new();
        let mut line_map: HashMap<u32, (String, u32, String)> = HashMap::new();

        // Parse IR to extract debug info (!dbg metadata)
        for line in ir.lines() {
            instrumented.push_str(line);
            instrumented.push('\n');

            // Track current function
            if line.trim_start().starts_with("define ") {
                in_function = true;
                // Extract function name from: define i32 @main()
                if let Some(at_pos) = line.find('@') {
                    if let Some(paren_pos) = line[at_pos..].find('(') {
                        current_function = line[at_pos + 1..at_pos + paren_pos].to_string();
                    }
                }
            }

            if in_function && line.trim() == "}" {
                in_function = false;
            }

            // Inject line tracing at instructions with !dbg metadata
            if in_function && line.contains("!dbg !") {
                // Extract debug info ID (e.g., "!dbg !42")
                if let Some(dbg_pos) = line.rfind("!dbg !") {
                    let dbg_id_str = &line[dbg_pos + 6..];
                    if let Some(_dbg_id) = dbg_id_str.split_whitespace().next() {
                        // Insert trace call BEFORE the instruction
                        // Format: __trace_line(seq, file_b64, line, func_b64)
                        let seq = self.line_counter;
                        let file_b64 = base64_str("unknown.c"); // TODO: Extract from !DILocation
                        let line_num = 0; // TODO: Extract from !DILocation
                        let func_b64 = base64_str(&current_function);

                        let trace_call = format!(
                            "  call void @__trace_line(i32 {}, i8* getelementptr inbounds ([{}x i8], [{}x i8]* @.str.file.{}, i32 0, i32 0), i32 {}, i8* getelementptr inbounds ([{}x i8], [{}x i8]* @.str.func.{}, i32 0, i32 0))\n",
                            seq,
                            file_b64.len() + 1, file_b64.len() + 1, seq,
                            line_num,
                            func_b64.len() + 1, func_b64.len() + 1, seq
                        );

                        // Insert trace call as previous line (before the instruction)
                        // We need to insert it into the already-built string
                        // For simplicity, we'll rebuild the last line
                        instrumented.pop(); // Remove \n
                        let last_line = instrumented.lines().last().unwrap_or("").to_string();
                        instrumented = instrumented[..instrumented.len() - last_line.len()].to_string();
                        instrumented.push_str(&trace_call);
                        instrumented.push_str(&last_line);
                        instrumented.push('\n');

                        line_map.insert(seq, ("unknown.c".to_string(), line_num, current_function.clone()));
                        self.line_counter += 1;
                    }
                }
            }
        }

        info!("Injected {} line trace points", line_map.len());

        Ok(instrumented)
    }

    /// Add runtime function declarations to IR
    fn add_runtime_declarations(&self, ir: &str, trace_mode: crate::TraceMode) -> Result<String> {
        let mut declarations = String::new();

        // Add necessary runtime function declarations based on trace mode
        match trace_mode {
            crate::TraceMode::Off => {}
            crate::TraceMode::BB | crate::TraceMode::ApiPlusBB | crate::TraceMode::All => {
                declarations.push_str("; BB coverage runtime\n");
                declarations.push_str("declare void @__coverage_bb(i32)\n");
                declarations.push_str("declare void @__coverage_init()\n");
                declarations.push_str("declare void @__coverage_flush()\n");
                declarations.push('\n');
            }
            crate::TraceMode::Api | crate::TraceMode::Lines | crate::TraceMode::LinesAroundBB(_) => {
                // API/line tracing needs checkpoint
                declarations.push_str("; Checkpoint runtime\n");
                declarations.push_str("declare void @__checkpoint(i8*)\n");
                declarations.push_str("declare void @__trace_line(i32, i8*, i32, i8*)\n");
                declarations.push_str("declare void @__trace_init(i8*)\n");
                declarations.push_str("declare void @__trace_flush()\n");
                declarations.push('\n');
            }
        }

        // Prepend declarations to IR
        Ok(format!("{}\n{}", declarations, ir))
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
