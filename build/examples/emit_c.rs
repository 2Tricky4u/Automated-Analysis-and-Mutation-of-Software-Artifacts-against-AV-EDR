//! Emit the instrumented C source from the modular build pipeline.
//!
//! Reproduces the exact same AST-level output as `build_modular_template()` +
//! `apply_instrumentation()` in builder.rs — stops right before clang/LLVM.
//!
//! Usage:
//!   cargo run -p build --example emit_c
//!   cargo run -p build --example emit_c -- --carrier peb_walk --encoding english -o out.c
//!   cargo run -p build --example emit_c -- --payload sc.bin -m ast.string_xor --trace on
//!   cargo run -p build --example emit_c -- --deconditioner basic -m ast.decon_rounds:count=50 --trace off -o mutated.c

use build::mutator::MutationSpec;
use build::{
    Assembler, EncodingType, ModuleSelection, PayloadEncoder, SourceLanguage, TraceFormat,
    inject_line_traces_with_opts, strip_mutation_markers,
};
use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs, process};

fn main() {
    let args = Args::parse();

    if args.help {
        print_help();
        return;
    }

    // ── Step 1: Payload (matches builder.rs:1297-1300) ──────────────────────
    let payload = match &args.payload {
        Some(path) => fs::read(path).unwrap_or_else(|e| {
            eprintln!("Error reading payload '{}': {}", path.display(), e);
            process::exit(1);
        }),
        None => {
            eprintln!("[emit_c] No --payload given, using 256-byte test payload (NOPs + INT3)");
            build::generate_test_payload(256)
        }
    };

    // Parse encoding
    let encoding = EncodingType::from_str(&args.encoding).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        process::exit(1);
    });

    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, encoding);
    let payload_header = encoder.generate_c_header(&encoded);

    eprintln!(
        "[emit_c] Payload: {} bytes → {} byte {}-encoded header",
        payload.len(),
        payload_header.len(),
        args.encoding
    );

    // ── Step 2: Assemble (matches builder.rs:1322-1327) ─────────────────────
    let templates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let mut assembler = Assembler::new(&templates_dir).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        process::exit(1);
    });

    let modules = ModuleSelection {
        carrier: args.carrier.clone(),
        decoder: args.decoder.clone(),
        antiemulation: args.antiemulation.clone(),
        deconditioner: args.deconditioner.clone(),
        guardrail: args.guardrail.clone(),
        virtualprotect: args.virtualprotect.clone(),
        decoy: args.decoy.clone(),
    };

    let assembled = assembler
        .assemble(&modules, &payload_header)
        .unwrap_or_else(|e| {
            eprintln!("Assembly error: {}", e);
            process::exit(1);
        });

    eprintln!(
        "[emit_c] Assembled: {} bytes (carrier={}, decoder={}, antiemulation={}, guardrail={}, virtualprotect={}, decoy={})",
        assembled.len(),
        modules.carrier,
        modules.decoder,
        modules.antiemulation,
        modules.guardrail,
        modules.virtualprotect,
        modules.decoy
    );

    // ── Step 3: Mutate + strip markers (matches builder.rs:1339-1351) ───────
    let final_source = if !args.mutations.is_empty() {
        for spec in &args.mutations {
            eprintln!("[emit_c] Mutation: {} {:?}", spec.id, spec.params);
        }
        let (mutated, applied) =
            build::mutator::Mutator::apply(assembled.as_bytes(), &args.mutations).unwrap_or_else(
                |e| {
                    eprintln!("Mutation error: {}", e);
                    process::exit(1);
                },
            );
        if !applied.is_empty() {
            eprintln!("[emit_c] Applied mutations: {:?}", applied);
        }
        strip_mutation_markers(&String::from_utf8_lossy(&mutated))
    } else {
        strip_mutation_markers(&assembled)
    };

    // ── Step 4: AST line tracing (matches builder.rs:899-937) ───────────────
    let output = if args.trace {
        // Exactly mirrors apply_instrumentation():
        //   language = SourceLanguage::from_path(...)  → C
        //   format   = TraceFormat::default()          → Binary
        let trace_filename = format!("modular_{}_{}.c", args.carrier, args.decoder);
        let instrumented = inject_line_traces_with_opts(
            &final_source,
            SourceLanguage::C,
            &trace_filename,
            TraceFormat::default(), // Binary — matches builder.rs:920
        )
        .unwrap_or_else(|e| {
            eprintln!("Trace injection error: {}", e);
            process::exit(1);
        });

        let trace_count = instrumented.matches("__trace_line_binary(\"").count();
        eprintln!(
            "[emit_c] Instrumented: {} bytes, {} trace calls (Binary format)",
            instrumented.len(),
            trace_count
        );
        instrumented
    } else {
        eprintln!(
            "[emit_c] Trace off — uninstrumented source ({} bytes)",
            final_source.len()
        );
        final_source
    };

    // ── Step 5: Output ──────────────────────────────────────────────────────
    match &args.output {
        Some(path) => {
            fs::write(path, &output).unwrap_or_else(|e| {
                eprintln!("Write error: {}", e);
                process::exit(1);
            });
            eprintln!("[emit_c] Wrote {} bytes → {}", output.len(), path.display());
        }
        None => print!("{}", output),
    }
}

// ── Args ────────────────────────────────────────────────────────────────────

struct Args {
    help: bool,
    payload: Option<PathBuf>,
    output: Option<PathBuf>,
    carrier: String,
    decoder: String,
    antiemulation: String,
    deconditioner: String,
    guardrail: String,
    virtualprotect: String,
    decoy: String,
    encoding: String,
    trace: bool,
    mutations: Vec<MutationSpec>,
    // Track which flags the user explicitly set (for auto-sync)
    decoder_set: bool,
    encoding_set: bool,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            help: false,
            payload: None,
            output: None,
            carrier: "change_rw_rx".into(),
            decoder: "xor".into(),
            antiemulation: "none".into(),
            deconditioner: "none".into(),
            guardrail: "env".into(),
            virtualprotect: "standard".into(),
            decoy: "winexec".into(),
            encoding: "xor".into(),
            trace: false,
            mutations: vec![],
            decoder_set: false,
            encoding_set: false,
        };
        let argv: Vec<String> = env::args().skip(1).collect();
        let mut i = 0;
        while i < argv.len() {
            match argv[i].as_str() {
                "--help" | "-h" => a.help = true,
                "--payload" | "-p" => {
                    i += 1;
                    a.payload = Some(PathBuf::from(&argv[i]));
                }
                "--output" | "-o" => {
                    i += 1;
                    a.output = Some(PathBuf::from(&argv[i]));
                }
                "--carrier" => {
                    i += 1;
                    a.carrier = argv[i].clone();
                }
                "--decoder" => {
                    i += 1;
                    a.decoder = argv[i].clone();
                    a.decoder_set = true;
                }
                "--antiemulation" => {
                    i += 1;
                    a.antiemulation = argv[i].clone();
                }
                "--deconditioner" => {
                    i += 1;
                    a.deconditioner = argv[i].clone();
                }
                "--guardrail" => {
                    i += 1;
                    a.guardrail = argv[i].clone();
                }
                "--virtualprotect" => {
                    i += 1;
                    a.virtualprotect = argv[i].clone();
                }
                "--decoy" => {
                    i += 1;
                    a.decoy = argv[i].clone();
                }
                "--encoding" => {
                    i += 1;
                    a.encoding = argv[i].clone();
                    a.encoding_set = true;
                }
                "--trace" => {
                    i += 1;
                    a.trace = !matches!(argv[i].as_str(), "off" | "false" | "0");
                }
                "--mutation" | "-m" => {
                    i += 1;
                    a.mutations.push(MutationSpec::from_cli_str(&argv[i]));
                }
                other => {
                    eprintln!("Unknown argument: {}", other);
                    process::exit(1);
                }
            }
            i += 1;
        }

        // Auto-sync encoding ↔ decoder (same logic as build_artifact)
        if a.decoder_set && !a.encoding_set {
            a.encoding = a.decoder.clone();
            eprintln!(
                "[emit_c] Auto-synced encoding to '{}' (from --decoder)",
                a.encoding
            );
        } else if a.encoding_set && !a.decoder_set {
            a.decoder = a.encoding.clone();
            eprintln!(
                "[emit_c] Auto-synced decoder to '{}' (from --encoding)",
                a.decoder
            );
        }

        a
    }
}

fn print_help() {
    eprintln!(
        r#"emit_c — Emit the instrumented C source from the modular build pipeline

Reproduces builder.rs Steps 1-4 + AST line tracing, stops before clang/LLVM.

USAGE:
    cargo run -p build --example emit_c -- [OPTIONS]

OPTIONS:
    -h, --help                      Show this help
    -p, --payload <FILE>            Raw .bin payload (default: 256-byte test payload)
    -o, --output <FILE>             Write to file (default: stdout)
    --carrier <NAME>                change_rw_rx | alloc_rw_rx | peb_walk  (default: change_rw_rx)
    --decoder <NAME>                xor | english  (default: xor)
    --antiemulation <NAME>          none | sirallocalot | timeraw | cpuburn | heapstress | fsenum | sleepaccel  (default: none)
    --deconditioner <NAME>          none | alloc_loop | alloc_exec | basic | thread_alloc | mixed_apis | entropy_flood  (default: none)
    --guardrail <NAME>              env | none  (default: env)
    --virtualprotect <NAME>         standard | undersized  (default: standard)
    --decoy <NAME>                  winexec | none  (default: winexec)
    --encoding <TYPE>               xor | english  (default: xor)
    --trace <on|off>                AST line tracing (default: off)
    -m, --mutation <ID[:params]>    Mutation to apply (repeatable). Params: key=val,key=val

MUTATION SYNTAX:
    -m <id>                         No params (uses defaults)
    -m <id>:key=val                 Single param
    -m <id>:k1=v1,k2=v2            Multiple params

AVAILABLE MUTATIONS:
    ast.string_xor                  XOR-encode string literals (param: xor_key=0xAA)
    ast.decon_rounds                Loop count (params: count=20, method=fixed|runtime)
    ast.fill_pattern                Fill data (params: pattern=xor|nop_sled|random|zero)
    ast.exec_decoy                  Execute from alloc'd mem (params: method=none|direct|thread)
    ast.timing_pattern              Inter-op delays (params: min_ms=10, max_ms=100)
    ast.protection_transition       Mem protection (params: pattern=rw_rx|rw_rwx|rw_r_rx)
    llvm.nop_insert                 LLVM IR NOP insertion (param: density=0.3)
    llvm.opaque_predicate           LLVM IR opaque predicates (param: density=0.3)
    llvm.junk_block                 LLVM IR dead blocks (param: count=2)

    Note: llvm.* mutations operate on IR text, not C source. In emit_c they
    will match block-label patterns in the C output but are intended for the
    IR stage of the full build pipeline.

EXAMPLES:
    cargo run -p build --example emit_c
    cargo run -p build --example emit_c -- -o instrumented.c
    cargo run -p build --example emit_c -- --carrier peb_walk --encoding english -o out.c
    cargo run -p build --example emit_c -- --antiemulation sirallocalot -o out.c
    cargo run -p build --example emit_c -- --trace off -o uninstrumented.c
    cargo run -p build --example emit_c -- -m ast.string_xor -o mutated.c

    # Deconditioning mutations (require --deconditioner basic):
    cargo run -p build --example emit_c -- --deconditioner basic -m ast.decon_rounds:count=50,method=fixed --trace off -o mutated.c
    cargo run -p build --example emit_c -- --deconditioner basic -m ast.fill_pattern:pattern=nop_sled -m ast.exec_decoy:method=direct --trace off -o mutated.c
    cargo run -p build --example emit_c -- --deconditioner basic -m ast.timing_pattern:min_ms=50,max_ms=200 --trace off -o mutated.c
    cargo run -p build --example emit_c -- --deconditioner basic -m ast.protection_transition:pattern=rw_rwx --trace off -o mutated.c"#
    );
}
