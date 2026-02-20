//! Build a Windows PE artifact using the full modular template pipeline.
//!
//! Calls `ArtifactBuilder.build(BuildInput::ModularTemplate { ... })` directly,
//! producing a cross-compiled Windows PE .exe via Clang/LLVM + xwin SDK.
//!
//! Requires: clang + lld-link + xwin SDK (run from WSL2).
//!
//! Usage:
//!   cargo run -p build --example build_artifact -- -p shellcode.bin
//!   cargo run -p build --example build_artifact -- -p sc.bin -o artifact.exe
//!   cargo run -p build --example build_artifact -- -p sc.bin --carrier peb_walk --encoding english
//!   cargo run -p build --example build_artifact -- -p sc.bin -m ast.string_xor --trace lines

use build::mutator::MutationSpec;
use build::{ArtifactBuilder, BuildInput, BuilderConfig, EncodingType, ModuleSelection};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs, process};

fn main() {
    let args = Args::parse();

    if args.help {
        print_help();
        return;
    }

    // Payload is required — no test fallback for real .exe builds
    let payload_path = match &args.payload {
        Some(p) => p.clone(),
        None => {
            eprintln!("Error: --payload / -p is required");
            eprintln!("Usage: cargo run -p build --example build_artifact -- -p <PAYLOAD.bin>");
            process::exit(1);
        }
    };

    let payload = fs::read(&payload_path).unwrap_or_else(|e| {
        eprintln!("Error reading payload '{}': {}", payload_path.display(), e);
        process::exit(1);
    });

    eprintln!(
        "[build_artifact] Payload: {} ({} bytes)",
        payload_path.display(),
        payload.len()
    );

    // Resolve paths from CARGO_MANIFEST_DIR (build/)
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let config = BuilderConfig {
        output_dir: args.output_dir.clone(),
        xwin_dir: args.xwin,
        runtime_src: manifest.join("runtime/instrumentation_runtime.c"),
        minimal_runtime_src: manifest.join("runtime/minimal_runtime.c"),
        modular_template_dir: manifest.join("templates"),
    };

    let builder = ArtifactBuilder::new(config).unwrap_or_else(|e| {
        eprintln!("Error initializing builder: {}", e);
        process::exit(1);
    });

    // Parse encoding
    let encoding = EncodingType::from_str(&args.encoding).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        process::exit(1);
    });

    // Build module selection
    let modules = ModuleSelection {
        carrier: args.carrier,
        decoder: args.decoder,
        antiemulation: args.antiemulation,
        deconditioner: args.deconditioner,
        guardrail: args.guardrail,
        virtualprotect: args.virtualprotect,
        decoy: args.decoy,
    };

    // Parse mutation specs
    let mutations: Vec<MutationSpec> = args
        .mutations
        .iter()
        .map(|id| MutationSpec {
            id: id.clone(),
            params: HashMap::new(),
        })
        .collect();

    let input = BuildInput::ModularTemplate {
        modules: modules.clone(),
        payload,
        encoding,
        mutations,
        trace_mode: args.trace.clone(),
    };

    eprintln!(
        "[build_artifact] Modules: carrier={}, decoder={}, antiemulation={}, deconditioner={}, guardrail={}, virtualprotect={}, decoy={}",
        modules.carrier,
        modules.decoder,
        modules.antiemulation,
        modules.deconditioner,
        modules.guardrail,
        modules.virtualprotect,
        modules.decoy,
    );
    eprintln!(
        "[build_artifact] Encoding: {}, Trace: {}",
        args.encoding, args.trace
    );
    if !args.mutations.is_empty() {
        eprintln!("[build_artifact] Mutations: {:?}", args.mutations);
    }

    // builder.build() is async — spin up a tokio runtime
    let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e| {
        eprintln!("Failed to create tokio runtime: {}", e);
        process::exit(1);
    });

    let artifact = rt.block_on(builder.build(input)).unwrap_or_else(|e| {
        eprintln!("[build_artifact] Build FAILED: {:#}", e);
        process::exit(1);
    });

    // Print metadata
    eprintln!("[build_artifact] ─── Build succeeded ───");
    eprintln!("[build_artifact] artifact_id:  {}", artifact.artifact_id);
    eprintln!(
        "[build_artifact] size:         {} bytes",
        artifact.size_bytes
    );
    eprintln!(
        "[build_artifact] output:       {}",
        artifact.output_path.display()
    );
    eprintln!(
        "[build_artifact] mutations:    {:?}",
        artifact.mutations_applied
    );
    eprintln!(
        "[build_artifact] compiler:     {}",
        artifact.compiler_version
    );

    // Copy to user-specified path if -o given
    if let Some(dest) = &args.output {
        fs::copy(&artifact.output_path, dest).unwrap_or_else(|e| {
            eprintln!("Error copying artifact to '{}': {}", dest.display(), e);
            process::exit(1);
        });
        eprintln!("[build_artifact] Copied -> {}", dest.display());
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
    trace: String,
    mutations: Vec<String>,
    xwin: PathBuf,
    output_dir: PathBuf,
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
            trace: "off".into(),
            mutations: vec![],
            xwin: PathBuf::from("/root/.xwin"),
            output_dir: PathBuf::from("./artifacts"),
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
                    a.trace = argv[i].clone();
                }
                "--mutation" | "-m" => {
                    i += 1;
                    a.mutations.push(argv[i].clone());
                }
                "--xwin" => {
                    i += 1;
                    a.xwin = PathBuf::from(&argv[i]);
                }
                "--output-dir" => {
                    i += 1;
                    a.output_dir = PathBuf::from(&argv[i]);
                }
                other => {
                    eprintln!("Unknown argument: {}", other);
                    process::exit(1);
                }
            }
            i += 1;
        }

        // Auto-sync encoding ↔ decoder:
        // - If user set only --decoder, sync encoding to match
        // - If user set only --encoding, sync decoder to match
        // - If both set, encoding wins (builder enforces this too)
        // - If neither set, both default to "xor"
        if a.decoder_set && !a.encoding_set {
            a.encoding = a.decoder.clone();
            eprintln!(
                "[build_artifact] Auto-synced encoding to '{}' (from --decoder)",
                a.encoding
            );
        } else if a.encoding_set && !a.decoder_set {
            a.decoder = a.encoding.clone();
            eprintln!(
                "[build_artifact] Auto-synced decoder to '{}' (from --encoding)",
                a.decoder
            );
        }

        a
    }
}

fn print_help() {
    eprintln!(
        r#"build_artifact — Build a Windows PE artifact using the full modular template pipeline

Calls ArtifactBuilder.build(BuildInput::ModularTemplate) to produce a cross-compiled
Windows PE .exe via Clang/LLVM + xwin SDK.  Requires WSL2 with clang + lld + xwin.

USAGE:
    cargo run -p build --example build_artifact -- -p <PAYLOAD> [OPTIONS]

REQUIRED:
    -p, --payload <FILE>            Raw .bin payload

OPTIONS:
    -h, --help                      Show this help
    -o, --output <FILE>             Copy final .exe to this path (default: print artifact path)
    --carrier <NAME>                change_rw_rx | alloc_rw_rx | peb_walk  (default: change_rw_rx)
    --decoder <NAME>                xor | english  (default: xor)
    --antiemulation <NAME>          none | sirallocalot | timeraw | cpuburn | heapstress | fsenum | sleepaccel  (default: none)
    --deconditioner <NAME>          none | alloc_loop | alloc_exec | thread_alloc | mixed_apis | entropy_flood  (default: none)
    --guardrail <NAME>              env | none  (default: env)
    --virtualprotect <NAME>         standard | undersized  (default: standard)
    --decoy <NAME>                  winexec | none  (default: winexec)
    --encoding <TYPE>               xor | english  (default: xor)
    --trace <MODE>                  off | lines  (default: off)
    -m, --mutation <ID>             Mutation to apply (repeatable)
    --xwin <DIR>                    xwin SDK path (default: /root/.xwin)
    --output-dir <DIR>              Build output dir (default: ./artifacts)

EXAMPLES:
    # Default build
    cargo run -p build --example build_artifact -- -p shellcode.bin -o artifact.exe

    # With mutation
    cargo run -p build --example build_artifact -- -p sc.bin -m ast.string_xor -o mutated.exe

    # With tracing
    cargo run -p build --example build_artifact -- -p sc.bin --trace lines -o traced.exe

    # Different modules
    cargo run -p build --example build_artifact -- -p sc.bin --carrier peb_walk --encoding english --decoder english -o peb.exe

    # Custom xwin path
    cargo run -p build --example build_artifact -- -p sc.bin --xwin /opt/xwin -o out.exe"#
    );
}
