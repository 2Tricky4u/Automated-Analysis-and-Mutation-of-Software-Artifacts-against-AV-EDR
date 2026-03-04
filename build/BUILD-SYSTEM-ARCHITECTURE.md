# Build System Architecture — `examples/`, `runtime/`, `src/`

## Overview

The `build/` crate is the **artifact factory** of AutoMutate++. It takes a mutation recipe (module selection + mutation specs + payload) and produces a Windows PE executable, applying transformations at three layers: **C source (AST)**, **LLVM IR**, and **post-link binary (PE)**. It also injects optional telemetry instrumentation for the Two-Run Differential Protocol described in CLAUDE.md Section 5.

The system spans three directories that form a clean separation of concerns:

| Directory | Language | Responsibility |
|-----------|----------|----------------|
| `build/src/` | Rust | Build orchestration, mutation engines, instrumentation injection, compiler invocation |
| `build/runtime/` | C | Linked runtime libraries (telemetry, coverage, checkpoint, clean exit) |
| `build/examples/` | Rust | CLI entry points for manual/scripted builds |

For the template assembly subsystem (`build/src/template/` + `build/templates/`), see `TEMPLATE-SYSTEM.md`.

---

## Role in the Global Project

```
Controller / JobWorker
    │
    │  BuildInput::ModularTemplate { modules, payload, encoding, mutations, trace_mode, ... }
    │
    ▼
┌──────────────────────────────────────────────────────────────────────────┐
│                        BUILD CRATE (build/)                              │
│                                                                          │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  src/builder.rs — ArtifactBuilder                                │    │
│  │                                                                   │    │
│  │  Step 0: Sync decoder ↔ encoding                                 │    │
│  │  Step 1: Encode payload (stub + INT3 patch + XOR/English)        │    │
│  │          [src/template/payload.rs, shellcode_stub.rs,            │    │
│  │           sc_checkpoints.rs]                                      │    │
│  │  Step 2: Assemble template (loader_template.c + modules)         │    │
│  │          [src/template/assembler.rs]                               │    │
│  │  Step 3: Scope & strip @MUTATE markers                           │    │
│  │  Step 4: Apply AST mutations (tree-sitter)                       │    │
│  │          [src/transform/ast_mutator.rs]                           │    │
│  │  Step 5: Write .c → disk                                         │    │
│  │                                                                   │    │
│  │       ┌── IF llvm.* mutations ──────────────────────────┐        │    │
│  │       │  Compile C → .ll (LLVM IR)                       │        │    │
│  │       │  Apply IR mutations [src/transform/ir_mutator.rs]│        │    │
│  │       │  Compile IR → .o → link → .exe                   │        │    │
│  │       └──────────────────────────────────────────────────┘        │    │
│  │       ┌── ELSE (direct path) ───────────────────────────┐        │    │
│  │       │  clang .c → .exe (single step)                   │        │    │
│  │       └──────────────────────────────────────────────────┘        │    │
│  │                                                                   │    │
│  │  Step 6: Apply binary mutations (PE post-link)                   │    │
│  │          [src/transform/binary_mutator.rs]                        │    │
│  │  Step 7: Hash, record metadata → BuiltArtifact                   │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                                                          │
│  Linked at compile time:                                                 │
│    runtime/minimal_runtime.c        → always (baseline + instrumented)   │
│    runtime/instrumentation_runtime.c → only when trace_mode != "off"     │
│    runtime/sc_checkpoint_runtime.c   → only with sc-checkpoints feature  │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
    │
    ▼
BuiltArtifact { artifact_id (SHA256), output_path, mutations_applied, ... }
    │
    ▼
Worker VM (execution + telemetry collection)
```

The build crate is **the only component that produces artifacts**. The controller dispatches `BuildInput` specs; the worker VMs execute the resulting `.exe` files. Telemetry flows back through the runtime libraries embedded in each artifact.

---

## `build/examples/` — CLI Entry Points

### `build_artifact.rs` — Full Pipeline CLI

**Purpose:** End-to-end artifact production from the command line. Exercises the full `ArtifactBuilder.build(BuildInput::ModularTemplate)` path.

**Architecture:** Parses CLI arguments into a `BuildInput::ModularTemplate` struct, initializes `ArtifactBuilder` with `BuilderConfig`, and calls `builder.build(input)` inside a Tokio runtime. Supports all module selections, encoding types, trace modes, and mutation specs (AST + LLVM + binary).

**Key behaviors:**
- Requires a raw `.bin` payload file (`--payload / -p`)
- Auto-syncs `--decoder` ↔ `--encoding` when only one is specified
- Supports MSVC-compatible build mode (`--msvc-compat`) for genuine MSVC PE metadata
- Outputs `BuiltArtifact` metadata (artifact_id, size, path, mutations applied)

**Usage patterns:**
```bash
# Baseline build
cargo run -p build --example build_artifact -- -p shellcode.bin -o artifact.exe

# With AST + binary mutations
cargo run -p build --example build_artifact -- -p sc.bin \
  -m ast.string_xor:xor_key=0xBB \
  -m binary.rich_header:donor=notepad \
  -m binary.import_pad:count=50 \
  -o mutated.exe

# Instrumented build with tracing
cargo run -p build --example build_artifact -- -p sc.bin --trace lines -o traced.exe
```

### `emit_c.rs` — C Source Emission (Debug/Inspect)

**Purpose:** Reproduce the exact C source output from Steps 1–4 of the build pipeline **without** invoking Clang. Stops right before compilation. Used for inspecting what the assembler + mutator produce.

**Architecture:** Manually calls `PayloadEncoder`, `Assembler`, `Mutator`, `strip_mutation_markers`, and `inject_line_traces_with_opts` in the same order as `builder.rs`, then outputs the result to stdout or a file.

**Key behaviors:**
- Uses a 256-byte test payload if no `--payload` given
- Supports `--trace on/off` to toggle AST line instrumentation
- Supports `--mutation / -m` for AST mutations (same syntax as `build_artifact`)
- Fixed modules (carrier=change_rw_rx, decoder=xor, guardrail=env, decoy=winexec), configurable antiemulation and deconditioner

**Why it exists:** Decouples C source inspection from the full compile step. Developers can verify template assembly, mutation correctness, and instrumentation injection without needing clang/xwin installed.

---

## `build/runtime/` — C Runtime Libraries

These C files are cross-compiled into `.o` object files and linked into the final artifact. They provide the runtime infrastructure that makes telemetry collection possible. The key design principle is **conditional compilation**: the same artifact source compiles to either an instrumented or baseline binary based on `ENABLE_INSTRUMENTATION`.

### Linking Rules

| Runtime Object | When Linked | Controlled By |
|----------------|-------------|---------------|
| `minimal_runtime.o` | **Always** (instrumented + baseline) | Builder always compiles and links it |
| `instrumentation_runtime.o` | Only when `trace_mode != "off"` | `-DENABLE_INSTRUMENTATION` flag |
| `sc_checkpoint_runtime.o` | Only with `sc-checkpoints` feature + instrumented | `-DENABLE_SC_CHECKPOINTS` flag |

### `instrumentation.h` — Conditional Macro API

**Purpose:** The single include for all artifact C code. Provides macros that expand to real function calls (instrumented) or no-ops (baseline).

| Macro | Instrumented Expansion | Baseline Expansion |
|-------|----------------------|-------------------|
| `ARTIFACT_CHECKPOINT(name)` | `__artifact_checkpoint(name)` | `((void)0)` |
| `ARTIFACT_SUCCESS(msg)` | `__artifact_success(msg)` | `((void)0)` |
| `ARTIFACT_FAILURE(msg, code)` | `__artifact_failure(msg, code)` | `((void)0)` |

When `ENABLE_INSTRUMENTATION` is defined, it also includes `minimal_runtime.h` to make `__runtime_exit()` available.

### `minimal_runtime.c` + `minimal_runtime.h` — Clean Exit

**Purpose:** Provides `__runtime_exit(int exit_code)` — a process termination function that **bypasses all usermode hooks** (including RedEDR detours) via direct syscalls.

**Architecture (x64 path):**

1. **Telemetry flush** — Calls `__coverage_flush`, `__trace_flush`, `__checkpoint_flush` via **weak symbols**. If `instrumentation_runtime.o` is not linked (baseline build), these resolve to NULL and are safely skipped.
2. **50ms delay** — Ensures file writes complete before process death.
3. **Syscall number resolution** — Parses the ntdll.dll syscall stub bytes at runtime to extract `NtTerminateProcess` and `NtClose` syscall numbers. Falls back to known Win10/11 constants if parsing fails.
4. **Direct syscall** — Invokes `syscall` instruction directly via a naked function (`DirectSyscall2`), skipping the entire ntdll.dll code path.
5. **Fallback** — If direct syscall fails, calls `ExitProcess()` as last resort.

**Why direct syscalls:** RedEDR (and real EDRs) hook `NtTerminateProcess` via detours. A hooked exit can deadlock or trigger post-mortem analysis that corrupts telemetry timing. Direct syscalls guarantee clean, deterministic exits.

**Weak symbol pattern:** The key architectural insight. `__coverage_flush`, `__trace_flush`, `__checkpoint_flush` are declared as `__attribute__((weak)) extern`. When `instrumentation_runtime.o` is linked, the real implementations win. When it's absent, the symbols resolve to NULL, and `minimal_runtime.c` checks for NULL before calling. This allows a **single object file** to serve both instrumented and baseline builds.

### `instrumentation_runtime.c` — Telemetry Collection Engine

**Purpose:** The main telemetry runtime. Provides three independent subsystems:

#### 1. BB Coverage (AFL-style Edge Bitmap)

Implements LLVM SanitizerCoverage callbacks for basic-block coverage tracking.

| Callback | Mode | Description |
|----------|------|-------------|
| `__sanitizer_cov_trace_pc()` | trace-pc | Simple callback, uses return address as BB ID |
| `__sanitizer_cov_trace_pc_guard_init()` | guard | Initializes unique guard IDs at startup |
| `__sanitizer_cov_trace_pc_guard()` | guard | Per-BB callback with guard pointer |

**Data structures:**
- `__coverage_map[64KB]` — AFL-compatible edge bitmap (prev_bb XOR cur_bb, saturating at 255)
- `__bb_ids[1024]` + `__bb_hit_counts[1024]` — Per-BB hit tracking
- `__sancov_guards_start/end` — LLVM SanitizerCoverage guard section markers (`.SCOV$GA`/`.SCOV$GZ`)

**Output files:**
- `coverage.bin` — 64KB raw edge bitmap
- `coverage_bbs.txt` — Human-readable BB report (BB_ID, HIT_COUNT)

**Incremental flush:** Every 50 BB executions, the coverage bitmap is written to disk. This is critical because **EDR can kill the process at any moment** — without incremental flush, all coverage data would be lost if the process never reaches `atexit`.

#### 2. Line-Level Tracing

Three trace formats, all writing to a named pipe (`\\.\pipe\rededr_trace`) or fallback files:

| Function | Format | Protocol |
|----------|--------|----------|
| `__trace_line()` | JSON | `{"seq":N,"file":"path","line":N,"func":"name"}` |
| `__trace_line_b64()` | Base64 | `YjY0<base64(line:file:line:)>` (Lepori thesis format) |
| `__trace_line_binary()` | Binary | `InstRecordHeader` (32B) + `"file:line:func"` payload |

**Binary protocol (`InstRecordHeader`):**
```c
struct InstRecordHeader {
    uint32_t magic;       // 0x49535452 ('ISTR')
    uint16_t version;     // 1
    uint16_t event_type;  // 1=line, 2=func_enter, 3=syscall, 4=bb
    uint32_t thread_id;
    uint64_t seq_no;      // Monotonic (InterlockedIncrement64)
    uint64_t ts_us;       // Microseconds since process start (QPC)
    uint32_t payload_len;
};
```

**Aggressive flushing:** Every trace event is flushed immediately (`__trace_flush()` after each call). This trades throughput for **death-bed telemetry** — capturing the exact line where EDR kills the process.

**Timestamp system:** Uses `QueryPerformanceCounter` for microsecond-resolution timing relative to process start. Thread-safe via `InterlockedIncrement64` for sequence counters.

#### 3. Checkpoint Markers

Writes structured JSON events to `\\.\pipe\rededr_checkpoints` (or fallback files):

| Function | JSON Type |
|----------|-----------|
| `__artifact_checkpoint(name)` | `{"ts_us":N,"checkpoint":"name","type":"artifact_checkpoint"}` |
| `__artifact_success(msg)` | `{"ts_us":N,"checkpoint":"msg","type":"success"}` |
| `__artifact_failure(msg, code)` | `{"ts_us":N,"checkpoint":"msg","type":"failure","error_code":N}` |

These are the primary progress markers used by the triage engine to determine detection outcomes (MUTATION_FAILED, MUTATION_SUCCESS, FULL_EVASION).

**Auto-initialization:** All subsystems use lazy initialization — they initialize on first use, not at process startup. `atexit` handlers are registered for cleanup. A `DllMain` handler provides additional cleanup on `DLL_PROCESS_DETACH`.

### `sc_checkpoint.h` — Shellcode Execution Macro

**Purpose:** Provides the `EXECUTE_SHELLCODE(addr)` macro that encapsulates all the complexity of shellcode invocation with optional VEH-based INT3 checkpoints.

**Instrumented expansion:**
```c
#define EXECUTE_SHELLCODE(addr) do {
    __sc_set_base_addr(addr);      // Store shellcode base for VEH handler
    __sc_veh_install();            // Register VEH (INT3 handler)
    ((void(*)(void(*)(const char*)))(addr))(__artifact_checkpoint);  // Call shellcode, passing checkpoint fn
    __sc_veh_remove();             // Unregister VEH
} while(0)
```

**Baseline expansion:**
```c
#define EXECUTE_SHELLCODE(addr) do {
    void (*volatile __sc_fn)(void) = (void(*)(void))(addr);
    __sc_fn();                     // Direct call, no instrumentation
} while(0)
```

The `volatile` qualifier on the baseline path prevents the compiler from optimizing away the function pointer indirection.

### `sc_checkpoint_runtime.c` — VEH INT3 Handler

**Purpose:** Catches `EXCEPTION_BREAKPOINT` exceptions at shellcode checkpoint offsets, reports them, restores original bytes, and resumes execution.

**Architecture:**
1. `__sc_veh_install()` — Registers a first-chance VEH handler via `AddVectoredExceptionHandler(1, ...)` (priority=1, first in chain)
2. On INT3 hit:
   - Verify exception address is inside shellcode region (`__sc_base_addr` to end)
   - Look up offset in `SC_CHECKPOINTS` table (generated per-build by `sc_checkpoints.rs`)
   - Report via `__artifact_checkpoint(name)`
   - Temporarily flip page to RWX, restore original byte, flip back
   - Set `Rip = exc_addr` (no adjustment needed on x64) and resume
3. If offset not in table, pass to next exception handler (`EXCEPTION_CONTINUE_SEARCH`)

---

## `build/src/` — Rust Build Logic

### `lib.rs` — Crate Root

Defines the module hierarchy and re-exports the public API:

```
build/src/
├── lib.rs          → TraceMode enum, module declarations, re-exports
├── builder.rs      → ArtifactBuilder (main API)
├── msvc_compat.rs  → MSVC-compatible build mode
├── template/       → Template assembly + payload encoding (see TEMPLATE-SYSTEM.md)
├── instrument/     → Line tracing + BB coverage injection
├── mutator/        → Mutation spec parsing + routing
└── transform/      → AST, IR, and binary mutation implementations
```

**`TraceMode` enum** — Central trace mode definition used across the crate:

| Variant | Description |
|---------|-------------|
| `Off` | No instrumentation (baseline) |
| `Api` | API tracing only |
| `BB` | Basic-block coverage only |
| `ApiPlusBB` | API + BB (default for mutation loop) |
| `Lines` | Line-level tracing (diagnostic) |
| `LinesAroundBB(u32)` | Targeted line tracing around specific BB |
| `All` | All instrumentation (debug mode) |

### `builder.rs` — ArtifactBuilder (Orchestrator)

**Purpose:** The central build orchestrator. Takes `BuildInput` and produces `BuiltArtifact` by coordinating all subsystems.

**Core types:**

| Type | Role |
|------|------|
| `BuilderConfig` | Paths to xwin SDK, runtime sources, template dir, output dir, MSVC compat |
| `BuildInput::ModularTemplate` | Full specification: modules, payload, encoding, mutations, trace_mode, etc. |
| `BuiltArtifact` | Build result: artifact_id (SHA256), output_path, size, mutations applied, compiler info |
| `PreparedPayload` | Cached encoded payload (for reuse across builds with same payload/encoding) |

**`build_modular_template()` — The Main Pipeline:**

| Step | Operation | Component |
|------|-----------|-----------|
| 0 | Sync decoder module to match encoding | Direct field assignment |
| 1 | Encode payload (stub + INT3 + encoding) | `prepare_payload()` → `template/payload.rs`, `shellcode_stub.rs`, `sc_checkpoints.rs` |
| 2 | Assemble template with modules | `template/assembler.rs` |
| 3 | Scope mutations to target modules | `strip_markers_outside_targets()` |
| 4 | Apply AST mutations + strip markers | `mutator/mod.rs` → `transform/ast_mutator.rs` |
| 5a | (if LLVM mutations) C → IR → mutate IR → .o → .exe | `transform/ir_mutator.rs` |
| 5b | (else) C → .exe direct | `invoke_clang_internal()` |
| 6 | Apply binary mutations to PE | `transform/binary_mutator.rs` |
| 7 | Hash, metadata, return `BuiltArtifact` | SHA256, `chrono::Utc::now()` |

**Compilation modes:**

| Mode | Compiler | Linker | PE Characteristics |
|------|----------|--------|--------------------|
| Standard (default) | `clang -target x86_64-pc-windows-msvc` | `lld-link` | LLD signatures, minimal Rich header |
| MSVC-compat | `clang --driver-mode=cl` | `link.exe` (via vcvarsall.bat) | Genuine MSVC Rich header, MSVC linker version |

**Compiler flags (standard mode):**
- `-target x86_64-pc-windows-msvc` — Windows x64 PE target
- `-fuse-ld=lld` — Use LLD linker
- `-O2` — Optimization level 2
- `-Wl,/Brepro` — Reproducible builds (deterministic timestamps)
- `-Wl,/DEBUG:NONE` — No debug info
- `-DENABLE_INSTRUMENTATION` — Conditional, only for instrumented builds
- `--sysroot=<xwin>` — Windows SDK from xwin

### `msvc_compat.rs` — MSVC-Compatible Builds

**Purpose:** Alternative build path that produces PE binaries with **genuine MSVC metadata** (Rich header, linker version, section layout) to reduce static detection signals from LLD signatures.

**Architecture:**
1. Compile step: `clang --driver-mode=cl /c <source> /Fo<obj>` — Makes clang behave as `clang-cl` (sets `_MSC_VER`, MSVC defaults)
2. Link step: `cmd.exe /c "call vcvarsall.bat x64 && link.exe <args>"` — Uses real MSVC `link.exe` via WSL2 Windows interop

**Key implementation detail:** A temporary `.bat` file is written to disk and executed via `cmd.exe` to avoid WSL→Windows argument escaping issues. The bat file calls `vcvarsall.bat x64` to set up the MSVC environment, then invokes `link.exe` with converted Windows paths (via `wslpath -wa`).

### `instrument/` — Instrumentation Injection

#### `instrumenter.rs` — LLVM IR-Level Instrumentation

**Purpose:** Manages IR-level instrumentation (BB coverage via SanitizerCoverage, API tracing).

**Architecture:**
- **BB coverage:** Handled natively by Clang via `-fsanitize-coverage=trace-pc` flag. The instrumenter simply counts `__sanitizer_cov_trace_pc()` callbacks in the IR for logging. No separate `opt` pass needed (previous approach broke in LLVM 17+).
- **API tracing:** Text-based LLVM IR injection. Scans for `call` instructions targeting Windows APIs (VirtualAlloc, VirtualProtect, WriteProcessMemory, etc.) and inserts `__checkpoint()` calls before them with string constant metadata.
- **Runtime declarations:** Adds `declare void @__checkpoint(i8*)`, `declare void @__trace_line(...)`, etc. after IR metadata.

#### `line_tracer.rs` — AST-Level Line Tracing

**Purpose:** Injects trace calls at every traceable statement in C/C++ source code using **tree-sitter** parsing. This is the primary instrumentation for the "lines" trace mode used in the Two-Run Differential Protocol.

**Architecture:**
1. Parse source with tree-sitter C++ parser (handles both C and C++)
2. Walk AST recursively, collecting injection points at `compound_statement` children
3. For each traceable statement (expression, declaration, if, for, while, return, break, switch, etc.), generate a trace call
4. Insert calls in reverse offset order to avoid shifting byte positions

**Loop optimization (deferred tracing):**
Inside loops, most statements use **deferred flag-based tracing** instead of eager tracing:
```c
static int __seen_L42 = 0;        // Before loop (declaration)
for (...) {
    __seen_L42 = 1;               // Inside loop (flag set, not trace call)
    ...
}
if (__seen_L42) __trace_line_binary("file", 42, __func__);  // After loop (deferred)
```

This avoids emitting a trace event per loop iteration (which could be millions of events for deconditioner loops). Only `return_statement` and `goto_statement` keep eager tracing inside loops (they transfer control past the deferred block).

**Preprocessor handling:** `#ifdef`/`#else`/`#endif` blocks are handled specially. Statements inside these blocks get traced, but only if the block is inside a function body (has a `compound_statement` ancestor). Top-level `#ifdef` blocks (containing typedefs, includes) are correctly skipped.

**Trace formats:**

| Format | Function | Wire Format |
|--------|----------|-------------|
| `Binary` (default) | `__trace_line_binary("file", line, __func__)` | InstRecordHeader + payload (see runtime section) |
| `Base64` | `__trace_line_b64("YjY0<base64>")` | Lepori thesis format |

### `mutator/mod.rs` — Mutation Specification & Routing

**Purpose:** Parses mutation specs and routes them to the correct mutation engine.

**`MutationSpec`:**
```rust
pub struct MutationSpec {
    pub id: String,                    // e.g., "ast.string_xor", "llvm.nop_insert", "binary.rich_header"
    pub params: HashMap<String, String>, // e.g., {"xor_key": "0xBB", "density": "0.3"}
}
```

**CLI syntax:** `id:key=val,key=val` — e.g., `ast.decon_rounds:count=50,method=fixed`

**Routing (3-way):**

| Prefix | Engine | Phase |
|--------|--------|-------|
| `ast.*` | `AstMutator` (tree-sitter) | Pre-compilation (C source) |
| `llvm.*` | `IrMutator` (text-based) | Post-compilation to IR (LLVM .ll text) |
| `binary.*` | `BinaryMutator` (PE manipulation) | Post-link (PE bytes) |

### `transform/` — Mutation Implementations

#### `ast_mutator.rs` — Tree-Sitter AST Mutations

**Purpose:** Source-level C code transformations using tree-sitter for structural awareness.

**Two mutation modes:**
1. **Marker-based** — Find `@MUTATE:<name>` comments in source, apply transformation at that location
2. **Global** — Transform all matching nodes across entire source (no markers needed)

**Implemented mutations:**

| Mutation ID | Mode | Description |
|-------------|------|-------------|
| `ast.decon_rounds` | Marker | Replace loop bound with fixed count or `GetTickCount() % N` |
| `ast.fill_pattern` | Marker | Change benign fill pattern (xor, nop_sled, random, zero) |
| `ast.exec_decoy` | Marker | Insert direct/threaded execution from decoy memory |
| `ast.timing_pattern` | Marker | Insert `Sleep()` delays between operations |
| `ast.protection_transition` | Marker | Change memory protection sequence (RW→RX, RW→RWX, RW→R→RX) |
| `ast.benign_preamble` | Marker | Insert benign Windows API calls at preamble points |
| `ast.benign_syscall_insert` | Global | Insert benign API call sequences between statements (N-gram dilution) |
| `ast.const_obfuscation` | Global | Decompose integer constants via volatile arithmetic |
| `ast.string_xor` | Global | XOR-encode string literals with runtime decode |

**Processing order matters:** Marker-based mutations run first (they reference source locations), then global `benign_syscall_insert`, then `const_obfuscation`, then `string_xor` (last, because it rewrites string literals that other mutations may have introduced).

#### `ir_mutator.rs` — LLVM IR Text Mutations

**Purpose:** Semantic-preserving transformations on `.ll` text files. No LLVM C-API dependency — pure text manipulation.

| Mutation ID | Description | Default Params |
|-------------|-------------|----------------|
| `llvm.nop_insert` | Insert inline asm NOPs after block labels | `density=0.3` |
| `llvm.opaque_predicate` | Convert `br label %X` to always-true conditional branch | `density=0.3, mode=robust` |
| `llvm.junk_block` | Append dead unreachable blocks before function `}` | `count=2` |

**Opaque predicate modes:**
- **Robust** (default): Uses inline asm (`xor reg, reg`) — survives `-O2` optimization because LLVM can't constant-fold opaque asm
- **Trivial**: Uses `icmp eq i32 0, 0` — folds away at `-O2`, useful for `-O0` builds

**Deterministic RNG:** Uses a seeded LCG (`state = state * 1103515245 + 12345`) for density-based insertion decisions. Same seed → same mutations.

#### `binary_mutator.rs` — Post-Link PE Transforms

**Purpose:** Modify compiled PE bytes to shift static/ML feature vectors toward benign classification.

| Mutation ID | Effect | Key Params |
|-------------|--------|------------|
| `binary.rich_header` | Inject MSVC-style Rich header (donor profiles) | `donor=notepad\|calc\|explorer` |
| `binary.import_pad` | Add benign DLL imports to IAT | `count=50` |
| `binary.resource_inject` | Add version info + manifest resources | `product_name=..., company=...` |
| `binary.section_rename` | Rename sections to MSVC defaults (.text, .rdata, .data, .pdata) | — |
| `binary.debug_dir` | Add fake PDB debug directory | `pdb_path=...` |
| `binary.timestamp` | Backdate PE timestamp | `age_days=365` |
| `binary.string_inject` | Inject benign strings (consolidated) | `count=20` |
| `binary.entropy_normalize` | Low-entropy padding (consolidated) | `target=6.0` |
| `binary.size_pad` | Pad PE to target size (consolidated) | `target_kb=256` |

**Consolidation pattern:** `string_inject`, `entropy_normalize`, `size_pad`, and `debug_dir` are merged into a **single `.rdata` section** instead of adding separate sections. This avoids the detection signal of having many non-standard section names.

**Validation:** After all transforms, the PE is re-parsed with `goblin::pe::PE::parse()` to ensure structural validity. A proper PE checksum is computed (matching MSVC `link.exe` behavior).

#### `benign_catalog.rs` — Benign API Call Catalog

**Purpose:** Provides a catalog of real, benign Windows API call sequences for N-gram dilution. Used by `ast.benign_syscall_insert`.

**Groups:**

| Group | APIs | Dependencies |
|-------|------|-------------|
| `SystemQuery` | `GetEnvironmentVariableA`, `GetComputerNameA`, `GetTickCount` | None (all independent) |
| `FileIo` | `CreateFileA` → `ReadFile` → `CloseHandle` | Chained (11 depends on 10, 12 depends on 10) |
| `RegistryIo` | `RegOpenKeyExA` → `RegQueryValueExA` → `RegCloseKey` | Chained (21 depends on 20, 22 depends on 20) |

Each `BehaviorEntry` includes:
- Required variable declarations (inserted at function scope)
- The C statement(s) to insert
- Dependency IDs (topological ordering enforced)

#### `binary_data.rs` — Embedded Donor Data

**Purpose:** Pre-built data for binary transforms.

- **Rich header profiles:** Three MSVC compiler metadata profiles (notepad, calc, explorer) with realistic product IDs, build numbers, and object counts
- **Benign import pool:** DLL names + function names for IAT padding
- **Application manifest template**
- **Version info builder**
- **Low-entropy padding generator**
- **FNV-1a hash** for Rich header checksum computation

---

## Data Flow: Instrumented vs. Baseline Build

### Baseline Build (`trace_mode = "off"`)

```
payload.bin
    → PayloadEncoder (XOR/English)
    → Assembler (modules + payload.h)
    → AST mutations + strip markers
    → clang -O2 (NO -DENABLE_INSTRUMENTATION)
    → link with: [minimal_runtime.o]  ← only runtime linked
    → binary mutations
    → artifact.exe

Runtime behavior:
    ARTIFACT_CHECKPOINT("x") → ((void)0)  // no-op
    EXECUTE_SHELLCODE(addr)  → direct call // no VEH, no checkpoint fn
    __runtime_exit(0)        → direct syscall (weak symbols for flush = NULL, skipped)
```

### Instrumented Build (`trace_mode = "lines"`)

```
payload.bin
    → prepend_checkpoint_stub() (41B stub)
    → patch_shellcode() (INT3 breakpoints, if sc-checkpoints enabled)
    → PayloadEncoder (XOR/English)
    → Assembler (modules + payload.h)
    → AST mutations + strip markers
    → inject_line_traces() (tree-sitter trace injection)
    → clang -O2 -DENABLE_INSTRUMENTATION -fsanitize-coverage=trace-pc
    → link with: [minimal_runtime.o, instrumentation_runtime.o, sc_checkpoint_runtime.o?]
    → binary mutations
    → artifact.exe

Runtime behavior:
    ARTIFACT_CHECKPOINT("x") → __artifact_checkpoint("x")  // JSON to checkpoint pipe
    __trace_line_binary(...)  → binary protocol to trace pipe
    __sanitizer_cov_trace_pc() → AFL edge bitmap + incremental flush
    EXECUTE_SHELLCODE(addr)   → VEH install → call(addr, checkpoint_fn) → VEH remove
    __runtime_exit(0)         → flush coverage + trace + checkpoints → direct syscall
```

---

## Key Design Decisions

1. **Weak symbol linkage for conditional instrumentation** — `minimal_runtime.c` declares flush functions as weak externs. When `instrumentation_runtime.o` is linked, the real implementations win; when absent, they resolve to NULL. This avoids conditional compilation of the runtime itself and enables a single `minimal_runtime.o` binary for both build modes.

2. **Aggressive flush strategy** — Every trace event and every 50 BB executions trigger an immediate disk write. The assumption: EDR will kill the process at an unpredictable point. Throughput is sacrificed for telemetry completeness. The exact line of death is captured.

3. **Three-layer mutation architecture** — AST mutations operate on C source (structural, human-readable), IR mutations on LLVM IR (control-flow, survives optimization), binary mutations on PE bytes (static features, post-link). Each layer has different trade-offs between expressiveness and persistence through compilation.

4. **Direct syscall exit** — Artifacts must exit cleanly under EDR observation. Hooked `NtTerminateProcess` can deadlock with RedEDR detours. The runtime resolves syscall numbers dynamically from ntdll.dll stubs, then invokes `syscall` directly.

5. **Deferred loop tracing** — Line tracer uses flag-set-in-loop + check-after-loop pattern to avoid O(N) trace events for deconditioner loops (which can run 20-100+ iterations). Only one trace event per unique line, regardless of iteration count.

6. **Consolidated binary sections** — Multiple binary mutations (strings, entropy padding, size padding, debug directory) are merged into a single `.rdata` section instead of adding separate sections. Non-standard section names are a strong static detection signal.

7. **Deterministic builds** — Payload encoding uses fixed XOR keys (`0xAA, 0x55`), IR mutations use seeded LCG, `clang -Wl,/Brepro` produces reproducible PE timestamps. Same inputs → same artifact (critical for the differential protocol).
