//! AST-level mutations using Tree-sitter
//!
//! Transformations applied to source code before LLVM IR generation:
//! - Control-flow jitter (add random branches)
//! - Constant encoding (XOR, stack strings)
//! - Import reshaping (delay-load, hash-based resolution)
//! - Function inlining/outlining
//! - Line-level tracing (preprocessor-based, no debug info)
use anyhow::Result;
use std::path::Path;

pub struct AstMutator {
    // TODO: Add tree-sitter parser state
}

impl AstMutator {
    pub fn new() -> Self {
        Self {}
    }

    /// Apply AST mutations to source file
    pub async fn mutate(
        &self,
        _source: &Path,
        _mutations: &[crate::Mutation],
        _output: &Path,
    ) -> Result<()> {
        // TODO: Implement AST mutation pipeline
        // 1. Parse C source with tree-sitter
        // 2. Traverse AST
        // 3. Apply mutations (control-flow jitter, constant encoding, etc.)
        // 4. Generate mutated source

        anyhow::bail!("AST mutations not yet implemented")
    }

    /// Inject line-level tracing macros into C source (preprocessor-based)
    /// Based on Lepori (2023) thesis Section 6.4.2 - adds delays + Base64 encoding
    /// This is done at source level to avoid needing debug info (-g flag)
    pub fn inject_line_tracing(&self, source_code: &str) -> Result<String> {
        self.inject_line_tracing_with_delay(source_code, 0)
    }

    /// Inject line-level tracing with configurable delay (microseconds)
    /// delay_us: 0 = no delay, 50 = 50µs (narrowing mode), 1000000 = 1s (blocking mode)
    pub fn inject_line_tracing_with_delay(
        &self,
        source_code: &str,
        delay_us: u32,
    ) -> Result<String> {
        let mut result = String::new();

        // Add trace infrastructure at the top (Lepori-style with Base64)
        result.push_str("// Auto-injected line tracing (Lepori 2023 thesis-inspired)\n");
        result.push_str("// Magic signature: b64line: (identifies trace output in stdout)\n");
        result.push_str("#include <windows.h>\n");
        result.push_str("#include <stdio.h>\n\n");

        result.push_str("extern void __trace_line(unsigned int seq, const char* file, unsigned int line, const char* func);\n");
        result.push_str("static unsigned int __trace_seq = 0;\n\n");

        // Base64 encoding helper (inline to avoid linking issues)
        result.push_str("static const char __b64_table[] = \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\";\n");
        result.push_str("static void __b64_encode(const char* input, char* output, int len) {\n");
        result.push_str("    int i = 0, j = 0;\n");
        result.push_str("    unsigned char a3[3], a4[4];\n");
        result.push_str("    while (len--) {\n");
        result.push_str("        a3[i++] = *(input++);\n");
        result.push_str("        if (i == 3) {\n");
        result.push_str("            a4[0] = (a3[0] & 0xfc) >> 2;\n");
        result.push_str("            a4[1] = ((a3[0] & 0x03) << 4) + ((a3[1] & 0xf0) >> 4);\n");
        result.push_str("            a4[2] = ((a3[1] & 0x0f) << 2) + ((a3[2] & 0xc0) >> 6);\n");
        result.push_str("            a4[3] = a3[2] & 0x3f;\n");
        result.push_str("            for (i = 0; i < 4; i++) output[j++] = __b64_table[a4[i]];\n");
        result.push_str("            i = 0;\n");
        result.push_str("        }\n");
        result.push_str("    }\n");
        result.push_str("    if (i) {\n");
        result.push_str("        for (int k = i; k < 3; k++) a3[k] = 0;\n");
        result.push_str("        a4[0] = (a3[0] & 0xfc) >> 2;\n");
        result.push_str("        a4[1] = ((a3[0] & 0x03) << 4) + ((a3[1] & 0xf0) >> 4);\n");
        result.push_str("        a4[2] = ((a3[1] & 0x0f) << 2) + ((a3[2] & 0xc0) >> 6);\n");
        result.push_str(
            "        for (int k = 0; k < i + 1; k++) output[j++] = __b64_table[a4[k]];\n",
        );
        result.push_str("        while (i++ < 3) output[j++] = '=';\n");
        result.push_str("    }\n");
        result.push_str("    output[j] = 0;\n");
        result.push_str("}\n\n");

        // Trace macro with Base64 encoding + named pipe with stderr fallback
        // Writes to \\.\pipe\rededr_trace (collector listens here)
        // Falls back to stderr if pipe unavailable (local testing)
        result.push_str("#define __TRACE_LINE() do { \\\n");
        result.push_str("    char __trace_buf[512]; \\\n");
        result.push_str("    char __trace_b64[1024]; \\\n");
        result.push_str("    char __trace_output[2048]; \\\n");
        result.push_str("    snprintf(__trace_buf, sizeof(__trace_buf), \"line:%s:%d:%s\", __FILE__, __LINE__, __func__); \\\n");
        result.push_str("    __b64_encode(__trace_buf, __trace_b64, strlen(__trace_buf)); \\\n");
        result.push_str("    snprintf(__trace_output, sizeof(__trace_output), \"b64line:%s\\n\", __trace_b64); \\\n");
        result.push_str("    HANDLE hPipe = CreateFileA(\"\\\\\\\\.\\\\pipe\\\\rededr_trace\", GENERIC_WRITE, 0, NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL); \\\n");
        result.push_str("    if (hPipe != INVALID_HANDLE_VALUE) { \\\n");
        result.push_str("        DWORD written; \\\n");
        result.push_str("        WriteFile(hPipe, __trace_output, strlen(__trace_output), &written, NULL); \\\n");
        result.push_str("        CloseHandle(hPipe); \\\n");
        result.push_str("    } else { \\\n");
        result.push_str("        fprintf(stderr, \"%s\", __trace_output); \\\n");
        result.push_str("    } \\\n");

        // Add delay if specified (Lepori thesis Section 6.4.2 - helps pinpoint exact detection line)
        if delay_us > 0 {
            result.push_str("    volatile long __inst_wait = 1; \\\n");
            result.push_str(&format!(
                "    for (; __inst_wait < {}; __inst_wait += 2) {{}} \\\n",
                delay_us * 10
            ));
        }

        result.push_str("} while(0)\n\n");

        // Inject __TRACE_LINE() after each statement
        let mut in_function = false;

        for line in source_code.lines() {
            result.push_str(line);
            result.push('\n');

            let trimmed = line.trim();

            // Track function boundaries
            if trimmed.contains("(")
                && trimmed.contains(")")
                && trimmed.contains("{")
                && !trimmed.starts_with("//")
                && !trimmed.starts_with("/*")
            {
                in_function = true;
            }

            if trimmed == "}" && in_function {
                in_function = false;
                continue;
            }

            // Skip empty lines, preprocessor directives, comments, braces, our own injected code
            if trimmed.is_empty()
                || trimmed.starts_with('#')
                || trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed == "{"
                || trimmed == "}"
                || trimmed.contains("__TRACE_LINE")
                || trimmed.contains("__trace_")
                || trimmed.contains("__b64_")
                || trimmed.contains("__inst_wait")
            {
                continue;
            }

            // Inject trace after statements (lines ending with ; or } in function body)
            if in_function && trimmed.ends_with(';') {
                // Get indentation from original line
                let indent = &line[..line.len() - line.trim_start().len()];
                result.push_str(indent);
                result.push_str("__TRACE_LINE();\n");
            }
        }

        Ok(result)
    }
}

impl Default for AstMutator {
    fn default() -> Self {
        Self::new()
    }
}
