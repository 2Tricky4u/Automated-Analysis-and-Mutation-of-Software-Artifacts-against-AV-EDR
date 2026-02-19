//! Shared fixtures and helpers for build crate integration tests.

#![allow(dead_code)]

use std::path::PathBuf;

use build::mutator::MutationSpec;
use build::{
    Assembler, EncodingType, ModuleSelection, PayloadEncoder, SourceLanguage, TraceFormat,
    inject_line_traces_with_opts, strip_mutation_markers,
};

// ── Pipeline output types ──────────────────────────────────────────────────

/// Output from running pipeline steps 1–3 (encode → assemble → mutate → strip).
#[derive(Debug)]
pub struct PipelineOutput {
    pub payload_header: String,
    pub assembled_source: String,
    pub final_source: String,
    pub applied_mutations: Vec<String>,
}

/// Output from running the full pipeline steps 1–4 (+ line trace injection).
#[derive(Debug)]
pub struct TracedPipelineOutput {
    pub pipeline: PipelineOutput,
    pub instrumented_source: String,
}

// ── Pipeline helpers ───────────────────────────────────────────────────────

/// Run pipeline steps 1–3: encode → assemble → mutate → strip markers.
pub fn run_pipeline(
    modules: ModuleSelection,
    payload: &[u8],
    encoding: EncodingType,
    mutations: &[MutationSpec],
) -> anyhow::Result<PipelineOutput> {
    // Step 1: Encode payload
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(payload, encoding);
    let payload_header = encoder.generate_c_header(&encoded);

    // Step 2: Assemble template
    let dir = templates_dir();
    let mut assembler = Assembler::new(&dir)?;
    let assembled_source = assembler.assemble(&modules, &payload_header)?;

    // Step 3: Apply mutations + strip markers
    let (final_source, applied_mutations) = if !mutations.is_empty() {
        let source_bytes = assembled_source.as_bytes().to_vec();
        let (mutated, applied) = build::mutator::Mutator::apply(&source_bytes, mutations)?;
        let stripped = strip_mutation_markers(&String::from_utf8_lossy(&mutated));
        (stripped, applied)
    } else {
        let stripped = strip_mutation_markers(&assembled_source);
        (stripped, vec![])
    };

    Ok(PipelineOutput {
        payload_header,
        assembled_source,
        final_source,
        applied_mutations,
    })
}

/// Run the full pipeline steps 1–4: encode → assemble → mutate → strip → trace.
pub fn run_pipeline_with_tracing(
    modules: ModuleSelection,
    payload: &[u8],
    encoding: EncodingType,
    mutations: &[MutationSpec],
    trace_format: TraceFormat,
) -> anyhow::Result<TracedPipelineOutput> {
    let pipeline = run_pipeline(modules, payload, encoding, mutations)?;

    // Step 4: Inject line traces
    let instrumented_source = inject_line_traces_with_opts(
        &pipeline.final_source,
        SourceLanguage::C,
        "loader.c",
        trace_format,
    )?;

    Ok(TracedPipelineOutput {
        pipeline,
        instrumented_source,
    })
}

/// Resolves the `build/templates` directory using CARGO_MANIFEST_DIR.
pub fn templates_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    PathBuf::from(manifest).join("templates")
}

// ── Payload fixtures ────────────────────────────────────────────────────────

pub fn payload_empty() -> Vec<u8> {
    vec![]
}

pub fn payload_tiny() -> Vec<u8> {
    vec![0xCC]
}

pub fn payload_small() -> Vec<u8> {
    vec![0x41, 0x42, 0x43, 0x44]
}

/// 256 bytes: NOPs + INT3 trailer via generate_test_payload layout.
pub fn payload_typical() -> Vec<u8> {
    let mut v = vec![0x90; 256];
    v[254] = 0xCC;
    v[255] = 0xCC;
    v
}

pub fn payload_large() -> Vec<u8> {
    let mut v = vec![0x90; 8192];
    v[8190] = 0xCC;
    v[8191] = 0xCC;
    v
}

/// 64 x 0x00 — XOR key cancellation edge case.
pub fn payload_all_zeros() -> Vec<u8> {
    vec![0x00; 64]
}

/// 64 x 0xFF.
pub fn payload_all_ff() -> Vec<u8> {
    vec![0xFF; 64]
}

/// 0x00..=0xFF (256 bytes) — exercises all English dictionary entries.
pub fn payload_sequential() -> Vec<u8> {
    (0x00..=0xFFu8).collect()
}

// ── C source fixtures ───────────────────────────────────────────────────────

pub fn c_source_minimal() -> &'static str {
    "int main() {\n    return 0;\n}\n"
}

pub fn c_source_with_strings() -> &'static str {
    r#"#include <stdio.h>
int main() {
    char *a = "Hello, World!";
    char *b = "foo\tbar\nbaz";
    char *c = "";
    printf("%s %s %s\n", a, b, c);
    return 0;
}
"#
}

pub fn c_source_with_mutate_markers() -> &'static str {
    r#"int main() {
    // @MUTATE:timing_jitter
    // @MUTATE:execution_method(direct|callback|fiber|threadpool)
    // @MUTATE:opaque_predicate
    do_something();
    // @MUTATE:control_flow_flattening
    return 0;
}
"#
}

pub fn c_source_nested_blocks() -> &'static str {
    r#"int main() {
    if (1) {
        while (1) {
            int x = 0;
        }
    }
    return 0;
}
"#
}

pub fn c_source_empty_body() -> &'static str {
    "void f() {}\n"
}

pub fn c_source_cpp_style() -> &'static str {
    r#"#include <iostream>
#include <vector>
int main() {
    std::vector<int> v = {1, 2, 3};
    for (auto& x : v) {
        std::cout << x << std::endl;
    }
    try {
        throw 42;
    } catch (int e) {
        std::cout << e << std::endl;
    }
    return 0;
}
"#
}

pub fn c_source_pragma_and_strings() -> &'static str {
    // Pragma strings are inside preproc_call and won't be seen as string_literal
    // by tree-sitter. The regular string should still be encoded.
    r#"#pragma comment(lib, "user32")
int setup(void);
int init_subsystem(void);
int verify_environment(void);
int run_preflight_checks_and_validate_configuration(void);
int main() {
    char *msg = "regular string";
    return 0;
}
"#
}

// ── Payload header parsing helpers ────────────────────────────────────────

/// Parse PAYLOAD_LEN from a generated C header: `#define PAYLOAD_LEN <N>`
pub fn parse_payload_len(header: &str) -> usize {
    for line in header.lines() {
        if let Some(rest) = line.strip_prefix("#define PAYLOAD_LEN ") {
            return rest.trim().parse().expect("PAYLOAD_LEN must be a number");
        }
    }
    panic!("PAYLOAD_LEN not found in header");
}

/// Count hex byte literals (0xNN) in the supermega_payload array
pub fn count_hex_bytes_in_array(header: &str) -> usize {
    let start = header
        .find("supermega_payload[PAYLOAD_LEN] = {")
        .or_else(|| header.find("supermega_payload[1] = {"))
        .expect("supermega_payload array not found");
    let after_brace = header[start..].find('{').unwrap() + start + 1;
    let end_brace = header[after_brace..].find("};").unwrap() + after_brace;
    let array_body = &header[after_brace..end_brace];
    array_body.matches("0x").count()
}

// ── LLVM IR fixtures ────────────────────────────────────────────────────────

pub fn ir_simple_function() -> &'static str {
    r#"define i32 @main() {
entry:
  %result = add i32 1, 2
  ret i32 %result
}
"#
}

pub fn ir_multi_block() -> &'static str {
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

pub fn ir_no_blocks() -> &'static str {
    "declare i32 @printf(i8*, ...)\ndeclare void @exit(i32)\n"
}
