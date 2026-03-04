# Build Crate — Struct & Module Reference

Companion to `BUILD-SYSTEM-ARCHITECTURE.md` (behavioral documentation) and `templates/TEMPLATE-SYSTEM.md` (template assembly). This document provides a **type-level reference** for every public struct, enum, and their methods in the `build` crate.

---

## Module Tree

```
crate build
├── enum TraceMode
├── mod builder
│   ├── struct ArtifactBuilder
│   ├── enum BuildInput
│   ├── struct BuilderConfig
│   ├── struct BuiltArtifact
│   ├── struct PreparedPayload
│   ├── struct XwinPaths
│   └── fn prepare_payload
├── mod instrument
│   ├── mod instrumenter
│   │   └── struct Instrumenter
│   └── mod line_tracer
│       ├── enum SourceLanguage
│       ├── enum TraceFormat
│       └── fn inject_line_traces*
├── mod msvc_compat
│   ├── struct MsvcCompat
│   ├── async fn invoke_msvc_link
│   └── async fn wsl_to_win_path
├── mod mutator
│   ├── struct MutationSpec
│   └── struct Mutator
├── mod template
│   ├── mod assembler
│   │   ├── struct Assembler
│   │   ├── struct ModuleSelection
│   │   ├── struct MutationMarker
│   │   ├── fn extract_mutation_markers
│   │   ├── fn strip_markers_outside_targets
│   │   └── fn strip_mutation_markers
│   ├── mod payload
│   │   ├── struct PayloadEncoder
│   │   ├── struct EncodedPayload
│   │   ├── enum EncodingType
│   │   └── fn generate_test_payload
│   ├── mod sc_checkpoints
│   │   ├── struct PatchedShellcode
│   │   ├── struct BreakpointEntry
│   │   ├── fn patch_shellcode
│   │   └── fn generate_c_header
│   └── mod shellcode_stub
│       └── fn prepend_checkpoint_stub
└── mod transform
    ├── mod ast_mutator
    │   ├── struct AstMutator
    │   ├── struct NumberLiteralInfo  (private)
    │   └── struct VpCallParts       (private)
    ├── mod benign_catalog
    │   ├── struct BehaviorEntry
    │   ├── struct BehaviorGraph
    │   ├── enum BehaviorGroup
    │   ├── fn default_catalog
    │   └── fn generate_insertion
    ├── mod binary_data
    │   ├── struct RichProfile
    │   ├── struct RichRecord
    │   ├── struct BenignImport
    │   └── fn build_manifest, build_version_info, encode_rich_header, ...
    ├── mod binary_mutator
    │   └── struct BinaryMutator
    └── mod ir_mutator
        └── struct IrMutator
```

---

## 1. Core Types (`lib.rs`)

### `TraceMode` enum

Controls what instrumentation is injected into artifacts. Drives the Two-Run Differential Protocol.

| Variant | String | Description |
|---------|--------|-------------|
| `Off` | `"off"` | No instrumentation — baseline binary |
| `Api` | `"api"` | API call tracing only |
| `BB` | `"bb"` | Basic-block coverage (SanitizerCoverage) |
| `ApiPlusBB` | `"api+bb"` | API tracing + BB coverage (default for mutation loop) |
| `Lines` | `"lines"` | Line-level tracing (Run A of differential protocol) |
| `LinesAroundBB(u32)` | `"lines-around-bb:N"` | Targeted line tracing around BB `N` |
| `All` | `"all"` | All instrumentation combined (debug) |

**Traits:** `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Serialize`, `Deserialize`, `FromStr`, `Display`

**Roundtrip:** `"api+bb".parse::<TraceMode>().to_string() == "api+bb"` — full symmetry.

---

## 2. Builder Module (`builder.rs`)

### `BuilderConfig`

Configuration bag passed at construction time. Defines all paths the builder needs.

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `output_dir` | `PathBuf` | `"artifacts"` | Where temp files and final `.exe` are written |
| `xwin_dir` | `PathBuf` | `"/root/.xwin"` | Root of xwin SDK (headers + libs) |
| `runtime_src` | `PathBuf` | `"build/runtime/instrumentation_runtime.c"` | Instrumented runtime source |
| `minimal_runtime_src` | `PathBuf` | `"build/runtime/minimal_runtime.c"` | Minimal runtime (direct-syscall exit) |
| `modular_template_dir` | `PathBuf` | `"build/templates"` | Template + module directory |
| `msvc_compat` | `Option<MsvcCompat>` | `None` | `None` = Clang+LLD, `Some` = clang-cl+link.exe |

---

### `XwinPaths`

Resolved absolute paths into the xwin SDK tree. Constructed once in `ArtifactBuilder::new()`.

| Field | Type | Example |
|-------|------|---------|
| `crt_include` | `String` | `<xwin>/crt/include` |
| `sdk_ucrt_include` | `String` | `<xwin>/sdk/include/ucrt` |
| `sdk_shared_include` | `String` | `<xwin>/sdk/include/shared` |
| `sdk_um_include` | `String` | `<xwin>/sdk/include/um` |
| `sdk_winrt_include` | `String` | `<xwin>/sdk/include/winrt` |
| `crt_lib` | `String` | `<xwin>/crt/lib/x86_64` |
| `sdk_ucrt_lib` | `String` | `<xwin>/sdk/lib/ucrt/x86_64` |
| `sdk_um_lib` | `String` | `<xwin>/sdk/lib/um/x86_64` |

**Methods:**

| Method | Returns | Used by |
|--------|---------|---------|
| `new(xwin_dir)` | `Result<Self>` | `ArtifactBuilder::new()` |
| `include_args()` | `Vec<&str>` — `-isystem <path>` pairs | Standard Clang driver |
| `clang_cl_include_args()` | `Vec<String>` — `/imsvc<path>` | `--driver-mode=cl` |
| `lib_args()` | `Vec<&str>` — `-L <path>` pairs | Single-step Clang |
| `lld_lib_args()` | `Vec<String>` — `/libpath:<path>` | Two-step lld-link |

---

### `BuildInput` enum

Currently a single variant — the entry point for all builds.

```rust
pub enum BuildInput {
    ModularTemplate {
        modules:              ModuleSelection,
        payload:              Vec<u8>,              // Raw shellcode
        encoding:             EncodingType,
        mutations:            Vec<MutationSpec>,
        trace_mode:           String,               // "off"|"lines"|"api"|"bb"|"api+bb"|"all"
        mutation_targets:     Vec<String>,           // Scope mutations to specific modules
        sc_checkpoint_count:  Option<u32>,           // INT3 shellcode checkpoints
        precomputed_payload:  Option<PreparedPayload>, // Skip encoding if provided
    },
}
```

---

### `ArtifactBuilder`

The main facade. Thin wrapper holding config and resolved SDK paths.

| Field | Type | Visibility |
|-------|------|------------|
| `config` | `BuilderConfig` | private |
| `xwin` | `XwinPaths` | private |

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `(config: BuilderConfig) -> Result<Self>` | Validates xwin_dir, creates output_dir, resolves XwinPaths |
| `build` | `async (&self, BuildInput) -> Result<BuiltArtifact>` | Public entry — delegates to `build_modular_template` |
| `build_modular_template` | `async (self, ...)` | 7-step pipeline (see below) |
| `invoke_clang_internal` | `async (self, ...)` | Single-step C→EXE (no LLVM mutations) |
| `compile_source_to_ir` | `async (self, ...)` | C→LLVM IR via `clang -S -emit-llvm` |
| `compile_ir_to_object` | `async (self, ...)` | `.ll`→`.o` via `clang -c` |
| `link_baseline_exe` | `async (self, ...)` | `.o`→`.exe` via lld-link (no runtime) |
| `link_instrumented_exe` | `async (self, ...)` | `.o` + runtime `.o`→`.exe` |
| `apply_instrumentation` | `async (self, ...)` | Re-build with line tracing + coverage |
| `compile_runtime` | `async (self, ...)` | Compile `instrumentation_runtime.c`→`.o` |
| `ensure_minimal_runtime` | `async (self, ...)` | Compile + cache `minimal_runtime.c`→`.o` |
| `finalize_artifact` | `async (self, ...)` | SHA256 → rename to `<hash>.exe` |

**Build pipeline (7 steps):**

```
Step 0: Decoder sync (auto-match decoder module to encoding type)
Step 1: Payload encoding (prepare_payload → PreparedPayload)
Step 2: Template assembly (Assembler.assemble → single .c)
Step 3: Marker scoping (strip_markers_outside_targets)
Step 4: AST mutations → strip markers
Step 5: Compile
  ├── [LLVM path] C→IR→mutate IR→.o→link_baseline_exe
  └── [Direct path] C→invoke_clang_internal→.exe
Step 5b: Binary mutations (BinaryMutator on PE bytes)
Step 6: Finalize (SHA256 rename → BuiltArtifact)
Step 7: [if trace_mode!="off"] apply_instrumentation (re-build with tracing)
```

---

### `BuiltArtifact`

Return value from a successful build. Serializable for Elastic ingestion.

| Field | Type | Description |
|-------|------|-------------|
| `artifact_id` | `String` | SHA256 hex (canonical identifier) |
| `source_path` | `PathBuf` | Path to assembled `.c` source |
| `output_path` | `PathBuf` | Final `<sha256>.exe` path |
| `size_bytes` | `u64` | PE file size |
| `sha256` | `String` | Same as `artifact_id` |
| `build_timestamp` | `DateTime<Utc>` | Build time |
| `compiler_version` | `String` | `clang --version` first line |
| `compiler_flags` | `Vec<String>` | Flag snapshot for provenance |
| `mutations_applied` | `Vec<String>` | IDs of mutations that succeeded |
| `assembled_source` | `Option<String>` | Full assembled C source (ModularTemplate only) |

---

### `PreparedPayload`

Caching intermediate — skip re-encoding for repeated builds with the same shellcode.

| Field | Type | Description |
|-------|------|-------------|
| `payload_header` | `String` | Generated `payload.h` content (C header) |
| `sc_header` | `Option<String>` | `sc_checkpoints_table.h` content (if INT3 patching active) |

**Construction:** `prepare_payload(payload, encoding, encoder, trace_mode, sc_count, stub_size)` free function.

---

## 3. Template Module (`template/`)

### `ModuleSelection` (`assembler.rs`)

Declares which module variant fills each gene slot in the loader template.

| Field | Type | Default | Available Variants |
|-------|------|---------|-------------------|
| `carrier` | `String` | `"alloc_rw_rx"` | `alloc_rw_rx`, `change_rw_rx`, `peb_walk` |
| `decoder` | `String` | `"xor"` | `xor`, `english`, `none`, `subbyte` |
| `antiemulation` | `String` | `"none"` | `none`, `sirallocalot`, `timeraw`, `cpuburn`, `heapstress`, `fsenum`, `sleepaccel` |
| `deconditioner` | `String` | `"none"` | `none`, `basic`, `alloc_loop`, `alloc_exec`, `thread_alloc`, `mixed_apis`, `entropy_flood` |
| `guardrail` | `String` | `"none"` | `none`, `env` |
| `virtualprotect` | `String` | `"standard"` | `standard`, `undersized` |
| `decoy` | `String` | `"none"` | `none`, `winexec` |

**Methods:**

| Method | Description |
|--------|-------------|
| `new()` | Constructs with documented defaults |
| `validate(&self, template_dir)` | Checks all 7 module files exist on disk |

---

### `Assembler` (`assembler.rs`)

Stateful template assembler with module file caching.

| Field | Type | Purpose |
|-------|------|---------|
| `template_dir` | `PathBuf` | Root of templates directory |
| `module_cache` | `HashMap<String, String>` | Caches cleaned module file contents |

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `(template_dir) -> Result<Self>` | Validates directory exists |
| `assemble` | `(&mut self, &ModuleSelection, &str) -> Result<String>` | Replace all `@MODULE:xxx` markers → single `.c` file |
| `list_modules` | `(&self, category) -> Result<Vec<String>>` | List available `.c` files in a category |
| `clear_cache` | `(&mut self)` | Empty module cache |

**Assembly order:** payload → definitions → decoder → virtualprotect → antiemulation → deconditioner → guardrail → decoy → carrier. Each replacement wrapped in `// --- BEGIN/END MODULE: xxx ---` boundaries.

---

### `MutationMarker` (`assembler.rs`)

Parsed `// @MUTATE:name(params)` annotation from source code.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Mutation name (e.g., `"timing_jitter"`) |
| `params` | `Vec<String>` | Pipe-separated params from parens |
| `line` | `usize` | 1-indexed line number |
| `column` | `usize` | Byte offset of `//` in line |

**Construction:** Only via `extract_mutation_markers(source)`.

**Related free functions:**

| Function | Description |
|----------|-------------|
| `extract_mutation_markers(source)` | Parse all `@MUTATE` annotations → `Vec<MutationMarker>` |
| `strip_markers_outside_targets(source, targets)` | Remove `@MUTATE` lines outside targeted module boundaries |
| `strip_mutation_markers(source)` | Remove all markers + boundary comments before compilation |

---

### `EncodingType` (`payload.rs`)

| Variant | Size Impact | Decoder Module |
|---------|-------------|----------------|
| `Xor` (default) | 1:1 | `"xor"` |
| `English` | ~4-6x | `"english"` |
| `None` | 1:1 | `"none"` |
| `SubByte` | 2:1 | `"subbyte"` |

**Method:** `decoder_module() -> &'static str` — Maps encoding to its required decoder module name.

---

### `PayloadEncoder` (`payload.rs`)

Stateful encoder holding encoding parameters.

| Field | Type | Purpose |
|-------|------|---------|
| `xor_key` | `[u8; 2]` | Rolling 2-byte XOR key (default: `[0xAA, 0x55]`) |
| `dictionary` | `Vec<String>` | 256-word list for English encoding |
| `subbyte_mapping` | `[u8; 16]` | 16-entry nibble→byte lookup table |

**Construction:**

| Constructor | Description |
|-------------|-------------|
| `new()` | Default key `[0xAA, 0x55]`, default dictionary, default subbyte mapping |
| `with_xor_key(key)` | Custom XOR key |
| `with_subbyte_mapping(mapping)` | Custom nibble mapping |

**Methods:**

| Method | Description |
|--------|-------------|
| `encode(&self, payload, encoding) -> EncodedPayload` | Route to encoding-specific encoder |
| `generate_c_header(&self, encoded) -> String` | Produce C header code for the encoded payload |
| `generate_dictionary() -> Vec<String>` | Build 256-word dictionary (60 common + 196 synthetic) |

---

### `EncodedPayload` (`payload.rs`)

Output of `PayloadEncoder::encode()`.

| Field | Type | Description |
|-------|------|-------------|
| `encoding` | `EncodingType` | Which encoding was applied |
| `data` | `Vec<u8>` | Encoded bytes |
| `metadata` | `HashMap<String, String>` | Encoding-specific params (keys, mapping, original_len) |

---

### `PatchedShellcode` (`sc_checkpoints.rs`)

Return of `patch_shellcode()`. Bundles modified shellcode + checkpoint metadata.

| Field | Type | Description |
|-------|------|-------------|
| `bytes` | `Vec<u8>` | Shellcode with `0xCC` at checkpoint offsets |
| `table` | `Vec<BreakpointEntry>` | Ordered checkpoint entries |

---

### `BreakpointEntry` (`sc_checkpoints.rs`)

One INT3 breakpoint inserted into shellcode.

| Field | Type | Description |
|-------|------|-------------|
| `offset` | `usize` | Byte offset from shellcode start (includes stub prefix) |
| `original_byte` | `u8` | Byte replaced by `0xCC` (VEH restores this) |
| `name` | `String` | `"sc_checkpoint_0"`, `"sc_checkpoint_1"`, ... |
| `progress_pct` | `u8` | 0–100 progress estimate through shellcode body |

**Related functions:**

| Function | Description |
|----------|-------------|
| `patch_shellcode(shellcode, count, stub_size)` | Insert evenly-spaced INT3 at instruction boundaries (iced-x86 recursive descent) |
| `generate_c_header(patched)` | Emit `sc_checkpoints_table.h` for VEH runtime |

---

## 4. Mutator Module (`mutator/`)

### `MutationSpec`

A parsed mutation specification. Layer-agnostic.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Dot-namespaced ID: `"ast.decon_rounds"`, `"llvm.nop_insert"`, `"binary.rich_header"` |
| `params` | `HashMap<String, String>` | Key-value parameters |

**Methods:**

| Method | Description |
|--------|-------------|
| `from_cli_str(s)` | Parse `"id:key=val,key=val"` CLI syntax |
| `parse()` | Split `id` on `.` → `(category, name)` tuple |

**CLI format examples:**
```
ast.decon_rounds:count=50,method=fixed
llvm.nop_insert:density=0.3
binary.rich_header:donor=notepad
ast.string_xor                         # no params
```

---

### `Mutator`

Stateless router. Zero-size unit struct.

| Method | Signature | Description |
|--------|-----------|-------------|
| `apply` | `(input: &[u8], mutations: &[MutationSpec]) -> Result<(Vec<u8>, Vec<String>)>` | Route mutations to AST/IR engines |

**Routing logic:**

| Category prefix | Routed to | Phase |
|----------------|-----------|-------|
| `ast.*` | `AstMutator::apply()` | Phase 1 (C source) |
| `llvm.*` | `IrMutator::apply()` | Phase 2 (LLVM IR) |
| `binary.*` | Skipped (handled post-link in `builder.rs`) | — |

---

## 5. Transform Module (`transform/`)

### `AstMutator` (`ast_mutator.rs`)

Tree-sitter-based C source transformer.

| Field | Type | Purpose |
|-------|------|---------|
| `parser` | `tree_sitter::Parser` | Initialized with C grammar |

**Construction:** `AstMutator::new() -> Result<Self>` or `Default::default()`.

**Primary method:**

```rust
pub fn apply(&mut self, source: &str, mutations: &[&MutationSpec]) -> Result<(String, Vec<String>)>
```

**Processing order:**

| Phase | Mutations | Mechanism |
|-------|-----------|-----------|
| 1 | Marker-based (`decon_rounds`, `fill_pattern`, etc.) | Scan for `@MUTATE:name`, apply bottom-up (reverse line order) |
| 1.5 | `ast.benign_syscall_insert` | Global: tree-sitter walk on target function body |
| 2a | `ast.const_obfuscation` | Global: all `number_literal` nodes |
| 2b | `ast.string_xor` | Global: all `string_literal` nodes (runs last) |

**Marker-based mutations (via `apply_at_marker`):**

| Marker | Method | Key Params | Effect |
|--------|--------|------------|--------|
| `decon_rounds` | `apply_decon_rounds` | `count`, `method` | Replace loop bound with fixed or `GetTickCount() % N` |
| `fill_pattern` | `apply_fill_pattern` | `pattern` | Change fill data: xor/nop_sled/random/zero |
| `exec_decoy` | `apply_exec_decoy` | `method` | Insert direct/threaded execution from decoy memory |
| `timing_pattern` | `apply_timing_pattern` | `min_ms`, `max_ms` | Insert `Sleep()` before next statement |
| `protection_transition` | `apply_protection_transition` | `pattern` | Alter VirtualProtect sequence (rw_rx/rw_rwx/rw_r_rx) |
| `benign_preamble` | `apply_benign_preamble` | `count`, `seed` | Insert benign API calls from catalog |
| `api_sequence_obfuscation` | `apply_api_sequence_obfuscation` | `count`, `seed` | Insert benign calls to dilute N-gram signatures |

**Global mutations:**

| ID | Method | Key Params | Effect |
|----|--------|------------|--------|
| `ast.benign_syscall_insert` | `apply_benign_syscall_insert` | `groups`, `count`, `density`, `seed`, `target_fn` | Distribute benign API calls across function body |
| `ast.const_obfuscation` | `apply_const_obfuscation` | `min_value`, `seed` | Decompose integer constants via volatile arithmetic |
| `ast.string_xor` | `apply_string_xor` | `xor_key` | XOR-encode string literals with runtime decode |

---

### `NumberLiteralInfo` (`ast_mutator.rs`, private)

Metadata for one qualifying integer literal (used by `const_obfuscation`).

| Field | Type | Purpose |
|-------|------|---------|
| `start_byte` | `usize` | Byte offset of literal in source |
| `end_byte` | `usize` | End byte offset |
| `value` | `u64` | Parsed integer value |
| `stmt_start_byte` | `usize` | Byte offset of containing statement |
| `stmt_indent` | `String` | Whitespace prefix for declaration alignment |

**Filters:** Skips floats, preprocessor directives, array sizes, case labels, initializer lists, global-scope constants, already-obfuscated `__obf_c` declarations, and values below `min_value`.

---

### `VpCallParts` (`ast_mutator.rs`, private)

Parsed `VirtualProtect`-like call for `protection_transition` mutation.

| Field | Type | Purpose |
|-------|------|---------|
| `func_name` | `String` | Full function name (e.g., `"MyVirtualProtect"`) |
| `args` | `Vec<String>` | Exactly 4 args: `[addr, size, protection, old_prot_ptr]` |

**Construction:** `parse_vp_call(line) -> Option<VpCallParts>` — finds `VirtualProtect`, walks back for wrapper prefix, depth-tracks parentheses, splits on commas.

---

### `BehaviorEntry` (`benign_catalog.rs`)

One benign Windows API call with dependencies and declarations.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `u32` | Unique ID (namespace: 0-9 SystemQuery, 10-19 FileIo, 20-29 RegistryIo) |
| `group` | `BehaviorGroup` | Group membership |
| `deps` | `Vec<u32>` | IDs that must execute before this entry |
| `declarations` | `Vec<&'static str>` | C variable declarations (e.g., `"char __be_env[256];"`) |
| `code` | `&'static str` | C statement to insert |

**Current catalog (9 entries):**

| Group | Entries | API Chain |
|-------|---------|-----------|
| `SystemQuery` | 0, 1, 2 | `GetEnvironmentVariableA`, `GetComputerNameA`, `GetTickCount` (independent) |
| `FileIo` | 10→11→12 | `CreateFileA` → `ReadFile` → `CloseHandle` (chained deps) |
| `RegistryIo` | 20→21→22 | `RegOpenKeyExA` → `RegQueryValueExA` → `RegCloseKey` (chained deps) |

---

### `BehaviorGroup` (`benign_catalog.rs`)

| Variant | String | APIs |
|---------|--------|------|
| `SystemQuery` | `"system_query"` | GetTickCount, GetEnvironmentVariable, GetComputerName |
| `FileIo` | `"file_io"` | CreateFileA, ReadFile, CloseHandle |
| `RegistryIo` | `"registry_io"` | RegOpenKeyExA, RegQueryValueExA, RegCloseKey |

---

### `BehaviorGraph` (`benign_catalog.rs`)

Dependency-aware topological scheduler for benign API insertion.

| Field | Type | Purpose |
|-------|------|---------|
| `entries` | `HashMap<u32, BehaviorEntry>` | All filtered entries by ID |
| `remaining_deps` | `HashMap<u32, HashSet<u32>>` | Unsatisfied parent IDs per node |
| `children` | `HashMap<u32, Vec<u32>>` | Reverse: parent → children |
| `frontier` | `Vec<u32>` | Ready-to-consume (all deps satisfied), sorted |
| `consumed` | `HashSet<u32>` | Already-popped IDs |
| `rng_state` | `u64` | xorshift64 state for randomized selection |

**Methods:**

| Method | Description |
|--------|-------------|
| `new(catalog, allowed_groups, seed)` | Filter catalog, build dependency graph, seed frontier |
| `pop() -> Option<BehaviorEntry>` | Random frontier pick, unlock children, return entry |
| `remaining() -> usize` | Count of unconsumed entries |

**Public API:** `generate_insertion(groups, count, seed) -> (Vec<String>, Vec<String>)` — Returns `(declarations, statements)` with deduped declarations.

---

### `BinaryMutator` (`binary_mutator.rs`)

Post-link PE binary transformer.

| Field | Type | Purpose |
|-------|------|---------|
| `pe_bytes` | `Vec<u8>` | Owned PE file bytes, mutated in-place |

**Construction:** `BinaryMutator::new(pe_bytes: Vec<u8>)` — takes ownership.

**Primary method:**

```rust
pub fn apply(mut self, mutations: &[&MutationSpec]) -> Result<(Vec<u8>, Vec<String>)>
```

Consumes `self`, returns `(modified_bytes, applied_ids)`. Validates MZ signature. Applies in two phases:

**Phase 1 — Individual section additions:**

| Mutation ID | Params | Effect |
|-------------|--------|--------|
| `binary.rich_header` | `donor`: notepad/calc/explorer | Inject MSVC Rich header between DOS stub and PE sig |
| `binary.import_pad` | `count` (default 50) | Add `.idata` section with benign dead imports |
| `binary.resource_inject` | `product_name`, `company`, `original_filename` | Add `.rsrc` with VS_VERSIONINFO + XML manifest |
| `binary.section_rename` | — | Rename non-standard sections to MSVC names |
| `binary.timestamp` | `age_days` (default 365) or `timestamp` | Backdate COFF timestamp with FNV1a jitter |

**Phase 2 — Consolidated `.rdata` append:**

| Mutation ID | Params | Effect |
|-------------|--------|--------|
| `binary.debug_dir` | `pdb_path` | Embed IMAGE_DEBUG_DIRECTORY + CodeView RSDS record |
| `binary.string_inject` | `count` (default 20) | Append benign Windows-flavored strings |
| `binary.size_pad` | `target_kb` (default 256) | Pad to target file size |
| `binary.entropy_normalize` | `target` (default 6.0) | Adjust file entropy via low-entropy padding |

After both phases: recomputes PE checksum and validates with `goblin::pe::PE::parse()`.

**Key internal method:** `add_section(name, data, characteristics) -> Result<u32>` — Appends data at file-aligned EOF, writes section header, increments NumberOfSections, updates SizeOfImage.

---

### `RichProfile` / `RichRecord` (`binary_data.rs`)

Donor Rich header profiles from real MSVC-compiled executables.

**`RichRecord`:**

| Field | Type | Purpose |
|-------|------|---------|
| `product_id` | `u16` | PE tool type (e.g., `0x0104` = Linker14) |
| `build_number` | `u16` | Tool minor build number |
| `count` | `u32` | Objects compiled with this tool |

**`RichProfile`:**

| Field | Type | Purpose |
|-------|------|---------|
| `name` | `&'static str` | Profile identifier |
| `records` | `&'static [RichRecord]` | Decoded records |

**Static profiles:**

| Profile | Based on | Records | Character |
|---------|----------|---------|-----------|
| `notepad` | MSVC 2022 v17.8 | 5 | Small C app |
| `calc` | MSVC 2019 v16.11 | 5 | Medium app |
| `explorer` | MSVC 2022 v17.4 | 7 | Large C++ app |

**Lookup:** `get_rich_profile("calc") -> &'static RichProfile`

---

### `BenignImport` (`binary_data.rs`)

One DLL and its exported functions for dead import injection.

| Field | Type | Purpose |
|-------|------|---------|
| `dll` | `&'static str` | DLL name (e.g., `"user32.dll"`) |
| `functions` | `&'static [(&'static str, u16)]` | `(func_name, import_hint)` pairs |

**Pool:** 15 DLLs, ~45 total functions covering user32, advapi32, shell32, ole32, gdi32, version, winhttp, ws2_32, bcrypt, crypt32, shlwapi, comctl32, msvcrt, secur32, wtsapi32.

---

### `IrMutator` (`ir_mutator.rs`)

Text-based LLVM IR transformer. No LLVM C-API dependency.

| Field | Type | Purpose |
|-------|------|---------|
| `rng_state` | `u32` | LCG state (glibc constants) |

**Construction:**

| Constructor | Seed |
|-------------|------|
| `new()` | `1234` |
| `with_seed(seed)` | `seed` (deterministic) |

**Primary method:**

```rust
pub fn apply(&mut self, ir_text: &str, mutations: &[&MutationSpec]) -> Result<(String, Vec<String>)>
```

**Supported mutations:**

| Mutation ID | Params | Effect |
|-------------|--------|--------|
| `llvm.nop_insert` | `density` (f32, default 0.3) | Insert `asm sideeffect "nop"` after BB labels |
| `llvm.opaque_predicate` | `density`, `mode` (robust/trivial) | Replace `br label` with opaque `br i1` (both targets identical) |
| `llvm.junk_block` | `count` (u32, default 2) | Add `unreachable` dead blocks before function `}` |

**RNG:** glibc LCG `state = state * 1103515245 + 12345`, output `(state >> 16) / 65536.0`.

---

## 6. Instrument Module (`instrument/`)

### `Instrumenter` (`instrumenter.rs`)

LLVM IR instrumentation injector. Zero-size unit struct.

**Construction:** `Instrumenter::new()` or `Default::default()`.

**Primary method:**

```rust
pub async fn instrument(&mut self, ir_path: &Path, trace_mode: TraceMode, output_path: &Path) -> Result<()>
```

**Behavior by trace mode:**

| Flag | Condition | Action |
|------|-----------|--------|
| `needs_bb` | `BB \| ApiPlusBB \| All` | Count SanitizerCoverage callbacks (validation only — Clang inserts them) |
| `needs_api` | `Api \| ApiPlusBB \| Lines \| LinesAroundBB \| All` | `inject_api_tracing()` — insert checkpoint calls before target API calls |

**Target APIs for tracing:** VirtualAlloc, VirtualProtect, WriteProcessMemory, CreateRemoteThread, LoadLibrary, GetProcAddress, CreateProcess, OpenProcess.

---

### `TraceFormat` / `SourceLanguage` (`line_tracer.rs`)

| `TraceFormat` | Description |
|---------------|-------------|
| `Base64` | Base64-encoded trace events |
| `Binary` | 32-byte structured `InstRecordHeader` |

| `SourceLanguage` | Detection |
|-------------------|-----------|
| `C` | `.c`, `.h` extensions |
| `Rust` | `.rs` extension |

---

## 7. MSVC Compat Module (`msvc_compat.rs`)

### `MsvcCompat`

| Field | Type | Purpose |
|-------|------|---------|
| `vcvarsall_path` | `PathBuf` | WSL path to `vcvarsall.bat` |

**Method:** `default_vcvarsall() -> PathBuf` — Probes VS 2022 editions: BuildTools → Community → Professional → Enterprise.

**Module functions:**

| Function | Description |
|----------|-------------|
| `wsl_to_win_path(wsl_path) -> Result<String>` | Convert via `wslpath -wa` |
| `invoke_msvc_link(vcvarsall, objects, output, libs, extra_flags)` | Write temp `.bat`, invoke via `cmd.exe /c` |

**Constant:** `DRIVER_MODE_CL = "--driver-mode=cl"` — Activates `clang-cl` behavior from regular `clang`.

---

## Data Flow Summary

```
BuildInput::ModularTemplate
    │
    ├── ModuleSelection ───────────► Assembler.assemble()
    │                                    └── loader_template.c + @MODULE replacements
    │
    ├── payload: Vec<u8> ──────────► PayloadEncoder.encode() → EncodedPayload
    │   encoding: EncodingType              └── generate_c_header() → payload.h
    │                                            │
    │   sc_checkpoint_count ───────► patch_shellcode() → PatchedShellcode
    │                                    └── generate_c_header() → sc_checkpoints_table.h
    │
    ├── mutations: Vec<MutationSpec>
    │       │
    │       ├── ast.* ─────────────► AstMutator.apply(C source)
    │       │                            ├── @MUTATE marker handlers
    │       │                            ├── benign_syscall_insert (BehaviorGraph)
    │       │                            ├── const_obfuscation (NumberLiteralInfo)
    │       │                            └── string_xor
    │       │
    │       ├── llvm.* ────────────► IrMutator.apply(LLVM IR text)
    │       │                            ├── nop_insert
    │       │                            ├── opaque_predicate
    │       │                            └── junk_block
    │       │
    │       └── binary.* ──────────► BinaryMutator.apply(PE bytes)
    │                                    ├── rich_header (RichProfile)
    │                                    ├── import_pad (BenignImport)
    │                                    ├── resource_inject
    │                                    ├── section_rename
    │                                    ├── timestamp
    │                                    └── consolidated: debug_dir, string_inject,
    │                                        size_pad, entropy_normalize
    │
    ├── trace_mode ────────────────► Instrumenter.instrument(LLVM IR)
    │                                    └── inject_api_tracing + runtime declarations
    │
    └── BuilderConfig
            ├── xwin_dir ──────────► XwinPaths (include/lib args)
            └── msvc_compat ───────► MsvcCompat (clang-cl + link.exe path)
```
