mod common;

use build::mutator::MutationSpec;
use build::{EncodingType, ModuleSelection, TraceFormat};
use std::collections::HashMap;

// ============================================================================
// A) Happy-path pipeline tests (encode → assemble → strip)
// ============================================================================

#[test]
fn test_default_pipeline_xor() {
    let modules = ModuleSelection::new();
    let payload = common::payload_typical();

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    // No markers should remain
    assert!(
        !out.final_source.contains("// @MODULE:"),
        "Final source still has @MODULE markers"
    );
    assert!(
        !out.final_source.contains("// @MUTATE:"),
        "Final source still has @MUTATE markers"
    );

    // Payload header content should be embedded
    assert!(
        out.final_source.contains("PAYLOAD_LEN"),
        "Missing PAYLOAD_LEN in final source"
    );
    assert!(
        out.final_source.contains("XOR_KEY"),
        "Missing XOR_KEY in final source"
    );
    assert!(
        out.final_source.contains("supermega_payload"),
        "Missing supermega_payload in final source"
    );

    // Carrier and decoder function bodies should be present
    assert!(
        out.final_source.contains("carrier"),
        "Missing carrier function content"
    );
    assert!(
        out.final_source.contains("decode_payload"),
        "Missing decode_payload function content"
    );
}

#[test]
fn test_default_pipeline_english() {
    let modules = ModuleSelection {
        decoder: "english".to_string(),
        ..ModuleSelection::new()
    };
    let payload = common::payload_small();

    let out = common::run_pipeline(modules, &payload, EncodingType::English, &[]).unwrap();

    assert!(
        out.final_source.contains("DICTIONARY[]"),
        "Missing DICTIONARY[] for English encoding"
    );
    assert!(
        out.final_source.contains("supermega_payload_str"),
        "Missing supermega_payload_str for English encoding"
    );
    assert!(
        !out.final_source.contains("// @MODULE:"),
        "Still has @MODULE markers"
    );
}

#[test]
fn test_all_carrier_decoder_combos() {
    let carriers = ["alloc_rw_rx", "change_rw_rx", "peb_walk"];
    let decoders = ["xor", "english"];
    let payload = common::payload_small();

    for carrier in &carriers {
        for decoder in &decoders {
            let encoding = if *decoder == "english" {
                EncodingType::English
            } else {
                EncodingType::Xor
            };

            let modules = ModuleSelection {
                carrier: carrier.to_string(),
                decoder: decoder.to_string(),
                ..ModuleSelection::new()
            };

            let result = common::run_pipeline(modules, &payload, encoding, &[]);
            assert!(
                result.is_ok(),
                "Pipeline failed for carrier={}, decoder={}: {:?}",
                carrier,
                decoder,
                result.err()
            );
            let out = result.unwrap();
            assert!(
                !out.final_source.contains("// @MODULE:"),
                "Remaining @MODULE in {}/{}",
                carrier,
                decoder
            );
            assert!(
                out.final_source.len() > 1000,
                "Output too short ({}) for {}/{}",
                out.final_source.len(),
                carrier,
                decoder
            );
        }
    }
}

#[test]
fn test_all_module_options_nondefault() {
    let modules = ModuleSelection {
        carrier: "alloc_rw_rx".to_string(),
        decoder: "xor".to_string(),
        antiemulation: "sirallocalot".to_string(),
        deconditioner: "none".to_string(),
        guardrail: "env".to_string(),
        virtualprotect: "undersized".to_string(),
        decoy: "winexec".to_string(),
    };
    let payload = common::payload_small();

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    // sirallocalot module should contain its characteristic allocation pattern
    assert!(
        out.final_source.contains("VirtualAlloc"),
        "Missing sirallocalot content"
    );
    // env guardrail checks environment variables
    assert!(
        out.final_source.contains("GetEnvironmentVariable")
            || out.final_source.contains("guardrail"),
        "Missing env guardrail content"
    );
    // winexec decoy
    assert!(
        out.final_source.contains("WinExec") || out.final_source.contains("decoy"),
        "Missing winexec decoy content"
    );
}

#[test]
fn test_payload_header_embedded_correctly() {
    let payload = common::payload_typical(); // 256 bytes
    let modules = ModuleSelection::new();

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    // PAYLOAD_LEN should match the encoded data length (same as payload length for XOR)
    let expected_len_define = format!("#define PAYLOAD_LEN {}", payload.len());
    assert!(
        out.payload_header.contains(&expected_len_define),
        "Payload header should contain '{}', got header starting with: {}",
        expected_len_define,
        &out.payload_header[..out.payload_header.len().min(200)]
    );

    // XOR key bytes should be the encoder's default [0xAA, 0x55]
    assert!(
        out.payload_header.contains("0xAA"),
        "Missing XOR key byte 0xAA"
    );
    assert!(
        out.payload_header.contains("0x55"),
        "Missing XOR key byte 0x55"
    );
}

#[test]
fn test_assembled_source_includes_main() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    assert!(
        out.assembled_source.contains("int main(void)"),
        "Assembled source should contain 'int main(void)' from loader_template.c"
    );
    // final_source should also have it (stripping markers doesn't remove main)
    assert!(
        out.final_source.contains("int main(void)"),
        "Final source should contain 'int main(void)'"
    );
}

#[test]
fn test_assembled_source_has_definitions() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    assert!(out.final_source.contains("p_RW"), "Missing p_RW define");
    assert!(out.final_source.contains("p_RX"), "Missing p_RX define");
    assert!(
        out.final_source.contains("VirtualProtect_t"),
        "Missing VirtualProtect_t typedef"
    );
    assert!(
        out.final_source.contains("carrier(void)"),
        "Missing carrier(void) declaration"
    );
}

// ============================================================================
// B) Mutation pipeline tests (encode → assemble → mutate → strip)
// ============================================================================

#[test]
fn test_string_xor_on_assembled_source() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();
    let mutations = vec![MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    }];

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &mutations).unwrap();

    assert!(
        out.applied_mutations.contains(&"ast.string_xor".to_string()),
        "ast.string_xor should be in applied list"
    );

    // xor_str_ variables should be present (string literals were transformed)
    assert!(
        out.final_source.contains("xor_str_"),
        "Missing xor_str_ variables after string_xor mutation"
    );
}

#[test]
fn test_mutations_strip_all_markers() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();
    let mutations = vec![MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    }];

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &mutations).unwrap();

    // Zero @MUTATE markers should remain regardless of mutation list
    assert!(
        !out.final_source.contains("// @MUTATE:"),
        "Final source should have zero @MUTATE markers after pipeline"
    );
}

#[test]
fn test_no_mutations_strips_markers() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    // No mutations → markers stripped, code otherwise intact
    assert!(
        !out.final_source.contains("// @MUTATE:"),
        "@MUTATE markers should be stripped even with no mutations"
    );
    // The assembled source SHOULD have markers (they come from loader_template.c)
    assert!(
        out.assembled_source.contains("// @MUTATE:"),
        "Assembled source should still have @MUTATE markers before stripping"
    );
}

#[test]
fn test_unknown_mutation_still_strips() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();
    let mutations = vec![MutationSpec {
        id: "unknown.mutation_id".to_string(),
        params: HashMap::new(),
    }];

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &mutations).unwrap();

    assert!(
        out.applied_mutations.is_empty(),
        "Unknown mutation should not appear in applied list"
    );
    assert!(
        !out.final_source.contains("// @MUTATE:"),
        "Markers should still be stripped even with unknown mutations"
    );
    // Source should still be valid (carrier, main, etc.)
    assert!(
        out.final_source.contains("int main(void)"),
        "Source integrity should be preserved"
    );
}

#[test]
fn test_string_xor_preserves_payload_header() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();
    let mutations = vec![MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    }];

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &mutations).unwrap();

    // The payload hex array (0xNN values inside supermega_payload[]) should NOT be
    // transformed by string_xor — they're not quoted string literals
    assert!(
        out.final_source.contains("supermega_payload"),
        "supermega_payload array should survive string_xor mutation"
    );
    assert!(
        out.final_source.contains("PAYLOAD_LEN"),
        "PAYLOAD_LEN should survive string_xor mutation"
    );
}

// ============================================================================
// C) Instrumentation pipeline tests (full chain → line traces)
// ============================================================================

#[test]
fn test_tracing_on_assembled_source() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();

    let out = common::run_pipeline_with_tracing(
        modules,
        &payload,
        EncodingType::Xor,
        &[],
        TraceFormat::Binary,
    )
    .unwrap();

    let src = &out.instrumented_source;

    // Trace calls should be injected inside function bodies
    // Use the opening quote pattern to count actual calls (not the declaration)
    let trace_count = src.matches("__trace_line_binary(\"").count();
    assert!(
        trace_count > 0,
        "Expected trace calls in instrumented source"
    );

    // Should have traces inside carrier(), main(), decode_payload() areas
    assert!(
        src.contains("__trace_line_binary("),
        "Trace calls should exist in instrumented source"
    );
}

#[test]
fn test_tracing_binary_format() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();

    let out = common::run_pipeline_with_tracing(
        modules,
        &payload,
        EncodingType::Xor,
        &[],
        TraceFormat::Binary,
    )
    .unwrap();

    assert!(
        out.instrumented_source.contains("__trace_line_binary"),
        "Binary format: __trace_line_binary should be present"
    );
    assert!(
        !out.instrumented_source.contains("__trace_line_b64"),
        "Binary format: __trace_line_b64 should be absent"
    );
}

#[test]
fn test_tracing_base64_format() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();

    let out = common::run_pipeline_with_tracing(
        modules,
        &payload,
        EncodingType::Xor,
        &[],
        TraceFormat::Base64,
    )
    .unwrap();

    assert!(
        out.instrumented_source.contains("__trace_line_b64"),
        "Base64 format: __trace_line_b64 should be present"
    );
    assert!(
        !out.instrumented_source.contains("__trace_line_binary"),
        "Base64 format: __trace_line_binary should be absent"
    );
}

#[test]
fn test_tracing_preserves_payload_data() {
    let modules = ModuleSelection::new();
    let payload = common::payload_typical();

    let out = common::run_pipeline_with_tracing(
        modules,
        &payload,
        EncodingType::Xor,
        &[],
        TraceFormat::Binary,
    )
    .unwrap();

    assert!(
        out.instrumented_source.contains("PAYLOAD_LEN"),
        "PAYLOAD_LEN should survive line trace injection"
    );
    assert!(
        out.instrumented_source.contains("supermega_payload"),
        "supermega_payload should survive line trace injection"
    );
}

#[test]
fn test_tracing_adds_delay_loops() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();

    let out = common::run_pipeline_with_tracing(
        modules,
        &payload,
        EncodingType::Xor,
        &[],
        TraceFormat::Binary,
    )
    .unwrap();

    let src = &out.instrumented_source;
    let trace_count = src.matches("__trace_line_binary(\"").count();
    let delay_count = src.matches("__inst_wait").count();

    assert!(delay_count > 0, "Expected __inst_wait delay loops");
    assert_eq!(
        delay_count,
        trace_count * 3,
        "__inst_wait count ({}) should be 3x trace call count ({})",
        delay_count,
        trace_count
    );
}

#[test]
fn test_tracing_declaration_present() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();

    let out = common::run_pipeline_with_tracing(
        modules,
        &payload,
        EncodingType::Xor,
        &[],
        TraceFormat::Binary,
    )
    .unwrap();

    assert!(
        out.instrumented_source
            .contains("void __trace_line_binary("),
        "Runtime declaration 'void __trace_line_binary(...)' should be at top of instrumented source"
    );
}

// ============================================================================
// D) Payload size edge cases through full pipeline
// ============================================================================

#[test]
fn test_pipeline_empty_payload() {
    let modules = ModuleSelection::new();
    let payload = common::payload_empty();

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    assert!(
        out.payload_header.contains("#define PAYLOAD_LEN 0"),
        "Empty payload should have PAYLOAD_LEN 0"
    );
    assert!(
        out.final_source.contains("int main(void)"),
        "Empty payload should still produce valid assembled source"
    );
}

#[test]
fn test_pipeline_tiny_payload() {
    let modules = ModuleSelection::new();
    let payload = common::payload_tiny(); // 1 byte

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    assert!(
        out.payload_header.contains("#define PAYLOAD_LEN 1"),
        "Tiny payload should have PAYLOAD_LEN 1"
    );
    assert!(
        !out.final_source.contains("// @MODULE:"),
        "Tiny payload: no remaining markers"
    );
}

#[test]
fn test_pipeline_large_payload() {
    let modules = ModuleSelection::new();
    let payload = common::payload_large(); // 8192 bytes

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    assert!(
        out.payload_header.contains("PAYLOAD_LEN"),
        "Large payload header should have PAYLOAD_LEN"
    );
    assert!(
        out.final_source.contains("supermega_payload"),
        "Large payload should have supermega_payload array"
    );
    assert!(
        out.final_source.len() > 10_000,
        "Large payload total source should be >10KB, got {} bytes",
        out.final_source.len()
    );
}

// ============================================================================
// E) Error paths through composed pipeline
// ============================================================================

#[test]
fn test_pipeline_invalid_carrier() {
    let modules = ModuleSelection {
        carrier: "nonexistent_carrier_xyz".to_string(),
        ..ModuleSelection::new()
    };
    let payload = common::payload_small();

    let result = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]);
    assert!(result.is_err(), "Invalid carrier should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Module not found"),
        "Error should mention 'Module not found', got: {}",
        err_msg
    );
}

#[test]
fn test_pipeline_invalid_decoder() {
    let modules = ModuleSelection {
        decoder: "nonexistent_decoder_xyz".to_string(),
        ..ModuleSelection::new()
    };
    let payload = common::payload_small();

    let result = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]);
    assert!(result.is_err(), "Invalid decoder should fail");
}

#[test]
fn test_pipeline_all_invalid_modules() {
    let modules = ModuleSelection {
        carrier: "bad_carrier".to_string(),
        decoder: "bad_decoder".to_string(),
        antiemulation: "bad_antiemu".to_string(),
        deconditioner: "bad_decon".to_string(),
        guardrail: "bad_guard".to_string(),
        virtualprotect: "bad_vp".to_string(),
        decoy: "bad_decoy".to_string(),
    };
    let payload = common::payload_small();

    let result = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]);
    assert!(
        result.is_err(),
        "All invalid modules should fail on first validation"
    );
}

// ============================================================================
// F) Determinism / reproducibility
// ============================================================================

#[test]
fn test_pipeline_deterministic() {
    let payload = common::payload_typical();

    let out1 = common::run_pipeline(
        ModuleSelection::new(),
        &payload,
        EncodingType::Xor,
        &[],
    )
    .unwrap();

    let out2 = common::run_pipeline(
        ModuleSelection::new(),
        &payload,
        EncodingType::Xor,
        &[],
    )
    .unwrap();

    assert_eq!(
        out1.final_source, out2.final_source,
        "Same inputs should produce identical final_source (deterministic pipeline)"
    );
    assert_eq!(
        out1.payload_header, out2.payload_header,
        "Same inputs should produce identical payload_header"
    );
}

// ============================================================================
// G) Combined mutation + tracing (encode → assemble → mutate → strip → trace)
// ============================================================================

#[test]
fn test_mutation_and_tracing_combined() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();
    let mutations = vec![MutationSpec {
        id: "ast.string_xor".to_string(),
        params: HashMap::new(),
    }];

    let out = common::run_pipeline_with_tracing(
        modules,
        &payload,
        EncodingType::Xor,
        &mutations,
        TraceFormat::Binary,
    )
    .unwrap();

    // Mutation should have been applied
    assert!(
        out.pipeline.applied_mutations.contains(&"ast.string_xor".to_string()),
        "ast.string_xor should be applied in combined pipeline"
    );

    // Tracing should have been injected
    assert!(
        out.instrumented_source.contains("__trace_line_binary("),
        "Trace calls should be present after mutation + tracing"
    );

    // No markers should remain
    assert!(
        !out.instrumented_source.contains("// @MUTATE:"),
        "No @MUTATE markers should remain after combined pipeline"
    );
    assert!(
        !out.instrumented_source.contains("// @MODULE:"),
        "No @MODULE markers should remain after combined pipeline"
    );

    // Payload header should survive both mutation and tracing
    assert!(
        out.instrumented_source.contains("PAYLOAD_LEN"),
        "PAYLOAD_LEN should survive mutation + tracing"
    );
}

// ============================================================================
// H) Carrier module content validation
// ============================================================================

#[test]
fn test_carrier_source_calls_decode_payload() {
    let carriers = ["alloc_rw_rx", "change_rw_rx", "peb_walk"];
    let payload = common::payload_small();

    for carrier in &carriers {
        let modules = ModuleSelection {
            carrier: carrier.to_string(),
            ..ModuleSelection::new()
        };

        let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

        // Every carrier must call decode_payload() to decode the encoded payload
        assert!(
            out.final_source.contains("decode_payload("),
            "Carrier '{}' must call decode_payload()",
            carrier
        );

        // Every carrier must reference PAYLOAD_LEN for memory sizing
        assert!(
            out.final_source.contains("PAYLOAD_LEN"),
            "Carrier '{}' must reference PAYLOAD_LEN",
            carrier
        );

        // Every carrier defines `int carrier()` function
        assert!(
            out.final_source.contains("int carrier()"),
            "Carrier '{}' must define int carrier()",
            carrier
        );
    }
}

#[test]
fn test_carrier_alloc_rw_rx_uses_virtualalloc() {
    let modules = ModuleSelection {
        carrier: "alloc_rw_rx".to_string(),
        ..ModuleSelection::new()
    };
    let payload = common::payload_small();
    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    assert!(
        out.final_source.contains("VirtualAlloc"),
        "alloc_rw_rx carrier must use VirtualAlloc"
    );
    assert!(
        out.final_source.contains("MyVirtualProtect"),
        "alloc_rw_rx carrier must use MyVirtualProtect for RW→RX transition"
    );
}

#[test]
fn test_carrier_peb_walk_has_resolver() {
    let modules = ModuleSelection {
        carrier: "peb_walk".to_string(),
        ..ModuleSelection::new()
    };
    let payload = common::payload_small();
    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    assert!(
        out.final_source.contains("get_module_by_name"),
        "peb_walk carrier must define get_module_by_name()"
    );
    assert!(
        out.final_source.contains("get_func_by_name"),
        "peb_walk carrier must define get_func_by_name()"
    );
    assert!(
        out.final_source.contains("PEB"),
        "peb_walk carrier must define PEB structures"
    );
}

// ============================================================================
// I) Module replacement edge cases
// ============================================================================

#[test]
fn test_no_double_module_replacement() {
    // If the payload header happens to contain text like "// @MODULE:carrier",
    // the assembler should NOT double-replace it (payload is inserted first).
    // We can't inject arbitrary marker text into real payload, but we can verify
    // that after assembly no @MODULE markers remain regardless of payload content.
    let modules = ModuleSelection::new();
    let payload = common::payload_typical();

    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    let module_marker_count = out.assembled_source.matches("// @MODULE:").count();
    assert_eq!(
        module_marker_count, 0,
        "No @MODULE markers should remain in assembled source (found {})",
        module_marker_count
    );
}

#[test]
fn test_assembled_output_has_all_section_comments() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();
    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    // The loader_template.c has section comments that should survive assembly
    let required_sections = [
        "PAYLOAD",
        "TYPE DEFINITIONS",
        "DECODER MODULE",
        "CARRIER MODULE",
        "MAIN ENTRY POINT",
    ];

    for section in &required_sections {
        assert!(
            out.assembled_source.contains(section),
            "Assembled source should contain section header '{}'",
            section
        );
    }
}

#[test]
fn test_assembled_source_no_duplicate_definitions_include() {
    let modules = ModuleSelection::new();
    let payload = common::payload_small();
    let out = common::run_pipeline(modules, &payload, EncodingType::Xor, &[]).unwrap();

    // Assembler strips `#include "definitions.h"` from modules since definitions
    // are injected via @MODULE:definitions. No duplicate includes should exist.
    let def_include_count = out
        .assembled_source
        .matches(r#"#include "../header/definitions.h""#)
        .count();
    assert_eq!(
        def_include_count, 0,
        "Module-level definitions.h includes should be stripped, found {}",
        def_include_count
    );
}
