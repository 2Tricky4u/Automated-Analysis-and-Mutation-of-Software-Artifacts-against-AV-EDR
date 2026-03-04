# Build Crate — Mermaid Diagrams

Visual companion to `BUILD-SYSTEM-ARCHITECTURE.md` (behavior) and `BUILD-STRUCTS-REFERENCE.md` (types). The **Overview** diagram is conceptual; all others detail specific subsystems.

---

## 1. Overview — Conceptual Architecture

High-level view of the build crate showing the major subsystems and how data flows from input to artifact output. Each colored zone maps to a detailed diagram below.

```mermaid
graph TB
    subgraph Input["Controller / JobWorker"]
        BI["BuildInput::ModularTemplate
        modules + payload + encoding
        mutations + trace_mode"]
    end

    subgraph BuildCrate["build/ crate"]
        direction TB

        subgraph Config["Configuration"]
            BC[BuilderConfig]
            XW[XwinPaths]
            MC[MsvcCompat]
        end

        subgraph TemplateEngine["Template Engine"]
            direction LR
            PE[PayloadEncoder]
            ASM[Assembler]
            SC[ShellcodeStub +
            ScCheckpoints]
        end

        subgraph MutationEngine["Mutation Engine"]
            direction LR
            ASTM[AstMutator
            tree-sitter / C source]
            IRM[IrMutator
            text-based / LLVM IR]
            BINM[BinaryMutator
            PE byte manipulation]
        end

        subgraph Compilation["Compiler Invocation"]
            direction LR
            CLANG["clang / clang-cl"]
            LLD["lld-link / link.exe"]
        end

        subgraph Instrumentation["Instrumentation"]
            direction LR
            LT[LineTracer
            AST injection]
            INST[Instrumenter
            IR injection]
            RT["C Runtimes
            minimal + instrumentation
            + sc_checkpoint"]
        end

        AB[ArtifactBuilder
        orchestrates all steps]
    end

    subgraph Output["Build Output"]
        BA["BuiltArtifact
        SHA256-named .exe
        + metadata"]
    end

    BI --> AB
    BC --> AB
    XW --> CLANG
    MC -.->|optional| LLD

    AB --> TemplateEngine
    AB --> MutationEngine
    AB --> Compilation
    AB -->|"if trace≠off"| Instrumentation

    TemplateEngine --> |"assembled .c"| MutationEngine
    ASTM --> |"mutated .c"| Compilation
    Compilation --> |".ll"| IRM
    IRM --> |"mutated .ll"| Compilation
    Compilation --> |".exe"| BINM

    LT --> |"traced .c"| Compilation
    INST --> |"instrumented .ll"| Compilation
    RT --> |".o objects"| LLD

    BINM --> |"mutated PE"| BA
    AB --> BA

    style BuildCrate fill:#1a1a2e,color:#e0e0e0,stroke:#16213e
    style TemplateEngine fill:#0f3460,color:#e0e0e0,stroke:#533483
    style MutationEngine fill:#533483,color:#e0e0e0,stroke:#e94560
    style Compilation fill:#16213e,color:#e0e0e0,stroke:#0f3460
    style Instrumentation fill:#1a1a2e,color:#e0e0e0,stroke:#e94560
    style Input fill:#0f3460,color:#e0e0e0,stroke:#533483
    style Output fill:#0f3460,color:#e0e0e0,stroke:#533483
```

---

## 2. Build Pipeline — The 7-Step Orchestration

Details `ArtifactBuilder::build_modular_template()`. Shows the exact step order, branching at Step 5 (LLVM vs direct path), and the optional instrumentation re-build.

```mermaid
flowchart TD
    START(["BuildInput::ModularTemplate"])

    S0["Step 0: Decoder Sync
    auto-match modules.decoder
    to encoding type"]

    S1["Step 1: Payload Encoding
    prepare_payload()"]
    S1a{"precomputed_payload
    provided?"}
    S1b["PayloadEncoder.encode()
    + shellcode_stub
    + sc_checkpoints"]
    S1c["Use cached
    PreparedPayload"]

    S2["Step 2: Template Assembly
    Assembler.assemble()
    @MODULE markers → module code"]

    S3["Step 3: Marker Scoping
    strip_markers_outside_targets()
    scope @MUTATE to target modules"]

    S4["Step 4: AST Mutations
    Mutator::apply(ast.* specs)
    then strip_mutation_markers()"]

    S5{"LLVM mutations
    present?"}

    S5A["LLVM Path:
    C → .ll (clang -S -emit-llvm)
    → IrMutator.apply()
    → .o (clang -c)
    → link_baseline_exe()"]

    S5B["Direct Path:
    invoke_clang_internal()
    C → .exe (single step)"]

    S6{"binary.*
    mutations?"}
    S6A["BinaryMutator.apply()
    on PE bytes"]

    S7["Step 6: Finalize
    SHA256 → rename to hash.exe
    → BuiltArtifact"]

    S8{"trace_mode
    ≠ off?"}
    S8A["apply_instrumentation()
    re-build with:
    • line traces (AST)
    • coverage (clang flag)
    • API tracing (IR)
    • link runtimes"]

    S8B{"binary mutations
    on instrumented?"}
    S8C["Re-apply BinaryMutator
    on instrumented .exe"]

    DONE(["BuiltArtifact returned"])

    START --> S0 --> S1
    S1 --> S1a
    S1a -->|No| S1b --> S2
    S1a -->|Yes| S1c --> S2
    S2 --> S3 --> S4 --> S5
    S5 -->|Yes| S5A --> S6
    S5 -->|No| S5B --> S6
    S6 -->|Yes| S6A --> S7
    S6 -->|No| S7
    S7 --> S8
    S8 -->|No| DONE
    S8 -->|Yes| S8A --> S8B
    S8B -->|Yes| S8C --> DONE
    S8B -->|No| DONE
```

---

## 3. Mutation Routing — MutationSpec Dispatch

Shows how `MutationSpec` is parsed, categorized, and routed to three independent engines at different build stages.

```mermaid
flowchart TB
    CLI["CLI string
    ast.decon_rounds:count=50"]
    -->|from_cli_str| MS["MutationSpec
    id: ast.decon_rounds
    params: {count: 50}"]
    -->|parse| CAT{category?}

    CAT -->|"ast.*"| PHASE1["Phase 1: C Source
    Mutator::apply()"]
    CAT -->|"llvm.*"| PHASE2["Phase 2: LLVM IR
    Mutator::apply()"]
    CAT -->|"binary.*"| PHASE3["Phase 3: PE bytes
    builder.rs post-link"]

    subgraph AST["AstMutator (tree-sitter)"]
        direction TB
        ASTM1["Marker-based
        @MUTATE:name → apply_at_marker()
        bottom-up (reverse line order)"]
        ASTM2["Global: benign_syscall_insert
        tree-sitter function body walk"]
        ASTM3["Global: const_obfuscation
        number_literal nodes"]
        ASTM4["Global: string_xor
        string_literal nodes (last)"]
        ASTM1 --> ASTM2 --> ASTM3 --> ASTM4
    end

    subgraph IR["IrMutator (text-based)"]
        direction TB
        IRM1["nop_insert
        asm NOPs after BB labels"]
        IRM2["opaque_predicate
        br label → opaque br i1"]
        IRM3["junk_block
        unreachable dead blocks"]
    end

    subgraph BIN["BinaryMutator (PE)"]
        direction TB
        BIN1["Phase 1: Sections
        rich_header, import_pad
        resource_inject, section_rename
        timestamp"]
        BIN2["Phase 2: Consolidated .rdata
        debug_dir, string_inject
        size_pad, entropy_normalize"]
        BIN1 --> BIN2
    end

    PHASE1 --> AST
    PHASE2 --> IR
    PHASE3 --> BIN
```

---

## 4. Template Assembly — Modules into Source

Shows how `Assembler` replaces `@MODULE` markers in `loader_template.c` with selected module implementations, producing a single compilable `.c` file.

```mermaid
flowchart TD
    subgraph Inputs
        MS["ModuleSelection
        carrier: alloc_rw_rx
        decoder: xor
        antiemulation: none
        deconditioner: alloc_loop
        guardrail: env
        virtualprotect: standard
        decoy: winexec"]

        PH["payload_header
        (generated C code)"]
    end

    LT["loader_template.c
    contains @MODULE:xxx markers
    and @MUTATE:xxx markers"]

    subgraph Replacements["Assembler.assemble() — 9 replacements in order"]
        direction TB
        R1["@MODULE:payload → payload.h code"]
        R2["@MODULE:definitions → definitions.h"]
        R3["@MODULE:decoder → decoder/xor.c"]
        R4["@MODULE:virtualprotect → virtualprotect/standard.c"]
        R5["@MODULE:antiemulation → antiemulation/none.c"]
        R6["@MODULE:deconditioner → deconditioner/alloc_loop.c"]
        R7["@MODULE:guardrail → guardrails/env.c"]
        R8["@MODULE:decoy → decoy/winexec.c"]
        R9["@MODULE:carrier → carrier/alloc_rw_rx.c"]
        R1 --> R2 --> R3 --> R4 --> R5 --> R6 --> R7 --> R8 --> R9
    end

    OUT["Assembled .c source
    with // --- BEGIN/END MODULE ---
    boundary comments
    and @MUTATE markers intact"]

    SCOPE{"mutation_targets
    specified?"}

    SCOPED["strip_markers_outside_targets()
    remove @MUTATE outside
    targeted modules"]

    PASS["All @MUTATE markers
    kept as-is"]

    MS --> Replacements
    PH --> R1
    LT --> Replacements
    Replacements --> OUT --> SCOPE
    SCOPE -->|"Yes"| SCOPED
    SCOPE -->|"No (empty)"| PASS
```

---

## 5. Payload Encoding Pipeline

Details the shellcode preparation flow: optional checkpoint stub prepend, optional INT3 patching, encoding, and C header generation.

```mermaid
flowchart TD
    RAW["Raw shellcode bytes
    Vec&lt;u8&gt;"]

    TRACE{"trace_mode
    ≠ off?"}

    STUB["prepend_checkpoint_stub()
    41-byte x64 PIC stub:
    [24B code][17B string][shellcode...]
    receives checkpoint fn in RCX"]

    INT3{"sc_checkpoint_count
    > 0?"}

    PATCH["patch_shellcode()
    iced-x86 recursive descent:
    • follow branches, calls
    • skip inline data
    • evenly space INT3 (0xCC)"]

    PS["PatchedShellcode
    { bytes, table: Vec&lt;BreakpointEntry&gt; }"]

    SCH["generate_c_header()
    → sc_checkpoints_table.h
    SC_CHECKPOINT_COUNT
    ScCheckpointEntry array"]

    ENC{"EncodingType?"}

    XOR["encode_xor()
    rolling 2-byte XOR
    key: [0xAA, 0x55]
    size: 1:1"]

    ENG["encode_english()
    byte → word lookup
    256-word dictionary
    size: ~4-6x"]

    SUB["encode_subbyte()
    nibble → LUT mapping
    16-entry table
    size: 2:1"]

    NONE["encode_none()
    identity / passthrough
    size: 1:1"]

    EP["EncodedPayload
    { encoding, data, metadata }"]

    HDR["generate_c_header()
    → PAYLOAD_LEN, XOR_KEY[],
    supermega_payload[]
    + encoding-specific defs"]

    PP["PreparedPayload
    { payload_header, sc_header }"]

    RAW --> TRACE
    TRACE -->|Yes| STUB --> INT3
    TRACE -->|No| INT3
    INT3 -->|Yes| PATCH --> PS
    PS --> SCH
    INT3 -->|No| ENC
    PS --> ENC

    ENC -->|Xor| XOR --> EP
    ENC -->|English| ENG --> EP
    ENC -->|SubByte| SUB --> EP
    ENC -->|None| NONE --> EP

    EP --> HDR --> PP
    SCH --> PP
```

---

## 6. AST Mutator — Processing Phases

Details the four-phase processing order inside `AstMutator::apply()` and the marker-based dispatch table.

```mermaid
flowchart TD
    SRC["Assembled C source
    with @MUTATE markers"]

    subgraph Phase1["Phase 1 — Marker-Based Mutations"]
        direction TB
        EXT["extract_mutation_markers()
        parse all @MUTATE annotations"]
        REV["Sort markers reverse line order
        (bottom-up to preserve offsets)"]
        DISP{"apply_at_marker()
        dispatch by name"}

        DR["decon_rounds
        replace loop bound"]
        FP["fill_pattern
        change fill data"]
        ED["exec_decoy
        insert execution"]
        TP["timing_pattern
        insert Sleep()"]
        PT["protection_transition
        alter VirtualProtect"]
        BP["benign_preamble
        insert API calls"]
        ASO["api_sequence_obfuscation
        dilute N-grams"]
        SKIP["unknown marker
        → skip (debug log)"]

        EXT --> REV --> DISP
        DISP --> DR
        DISP --> FP
        DISP --> ED
        DISP --> TP
        DISP --> PT
        DISP --> BP
        DISP --> ASO
        DISP --> SKIP
    end

    subgraph Phase15["Phase 1.5 — Global: benign_syscall_insert"]
        direction TB
        BSI["Find target function body
        via tree-sitter"]
        BG["BehaviorGraph
        topological schedule"]
        INS["Distribute statements
        across inter-statement gaps
        using density + xorshift64"]
        BSI --> BG --> INS
    end

    subgraph Phase2a["Phase 2a — Global: const_obfuscation"]
        direction TB
        INLINE["inline_protection_macros()
        expand p_RW, p_RX, etc."]
        COLLECT["collect_number_literals()
        tree-sitter walk
        filter: min_value, skip
        preprocessor/case/arrays"]
        DECOMP["For each literal:
        volatile __obf_cN_p = X;
        volatile __obf_cN = __obf_cN_p + Y;
        replace literal with (int)__obf_cN"]
        INLINE --> COLLECT --> DECOMP
    end

    subgraph Phase2b["Phase 2b — Global: string_xor (runs last)"]
        direction TB
        STRS["collect_string_literals()
        tree-sitter walk
        skip preprocessor children"]
        XORE["For each string:
        static char[] with XOR bytes
        + lazy decode on first use
        replace with decoded reference"]
        STRS --> XORE
    end

    STRIP["strip_mutation_markers()
    remove all @MUTATE + boundary
    comments before compilation"]

    SRC --> Phase1 --> Phase15 --> Phase2a --> Phase2b --> STRIP
```

---

## 7. Binary Mutator — Two-Phase PE Transforms

Details the `BinaryMutator::apply()` two-phase architecture and the data sources for each transform.

```mermaid
flowchart TD
    PE["PE bytes (Vec&lt;u8&gt;)"]
    VALID{"MZ signature
    valid?"}

    subgraph Phase1["Phase 1 — Individual Section Additions"]
        direction TB
        RH["rich_header
        encode_rich_header()
        inject between DOS stub
        and PE signature"]
        IP["import_pad
        build_import_section()
        add .idata with
        benign dead imports"]
        RI["resource_inject
        build_resource_section()
        add .rsrc with
        VS_VERSIONINFO + manifest"]
        SR["section_rename
        .text/.rdata/.data
        based on characteristics"]
        TS["timestamp
        backdate COFF + debug
        with FNV1a jitter"]
    end

    subgraph Phase2["Phase 2 — Consolidated .rdata Append"]
        direction TB
        DD["debug_dir
        IMAGE_DEBUG_DIRECTORY
        + CodeView RSDS record"]
        SI["string_inject
        benign Windows strings
        null-terminated, DWORD-aligned"]
        SP["size_pad
        low-entropy padding
        to target file size"]
        EN["entropy_normalize
        low-entropy padding
        to reach target entropy"]
        MERGE["Merge all into single
        .rdata section via
        add_section()"]
        DD --> MERGE
        SI --> MERGE
        SP --> MERGE
        EN --> MERGE
    end

    subgraph Data["binary_data.rs — Donor Data"]
        direction TB
        RP["RichProfile
        notepad / calc / explorer
        RichRecord[]"]
        BI["BenignImport
        15 DLLs, ~45 functions"]
        BS["BENIGN_STRINGS
        30 Windows-style messages"]
        LEP["generate_low_entropy_padding()
        strings + int sequences
        + pointer-zero blocks"]
        BM["build_manifest()
        XML manifest template"]
        BVI["build_version_info()
        VS_FIXEDFILEINFO binary"]
    end

    CHECK["compute_pe_checksum()
    16-bit carry-fold"]

    GOBLIN["goblin::pe::PE::parse()
    structural validation"]

    OUT["Mutated PE bytes
    + applied mutation IDs"]

    PE --> VALID
    VALID -->|Yes| Phase1
    VALID -->|No| ERR["Error: invalid MZ"]
    Phase1 --> Phase2

    RP -.-> RH
    BI -.-> IP
    BM -.-> RI
    BVI -.-> RI
    BS -.-> SI
    LEP -.-> SP
    LEP -.-> EN

    Phase2 --> CHECK --> GOBLIN --> OUT
```

---

## 8. Benign Catalog — Dependency Graph Scheduling

Shows how `BehaviorGraph` performs topological scheduling of benign API call insertion, respecting chained dependencies within groups.

```mermaid
flowchart TD
    subgraph Catalog["default_catalog() — 9 BehaviorEntry items"]
        direction LR
        subgraph SQ["SystemQuery (independent)"]
            direction TB
            SQ0["ID 0
            GetEnvironmentVariableA"]
            SQ1["ID 1
            GetComputerNameA"]
            SQ2["ID 2
            GetTickCount"]
        end
        subgraph FI["FileIo (chained)"]
            direction TB
            FI10["ID 10
            CreateFileA"]
            FI11["ID 11
            ReadFile"]
            FI12["ID 12
            CloseHandle"]
            FI10 --> FI11 --> FI12
        end
        subgraph RI["RegistryIo (chained)"]
            direction TB
            RI20["ID 20
            RegOpenKeyExA"]
            RI21["ID 21
            RegQueryValueExA"]
            RI22["ID 22
            RegCloseKey"]
            RI20 --> RI21 --> RI22
        end
    end

    FILTER["BehaviorGraph::new()
    filter by allowed BehaviorGroups
    build remaining_deps + children
    seed frontier with zero-dep entries"]

    subgraph Schedule["Topological Pop Schedule"]
        direction TB
        FRONT["Frontier (sorted, ready)
        e.g., [0, 1, 2, 10, 20]"]
        POP["pop() — xorshift64
        random pick from frontier"]
        UNLOCK["Unlock children
        whose deps are now satisfied
        → insert into frontier"]
        EMIT["Emit: declarations[]
        + code statement"]
        POP --> EMIT
        POP --> UNLOCK --> FRONT
        FRONT --> POP
    end

    OUTPUT["generate_insertion()
    returns:
    (deduplicated declarations,
    ordered statements)"]

    Catalog --> FILTER --> Schedule --> OUTPUT
```

---

## 9. Instrumentation — Baseline vs Instrumented Paths

Compares the two build modes side-by-side, showing which components activate for each.

```mermaid
flowchart TD
    subgraph Baseline["Baseline Build (trace_mode = off)"]
        direction TB
        B1["Payload encoding
        (no stub, no INT3)"]
        B2["Template assembly"]
        B3["AST mutations
        + strip markers"]
        B4["clang -O2
        NO -DENABLE_INSTRUMENTATION"]
        B5["Link: minimal_runtime.o only"]
        B6["Binary mutations"]
        B7["artifact.exe"]

        B1 --> B2 --> B3 --> B4 --> B5 --> B6 --> B7
    end

    subgraph Instrumented["Instrumented Build (trace_mode ≠ off)"]
        direction TB
        I1["Payload encoding
        + checkpoint stub (41B)
        + INT3 patching"]
        I2["Template assembly"]
        I3["AST mutations
        + strip markers"]
        I4["inject_line_traces()
        tree-sitter AST injection"]
        I5["clang -O2
        -DENABLE_INSTRUMENTATION
        -fsanitize-coverage=trace-pc"]
        I6["Instrumenter.instrument()
        inject_api_tracing() on IR"]
        I7["Link:
        minimal_runtime.o
        + instrumentation_runtime.o
        + sc_checkpoint_runtime.o"]
        I8["Binary mutations"]
        I9["artifact.exe"]

        I1 --> I2 --> I3 --> I4 --> I5 --> I6 --> I7 --> I8 --> I9
    end

    subgraph RuntimeBaseline["Runtime Behavior (Baseline)"]
        RB1["ARTIFACT_CHECKPOINT → no-op"]
        RB2["EXECUTE_SHELLCODE → direct call"]
        RB3["__runtime_exit → syscall
        (weak flush = NULL, skipped)"]
    end

    subgraph RuntimeInst["Runtime Behavior (Instrumented)"]
        RI1["ARTIFACT_CHECKPOINT → JSON to pipe"]
        RI2["__trace_line_binary → binary protocol"]
        RI3["__sanitizer_cov_trace_pc → AFL bitmap"]
        RI4["EXECUTE_SHELLCODE → VEH + checkpoint fn"]
        RI5["__runtime_exit → flush all → syscall"]
    end

    B7 --> RuntimeBaseline
    I9 --> RuntimeInst
```

---

## 10. Compilation Modes — Standard vs MSVC-Compat

Shows the two compiler/linker paths and how `MsvcCompat` switches the toolchain.

```mermaid
flowchart TD
    SRC[".c source file"]

    MODE{"BuilderConfig
    .msvc_compat?"}

    subgraph Standard["Standard Mode (default)"]
        direction TB
        SC["clang
        -target x86_64-pc-windows-msvc
        -fuse-ld=lld -O2
        -isystem &lt;xwin includes&gt;"]
        SL["lld-link
        /subsystem:console
        /machine:x64
        /DEBUG:NONE /Brepro
        /libpath:&lt;xwin libs&gt;"]
        SC --> SL
    end

    subgraph MSVC["MSVC-Compat Mode"]
        direction TB
        MC["clang --driver-mode=cl
        /c source.c /Fo temp.obj
        /imsvc&lt;xwin includes&gt;
        sets _MSC_VER"]
        ML["invoke_msvc_link()
        write _msvc_link.bat:
        call vcvarsall.bat x64
        link.exe /OUT:output.exe
        (genuine MSVC link.exe)"]
        WP["wsl_to_win_path()
        convert all paths
        via wslpath -wa"]
        MC --> WP --> ML
    end

    subgraph PE_Standard["PE Output (Standard)"]
        PS1["LLD linker version"]
        PS2["Minimal Rich header"]
        PS3["LLD section layout"]
    end

    subgraph PE_MSVC["PE Output (MSVC-Compat)"]
        PM1["MSVC linker version"]
        PM2["Genuine MSVC Rich header"]
        PM3["MSVC section layout"]
    end

    SRC --> MODE
    MODE -->|"None"| Standard --> PE_Standard
    MODE -->|"Some(MsvcCompat)"| MSVC --> PE_MSVC
```

---

## 11. Struct Relationships — Type-Level Class Diagram

Shows ownership and usage relationships between the key structs in the build crate.

```mermaid
classDiagram
    class ArtifactBuilder {
        -config: BuilderConfig
        -xwin: XwinPaths
        +new(BuilderConfig) Result~Self~
        +build(BuildInput) Result~BuiltArtifact~
    }

    class BuilderConfig {
        +output_dir: PathBuf
        +xwin_dir: PathBuf
        +runtime_src: PathBuf
        +minimal_runtime_src: PathBuf
        +modular_template_dir: PathBuf
        +msvc_compat: Option~MsvcCompat~
    }

    class XwinPaths {
        +crt_include: String
        +sdk_ucrt_include: String
        +crt_lib: String
        +sdk_ucrt_lib: String
        +sdk_um_lib: String
        +include_args() Vec
        +lib_args() Vec
    }

    class MsvcCompat {
        +vcvarsall_path: PathBuf
        +default_vcvarsall() PathBuf
    }

    class BuildInput {
        <<enum>>
        ModularTemplate
    }

    class ModuleSelection {
        +carrier: String
        +decoder: String
        +antiemulation: String
        +deconditioner: String
        +guardrail: String
        +virtualprotect: String
        +decoy: String
        +validate(Path) Result
    }

    class BuiltArtifact {
        +artifact_id: String
        +source_path: PathBuf
        +output_path: PathBuf
        +size_bytes: u64
        +sha256: String
        +mutations_applied: Vec~String~
        +assembled_source: Option~String~
    }

    class PreparedPayload {
        +payload_header: String
        +sc_header: Option~String~
    }

    class MutationSpec {
        +id: String
        +params: HashMap
        +from_cli_str(str) Self
        +parse() (str, str)
    }

    class Assembler {
        -template_dir: PathBuf
        -module_cache: HashMap
        +assemble(ModuleSelection, str) Result~String~
    }

    class PayloadEncoder {
        +xor_key: u8 array
        +dictionary: Vec~String~
        +subbyte_mapping: u8 array
        +encode(bytes, EncodingType) EncodedPayload
        +generate_c_header(EncodedPayload) String
    }

    class EncodedPayload {
        +encoding: EncodingType
        +data: Vec~u8~
        +metadata: HashMap
    }

    class AstMutator {
        -parser: tree_sitter Parser
        +apply(str, MutationSpec[]) Result
    }

    class IrMutator {
        -rng_state: u32
        +apply(str, MutationSpec[]) Result
    }

    class BinaryMutator {
        -pe_bytes: Vec~u8~
        +apply(MutationSpec[]) Result
    }

    class BehaviorGraph {
        -entries: HashMap
        -frontier: Vec
        -consumed: HashSet
        -rng_state: u64
        +new(BehaviorEntry[], BehaviorGroup[], u64) Self
        +pop() Option~BehaviorEntry~
    }

    class BehaviorEntry {
        +id: u32
        +group: BehaviorGroup
        +deps: Vec~u32~
        +declarations: Vec~str~
        +code: str
    }

    class PatchedShellcode {
        +bytes: Vec~u8~
        +table: Vec~BreakpointEntry~
    }

    class BreakpointEntry {
        +offset: usize
        +original_byte: u8
        +name: String
        +progress_pct: u8
    }

    class Instrumenter {
        +instrument(Path, TraceMode, Path) Result
    }

    ArtifactBuilder *-- BuilderConfig : owns
    ArtifactBuilder *-- XwinPaths : owns
    BuilderConfig o-- MsvcCompat : optional
    ArtifactBuilder ..> BuildInput : consumes
    ArtifactBuilder ..> BuiltArtifact : produces

    BuildInput *-- ModuleSelection : contains
    BuildInput *-- MutationSpec : contains list
    BuildInput o-- PreparedPayload : optional cache

    Assembler ..> ModuleSelection : reads
    PayloadEncoder ..> EncodedPayload : produces

    AstMutator ..> MutationSpec : reads
    AstMutator ..> BehaviorGraph : uses
    BehaviorGraph *-- BehaviorEntry : schedules
    IrMutator ..> MutationSpec : reads
    BinaryMutator ..> MutationSpec : reads

    PatchedShellcode *-- BreakpointEntry : contains

    ArtifactBuilder ..> Assembler : creates
    ArtifactBuilder ..> PayloadEncoder : creates
    ArtifactBuilder ..> AstMutator : delegates to
    ArtifactBuilder ..> IrMutator : delegates to
    ArtifactBuilder ..> BinaryMutator : delegates to
    ArtifactBuilder ..> Instrumenter : delegates to
```

---

## 12. Loader Runtime Execution Flow

Shows what happens at runtime when the compiled artifact executes, including the module call order and instrumentation hooks.

```mermaid
sequenceDiagram
    participant Main as main()
    participant Guard as guardrail()
    participant Anti as antiemulation()
    participant Decon as deconditioner()
    participant Decoy as decoy()
    participant Carrier as carrier()
    participant Decoder as decode_payload()
    participant VP as MyVirtualProtect()
    participant SC as EXECUTE_SHELLCODE
    participant VEH as VEH Handler
    participant Exit as __runtime_exit()
    participant Pipe as Named Pipes

    Main->>Pipe: CHECKPOINT("main_entry")
    Main->>Guard: guardrail()
    Guard-->>Main: 0 (safe) or 1 (bail)

    Main->>Pipe: CHECKPOINT("anti_emulation_start")
    Main->>Anti: antiemulation()
    Anti-->>Main: return

    Main->>Pipe: CHECKPOINT("deconditioner_start")
    Main->>Decon: deconditioner()
    Note over Decon: alloc→write→protect→free<br/>loop with benign data<br/>(rehearse carrier pattern)
    Decon-->>Main: return

    Main->>Pipe: CHECKPOINT("decoy_start")
    Main->>Decoy: decoy()
    Decoy-->>Main: return

    Main->>Pipe: CHECKPOINT("carrier_start")
    Main->>Carrier: carrier()
    Carrier->>Carrier: VirtualAlloc(RW)
    Carrier->>Decoder: decode_payload(buf, len)
    Note over Decoder: FORCE_INLINE<br/>XOR / English / SubByte
    Carrier->>VP: MyVirtualProtect(RX)
    Note over VP: FORCE_INLINE
    Carrier->>Pipe: CHECKPOINT("pre_execution")
    Carrier->>SC: EXECUTE_SHELLCODE(buf)

    alt Instrumented Build
        SC->>VEH: Install VEH handler
        SC->>SC: call shellcode(checkpoint_fn)
        Note over VEH: On INT3 hit:<br/>lookup table → report<br/>restore byte → resume
        SC->>VEH: Remove VEH handler
    else Baseline Build
        SC->>SC: volatile fn ptr call
    end

    Carrier-->>Main: return

    Main->>Pipe: SUCCESS("all_stages_complete")
    Main->>Exit: __runtime_exit(0)
    Note over Exit: flush coverage + trace<br/>+ checkpoints<br/>→ direct NtTerminateProcess<br/>syscall (bypass hooks)
```

---

## Diagram Index

| # | Diagram | Scope | Purpose |
|---|---------|-------|---------|
| 1 | Overview | Whole crate | Conceptual — understand subsystem boundaries and data flow |
| 2 | Build Pipeline | `builder.rs` | Step-by-step orchestration with branching paths |
| 3 | Mutation Routing | `mutator/` + `transform/` | How MutationSpec dispatches to 3 engines |
| 4 | Template Assembly | `template/assembler.rs` | @MODULE replacement and marker scoping |
| 5 | Payload Encoding | `template/payload.rs` + `sc_checkpoints.rs` | Shellcode preparation: stub → INT3 → encode → header |
| 6 | AST Mutator | `transform/ast_mutator.rs` | 4-phase processing and marker dispatch table |
| 7 | Binary Mutator | `transform/binary_mutator.rs` | Two-phase PE transforms with data sources |
| 8 | Benign Catalog | `transform/benign_catalog.rs` | Dependency-aware topological scheduling |
| 9 | Instrumentation | `builder.rs` + `instrument/` + `runtime/` | Baseline vs instrumented build comparison |
| 10 | Compilation Modes | `builder.rs` + `msvc_compat.rs` | Standard (Clang+LLD) vs MSVC-compat paths |
| 11 | Struct Relationships | All modules | UML class diagram of ownership and usage |
| 12 | Runtime Execution | `loader_template.c` + `runtime/` | Sequence diagram of artifact execution at runtime |
