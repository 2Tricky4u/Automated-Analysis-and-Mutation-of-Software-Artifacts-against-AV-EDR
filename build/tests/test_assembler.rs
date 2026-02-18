mod common;

use build::{Assembler, ModuleSelection, extract_mutation_markers, strip_mutation_markers};

// ── Assembler tests (require template files on disk) ────────────────────────

#[test]
fn test_assembler_default_modules() {
    let dir = common::templates_dir();
    let mut asm = Assembler::new(&dir).unwrap();
    let modules = ModuleSelection::new();

    let output = asm.assemble(&modules, "// payload placeholder").unwrap();

    // No @MODULE: markers should remain
    assert!(
        !output.contains("// @MODULE:"),
        "Assembled output still contains @MODULE markers"
    );

    // Module interface signatures should be present (from definitions.h)
    assert!(output.contains("int carrier(void)"), "Missing carrier decl");
    assert!(
        output.contains("void decode_payload("),
        "Missing decode_payload decl"
    );
}

#[test]
fn test_assembler_all_module_combinations() {
    let dir = common::templates_dir();
    let combos = [
        ("alloc_rw_rx", "xor"),
        ("alloc_rw_rx", "english"),
        ("change_rw_rx", "xor"),
        ("peb_walk", "xor"),
    ];

    for (carrier, decoder) in &combos {
        let mut asm = Assembler::new(&dir).unwrap();
        let modules = ModuleSelection {
            carrier: carrier.to_string(),
            decoder: decoder.to_string(),
            ..ModuleSelection::new()
        };

        let result = asm.assemble(&modules, "// payload");
        assert!(
            result.is_ok(),
            "Assembly failed for carrier={}, decoder={}: {:?}",
            carrier,
            decoder,
            result.err()
        );
        let output = result.unwrap();
        assert!(
            output.len() > 500,
            "Output too short ({}) for {}/{}",
            output.len(),
            carrier,
            decoder
        );
    }
}

#[test]
fn test_assembler_module_content_injected() {
    let dir = common::templates_dir();
    let mut asm = Assembler::new(&dir).unwrap();
    let modules = ModuleSelection::new(); // alloc_rw_rx + xor

    let output = asm.assemble(&modules, "// payload").unwrap();

    // alloc_rw_rx carrier contains VirtualAlloc call
    assert!(
        output.contains("VirtualAlloc"),
        "Missing carrier content (VirtualAlloc)"
    );
    // xor decoder contains XOR_KEY
    assert!(
        output.contains("XOR_KEY"),
        "Missing decoder content (XOR_KEY)"
    );
    // definitions.h defines p_RW and p_RX
    assert!(
        output.contains("p_RW") || output.contains("p_RX"),
        "Missing definitions content"
    );
}

#[test]
fn test_assembler_definitions_stripped() {
    let dir = common::templates_dir();
    let mut asm = Assembler::new(&dir).unwrap();
    let modules = ModuleSelection::new();

    let output = asm.assemble(&modules, "// payload").unwrap();

    // Module files' `#include "definitions.h"` should be stripped during read_module
    // The actual definitions content IS present (injected via @MODULE:definitions)
    // but duplicate includes from individual modules should be gone
    let include_count = output.matches(r#"#include "definitions.h""#).count();
    // There might be zero includes of definitions.h since they're all stripped
    // and the content itself is injected inline
    assert!(
        include_count == 0
            || output
                .matches(r#"#include "../header/definitions.h""#)
                .count()
                == 0,
        "definitions.h includes from modules should be stripped"
    );

    // But the actual definitions should be present
    assert!(output.contains("#define p_RW"), "p_RW should be defined");
    assert!(output.contains("#define p_RX"), "p_RX should be defined");
}

#[test]
fn test_assembler_invalid_module_fails() {
    let dir = common::templates_dir();
    let mut asm = Assembler::new(&dir).unwrap();
    let modules = ModuleSelection {
        carrier: "nonexistent_carrier".to_string(),
        ..ModuleSelection::new()
    };

    let result = asm.assemble(&modules, "// payload");
    assert!(result.is_err(), "Should fail for nonexistent module");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Module not found"),
        "Error should mention 'Module not found', got: {}",
        err_msg
    );
}

#[test]
fn test_assembler_invalid_template_dir() {
    let result = Assembler::new("/nonexistent/path/to/templates");
    assert!(result.is_err(), "Should fail for nonexistent directory");
}

#[test]
fn test_list_modules_categories() {
    let dir = common::templates_dir();
    let asm = Assembler::new(&dir).unwrap();

    let carriers = asm.list_modules("carrier").unwrap();
    assert!(carriers.contains(&"alloc_rw_rx".to_string()));
    assert!(carriers.contains(&"change_rw_rx".to_string()));
    assert!(carriers.contains(&"peb_walk".to_string()));

    let decoders = asm.list_modules("decoder").unwrap();
    assert!(decoders.contains(&"xor".to_string()));
    assert!(decoders.contains(&"english".to_string()));

    let antiemu = asm.list_modules("antiemulation").unwrap();
    assert!(antiemu.contains(&"none".to_string()));
    assert!(antiemu.contains(&"sirallocalot".to_string()));
    assert!(antiemu.contains(&"timeraw".to_string()));

    // "guardrail" maps to "guardrails/" directory
    let guardrails = asm.list_modules("guardrail").unwrap();
    assert!(guardrails.contains(&"none".to_string()));
    assert!(guardrails.contains(&"env".to_string()));

    let vp = asm.list_modules("virtualprotect").unwrap();
    assert!(vp.contains(&"standard".to_string()));
    assert!(vp.contains(&"undersized".to_string()));

    let decoys = asm.list_modules("decoy").unwrap();
    assert!(decoys.contains(&"none".to_string()));
    assert!(decoys.contains(&"winexec".to_string()));
}

#[test]
fn test_list_modules_unknown_category() {
    let dir = common::templates_dir();
    let asm = Assembler::new(&dir).unwrap();

    let result = asm.list_modules("foobar");
    assert!(result.is_err(), "Unknown category should fail");
}

#[test]
fn test_assembler_caching() {
    let dir = common::templates_dir();
    let mut asm = Assembler::new(&dir).unwrap();
    let modules = ModuleSelection::new();

    let output1 = asm.assemble(&modules, "// payload").unwrap();
    let output2 = asm.assemble(&modules, "// payload").unwrap();
    assert_eq!(output1, output2, "Cached result should match first");

    asm.clear_cache();
    let output3 = asm.assemble(&modules, "// payload").unwrap();
    assert_eq!(
        output1, output3,
        "Result after cache clear should match original"
    );
}

// ── Mutation marker tests (pure string, no FS) ─────────────────────────────

#[test]
fn test_extract_markers_basic() {
    let source = common::c_source_with_mutate_markers();
    let markers = extract_mutation_markers(source);

    assert_eq!(markers.len(), 4, "Expected 4 markers");

    assert_eq!(markers[0].name, "timing_jitter");
    assert!(markers[0].params.is_empty());

    assert_eq!(markers[1].name, "execution_method");
    assert_eq!(
        markers[1].params,
        vec!["direct", "callback", "fiber", "threadpool"]
    );

    assert_eq!(markers[2].name, "opaque_predicate");
    assert!(markers[2].params.is_empty());

    assert_eq!(markers[3].name, "control_flow_flattening");
    assert!(markers[3].params.is_empty());

    // Line numbers should be 1-indexed
    assert!(markers[0].line >= 1);
    assert!(markers[1].line > markers[0].line);
}

#[test]
fn test_extract_markers_empty_source() {
    let markers = extract_mutation_markers("");
    assert!(markers.is_empty());
}

#[test]
fn test_extract_markers_no_markers() {
    let source = common::c_source_minimal();
    let markers = extract_mutation_markers(source);
    assert!(markers.is_empty());
}

#[test]
fn test_extract_markers_params_with_pipes() {
    let source = "// @MUTATE:execution_method(direct|callback|fiber)\n";
    let markers = extract_mutation_markers(source);

    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].name, "execution_method");
    assert_eq!(markers[0].params, vec!["direct", "callback", "fiber"]);
}

#[test]
fn test_strip_markers_preserves_code() {
    let source = common::c_source_with_mutate_markers();
    let stripped = strip_mutation_markers(source);

    assert!(!stripped.contains("@MUTATE"), "Markers should be removed");
    assert!(
        stripped.contains("do_something();"),
        "Code should be preserved"
    );
    assert!(stripped.contains("return 0;"), "Code should be preserved");
    assert!(
        stripped.contains("int main()"),
        "Function signature preserved"
    );
}

#[test]
fn test_strip_markers_idempotent() {
    let source = common::c_source_with_mutate_markers();
    let once = strip_mutation_markers(source);
    let twice = strip_mutation_markers(&once);
    assert_eq!(once, twice, "strip(strip(x)) should equal strip(x)");
}

// ── Payload embedding in assembled output ───────────────────────────────────

#[test]
fn test_payload_header_embedded_in_assembled_output() {
    use build::{EncodingType, PayloadEncoder};

    let dir = common::templates_dir();
    let mut asm = Assembler::new(&dir).unwrap();
    let modules = ModuleSelection::new();

    // Generate a real payload header
    let payload = common::payload_small();
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::Xor);
    let header = encoder.generate_c_header(&encoded);

    let output = asm.assemble(&modules, &header).unwrap();

    // The payload header content should be present in assembled output
    assert!(
        output.contains("PAYLOAD_LEN"),
        "Assembled output should contain PAYLOAD_LEN from payload header"
    );
    assert!(
        output.contains("supermega_payload"),
        "Assembled output should contain supermega_payload array"
    );
    assert!(
        output.contains("XOR_KEY"),
        "Assembled output should contain XOR_KEY"
    );
}

#[test]
fn test_assembled_output_contains_loader_structure() {
    let dir = common::templates_dir();
    let mut asm = Assembler::new(&dir).unwrap();
    let modules = ModuleSelection::new();

    let output = asm.assemble(&modules, "// payload placeholder").unwrap();

    // The loader_template.c structure should be preserved
    assert!(output.contains("int main(void)"), "Missing main function");
    assert!(
        output.contains("guardrail()"),
        "Missing guardrail() call in main"
    );
    assert!(
        output.contains("antiemulation()"),
        "Missing antiemulation() call in main"
    );
    assert!(
        output.contains("return carrier()"),
        "Missing carrier() call in main"
    );
}

// ── Edge case: markers in unusual positions ─────────────────────────────────

#[test]
fn test_strip_markers_preserves_string_content() {
    // If a string literal contains @MUTATE text, strip_mutation_markers
    // will still strip it (it's a line-based filter). This tests that behavior.
    let source = r#"int main() {
    char *s = "// @MUTATE:timing_jitter";
    return 0;
}"#;
    let stripped = strip_mutation_markers(source);

    // The line containing @MUTATE is stripped (line-based filter)
    assert!(
        !stripped.contains("@MUTATE"),
        "Lines with @MUTATE should be stripped regardless of context"
    );
    assert!(
        stripped.contains("return 0;"),
        "Non-marker lines should be preserved"
    );
}

#[test]
fn test_strip_markers_multiple_markers_per_section() {
    // loader_template.c has multiple @MUTATE markers in main() body
    let dir = common::templates_dir();
    let mut asm = Assembler::new(&dir).unwrap();
    let modules = ModuleSelection::new();

    let output = asm.assemble(&modules, "// payload").unwrap();

    // Count markers before strip
    let marker_count = extract_mutation_markers(&output).len();
    assert!(
        marker_count > 5,
        "Assembled source should have many @MUTATE markers, found {}",
        marker_count
    );

    // All should be gone after strip
    let stripped = strip_mutation_markers(&output);
    assert!(
        !stripped.contains("// @MUTATE:"),
        "All markers should be stripped"
    );

    // Code between markers should survive
    assert!(stripped.contains("antiemulation()"));
    assert!(stripped.contains("decoy()"));
    assert!(stripped.contains("return carrier()"));
}
