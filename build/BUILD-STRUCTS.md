# Build System Module & Struct Reference

## Module Hierarchy

```
crate build
│
├── lib.rs                              Crate root, core types, LowLevelBuilder
│
├── mod builder                         High-level artifact builder (pub)
│   └── builder.rs                      ArtifactBuilder, BuilderConfig, BuildInput, BuiltArtifact
│
├── mod template                        Template assembly & payload encoding (pub)
│   ├── mod assembler                   @MODULE replacement, module cache
│   │   └── assembler.rs                Assembler, ModuleSelection, MutationMarker
│   └── mod payload                     Payload encoding (XOR, English)
│       └── payload.rs                  PayloadEncoder, EncodedPayload, EncodingType
│
├── mod transform                       AST & IR mutation (pub)
│   ├── mod ast_mutator                 Source-level transforms
│   │   └── ast_mutator.rs              AstMutator (line tracing, planned mutations)
│   └── mod ir_mutator                  LLVM IR transforms
│       └── ir_mutator.rs               IrMutator (stub)
│
├── mod instrument                      Instrumentation injection (pub)
│   ├── mod instrumenter                IR-level BB/API instrumentation
│   │   └── instrumenter.rs             Instrumenter (SanitizerCoverage, API tracing)
│   └── mod line_tracer                 AST-level line tracing
│       └── line_tracer.rs              SourceLanguage, TraceFormat, inject_line_traces
│
├── mod mutator                         Mutation application engine (pub)
│   └── mod.rs                          Mutator, MutationSpec
│
└── mod compiler                        Reserved placeholder (pub)
    └── mod.rs                          (empty — compilation lives in builder.rs)
```

---

## Module Details

### `lib.rs` — Crate Root & Core Types

**Enums**

```rust
enum TraceMode {
    Off,                // No instrumentation
    Api,                // API tracing only (checkpoint calls)
    BB,                 // Basic-block coverage (LLVM SanitizerCoverage)
    ApiPlusBB,          // BB + API (DEFAULT for mutation loop)
    Lines,              // Line-level tracing (diagnostic, source-level)
    LinesAroundBB(u32), // Targeted line tracing near specific BB
    All,                // Everything (debug mode)
}
// Serde: rename_all = "lowercase"
// ApiPlusBB → "api+bb", LinesAroundBB → "lines-around-bb"
```

**Core Structs**

```rust
struct BuildConfig {
    target: String,           // "x86_64-pc-windows-msvc"
    optimization: String,     // "0" | "1" | "2" | "3" | "s" | "z"
    trace_mode: TraceMode,    // Default: Lines
    deterministic: bool,      // Pin timestamps, disable ASLR entropy
    xwin_path: PathBuf,       // Default: ~/.xwin
    llvm_passes: Vec<String>, // Custom opt passes
    seed: u64,                // Mutation seed for reproducibility
}
// Used by: LowLevelBuilder

struct Mutation {
    id: String,                          // "ast.import_reshape", "ir.opaque_predicates"
    params: HashMap<String, String>,     // Key-value mutation parameters
}
// Used by: LowLevelBuilder, ArtifactMetadata

struct ArtifactMetadata {
    artifact_id: String,           // SHA256 of final PE ("sha256:...")
    artifact_path: PathBuf,        // Location of .exe
    source_template: String,       // Original source filename
    mutations: Vec<Mutation>,      // Applied mutations
    config: BuildConfig,           // Build settings used
    build_timestamp: String,       // RFC3339
    toolchain: ToolchainInfo,      // Compiler versions
}
// Produced by: LowLevelBuilder.generate_metadata()

struct ToolchainInfo {
    clang_version: String,
    llvm_version: String,
    xwin_version: String,
}
```

**LowLevelBuilder — LLVM IR Pipeline**

```rust
struct LowLevelBuilder {
    config: BuildConfig,
}
```

| Method | Visibility | Description |
|--------|------------|-------------|
| `new(config)` | pub | Create builder with given config |
| `build(source, mutations, output)` | pub async | Full pipeline: Source → PE executable |
| `apply_ast_mutations(source, mutations, output)` | pub(crate) async | Filter `ast.*` mutations, inject line tracing if needed |
| `compile_to_ir(source, output)` | pub(crate) async | `clang -emit-llvm -S` with xwin headers |
| `apply_ir_mutations(ir, mutations, output)` | pub(crate) async | Filter `ir.*` mutations (stub: copies input) |
| `inject_instrumentation(ir, output)` | pub(crate) async | Delegate to Instrumenter based on trace_mode |
| `compile_to_obj(ir, output)` | pub(crate) async | `llc -filetype=obj` |
| `compile_runtime_library(temp_dir)` | pub(crate) async | Compile instrumentation_runtime.c → .obj |
| `compile_minimal_runtime(temp_dir)` | pub(crate) async | Compile minimal_runtime.c → .obj |
| `link_to_pe(obj_files, output)` | pub(crate) async | `ld.lld -flavor link` with xwin libs |
| `find_runtime_include_dir()` | pub(crate) | Locate build/runtime include path |
| `generate_metadata(source, mutations, artifact)` | pub(crate) | SHA256 hash + metadata struct |

**LowLevelBuilder.build() Pipeline:**
```
source.c → apply_ast_mutations → mutated.c
                                      │
                         compile_to_ir ▼
                                  artifact.ll → apply_ir_mutations → mutated.ll
                                                                          │
                                                  inject_instrumentation ▼
                                                                  instrumented.ll
                                                                          │
                                                          compile_to_obj ▼
                                                                  artifact.obj
                                                                          │
                                     + minimal_runtime.obj ──────► link_to_pe
                                     + [instrumentation_runtime.obj]       │
                                                                          ▼
                                                                  artifact.exe
```

---

### `builder` — High-Level Artifact Builder

```rust
struct BuilderConfig {
    templates_dir: PathBuf,         // "corpus/templates"
    output_dir: PathBuf,            // "artifacts"
    xwin_dir: PathBuf,              // "/root/.xwin"
    runtime_src: PathBuf,           // "build/runtime/instrumentation_runtime.c"
    minimal_runtime_src: PathBuf,   // "build/runtime/minimal_runtime.c"
    modular_template_dir: PathBuf,  // "build/templates"
}
// Used by: ArtifactBuilder

struct ArtifactBuilder {
    config: BuilderConfig,
}

enum BuildInput {
    SourceFile {
        template_name: String,           // Template directory name
        source_file: String,             // Source filename
        mutations: Vec<MutationSpec>,    // Mutations to apply
        trace_mode: String,              // "off"|"lines"|"api"|"bb"|"api+bb"|"all"
    },
    LlvmIr {
        ir_code: Vec<u8>,               // Raw IR bytes
        artifact_name: String,           // Name for temp files
        mutations: Vec<MutationSpec>,    // LLVM IR mutations
        trace_mode: String,
    },
    SourceCode {
        source_code: Vec<u8>,           // Raw C code bytes
        artifact_name: String,
        mutations: Vec<MutationSpec>,
        trace_mode: String,
    },
    ModularTemplate {                    // PREFERRED
        modules: ModuleSelection,        // Carrier, decoder, etc.
        payload: Vec<u8>,                // Raw payload bytes
        encoding: EncodingType,          // XOR or English
        mutations: Vec<MutationSpec>,    // AST mutations at @MUTATE markers
        trace_mode: String,
    },
}

struct BuiltArtifact {
    artifact_id: String,                    // SHA256 hex digest
    source_path: PathBuf,                   // Original source location
    output_path: PathBuf,                   // Final .exe in artifacts/
    size_bytes: u64,                        // PE file size
    sha256: String,                         // Same as artifact_id
    build_timestamp: DateTime<Utc>,         // Build time
    compiler_version: String,               // "clang version X.Y.Z"
    compiler_flags: Vec<String>,            // Flags used
    mutations_applied: Vec<String>,         // Successfully applied mutation IDs
}
// Produced by: ArtifactBuilder.build()
```

| Method | Visibility | Description |
|--------|------------|-------------|
| `new(config)` | pub | Create builder, validate xwin_dir exists |
| `build(input)` | pub async | Dispatch to build variant by BuildInput |
| `build_template(name, file, trace)` | pub async | Build from template source (no mutations) |
| `build_template_with_mutations(name, file, mutations, trace)` | pub async | Build from template with mutations |
| `build_template_with_runtime(name, file, trace)` | self async | Compile source + runtime, link |
| `build_template_with_mutations_and_runtime(name, file, muts, trace)` | self async | Mutate → compile → link |
| `build_modular_template(modules, payload, enc, muts, trace)` | self async | Encode → assemble → mutate → compile |
| `build_from_source_code(code, name, muts, trace)` | self async | Build from in-memory C source |
| `build_from_source_code_internal(code, name, trace)` | self async | Compile in-memory source (no mutations) |
| `build_from_source_code_with_mutations(code, name, muts, trace)` | self async | Mutate in-memory source → compile |
| `build_from_llvm_ir(ir, name, muts, trace)` | self async | Build from LLVM IR bytes |
| `build_from_llvm_ir_internal(ir, name, trace)` | self async | Compile IR (no mutations) |
| `build_from_llvm_ir_with_mutations(ir, name, muts, trace)` | self async | Mutate IR → compile |
| `apply_instrumentation(artifact, source, trace)` | self async | Post-build: inject line traces → IR instrument → recompile |
| `compile_source_to_ir(source, output)` | self async | `clang -S -emit-llvm -O0` |
| `compile_ir_to_object(ir, output)` | self async | `clang -c` on IR file |
| `compile_ir_to_exe(ir, output)` | self async | `clang` IR directly to .exe |
| `invoke_clang(source, output)` | self async | Full clang compile+link (source → .exe) |
| `invoke_clang_internal(source, output, trace, extra_sources)` | self async | Core clang invocation with xwin flags |
| `invoke_clang_on_ir(ir, output)` | self async | Clang compile IR → .exe |
| `link_object_to_exe(obj, output)` | self async | `lld-link` object to PE |
| `link_instrumented_exe(source_obj, runtime_obj, minimal_obj, output)` | self async | `lld-link` all objects → PE |
| `compile_runtime(trace_mode)` | self async | Compile runtime .c → .obj |
| `verify_runtime_symbols(obj_path)` | self async | `llvm-nm` to check exported symbols |
| `compute_sha256(path)` | self | SHA256 digest of file |
| `get_clang_version()` | self | `clang --version` |
| `get_compiler_flags(trace, source, output)` | self | Build full clang arg list |

**Free Functions:**

| Function | Visibility | Description |
|----------|------------|-------------|
| `get_template_libs(template_name)` | pub(self) | Return extra linker libs per template (advapi32, user32, ws2_32, wininet) |

**ArtifactBuilder.build() Dispatch:**
```
build(input)
    │
    ├── SourceFile ───────────────────────────────────────────────────┐
    │     ├── mutations.is_empty()?                                   │
    │     │     YES ──► build_template_with_runtime()                 │
    │     │     NO  ──► build_template_with_mutations_and_runtime()   │
    │     │                  ├── has ast.*? → Mutator::apply()        │
    │     │                  ├── has llvm.*? → compile_source_to_ir() │
    │     │                  │                 Mutator::apply(ir)     │
    │     │                  │                 compile_ir_to_exe()    │
    │     │                  └── else → invoke_clang_internal()       │
    │     └── trace != "off"? → apply_instrumentation()              │
    │                                                                 │
    ├── LlvmIr ──► build_from_llvm_ir_with_mutations()               │
    │                  Mutator::apply(ir) → invoke_clang_on_ir()     │
    │                                                                 │
    ├── SourceCode ──► build_from_source_code_with_mutations()       │
    │                  Mutator::apply(src) → invoke_clang()           │
    │                                                                 │
    └── ModularTemplate ──► build_modular_template()                 │
             PayloadEncoder.encode() → Assembler.assemble()          │
             → [Mutator::apply()] / strip_mutation_markers()         │
             → invoke_clang_internal()                               │
             → [apply_instrumentation()]                             │
                                                                      │
    COMMON FINAL STEPS:                                               │
      1. Read artifact bytes                                          │
      2. compute_sha256() → artifact_id                              │
      3. Rename to artifacts/{sha256}.exe                            │
      4. Return BuiltArtifact                                        │
```

---

### `template::assembler` — Template Assembly

```rust
struct Assembler {
    template_dir: PathBuf,                    // Path to build/templates/
    module_cache: HashMap<String, String>,     // Cached module file contents
}

struct ModuleSelection {
    carrier: String,          // "alloc_rw_rx" | "change_rw_rx" | "peb_walk"
    decoder: String,          // "xor" | "english"
    antiemulation: String,    // "none" | "sirallocalot" | "timeraw"
    guardrail: String,        // "none" | "env"
    virtualprotect: String,   // "standard" | "undersized"
    decoy: String,            // "none" | "winexec"
}
// Defaults: alloc_rw_rx / xor / none / none / standard / none

struct MutationMarker {
    name: String,              // "timing_jitter", "execution_method", etc.
    params: Vec<String>,       // ["direct", "callback", "fiber", "threadpool"]
    line: usize,               // 1-indexed source line
    column: usize,             // Column position
}
// Parsed from: "// @MUTATE:name(param1|param2|...)" in assembled source
```

| Method / Function | Visibility | Description |
|-------------------|------------|-------------|
| `Assembler::new(template_dir)` | pub | Create assembler with template directory |
| `Assembler::assemble(modules, payload_header)` | pub | Read loader_template.c, replace all @MODULE markers |
| `Assembler::read_module(relative_path)` | self | Read module .c, strip redundant #includes, cache |
| `Assembler::list_modules()` | pub | List available modules per category |
| `Assembler::clear_cache()` | pub | Clear module_cache HashMap |
| `ModuleSelection::new()` | pub | Create with defaults |
| `ModuleSelection::validate(template_dir)` | pub | Check all referenced module .c files exist |
| `extract_mutation_markers(source)` | pub fn | Parse all `// @MUTATE:` comments → Vec<MutationMarker> |
| `strip_mutation_markers(source)` | pub fn | Remove all `// @MUTATE:` comment lines from source |

**Assembly Flow:**
```
loader_template.c
    │
    ├── // @MODULE:definitions  → read_module("header/definitions.h")
    ├── // @MODULE:payload      → payload_header (from PayloadEncoder)
    ├── // @MODULE:decoder      → read_module("decoder/{xor,english}.c")
    ├── // @MODULE:virtualprotect → read_module("virtualprotect/{standard,undersized}.c")
    ├── // @MODULE:antiemulation → read_module("antiemulation/{none,sirallocalot,timeraw}.c")
    ├── // @MODULE:guardrail    → read_module("guardrails/{none,env}.c")
    ├── // @MODULE:decoy        → read_module("decoy/{none,winexec}.c")
    └── // @MODULE:carrier      → read_module("carrier/{alloc_rw_rx,change_rw_rx,peb_walk}.c")
                                          │
                                          ▼
                                   assembled.c (single merged source)
                                          │
                                          ▼
                        extract_mutation_markers() → Vec<MutationMarker>
                        strip_mutation_markers()   → clean source
```

**read_module() Processing:**
1. Check cache → return if hit
2. Read file from templates/modules/{path}
3. Strip `#include "definitions.h"` (already in template)
4. Strip `#include <stdio.h>` (already in template)
5. Cache result, return

---

### `template::payload` — Payload Encoding

```rust
struct PayloadEncoder {
    xor_key: [u8; 2],            // Default: [0xAA, 0x55]
    dictionary: Vec<String>,      // 256 words (60 common English + 196 synthetic "wN")
}

struct EncodedPayload {
    encoding: EncodingType,
    data: Vec<u8>,                                // Encoded bytes
    metadata: HashMap<String, String>,            // "key", "dictionary_size", etc.
}

enum EncodingType {
    Xor,       // Rolling 2-byte XOR (low overhead)
    English,   // Dictionary word mapping (low entropy)
}
```

| Method / Function | Visibility | Description |
|-------------------|------------|-------------|
| `PayloadEncoder::new()` | pub | Create with default key [0xAA, 0x55] and dictionary |
| `PayloadEncoder::with_xor_key(key)` | pub | Create with custom XOR key |
| `PayloadEncoder::encode(bytes, encoding)` | pub | Dispatch to encode_xor or encode_english |
| `PayloadEncoder::encode_xor(bytes)` | self | Rolling XOR: `byte[i] ^= key[i % 2]` |
| `PayloadEncoder::encode_english(bytes)` | self | Map each byte → dictionary[byte] word |
| `PayloadEncoder::generate_c_header(encoded)` | pub | Dispatch to xor or english header generator |
| `PayloadEncoder::generate_xor_header(encoded)` | self | Emit PAYLOAD_LEN, XOR_KEY[], supermega_payload[] |
| `PayloadEncoder::generate_english_header(encoded)` | self | Emit DICTIONARY[], supermega_payload_str[], dummy array |
| `PayloadEncoder::generate_dictionary()` | self | Build 256-word list (60 English + 196 synthetic) |
| `EncodingType::decoder_module()` | pub | `Xor → "xor"`, `English → "english"` (maps to decoder module) |
| `EncodingType::from_str(s)` | pub | Parse "xor" or "english" string |
| `format_c_byte_array(bytes)` | pub(self) fn | Format bytes as `{ 0xAA, 0xBB, ... }` C array |
| `generate_test_payload(size)` | pub fn | `vec![0x90; size]` with INT3 (0xCC) at end |

**Encoding ↔ Decoder Pairing:**
```
PayloadEncoder.encode()                      Decoder Module (C)
         │                                          │
         ├── EncodingType::Xor ──────────────────── decoder/xor.c
         │   XOR_KEY[], supermega_payload[]          byte[i] ^= key[i%2]
         │                                          │
         └── EncodingType::English ──────────────── decoder/english.c
             DICTIONARY[], supermega_payload_str[]   strtok → strcmp → byte
```

---

### `transform::ast_mutator` — AST-Level Transforms

```rust
struct AstMutator;
// No fields (stateless)
```

| Method | Visibility | Description |
|--------|------------|-------------|
| `new()` | pub | Create (no-op) |
| `inject_line_tracing(source)` | pub | Legacy macro-based line tracing injection |
| `inject_line_tracing_with_delay(source, delay_us)` | pub | Line tracing with configurable delay |
| `mutate(source, mutations)` | pub async | **NOT IMPLEMENTED** — returns error |

**inject_line_tracing() Detail:**
- Emits `#define __TRACE_LINE()` macro at file top
  - Uses `snprintf` → Base64 encode → write to `\\.\pipe\rededr_trace`
  - Falls back to `stderr` if pipe unavailable
  - Optional delay loop (configurable microseconds)
- Inserts `__TRACE_LINE();` after each `;`-terminated statement
- Tracks function boundaries via `{`/`}` matching
- Skips: preprocessor directives, comments, empty lines

**Status:** Legacy — superseded by `line_tracer.rs` (tree-sitter based) for production use. Still used by `LowLevelBuilder.apply_ast_mutations()`.

---

### `transform::ir_mutator` — IR-Level Transforms

```rust
struct IrMutator;
// No fields (stateless)
```

| Method | Visibility | Description |
|--------|------------|-------------|
| `new()` | pub | Create (no-op) |
| `mutate(ir, mutations)` | pub async | **NOT IMPLEMENTED** — returns error |

**Status:** Stub only. Planned: opaque predicates, CFG flattening, API indirection, bogus control flow. Actual LLVM IR mutations are handled by `Mutator::apply()` via text-based transforms.

---

### `instrument::instrumenter` — IR-Level Instrumentation

```rust
struct Instrumenter {
    bb_counter: u32,     // Counter for BB coverage callbacks
    line_counter: u32,   // Counter for string constants
}
```

| Method | Visibility | Description |
|--------|------------|-------------|
| `new()` | pub | Create with counters at 0 |
| `instrument(ir, trace_mode, output)` | pub async | Main entry: apply BB + API instrumentation |
| `inject_bb_coverage_sancov(ir, output)` | self async | `opt -passes=sancov-module` on LLVM IR |
| `inject_api_tracing(ir_content)` | self | Text scan for API calls → insert `__checkpoint()` |
| `inject_string_constants(ir_content, constants)` | self | Add `@.str.checkpoint.N` string globals |
| `add_runtime_declarations(ir_content, needs_bb, needs_api)` | self | Add `declare` for runtime functions |

**instrument() Logic:**
```
TraceMode → needs_bb?  (BB | ApiPlusBB | All)
          → needs_api? (Api | ApiPlusBB | Lines | LinesAroundBB | All)

if needs_bb:
    opt -passes=sancov-module
        -sanitizer-coverage-level=3
        -sanitizer-coverage-trace-pc
        input.ll → output.bb.ll

if needs_api:
    Scan IR for: VirtualAlloc, VirtualProtect, WriteProcessMemory,
                 CreateRemoteThread, LoadLibrary, GetProcAddress,
                 CreateProcess, OpenProcess
    Insert: call void @__checkpoint(i8* @.str.checkpoint.N)
    Add:    @.str.checkpoint.N = "api:APIName\00"

add_runtime_declarations:
    declare void @__coverage_init()
    declare void @__coverage_flush()
    declare void @__checkpoint(i8*)
    declare void @__trace_line(i32, i8*, i32, i8*)
    declare void @__trace_init(i8*)
    declare void @__trace_flush()
```

---

### `instrument::line_tracer` — AST-Level Line Tracing

**Enums**

```rust
enum SourceLanguage {
    C,    // .c, .h, unknown
    Cpp,  // .cpp, .cc, .cxx, .hpp, .h++
}

enum TraceFormat {
    Base64,  // Lepori thesis format: "YjY0" + base64("line:file:N:")
    Binary,  // DEFAULT: direct call with file, line, __func__
}
```

**Free Functions**

| Function | Visibility | Description |
|----------|------------|-------------|
| `inject_line_traces(source, language)` | pub | Inject with defaults (file="source", format=Binary) |
| `inject_line_traces_with_path(source, language, path)` | pub | Inject with custom file path |
| `inject_line_traces_with_opts(source, language, path, format)` | pub | Full-option injection entry point |
| `SourceLanguage::from_path(path)` | pub | Detect language from file extension |
| `collect_injection_points(root, source, lang, path, fmt)` | self | Walk AST → Vec<(offset, trace_stmt)> |
| `visit_node(node, source, lang, path, fmt, injections)` | self | Recursive tree-sitter node visitor |
| `is_traceable_statement(node)` | self | Check if node kind is instrumentable |
| `calculate_indentation(source, offset)` | self | Extract whitespace indent at offset |
| `generate_trace_statement(line, indent, lang, path, fmt)` | self | Build trace call + delay loop string |

**Traceable Statement Kinds:**
```
expression_statement | declaration | if_statement | while_statement
| for_statement | for_range_loop | return_statement | break_statement
| continue_statement | switch_statement | case_statement | labeled_statement
| goto_statement | try_statement | throw_statement
```

**Generated Code (Binary format, DEFAULT):**
```c
__trace_line_binary("source", 42, __func__);
volatile long __inst_wait42 = 1; for (; __inst_wait42 < 10000; __inst_wait42 += 2) {}
// original statement follows
```

**Generated Code (Base64 format):**
```c
__trace_line_b64("YjY0bGluZTpzb3VyY2U6NDI6");
volatile long __inst_wait42 = 1; for (; __inst_wait42 < 10000; __inst_wait42 += 2) {}
// original statement follows
```

---

### `mutator` — Mutation Application Engine

```rust
struct MutationSpec {
    id: String,                          // "llvm.nop_insert" | "ast.string_xor"
    params: HashMap<String, String>,     // e.g. {"density": "0.3"}
}

struct Mutator;
// No fields (stateless)
```

| Method | Visibility | Description |
|--------|------------|-------------|
| `MutationSpec::parse()` | pub | Split `id` on `.` → `(&category, &name)` |
| `Mutator::apply(source, mutations)` | pub | Dispatch mutations by category prefix |
| `Mutator::insert_llvm_nops(ir, density)` | self | Insert NOP asm after BB labels in LLVM IR |
| `Mutator::xor_encode_strings(source, key)` | self | XOR-encode string literals in C source |

**Supported Mutations:**

```
┌──────────────────┬───────────────────────────────────────────────────────┐
│ llvm.nop_insert  │ Insert inline asm NOP after BB labels in LLVM IR     │
│                  │ Params: density (f32, default 0.3)                   │
│                  │ PRNG: deterministic LCG (seed 1234)                  │
│                  │ Output: `call void asm sideeffect "nop", ""()`       │
├──────────────────┼───────────────────────────────────────────────────────┤
│ ast.string_xor   │ XOR-encode string literals in C source               │
│                  │ Params: xor_key (u8, default 0xAA)                   │
│                  │ Uses GNU C statement expressions: ({...})            │
│                  │ Skips: #pragma, #include, #error contexts            │
│                  │ Output: runtime-decoded string via char[] + loop     │
└──────────────────┴───────────────────────────────────────────────────────┘

Unknown mutation IDs: silently skipped (not an error)
```

---

## Struct Relationships

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              OWNERSHIP GRAPH                                    │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  CALLER (controller JobWorker, CLI, tests) creates:                            │
│                                                                                 │
│    ArtifactBuilder ─────────────────────────────────────────────────────────┐  │
│    │ config: BuilderConfig (owned)                                          │  │
│    │                                                                        │  │
│    ├── uses ► Assembler (created per build_modular_template call)           │  │
│    │          │ template_dir: PathBuf (from config.modular_template_dir)    │  │
│    │          │ module_cache: HashMap (owned, per-call lifetime)            │  │
│    │          │                                                             │  │
│    │          ├── reads ► ModuleSelection (from BuildInput::ModularTemplate)│  │
│    │          │          carrier, decoder, antiemulation, guardrail,        │  │
│    │          │          virtualprotect, decoy → module file paths          │  │
│    │          │                                                             │  │
│    │          └── produces ► assembled source (String)                      │  │
│    │                         containing @MUTATE markers                     │  │
│    │                                                                        │  │
│    ├── uses ► PayloadEncoder (created per build_modular_template call)      │  │
│    │          │ xor_key: [u8; 2] (default [0xAA, 0x55])                    │  │
│    │          │ dictionary: Vec<String> (256 words)                         │  │
│    │          │                                                             │  │
│    │          ├── encode(payload, EncodingType) → EncodedPayload            │  │
│    │          └── generate_c_header(encoded) → String (payload.h code)     │  │
│    │                                                                        │  │
│    ├── uses ► Mutator (stateless, created per build call)                   │  │
│    │          │                                                             │  │
│    │          ├── apply(source, Vec<MutationSpec>) → mutated source         │  │
│    │          ├── insert_llvm_nops() for "llvm.*" mutations                │  │
│    │          └── xor_encode_strings() for "ast.*" mutations              │  │
│    │                                                                        │  │
│    ├── uses ► Instrumenter (created per apply_instrumentation call)         │  │
│    │          │ bb_counter: u32                                             │  │
│    │          │ line_counter: u32                                           │  │
│    │          │                                                             │  │
│    │          └── instrument(ir, trace_mode, output)                        │  │
│    │              ├── inject_bb_coverage_sancov() → opt pass               │  │
│    │              ├── inject_api_tracing() → text-based IR edit            │  │
│    │              └── add_runtime_declarations() → IR declarations         │  │
│    │                                                                        │  │
│    ├── uses ► inject_line_traces (free fn, during apply_instrumentation)   │  │
│    │          ├── tree-sitter C++ parser                                   │  │
│    │          └── injects __trace_line_binary() calls into source          │  │
│    │                                                                        │  │
│    ├── uses ► extract_mutation_markers / strip_mutation_markers (free fns) │  │
│    │                                                                        │  │
│    └── produces ► BuiltArtifact                                            │  │
│                   │ artifact_id (SHA256)                                    │  │
│                   │ output_path (artifacts/{sha256}.exe)                    │  │
│                   │ mutations_applied                                       │  │
│                   └── consumed by: controller dispatch, test harness       │  │
│                                                                             │  │
│  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ │  │
│                                                                             │  │
│  LowLevelBuilder (alternative, LLVM IR-focused pipeline)                   │  │
│    │ config: BuildConfig (owned)                                            │  │
│    │                                                                        │  │
│    ├── uses ► AstMutator.inject_line_tracing() (legacy)                    │  │
│    ├── uses ► Instrumenter.instrument() (same as above)                    │  │
│    ├── invokes: clang, llc, ld.lld (CLI tools)                            │  │
│    └── produces ► ArtifactMetadata                                         │  │
│                                                                             │  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Data Flow Types

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        TYPE FLOW THROUGH BUILD SYSTEM                          │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  Controller / CLI                                                              │
│       │                                                                         │
│       │  BuildInput::ModularTemplate {                                         │
│       │    modules: ModuleSelection,                                           │
│       │    payload: Vec<u8>,                                                   │
│       │    encoding: EncodingType,                                             │
│       │    mutations: Vec<MutationSpec>,                                       │
│       │    trace_mode: String,                                                 │
│       │  }                                                                     │
│       │                                                                         │
│       ▼                                                                         │
│  ArtifactBuilder.build(input)                                                  │
│       │                                                                         │
│       ├──────────────────────────────────── payload: Vec<u8>                   │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            PayloadEncoder.encode(bytes, EncodingType)  │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            EncodedPayload { data, encoding, metadata } │
│       │                                         │                              │
│       │                            generate_c_header()                          │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            String (payload.h C code)                   │
│       │                                         │                              │
│       ├── ModuleSelection ──────────────────────┤                              │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            Assembler.assemble(modules, payload_header) │
│       │                                         │                              │
│       │                            Reads: loader_template.c                    │
│       │                            Reads: modules/**/*.c via read_module()     │
│       │                            Replaces: @MODULE markers                   │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            String (assembled.c — single merged source) │
│       │                                         │                              │
│       ├── Vec<MutationSpec> ────────────────────┤                              │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            extract_mutation_markers() → Vec<Marker>    │
│       │                            Mutator::apply() or strip_mutation_markers()│
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            String (final_source.c — clean C code)      │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            invoke_clang_internal()                      │
│       │                              clang --target=x86_64-pc-windows-msvc     │
│       │                              + xwin headers/libs                       │
│       │                              + minimal_runtime.o                       │
│       │                              + [instrumentation_runtime.o]             │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            PathBuf (temp .exe)                          │
│       │                                         │                              │
│       ├── trace_mode ───────────────────────────┤                              │
│       │   (if != "off")                         │                              │
│       │                                         ▼                              │
│       │                            apply_instrumentation()                      │
│       │                              inject_line_traces() (tree-sitter)        │
│       │                              compile_source_to_ir() (clang -S)         │
│       │                              Instrumenter.instrument() (opt/text)      │
│       │                              compile + link → instrumented .exe        │
│       │                                         │                              │
│       │                                         ▼                              │
│       │                            compute_sha256() → artifact_id              │
│       │                            rename to artifacts/{sha256}.exe            │
│       │                                         │                              │
│       │                                         ▼                              │
│       └──────────────────────────── BuiltArtifact                              │
│                                      │ artifact_id: String (SHA256)            │
│                                      │ output_path: PathBuf                    │
│                                      │ source_path: PathBuf                    │
│                                      │ size_bytes: u64                         │
│                                      │ mutations_applied: Vec<String>          │
│                                      │                                         │
│                                      ▼                                         │
│                                 Controller / Test Harness                      │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Dual Builder Comparison

```
┌──────────────────────────────┬──────────────────────────────────────────┐
│   LowLevelBuilder (lib.rs)  │   ArtifactBuilder (builder.rs)          │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Config: BuildConfig          │ Config: BuilderConfig                   │
│   (target, optimization,     │   (templates_dir, output_dir, xwin_dir, │
│    trace_mode, seed, ...)    │    runtime_src, modular_template_dir)   │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Input: &Path + &[Mutation]   │ Input: BuildInput (enum, 4 variants)    │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Mutation type: Mutation      │ Mutation type: MutationSpec             │
│   {id, params}               │   {id, params} + parse()               │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Pipeline: explicit LLVM IR   │ Pipeline: Clang direct or LLVM IR      │
│   source → clang -emit-llvm  │   source → clang → exe                 │
│   → opt → llc → ld.lld      │   or source → clang -S → mutate → exe  │
│                              │   or assemble → clang → exe             │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Template support: NO         │ Template support: YES (ModularTemplate) │
│                              │   Assembler + PayloadEncoder            │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Instrumentation: inline      │ Instrumentation: post-build             │
│   (during IR pipeline)       │   apply_instrumentation() after compile │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Linker: ld.lld -flavor link  │ Linker: clang -fuse-ld=lld             │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Output: ArtifactMetadata     │ Output: BuiltArtifact                   │
├──────────────────────────────┼──────────────────────────────────────────┤
│ Use when: Direct LLVM IR     │ Use when: ALL OTHER CASES (PREFERRED)   │
│   pipeline control needed    │   especially ModularTemplate builds     │
└──────────────────────────────┴──────────────────────────────────────────┘
```

---

## External Tool Dependencies

| Tool | Invoked By | Usage |
|------|-----------|-------|
| `clang` | LowLevelBuilder, ArtifactBuilder | C → LLVM IR, C → .obj, C → .exe |
| `llc` | LowLevelBuilder | LLVM IR → .obj |
| `ld.lld` | LowLevelBuilder | .obj → PE |
| `lld-link` | ArtifactBuilder | .obj → PE (instrumented builds) |
| `opt` | Instrumenter | SanitizerCoverage pass on LLVM IR |
| `llvm-nm` | ArtifactBuilder | Verify runtime symbols in .obj |

---

## Stateless vs Stateful Structs

| Struct | State | Lifetime |
|--------|-------|----------|
| `ArtifactBuilder` | `config: BuilderConfig` | Per-session (reused across builds) |
| `LowLevelBuilder` | `config: BuildConfig` | Per-session |
| `Assembler` | `template_dir` + `module_cache` | Per-call (created fresh) |
| `PayloadEncoder` | `xor_key` + `dictionary` | Per-call |
| `Instrumenter` | `bb_counter` + `line_counter` | Per-call (counters grow) |
| `Mutator` | (none) | Stateless |
| `AstMutator` | (none) | Stateless |
| `IrMutator` | (none) | Stateless |
| `MutationSpec` | `id` + `params` | Value type (passed in) |
| `MutationMarker` | `name` + `params` + position | Value type (parsed from source) |
| `ModuleSelection` | 6 × String | Value type (passed in) |
| `EncodedPayload` | `data` + `metadata` | Value type (returned) |
| `BuiltArtifact` | Full metadata | Value type (returned) |
| `ArtifactMetadata` | Full metadata | Value type (returned) |