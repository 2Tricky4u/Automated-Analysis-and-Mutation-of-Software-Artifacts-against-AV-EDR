# Template System Architecture

## Overview

The template system is the **artifact generation engine** of AutoMutate++. It takes a set of module selections and a raw shellcode payload, and assembles them into a single compilable C source file that becomes the final Windows PE artifact.

It spans two directories:

| Directory | Language | Role |
|-----------|----------|------|
| `build/src/template/` | Rust | Assembler logic, payload encoding, shellcode instrumentation |
| `build/templates/` | C / Python | Template skeleton, pluggable module implementations, legacy encoder |

---

## Role in the Global Project

```
                        ┌─────────────────────────────┐
                        │      Controller / Job        │
                        │  (selects modules + encoding)│
                        └──────────┬──────────────────┘
                                   │ ModuleSelection + raw shellcode
                                   ▼
┌──────────────────────────────────────────────────────────────────┐
│                    TEMPLATE SYSTEM                                │
│                                                                  │
│  1. ShellcodeStub: optionally prepend checkpoint stub            │
│  2. ScCheckpoints: optionally patch INT3 breakpoints             │
│  3. PayloadEncoder: shellcode → encoded payload.h (C header)     │
│  4. Assembler: loader_template.c + modules → single .c file     │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
                                   │ assembled .c file
                                   ▼
                        ┌─────────────────────────────┐
                        │   AST Mutator (tree-sitter)  │
                        │   transforms @MUTATE markers │
                        │   + global mutations         │
                        └──────────┬──────────────────┘
                                   │
                                   ▼
                        ┌─────────────────────────────┐
                        │   Clang/LLVM Compiler        │
                        │   (optional IR mutations)    │
                        │   → Windows PE (.exe)         │
                        └──────────┬──────────────────┘
                                   │
                                   ▼
                        ┌─────────────────────────────┐
                        │   Binary Mutator (PE)        │
                        │   post-link PE transforms    │
                        └─────────────────────────────┘
```

The template system is **phase 1** of the build pipeline. It produces a self-contained C source file that subsequent phases (AST mutation, compilation, binary mutation) transform into the final Windows PE artifact. Mutation recipes control which modules are selected and how `@MUTATE` markers are transformed by the AST mutator (`build/src/transform/ast_mutator.rs`). For full details on the mutation and compilation pipeline, see `BUILD-SYSTEM-ARCHITECTURE.md`.

---

## Rust Components (`build/src/template/`)

### `mod.rs` — Module Root

Re-exports all public APIs. Entry points:
- `Assembler` + `ModuleSelection` — template assembly
- `PayloadEncoder` + `EncodingType` — payload encoding
- `extract_mutation_markers` / `strip_mutation_markers` — marker utilities

### `assembler.rs` — Template Assembler

**Core type: `Assembler`**

Replaces `// @MODULE:xxx` markers in `loader_template.c` with the content of the selected module file. Each replacement is wrapped in boundary comments:

```c
// --- BEGIN MODULE: carrier ---
<module code>
// --- END MODULE: carrier ---
```

**Core type: `ModuleSelection`**

Defines which module variant to use per category:

| Field | Default | Available Options |
|-------|---------|-------------------|
| `carrier` | `alloc_rw_rx` | `alloc_rw_rx`, `change_rw_rx`, `peb_walk` |
| `decoder` | `xor` | `xor`, `english`, `none`, `subbyte` |
| `antiemulation` | `none` | `none`, `sirallocalot`, `timeraw`, `cpuburn`, `heapstress`, `fsenum`, `sleepaccel` |
| `deconditioner` | `none` | `none`, `basic`, `alloc_loop`, `alloc_exec`, `thread_alloc`, `mixed_apis`, `entropy_flood` |
| `guardrail` | `none` | `none`, `env` |
| `virtualprotect` | `standard` | `standard`, `undersized` |
| `decoy` | `none` | `none`, `winexec` |

**Key functions:**

- `assemble(&modules, &payload_code) -> String` — Produces the final assembled C source
- `strip_markers_outside_targets(source, targets)` — Strips `@MUTATE` markers from non-targeted modules (scopes mutation to specific genes)
- `extract_mutation_markers(source) -> Vec<MutationMarker>` — Parses all `@MUTATE:name(params)` annotations
- `strip_mutation_markers(source)` — Removes all markers and boundary comments before compilation

### `payload.rs` — Payload Encoder

Encodes raw shellcode bytes into a C header (`payload.h`) compatible with the selected decoder module.

**Encoding types:**

| Type | Description | Decoder Module | Size Impact |
|------|-------------|---------------|-------------|
| `Xor` | Rolling 2-byte XOR key | `xor` | 1:1 |
| `English` | Dictionary-based word mapping (low entropy) | `english` | ~4-6x |
| `None` | Raw bytes, no encoding | `none` | 1:1 |
| `SubByte` | 4-bit nibble mapping (16-entry LUT) | `subbyte` | 2:1 |

Each encoding generates a `#define PAYLOAD_LEN`, the encoded data array (`supermega_payload[]`), and encoding-specific metadata (XOR keys, dictionary, mapping table).

### `shellcode_stub.rs` — Checkpoint Stub

For **instrumented builds** (`trace_mode != "off"`), prepends a 41-byte x64 PIC stub to the shellcode. The stub:

1. Receives `__artifact_checkpoint` function pointer in RCX (from the carrier)
2. Calls `checkpoint("payload_executed")` to signal the payload started
3. Falls through to the original shellcode

Layout: `[24B code] [17B "payload_executed\0"] [original shellcode...]`

Baseline builds skip this — shellcode runs directly.

### `sc_checkpoints.rs` — INT3 Breakpoint Patching

For instrumented builds, inserts evenly-spaced `INT3` (0xCC) breakpoints into shellcode at instruction boundaries. Uses **recursive descent disassembly** (via `iced-x86`) to:

- Follow control flow (branches, calls, conditionals)
- Avoid patching inline data (hashes, strings, config blobs)
- Skip the entry point instruction

Produces a C header table so the VEH (Vectored Exception Handler) runtime can recognize each checkpoint offset and restore the original byte at runtime.

---

## C Template & Modules (`build/templates/`)

### `loader_template.c` — Main Skeleton

The central template file. Contains `@MODULE:xxx` markers that the Rust assembler replaces, and `@MUTATE:xxx` markers that the AST mutator (`build/src/transform/ast_mutator.rs`) transforms at build time. Execution flow:

```
main()
  → guardrail()          // Environment check (bail if unsafe)
  → antiemulation()      // Burn emulator resources
  → deconditioner()      // Normalize EDR behavioral baselines
  → decoy()              // Benign activity to mislead analysis
  → carrier()            // Alloc → decode → protect → execute
```

Each stage is instrumented with `ARTIFACT_CHECKPOINT()` calls for telemetry.

### `modules/header/definitions.h` — Shared Interface

Defines the contract every module must implement:

| Function | Signature | Purpose |
|----------|-----------|---------|
| `carrier()` | `int carrier(void)` | Memory setup + payload execution |
| `decode_payload()` | `FORCE_INLINE void decode_payload(char*, int)` | Decrypts payload in-place |
| `antiemulation()` | `void antiemulation(void)` | Emulator stress/detection |
| `deconditioner()` | `void deconditioner(void)` | EDR baseline normalization |
| `guardrail()` | `int guardrail(void)` | Environment validation |
| `decoy()` | `void decoy(void)` | Benign activity injection |
| `MyVirtualProtect()` | `FORCE_INLINE BOOL MyVirtualProtect(...)` | Hookable VirtualProtect wrapper |

Also defines memory protection constants (`p_RW`, `p_R`, `p_RX`, `p_RWX`), type aliases for dynamic API resolution, and conditional payload accessors.

### Module Categories

#### Carrier (`modules/carrier/`)

The most critical module — handles memory allocation, decoding, protection changes, and shellcode execution.

| Module | Strategy | IAT Footprint |
|--------|----------|---------------|
| `alloc_rw_rx` | `VirtualAlloc(RW)` → decode → `VirtualProtect(RX)` → execute | Normal (imports visible) |
| `change_rw_rx` | In-place on existing payload buffer → RW → decode → RX → execute | Smaller (no VirtualAlloc) |
| `peb_walk` | PEB walk resolves `VirtualAlloc`/`VirtualProtect` dynamically → no IAT entries | Zero (import-free) |

#### Decoder (`modules/decoder/`)

| Module | Technique | Notes |
|--------|-----------|-------|
| `xor` | 8-byte-wide rolling XOR (2-byte key broadcast) | Fast, paired with `PayloadEncoder::Xor` |
| `english` | Dictionary lookup (word → byte) | Low entropy, paired with `PayloadEncoder::English` |
| `subbyte` | 4-bit nibble reverse mapping | Controlled byte distribution |
| `none` | No-op (direct copy) | For testing |

#### Anti-Emulation (`modules/antiemulation/`)

| Module | Technique |
|--------|-----------|
| `none` | No-op |
| `sirallocalot` | Mass VirtualAlloc + VirtualProtect (exhaust emulator memory) |
| `timeraw` | RDTSC timing check (detect time acceleration) |
| `cpuburn` | CPU-intensive computation |
| `heapstress` | Heap allocation stress |
| `fsenum` | Filesystem enumeration (real system = many files) |
| `sleepaccel` | Sleep timing verification (detect Sleep patching) |

#### Deconditioner (`modules/deconditioner/`)

Research-driven: rehearse the carrier's `alloc→write→protect→free` pattern with benign data to overflow EDR behavioral counters before the real execution.

| Module | Technique |
|--------|-----------|
| `none` | No-op |
| `basic` | Simple alloc/write/protect/free loop |
| `alloc_loop` | Loop with all `@MUTATE` markers for full mutation coverage |
| `alloc_exec` | Includes execution from decoy memory |
| `thread_alloc` | Thread-based allocation pattern |
| `mixed_apis` | Mixes different allocation APIs |
| `entropy_flood` | High-entropy benign data fills |

#### Other Modules

| Category | Modules | Purpose |
|----------|---------|---------|
| `guardrails/` | `none`, `env` | Environment check (e.g., domain/user validation) |
| `virtualprotect/` | `standard`, `undersized` | VirtualProtect wrapper (undersized = smaller region argument) |
| `decoy/` | `none`, `winexec` | Benign activity (e.g., launch calc.exe) |

### `@MUTATE` Markers

Annotations in C source that mark AST mutation points. The `AstMutator` (`build/src/transform/ast_mutator.rs`) locates these markers via `extract_mutation_markers()` and applies corresponding transformations. Format:

```c
// @MUTATE:mutation_name
// @MUTATE:mutation_name(param1|param2|param3)
```

#### Implemented Marker-Based Mutations

These markers are handled by `AstMutator.apply_at_marker()` when a matching `MutationSpec` is provided:

| Marker | Mutation Spec | Description |
|--------|---------------|-------------|
| `decon_rounds` | `ast.decon_rounds` | Replace loop bound with fixed count or `GetTickCount() % N` |
| `fill_pattern` | `ast.fill_pattern` | Change benign fill data (xor, nop_sled, random, zero) |
| `exec_decoy` | `ast.exec_decoy` | Insert direct/threaded execution from decoy memory |
| `timing_pattern` | `ast.timing_pattern` | Insert `Sleep()` delays between operations |
| `protection_transition` | `ast.protection_transition` | Alter memory protection sequence (RW→RX, RW→RWX, RW→R→RX) |
| `benign_preamble` | `ast.benign_preamble` | Insert benign Windows API calls before sensitive operations |
| `api_sequence_obfuscation` | `ast.api_sequence_obfuscation` | Insert benign API calls between statements to dilute N-gram signatures |

#### Implemented Global Mutations (no markers needed)

These operate on all matching AST nodes across the entire source:

| Mutation Spec | Description |
|---------------|-------------|
| `ast.benign_syscall_insert` | Insert benign API call sequences between statements (dependency-aware, uses `benign_catalog.rs`) |
| `ast.const_obfuscation` | Decompose integer constants via volatile arithmetic expressions |
| `ast.string_xor` | XOR-encode string literals with runtime decode loop |

#### Not Yet Implemented Markers

These markers are present in module source code but are silently skipped by the AST mutator (logged as "Unimplemented AST mutation: ... (skipping)"):

| Marker | Intended Purpose |
|--------|------------------|
| `timing_jitter` | Insert random delays (distinct from `timing_pattern` which inserts `Sleep()` calls) |
| `opaque_predicate` | Add always-true/false conditions |
| `dead_code_insertion` | Add unreachable code paths |
| `api_wrapper_injection(API)` | Wrap API call in indirection layer |
| `getprocaddress_indirection(API)` | Resolve API dynamically at runtime |
| `literal_encoding` | Obfuscate numeric/string constants inline |
| `loop_mutation(fixed->GetTickCount_modulo)` | Randomize loop bounds |
| `loop_restructuring(for->while\|unroll)` | Change loop structure |
| `execution_method(direct\|callback\|fiber\|threadpool)` | Change shellcode invocation method |
| `string_splitting` | Break string literals into char arrays |
| `logic_mutation` | Replace memory writes with CPU math equivalents |
| `inline_assembly` | Rewrite functions in inline assembly |

#### Processing Order

The `Mutator` in `build/src/mutator/mod.rs` routes mutations in order:
1. **Marker-based AST mutations** — Applied bottom-up (reverse line order) to preserve line numbers
2. **Global `benign_syscall_insert`** — Before const/string obfuscation
3. **Global `const_obfuscation`** — Number literal nodes
4. **Global `string_xor`** — String literal nodes (runs last, since earlier mutations may introduce new strings)
5. **Strip markers** — All `@MUTATE` and module boundary comments removed before compilation

### Legacy Files

| File | Purpose |
|------|---------|
| `encoder.py` | Standalone Python encoder (XOR/English). Superseded by `payload.rs` |
| `Makefile` | Direct Clang compilation. Superseded by Rust `ArtifactBuilder` |
| `build.sh` | Shell build script. Superseded by Rust build pipeline |
| `payload.h` | Pre-generated payload header. Now generated dynamically |

---

## Assembly Flow (End-to-End)

```
1. Raw shellcode bytes
       │
       ├──[if instrumented]──→ shellcode_stub::prepend_checkpoint_stub()
       │                              │
       │                              ├──→ sc_checkpoints::patch_shellcode()
       │                              │         (INT3 breakpoints)
       │                              ▼
       ▼                        patched shellcode
2. PayloadEncoder.encode(shellcode, encoding_type)
       │
       ▼
3. PayloadEncoder.generate_c_header() → payload_code string
       │
       ▼
4. Assembler.assemble(&modules, &payload_code)
       │
       ├── Read loader_template.c
       ├── Replace @MODULE:payload    → payload_code
       ├── Replace @MODULE:definitions → definitions.h
       ├── Replace @MODULE:decoder    → decoder/{xor,english,...}.c
       ├── Replace @MODULE:carrier    → carrier/{alloc_rw_rx,...}.c
       ├── ... (all other modules)
       ▼
5. Single assembled .c file (ready for AST mutation → compilation)
```

---

## Key Design Decisions

1. **Modular gene model** — Each behavioral aspect (carrier, decoder, anti-emulation, etc.) is an independent "gene" that can be swapped without affecting others. This enables combinatorial exploration of artifact variants.

2. **Two marker systems** — `@MODULE` markers control structural composition (which code gets included). `@MUTATE` markers control fine-grained transformation within included code. This separation allows the assembler and mutator to operate independently.

3. **Boundary comments for scoped mutation** — `strip_markers_outside_targets()` enables targeted mutation: only transform markers inside specific modules while leaving the rest untouched. This is critical for the feedback loop — if the triage engine identifies the carrier as the detection cause, mutations can be scoped to carrier code only.

4. **FORCE_INLINE on sensitive functions** — `decode_payload` and `MyVirtualProtect` are force-inlined to merge into the carrier, defeating function-boundary analysis by EDRs.

5. **Recursive descent for checkpoint patching** — Uses control-flow-aware disassembly instead of linear sweep to avoid placing INT3 bytes on inline data (hashes, strings, config blobs) that would corrupt the shellcode.
