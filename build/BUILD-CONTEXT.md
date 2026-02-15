# Build Crate — Working Context

## 1) Purpose

The `build` crate cross-compiles **C source code from WSL2 into Windows x64 PE executables** using Clang/LLVM + xwin SDK sysroot. It is the artifact factory for AutoMutate++: it takes a modular loader template, encodes a payload, applies mutations, instruments the result, and produces a SHA256-named `.exe` with full provenance metadata.

---

## 2) Directory Layout

```
build/
├── Cargo.toml                          # Crate manifest (lib crate)
├── src/
│   ├── lib.rs                          # Crate root: re-exports, TraceMode enum
│   ├── builder.rs                      # ArtifactBuilder (main API, ~1500 LOC)
│   ├── compiler/mod.rs                 # Placeholder (logic lives in builder.rs)
│   ├── mutator/mod.rs                  # MutationSpec + Mutator engine
│   ├── template/
│   │   ├── mod.rs                      # Re-exports assembler + payload
│   │   ├── assembler.rs                # @MODULE marker replacement
│   │   └── payload.rs                  # PayloadEncoder (XOR/English)
│   ├── transform/
│   │   ├── mod.rs                      # Re-exports AstMutator, IrMutator
│   │   ├── ast_mutator.rs             # STUB — empty struct, tree-sitter TODO
│   │   └── ir_mutator.rs             # STUB — empty struct, LLVM TODO
│   └── instrument/
│       ├── mod.rs                      # Re-exports Instrumenter + line_tracer
│       ├── instrumenter.rs            # LLVM IR instrumentation (BB coverage via SanitizerCoverage, API tracing)
│       └── line_tracer.rs             # AST-level line tracing via tree-sitter
├── runtime/
│   ├── instrumentation_runtime.c      # Linked into instrumented artifacts (BB coverage, trace pipe, checkpoints)
│   ├── instrumentation.h             # ARTIFACT_CHECKPOINT/SUCCESS/FAILURE macros
│   ├── minimal_runtime.c             # Always linked (__runtime_exit via direct syscall)
│   └── minimal_runtime.h             # Header for __runtime_exit
├── templates/
│   ├── loader_template.c             # Main template with @MODULE markers
│   ├── payload.h                     # Static fallback payload (XOR-encoded NOPs)
│   ├── encoder.py                    # Python encoder (standalone, XOR/English)
│   └── modules/
│       ├── header/definitions.h      # Interface contracts (carrier, decode_payload, antiemulation, decoy, guardrail, MyVirtualProtect)
│       ├── carrier/
│       │   ├── alloc_rw_rx.c         # VirtualAlloc(RW) → decode → VirtualProtect(RX) → exec
│       │   ├── change_rw_rx.c        # Allocate RW → decode → change to RX → exec
│       │   └── peb_walk.c            # Import-free via PEB walking (no IAT)
│       ├── decoder/
│       │   ├── xor.c                 # Rolling 2-byte XOR
│       │   └── english.c             # Dictionary word→byte mapping
│       ├── antiemulation/
│       │   ├── none.c                # No-op
│       │   ├── sirallocalot.c        # Mass VirtualAlloc stress
│       │   └── timeraw.c             # KUSER_SHARED_DATA busy-wait (bypasses hooked time APIs)
│       ├── guardrails/
│       │   ├── none.c                # Always returns 0 (safe)
│       │   └── env.c                 # Environment check
│       ├── virtualprotect/
│       │   ├── standard.c            # Normal VirtualProtect wrapper
│       │   └── undersized.c          # Undersized region trick
│       └── decoy/
│           ├── none.c                # No-op
│           └── winexec.c             # Benign WinExec activity
└── BUILD-CONTEXT.md                   # THIS FILE
```

---

## 3) Public API (lib.rs re-exports)

### Entry Points

| Type | Path | Description |
|------|------|-------------|
| `ArtifactBuilder` | `builder.rs` | Main builder — call `.build(input)` |
| `BuilderConfig` | `builder.rs` | Paths: templates_dir, output_dir, xwin_dir, runtime sources |
| `BuildInput` | `builder.rs` | Enum: `SourceFile{..}` or `ModularTemplate{..}` |
| `BuiltArtifact` | `builder.rs` | Output metadata: artifact_id, sha256, size, paths, mutations_applied |

### Supporting Types

| Type | Path | Description |
|------|------|-------------|
| `TraceMode` | `lib.rs` | Enum: Off, Api, BB, ApiPlusBB, Lines, LinesAroundBB(u32), All |
| `MutationSpec` | `mutator/mod.rs` | `{id: String, params: HashMap<String,String>}` |
| `Mutator` | `mutator/mod.rs` | Stateless transformer: `apply(input, mutations) -> (output, applied_ids)` |
| `ModuleSelection` | `template/assembler.rs` | 6 fields: carrier, decoder, antiemulation, guardrail, virtualprotect, decoy |
| `Assembler` | `template/assembler.rs` | Replaces `// @MODULE:xxx` markers with module file contents |
| `PayloadEncoder` | `template/payload.rs` | Encodes raw bytes → C header (XOR or English) |
| `EncodingType` | `template/payload.rs` | Enum: Xor, English |
| `MutationMarker` | `template/assembler.rs` | Parsed `// @MUTATE:name(params)` from source |
| `Instrumenter` | `instrument/instrumenter.rs` | LLVM IR instrumentation (BB via SanitizerCoverage + API checkpoints) |
| `inject_line_traces` | `instrument/line_tracer.rs` | AST-level tree-sitter line tracing injection |
| `AstMutator` | `transform/ast_mutator.rs` | **STUB** — empty struct |
| `IrMutator` | `transform/ir_mutator.rs` | **STUB** — empty struct |

---

## 4) Build Pipeline (Two Paths)

### Path A: ModularTemplate (preferred)

```
payload bytes
    │
    ▼
PayloadEncoder.encode(payload, encoding)
    │ → payload.h C code (XOR or English header)
    ▼
Assembler.assemble(modules, payload_header)
    │ → replaces // @MODULE:xxx markers
    │ → assembled.c (single file)
    ▼
extract_mutation_markers() → find @MUTATE locations
strip_mutation_markers()   → or apply AST mutations
    │
    ▼  final_source.c
    │
    ├── trace_mode == "off" ──────────────────────────────────────────┐
    │                                                                  │
    ├── trace_mode != "off" (instrumentation path)                     │
    │       │                                                          │
    │       ▼                                                          │
    │   inject_line_traces() (if Lines/All)                            │
    │       │ tree-sitter injects __trace_line_binary() calls          │
    │       ▼                                                          │
    │   clang -S -emit-llvm → LLVM IR (.ll)                           │
    │       │                                                          │
    │       ▼                                                          │
    │   Instrumenter.instrument()                                      │
    │       │ opt -passes=sancov-module (BB coverage)                  │
    │       │ inject API checkpoints (__checkpoint)                    │
    │       │ add runtime declarations                                 │
    │       ▼                                                          │
    │   clang IR → .obj                                                │
    │       │                                                          │
    │       ▼                                                          │
    │   lld-link .obj + instrumentation_runtime.o + minimal_runtime.o  │
    │       │                                                          │
    │       ▼                                                          │
    │   instrumented artifact.exe                                      │
    │                                                                  │
    └──────────────────────────────────────────────────────────────────┤
                                                                       │
invoke_clang_internal(source + minimal_runtime.o [+ instrumentation_runtime.o])
    │
    ▼
compute_sha256(exe) → artifact_id
rename to artifacts/<sha256>.exe
    │
    ▼
BuiltArtifact { artifact_id, paths, size, sha256, mutations_applied, ... }
```

### Path B: SourceFile (legacy / test)

Same as Path A but starts from a pre-existing `.c` file instead of template assembly.

---

## 5) Module Interface Contracts (definitions.h)

Every module file must implement exactly one of these functions:

```c
int  carrier(void);                               // Returns 0=success
void decode_payload(char *dest, int len);          // In-place decode
void antiemulation(void);                          // Stall/burn/check
void decoy(void);                                  // Benign activity
int  guardrail(void);                              // Returns 0=safe, nonzero=bail
BOOL MyVirtualProtect(LPVOID, SIZE_T, DWORD, PDWORD); // VirtualProtect wrapper
```

Global symbols from payload.h:
```c
extern unsigned char supermega_payload[];   // Encoded payload data
extern unsigned char XOR_KEY[2];           // XOR key (when using XOR encoding)
extern const char* DICTIONARY[];           // Word table (when using English encoding)
extern char supermega_payload_str[];       // Word string (when using English encoding)
```

Permission constants:
```c
#define p_RW  0x04   // PAGE_READWRITE
#define p_RX  0x20   // PAGE_EXECUTE_READ
#define p_RWX 0x40   // PAGE_EXECUTE_READWRITE
```

---

## 6) Available Modules (Combinatorial Space)

| Category | Options | Default |
|----------|---------|---------|
| carrier | `alloc_rw_rx`, `change_rw_rx`, `peb_walk` | `alloc_rw_rx` |
| decoder | `xor`, `english` | `xor` |
| antiemulation | `none`, `sirallocalot`, `timeraw` | `none` |
| guardrail | `none`, `env` | `none` |
| virtualprotect | `standard`, `undersized` | `standard` |
| decoy | `none`, `winexec` | `none` |

**Combinatorial space:** 3 × 2 × 3 × 2 × 2 × 2 = **144 distinct module combinations**

---

## 7) Mutation System

### Implemented Mutations (mutator/mod.rs)

| ID | Layer | What it does |
|----|-------|-------------|
| `llvm.nop_insert` | IR | Inserts `asm sideeffect "nop"` after BB labels (density param) |
| `ast.string_xor` | AST | XOR-encodes string literals with runtime decode (xor_key param) |

### @MUTATE Markers in Templates (NOT YET IMPLEMENTED)

These are annotations in module `.c` files that identify mutation points. Currently just **stripped** before compilation. Future mutations would read these markers and transform the surrounding code.

Key markers found across modules:
- `timing_jitter` — add random delays
- `benign_preamble` — add benign code before sensitive operations
- `opaque_predicate` — add always-true/false branch obfuscation
- `dead_code_insertion` — add unreachable code
- `api_wrapper_injection(ApiName)` — wrap API call through function pointer
- `getprocaddress_indirection(ApiName)` — resolve API at runtime
- `execution_method(direct|callback|fiber|threadpool)` — change payload execution method
- `staged_rx` — intermediate RW→R→RX instead of RW→RX
- `key_upgrade(rc4|aes|rolling_xor|multi_stage)` — upgrade decoder algorithm
- `control_flow_flattening` — dispatcher-based CFG
- `loop_restructuring(for→while|unroll|switch_dispatch)` — loop transformation
- `literal_encoding` — encode constants
- `string_splitting` — split string construction
- `inline_assembly` — replace C with inline asm
- `api_sequence_obfuscation` — reorder API call sequence
- `api_swap(Original→Alternative)` — substitute equivalent API
- `logic_mutation(memory_write→cpu_math)` — change operation semantics

### Stub Modules (empty, TODO)

- `AstMutator` in `transform/ast_mutator.rs` — tree-sitter AST transformations
- `IrMutator` in `transform/ir_mutator.rs` — LLVM IR transformations

---

## 8) Instrumentation Modes (TraceMode)

| Mode | Line Trace | BB Coverage | API Checkpoints | Use Case |
|------|-----------|------------|----------------|----------|
| `Off` | - | - | - | Baseline run (no overhead) |
| `Api` | - | - | yes | API-only tracking |
| `BB` | - | SanitizerCov | - | Coverage only |
| `ApiPlusBB` | - | SanitizerCov | yes | **Default for mutation loop** |
| `Lines` | tree-sitter | - | - | Diagnostic: execution path |
| `LinesAroundBB(id)` | targeted | - | - | Narrowing mode |
| `All` | tree-sitter | SanitizerCov | yes | Debug: everything |

### Runtime Objects (Always Linked)

| Object | Source | Purpose | When linked |
|--------|--------|---------|-------------|
| `minimal_runtime.o` | `runtime/minimal_runtime.c` | `__runtime_exit()` via direct syscall | **ALWAYS** |
| `instrumentation_runtime.o` | `runtime/instrumentation_runtime.c` | Coverage, trace, checkpoints | When trace_mode != off |

### I/O Channels from Artifact

| Channel | Pipe/File | Protocol | Flushing |
|---------|-----------|----------|----------|
| Line trace | `\\.\pipe\rededr_trace` | Binary (ISTR magic 0x49535452 header + "file:line:func" payload) | Aggressive (every event) |
| Checkpoints | `\\.\pipe\rededr_checkpoints` | JSON lines `{"ts_us":N,"checkpoint":"name"}` | Immediate |
| BB coverage | `coverage.bin` + `coverage_bbs.txt` | AFL-style 64KB bitmap + metadata | At exit + every 50 BBs |
| Artifact status | checkpoint pipe | JSON `{"type":"artifact_checkpoint"/"success"/"failure",...}` | Immediate |

### Binary Trace Protocol (InstRecordHeader)

```c
struct InstRecordHeader {  // 30 bytes, packed
    uint32_t magic;       // 0x49535452 ('ISTR')
    uint16_t version;     // 1
    uint16_t event_type;  // 1=line, 2=func_enter, 3=syscall, 4=bb
    uint32_t thread_id;   // GetCurrentThreadId()
    uint64_t seq_no;      // Monotonic (InterlockedIncrement64)
    uint64_t ts_us;       // Microseconds since process start (QPC)
    uint32_t payload_len; // Payload bytes following header
};
// Followed by: "file:line:func" as UTF-8
```

---

## 9) Integration with Controller

### gRPC RPCs that call build crate

| RPC | Proto | Handler | Build path used |
|-----|-------|---------|----------------|
| `BuildArtifact` | `controller.proto` | `controller/scheduler/src/api/artifact.rs::build_artifact()` | `BuildInput::SourceFile` |
| `DeployArtifact` | `controller.proto` | `controller/scheduler/src/api/artifact.rs::deploy_artifact()` | Reads from `artifacts/<sha256>.exe` |
| JobWorker round loop | N/A (internal) | `controller/scheduler/src/dispatch/job_worker.rs` | `BuildInput::ModularTemplate` |

### Proto → Rust Type Mapping

```
proto Mutation {id, params}  →  build::mutator::MutationSpec {id, params}
proto BuildRequest           →  BuildInput::SourceFile or ModularTemplate
proto BuildResponse          ←  BuiltArtifact fields
```

### Key Config Defaults (BuilderConfig::default)

```rust
templates_dir:       "corpus/templates"
output_dir:          "artifacts"
xwin_dir:            "/root/.xwin"
runtime_src:         "build/runtime/instrumentation_runtime.c"
minimal_runtime_src: "build/runtime/minimal_runtime.c"
modular_template_dir: "build/templates"
```

---

## 10) Compiler Flags

### Direct compilation (invoke_clang_internal)
```
clang -target x86_64-pc-windows-msvc
      -isystem <xwin>/crt/include -isystem <xwin>/sdk/include/{ucrt,shared,um,winrt}
      -L <xwin>/crt/lib/x86_64 -L <xwin>/sdk/lib/{ucrt,um}/x86_64
      -fuse-ld=lld -O2
      -Wl,/subsystem:console -Wl,/DEBUG:NONE -Wl,/Brepro
      -Wl,/INCREMENTAL:NO -Wl,/OPT:REF -Wl,/OPT:ICF
      -Wl,-defaultlib:libcmt -Wl,-defaultlib:kernel32
      [-DENABLE_INSTRUMENTATION]
      -I <runtime_dir>
      -o output.exe source.c minimal_runtime.o [instrumentation_runtime.o]
```

### IR path (instrumented)
```
Source → clang -S -emit-llvm -O0 → .ll
.ll → opt -passes=sancov-module -sanitizer-coverage-level=3 -sanitizer-coverage-trace-pc → .bb.ll
.bb.ll → API checkpoint injection → runtime declarations → final.ll
final.ll → clang -c → .obj
.obj → lld-link /subsystem:console /machine:x64 + runtime .o files + win libs → .exe
```

---

## 11) What Is Implemented vs TODO

### Fully Implemented
- [x] ModularTemplate assembly (Assembler + @MODULE markers)
- [x] Payload encoding (XOR 2-byte key, English dictionary)
- [x] PayloadEncoder Rust implementation (mirrors encoder.py)
- [x] Cross-compilation pipeline (clang + xwin + lld-link)
- [x] Two build paths (SourceFile and ModularTemplate)
- [x] Instrumentation pipeline (AST line tracing + IR BB coverage + API checkpoints)
- [x] Runtime objects (minimal_runtime.c + instrumentation_runtime.c)
- [x] Binary trace protocol (InstRecordHeader)
- [x] `instrumentation.h` macros (ARTIFACT_CHECKPOINT/SUCCESS/FAILURE)
- [x] SHA256 artifact naming
- [x] Concurrent build support (UUID-based temp filenames)
- [x] `llvm.nop_insert` mutation
- [x] `ast.string_xor` mutation
- [x] @MUTATE marker extraction and stripping
- [x] Module validation (ModuleSelection.validate)
- [x] gRPC integration (BuildArtifact, DeployArtifact RPCs)

### Partially Implemented
- [ ] `compiler/mod.rs` — placeholder, all logic in builder.rs (refactor candidate)

### Not Implemented (Stubs / TODOs)
- [ ] `AstMutator` (transform/ast_mutator.rs) — tree-sitter AST transformations
- [ ] `IrMutator` (transform/ir_mutator.rs) — LLVM IR transformations
- [ ] All @MUTATE marker mutations (timing_jitter, opaque_predicate, control_flow_flattening, etc.)
- [ ] Mutation catalogue / registry (described in CLAUDE.md but not built)
- [ ] Token extraction from telemetry
- [ ] Selector (returns empty vec![])
- [ ] Triage engine (lift × confidence scoring)
- [ ] Jaccard similarity between run coverages
- [ ] Binary-level mutations (PE structure changes)
- [ ] Behavioral mutations (callback abuse, fiber execution, etc.)

---

## 12) Dependencies (Cargo.toml)

```toml
serde, serde_json       # Serialization
tokio                   # Async runtime (process, fs)
anyhow, thiserror       # Error handling
tracing                 # Logging
sha2, hex               # SHA256 hashing
chrono                  # Timestamps
tempfile                # Temp file management
uuid                    # Unique build IDs
tree-sitter             # C/C++ parsing for line tracer
tree-sitter-c           # C grammar
tree-sitter-cpp         # C++ grammar
dirs                    # Home directory resolution
base64                  # Base64 encoding for trace format
```

---

## 13) Quick Reference: How to Build an Artifact

### From Rust code (ModularTemplate path)
```rust
let config = BuilderConfig::default();
let builder = ArtifactBuilder::new(config)?;

let modules = ModuleSelection {
    carrier: "alloc_rw_rx".into(),
    decoder: "xor".into(),
    antiemulation: "timeraw".into(),
    guardrail: "none".into(),
    virtualprotect: "standard".into(),
    decoy: "none".into(),
};

let artifact = builder.build(BuildInput::ModularTemplate {
    modules,
    payload: raw_shellcode_bytes,
    encoding: EncodingType::Xor,
    mutations: vec![],           // or vec![MutationSpec{...}]
    trace_mode: "api+bb".into(), // or "off" for baseline
}).await?;

// artifact.artifact_id   → SHA256
// artifact.output_path   → artifacts/<sha256>.exe
// artifact.size_bytes    → file size
```

### From gRPC (SourceFile path)
```
BuildArtifact RPC → BuildRequest {
    template_name: "eicar_test",
    source_file: "eicar_test.c",
    mutations: [],
    trace_mode: "lines"
}
→ BuildResponse { artifact_id, size_bytes, storage_path, ... }
```
