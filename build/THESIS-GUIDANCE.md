# Thesis Guidance — Build Crate Presentation

How to present the `build/` module in a Design chapter and an Implementation chapter, with required Background prerequisites.

---

## Background Prerequisites

These concepts should be introduced **before** the design/implementation chapters so the reader can follow the technical contributions.

### B1. PE File Format Essentials

- DOS header, PE signature, COFF header, Optional header, Section table
- Import Address Table (IAT) and how Windows resolves DLL imports at load time
- Rich header: undocumented MSVC build fingerprint (product IDs, build numbers, XOR-encoded)
- Resources section (VS_VERSIONINFO, manifests) and how security tools use them for trust signals
- **Why it matters:** Binary mutations directly manipulate these structures. The reader must understand what a Rich header *is* to appreciate why injecting a donor profile shifts ML classifiers.

### B2. LLVM Compilation Pipeline

- Source → AST → LLVM IR (`.ll` text) → object (`.o`) → linked PE (`.exe`)
- LLVM IR as a typed, SSA-form intermediate representation
- Basic blocks, `br` (branch) instructions, `phi` nodes
- SanitizerCoverage: compiler-inserted callbacks at basic-block boundaries (`__sanitizer_cov_trace_pc`)
- **Why it matters:** The three-layer mutation architecture operates at different stages of this pipeline. IR mutations exploit the fact that inline assembly is opaque to LLVM's optimizer.

### B3. EDR Detection Mechanisms (High-Level)

- Static analysis: PE metadata, section entropy, import table, string signatures
- Dynamic/behavioral analysis: API call sequences, memory protection transitions, timing patterns
- ML classifiers trained on feature vectors extracted from PE headers and runtime telemetry
- N-gram models: sequences of N consecutive API calls as behavioral fingerprints
- **Why it matters:** Each mutation family targets a specific detection mechanism. The design rationale is only meaningful if the reader understands *what* is being evaded and *why*.

### B4. Windows Internals for Shellcode Execution

- Virtual memory: `VirtualAlloc` (allocation), `VirtualProtect` (permission changes), RW/RX/RWX semantics
- Shellcode injection pattern: allocate → write → protect → execute
- PEB (Process Environment Block) walking for import-free API resolution
- Vectored Exception Handling (VEH) and INT3 breakpoint mechanics
- Direct syscalls: bypassing ntdll.dll usermode hooks by invoking `syscall` instruction directly
- **Why it matters:** The carrier module, checkpoint patching, and runtime exit all depend on these primitives. The deconditioner concept only makes sense if the reader understands that EDRs monitor this exact alloc→write→protect→execute sequence.

### B5. Tree-Sitter and AST-Based Program Transformation

- Concrete syntax trees (CST) vs abstract syntax trees (AST)
- Tree-sitter: incremental, error-tolerant parser used for structural code queries
- Byte-offset-based editing: transformations must preserve or compensate for position shifts
- **Why it matters:** The AST mutator's bottom-up (reverse line order) application strategy and the const_obfuscation two-pass approach are direct consequences of working with byte offsets in a CST.

---

## Design Chapter

Present the *architectural decisions* and their *rationale*. Focus on the "why" behind each design choice, connecting it to EDR evasion theory or experimental methodology requirements.

### D1. Three-Layer Mutation Architecture

**Core contribution.** Most prior work mutates at a single level (source or binary). This system applies transformations at three compilation stages, each targeting different detection surfaces.

| Layer | Representation | Survives Optimization | Target Detection |
|-------|---------------|----------------------|------------------|
| AST (C source) | Structural, readable | Partially (optimizer may simplify) | Behavioral signatures, API sequences |
| LLVM IR | SSA, typed | Yes (inline asm is opaque to optimizer) | Control-flow analysis, BB-level heuristics |
| Binary (PE) | Raw bytes | N/A (post-link) | Static ML features, PE metadata |

**Key design point:** Layers are *composable* — an artifact can receive mutations at all three layers in a single build. The ordering is fixed (AST → IR → Binary) because each layer operates on the output of the previous compilation stage.

**Highlight:** IR opaque predicates use inline assembly (`asm sideeffect "xor $0, $0"`) specifically because LLVM cannot constant-fold opaque assembly, making the mutations *optimization-resistant*. Contrast with the "trivial" mode (`icmp eq i32 0, 0`) that folds away at `-O2` — this provides a controlled experiment variable.

### D2. Modular Gene Model

**Core contribution.** The artifact is decomposed into independent behavioral "genes" (carrier, decoder, antiemulation, deconditioner, guardrail, virtualprotect, decoy). Each gene has multiple variants that can be selected independently.

**Design rationale:**
- Enables **combinatorial exploration** of variant space (3 carriers × 4 decoders × 7 antiemulation × ... = thousands of combinations)
- Each gene maps to a distinct behavioral aspect that EDRs may monitor independently
- Gene boundaries (`// --- BEGIN/END MODULE ---`) enable **scoped mutation**: when the triage engine identifies a specific gene as the detection cause, mutations can be restricted to that gene only

**Highlight:** The `@MODULE` / `@MUTATE` dual-marker system is a deliberate separation of concerns — structural composition (`@MODULE`) and fine-grained transformation (`@MUTATE`) operate independently. This allows the assembler and mutator to be developed and tested separately.

### D3. Deconditioner Concept

**Novel contribution.** The deconditioner module rehearses the carrier's `alloc→write→protect→free` pattern with *benign data* before the real payload execution.

**Design rationale:**
- EDR behavioral engines maintain internal counters/baselines for suspicious API patterns
- By flooding these counters with benign instances of the same pattern, the real execution blends into the statistical noise
- Variants explore different flooding strategies: simple loops, threaded allocation, mixed APIs, entropy-varied data

**Highlight:** This is a *behavioral-level mutation* that doesn't change the payload or the carrier — it changes the *statistical context* in which the carrier executes. This is worth discussing as a distinct evasion strategy from traditional code transformation.

### D4. Conditional Instrumentation via Weak Symbols

**Design contribution.** The same C source compiles to either an instrumented or baseline binary, controlled by a single preprocessor flag. The runtime uses weak symbol linkage to avoid conditional compilation of the runtime objects themselves.

**Design rationale for the Two-Run Differential Protocol:**
- Run A (instrumented): captures execution path, coverage, checkpoint timing
- Run B (baseline): ground-truth EDR behavior without instrumentation artifacts
- If Run A is detected but Run B is not → instrumentation caused the detection, not the payload
- This requires that baseline and instrumented binaries are *structurally identical except for instrumentation*

**Highlight:** The weak symbol pattern (`__attribute__((weak)) extern`) allows `minimal_runtime.o` to work in both modes without recompilation. When `instrumentation_runtime.o` is linked, its flush implementations win; when absent, the weak symbols resolve to NULL and are safely skipped. This elegance eliminates an entire class of build-mode divergence bugs.

### D5. Deterministic, Reproducible Builds

**Methodological contribution.** Every source of randomness is seeded and deterministic.

| Component | RNG | Seed |
|-----------|-----|------|
| IR mutations | glibc LCG | User-provided or fixed `1234` |
| Benign catalog scheduling | xorshift64 | User-provided or fixed `0xBE41` |
| Binary timestamp jitter | FNV-1a hash | Derived from artifact content |
| Payload encoding | Fixed keys | `[0xAA, 0x55]` or user-provided |
| Clang linker | `/Brepro` flag | Deterministic PE timestamps |

**Design rationale:** The differential protocol and the feedback loop both require that *the same mutation recipe always produces the same artifact*. Without reproducibility, you cannot attribute detection changes to specific mutations — you cannot establish causality.

### D6. Feedback-Loop-Ready Architecture

**Design contribution.** The build pipeline is structured for closed-loop mutation selection, even though the full loop is a separate system.

Key design choices that enable feedback:
- `MutationSpec` is layer-agnostic and serializable (id + params HashMap) — recipes can be stored, replayed, and compared
- `BuiltArtifact` captures full provenance (mutations applied, compiler flags, SHA256)
- `strip_markers_outside_targets()` enables mutation scoping — when triage identifies the carrier as the detection cause, only carrier markers are exposed
- Module boundary comments enable attribution — which gene contributed to detection

---

## Implementation Chapter

Present *how* each design decision was realized. Focus on non-obvious implementation challenges, clever solutions, and trade-offs.

### I1. Template Assembly — Text Substitution with Structural Awareness

**Implementation:** The assembler uses plain string replacement (`String::replace`) — no regex, no parser. Each `// @MODULE:xxx` marker is a unique string replaced exactly once.

**Why this is interesting:**
- Simplicity is deliberate — regex-based or AST-based assembly would be fragile against the variety of C constructs in modules
- The module cache (`HashMap<String, String>`) avoids redundant file I/O for repeated builds
- `definitions.h` include stripping prevents double-inclusion (the include is already in the template)
- The fixed replacement order (payload → definitions → decoder → ... → carrier) ensures that carrier code (which may reference decoder types) is inserted last

**Trade-off discussed:** No validation that module code is syntactically correct C — errors surface only at Clang compilation. This is acceptable because modules are hand-written and tested individually.

### I2. AST Mutations — Tree-Sitter with Byte-Offset Discipline

**Implementation:** The `AstMutator` uses tree-sitter to parse C into a CST, then applies transformations by byte-offset manipulation on the source string.

**Key implementation challenges:**

1. **Bottom-up application order:** Marker-based mutations are applied in reverse line order (highest line number first). This preserves byte offsets for subsequent mutations — insertions/replacements at line N don't shift the byte positions of markers at lines < N.

2. **Two-pass const_obfuscation:**
   - Pass 1: Replace integer literals inline with `(int)__obf_cN` (reverse byte order)
   - Pass 2: Insert `volatile` declarations before containing statements (line-based, forward order)
   - Separating passes avoids the problem where declaration insertion would shift the byte offsets of inline replacements in the same statement.

3. **string_xor runs last:** Because earlier mutations (benign_preamble, benign_syscall_insert) introduce new string literals, XOR encoding must run after all other mutations to catch them.

4. **Protection macro inlining:** `const_obfuscation` first expands `p_RW`/`p_RX`/`p_RWX` macros to their numeric values, because tree-sitter sees macros as identifiers, not number literals. Without this step, protection constants would be missed.

**Highlight for thesis:** The const_obfuscation filter chain is worth detailing — it skips floats, preprocessor directives, array sizes, case labels, initializer lists, global-scope constants, and already-obfuscated `__obf_c` declarations. Each filter prevents a specific class of compilation error that was discovered during development.

### I3. IR Mutations — Text-Based LLVM IR Manipulation

**Implementation:** The `IrMutator` operates on `.ll` text files with line-by-line processing — no LLVM C-API dependency.

**Why text-based:**
- LLVM's C-API changes between versions and requires linking against libLLVM
- `.ll` text format is stable and human-readable
- Line-by-line processing is sufficient for the three implemented mutations (nop insertion, opaque predicates, junk blocks)

**Key implementation detail — opaque predicates:**
```llvm
; Before: unconditional branch
br label %target

; After (robust mode): semantically identical, optimizer-resistant
%__op_N = call i32 asm sideeffect "xor $0, $0", "=r"()
%__op_cmp_N = icmp eq i32 %__op_N, 0
br i1 %__op_cmp_N, label %target, label %target
```

Both branch targets are identical — the predicate is always true (`xor reg, reg` = 0). But LLVM cannot prove this because inline assembly is opaque to the optimizer. The "trivial" mode (`icmp eq i32 0, 0`) serves as a control: it folds away at `-O2`, confirming that the robust mode's persistence is due to asm opacity.

**Highlight:** The deterministic LCG (`state * 1103515245 + 12345`, glibc constants) makes density-based insertion reproducible. Same seed → same mutation locations → same artifact. This is critical for the experimental methodology.

### I4. Binary Mutations — PE Byte-Level Manipulation

**Implementation:** `BinaryMutator` takes ownership of PE bytes and manipulates them with manual offset arithmetic. Validated with `goblin::pe::PE::parse()` after all transforms.

**Key implementation challenges:**

1. **Rich header injection:** The Rich header sits in the gap between the DOS stub (offset 0) and the PE signature (offset `e_lfanew`). Injection requires shifting all subsequent content and updating `e_lfanew`. The XOR-encoded format uses a checksum derived from both the DOS header bytes and the compiler records.

2. **Consolidated section pattern:** `string_inject`, `entropy_normalize`, `size_pad`, and `debug_dir` are merged into a single `.rdata` section instead of creating separate sections. This is because multiple non-standard section names (`.strings`, `.padding`, etc.) are themselves a detection signal — real MSVC binaries have a small, fixed set of section names.

3. **Import padding:** Existing imports are parsed with `goblin`, then a new `.idata` section is built containing both the original Import Directory Table and new benign entries. The `DataDirectory[1]` pointer is updated to the new section. Hint values in the import name table are approximate — the PE loader falls back to binary search, which works but is slower.

4. **PE checksum:** After all mutations, a proper PE checksum is computed using the standard 16-bit carry-fold algorithm (matching MSVC `link.exe` behavior). This is not a security feature but a compliance signal — Windows checks it for drivers and some security tools check it for all PEs.

**Highlight:** The `add_section()` primitive is the workhorse — it handles file alignment, section header insertion, `NumberOfSections` increment, and `SizeOfImage` update. All phase-1 mutations use it, and phase-2 mutations collect their data into a single buffer before one `add_section()` call.

### I5. Benign Catalog — Dependency-Aware API Insertion

**Implementation:** The `BehaviorGraph` implements a topological scheduler with randomized frontier selection.

**Why dependencies matter:**
```c
// FileIo group — order matters:
HANDLE h = CreateFileA("C:\\Windows\\explorer.exe", ...);  // ID 10
ReadFile(h, buf, sizeof(buf), &bytesRead, NULL);           // ID 11, depends on 10
CloseHandle(h);                                             // ID 12, depends on 11
```

Inserting `ReadFile` before `CreateFileA` would crash. The dependency graph ensures correct ordering while allowing randomized interleaving with other groups.

**Key implementation details:**
- Frontier is kept sorted for deterministic iteration order before random selection
- xorshift64 PRNG with user-provided seed enables reproducible insertion patterns
- Declarations are deduplicated by content (not by ID) using a `HashSet<String>`
- The `__be_` variable prefix prevents naming collisions with artifact code
- PEB-walk detection: if source contains `get_func_by_name`/`get_module_by_name`, the catalog restricts to SystemQuery only (FileIo/RegistryIo require imports that PEB-walk carriers don't have)

### I6. Shellcode Checkpoint Patching — Recursive Descent Disassembly

**Implementation:** `patch_shellcode()` uses `iced-x86` for recursive descent disassembly to find safe INT3 insertion points.

**Why recursive descent instead of linear sweep:**
- Linear sweep (decode instructions sequentially) misidentifies inline data after unconditional jumps as code
- Shellcode commonly embeds data inline: API name hashes, configuration blobs, encrypted strings
- Recursive descent follows control flow (branches, calls, conditionals), only visiting reachable code
- Data regions are never visited → never corrupted by INT3 insertion

**Key implementation detail:** The even-spacing formula `interval = max(1, (usable + 1) / (count + 1))` distributes checkpoints uniformly across the shellcode body. The entry point (first instruction) is never patched — it must execute to reach the checkpoint stub. The `progress_pct` field enables triage to report *how far* shellcode executed before EDR killed it.

### I7. Direct Syscall Exit — Bypassing Usermode Hooks

**Implementation:** `__runtime_exit()` resolves `NtTerminateProcess` syscall number at runtime from ntdll.dll stub bytes, then invokes `syscall` directly via a naked function.

**Why this matters for experimental validity:**
- RedEDR (and real EDRs) hook `NtTerminateProcess` via detours in ntdll.dll
- A hooked exit can deadlock, trigger post-mortem analysis, or corrupt telemetry timing
- Direct syscalls guarantee clean, deterministic process termination
- The telemetry flush (coverage bitmap, trace events, checkpoints) completes *before* the syscall — ensuring death-bed telemetry is captured

**Key implementation detail:** Syscall number extraction parses the `mov eax, NNN` instruction at the start of the ntdll stub for `NtTerminateProcess`. Falls back to known Win10/11 constants (`0x2C`) if parsing fails. The naked function `DirectSyscall2` contains only the `syscall` instruction — no prologue/epilogue that could be hooked.

### I8. Deferred Loop Tracing — O(1) per Unique Line

**Implementation:** The line tracer uses a flag-set-in-loop + check-after-loop pattern instead of emitting trace events inside loops.

**Problem:** Deconditioner loops run 20-100+ iterations. Naive per-iteration tracing produces millions of trace events, overwhelming the named pipe and making telemetry unusable.

**Solution:**
```c
static int __seen_L42 = 0;           // Before loop
for (int i = 0; i < DECON_ROUNDS; i++) {
    __seen_L42 = 1;                  // Flag set (not trace call)
    // ... loop body ...
}
if (__seen_L42) __trace_line_binary("file", 42, __func__);  // After loop
```

One trace event per unique line, regardless of iteration count. Exceptions: `return` and `goto` inside loops keep eager tracing because control exits the loop before the deferred check.

---

## Suggested Chapter Structure

### Background Chapter
- B3 → B4 → B1 → B2 → B5 (EDR context first, then the technical foundations)

### Design Chapter
- D2 (gene model — sets the conceptual framework)
- D1 (three-layer architecture — the core technical contribution)
- D3 (deconditioner — novel behavioral mutation)
- D6 (feedback-loop readiness — connects to the broader system)
- D5 (reproducibility — methodological rigor)
- D4 (conditional instrumentation — enables differential protocol)

### Implementation Chapter
- I1 (template assembly — entry point, easiest to follow)
- I2 (AST mutations — most complex, highest novelty)
- I4 (binary mutations — tangible PE-level results)
- I3 (IR mutations — opaque predicates, optimization resistance)
- I5 (benign catalog — dependency scheduling algorithm)
- I6 (shellcode checkpoints — recursive descent, safety argument)
- I7 (direct syscall — systems-level contribution)
- I8 (deferred loop tracing — performance optimization)

### What to Emphasize as Novel Contributions
1. **Three-layer composable mutation** — no prior work combines AST + IR + Binary in a single deterministic pipeline
2. **Deconditioner concept** — behavioral pre-flooding as a distinct evasion strategy
3. **Optimization-resistant IR mutations** — inline asm opacity as a deliberate design choice with trivial-mode control
4. **Dependency-aware benign API insertion** — topological scheduling with randomized interleaving for N-gram dilution
5. **Consolidated binary sections** — the insight that non-standard section names are themselves a detection signal
6. **Differential protocol support** — weak symbols enabling structurally identical baseline/instrumented builds
