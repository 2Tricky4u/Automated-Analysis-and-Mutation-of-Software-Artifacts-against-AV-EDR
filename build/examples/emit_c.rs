//! Emit the instrumented C source from the modular build pipeline.
//!
//! Reproduces the exact same AST-level output as `build_modular_template()` +
//! `apply_instrumentation()` in builder.rs — stops right before clang/LLVM.
//!
//! Usage:
//!   cargo run -p build --example emit_c
//!   cargo run -p build --example emit_c -- --antiemulation sirallocalot -o out.c
//!   cargo run -p build --example emit_c -- --payload sc.bin -m ast.string_xor --trace off

use build::mutator::MutationSpec;
use build::{
    Assembler, EncodingType, ModuleSelection, PayloadEncoder, SourceLanguage, TraceFormat,
    inject_line_traces_with_opts, strip_mutation_markers,
};
use std::collections::HashMap;
use std::path::PathBuf;
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

    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::Xor);
    let payload_header = encoder.generate_c_header(&encoded);

    eprintln!(
        "[emit_c] Payload: {} bytes → {} byte XOR-encoded header",
        payload.len(),
        payload_header.len()
    );

    // ── Step 2: Assemble (matches builder.rs:1322-1327) ─────────────────────
    let templates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let mut assembler = Assembler::new(&templates_dir).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        process::exit(1);
    });

    let modules = ModuleSelection {
        carrier: "change_rw_rx".into(),
        decoder: "xor".into(),
        antiemulation: args.antiemulation.clone(),
        deconditioner: args.deconditioner.clone(),
        guardrail: "env".into(),
        virtualprotect: "standard".into(),
        decoy: "winexec".into(),
    };

    let assembled = assembler
        .assemble(&modules, &payload_header)
        .unwrap_or_else(|e| {
            eprintln!("Assembly error: {}", e);
            process::exit(1);
        });

    eprintln!(
        "[emit_c] Assembled: {} bytes (carrier=change_rw_rx, decoder=xor, antiemulation={}, guardrail=env, decoy=winexec)",
        assembled.len(),
        modules.antiemulation
    );

    // ── Step 3: Mutate + strip markers (matches builder.rs:1339-1351) ───────
    let final_source = if !args.mutations.is_empty() {
        let specs: Vec<MutationSpec> = args
            .mutations
            .iter()
            .map(|id| MutationSpec {
                id: id.clone(),
                params: HashMap::new(),
            })
            .collect();
        let (mutated, applied) = build::mutator::Mutator::apply(assembled.as_bytes(), &specs)
            .unwrap_or_else(|e| {
                eprintln!("Mutation error: {}", e);
                process::exit(1);
            });
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
        let instrumented = inject_line_traces_with_opts(
            &final_source,
            SourceLanguage::C,
            "modular_change_rw_rx_xor.c",
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
    antiemulation: String,
    deconditioner: String,
    trace: bool,
    mutations: Vec<String>,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            help: false,
            payload: None,
            output: None,
            antiemulation: "none".into(),
            deconditioner: "none".into(),
            trace: true,
            mutations: vec![],
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
                "--antiemulation" => {
                    i += 1;
                    a.antiemulation = argv[i].clone();
                }
                "--deconditioner" => {
                    i += 1;
                    a.deconditioner = argv[i].clone();
                }
                "--trace" => {
                    i += 1;
                    a.trace = !matches!(argv[i].as_str(), "off" | "false" | "0");
                }
                "--mutation" | "-m" => {
                    i += 1;
                    a.mutations.push(argv[i].clone());
                }
                other => {
                    eprintln!("Unknown argument: {}", other);
                    process::exit(1);
                }
            }
            i += 1;
        }
        a
    }
}

fn print_help() {
    eprintln!(
        r#"emit_c — Emit the instrumented C source from the modular build pipeline

Reproduces builder.rs Steps 1-4 + AST line tracing, stops before clang/LLVM.

Fixed modules: carrier=change_rw_rx, decoder=xor, guardrail=env,
               virtualprotect=standard, decoy=winexec, encoding=xor,
               deconditioner=none (configurable via --deconditioner)

USAGE:
    cargo run -p build --example emit_c -- [OPTIONS]

OPTIONS:
    -h, --help                      Show this help
    -p, --payload <FILE>            Raw .bin payload (default: 256-byte test payload)
    -o, --output <FILE>             Write to file (default: stdout)
    --antiemulation <NAME>          none | sirallocalot | timeraw | cpuburn | heapstress | fsenum | sleepaccel  (default: none)
    --deconditioner <NAME>          none | alloc_loop | alloc_exec | thread_alloc | mixed_apis | entropy_flood  (default: none)
    --trace <on|off>                AST line tracing (default: on)
    -m, --mutation <ID>             Mutation to apply (repeatable)

EXAMPLES:
    cargo run -p build --example emit_c
    cargo run -p build --example emit_c -- -o instrumented.c
    cargo run -p build --example emit_c -- --antiemulation sirallocalot -o out.c
    cargo run -p build --example emit_c -- --trace off -o uninstrumented.c
    cargo run -p build --example emit_c -- -m ast.string_xor -o mutated.c
    cargo run -p build --example emit_c -- -p real_payload.bin -o full.c"#
    );
}
