# Triage Module — Deep Analysis

> **Scope:** `controller/src/triage/` — 10 files, ~5,721 lines of Rust
> **Generated from source code only** (no other `.md` referenced)

---

## 1. Overview

The `triage` module is the **intelligence layer** of AutoMutate++. It transforms raw execution results into actionable decisions about what mutations to try next. It owns:

- **Token extraction** — converting telemetry, modules, and mutations into normalized string tokens
- **Token scoring** — computing lift, confidence, and importance per token across rounds
- **Mutation selection** — 4 selector strategies that choose the next round's mutation recipe
- **Parameter exploration** — a registry of mutation parameter spaces with sampling, perturbation, and distance computation
- **Token comparison** — Jaccard distance and per-parameter distance for cross-round analysis
- **Source resolution** — mapping line trace data to C source code with function-level coverage

This module closes the **feedback loop**: execution results (detected/evaded) → token extraction → token scoring → mutation guidance → next mutation selection.

---

## 2. File Inventory

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | 202 | `Selector` trait, `Selection`/`SearchSpace`/`TriageGuidance` types, `SelectorType`/`VariationStrategy` enums, default mutation pools |
| `extractor.rs` | 635 | Token extraction from 3 sources (in-memory, ES telemetry, ES checkpoints) + combined `extract_and_score` background task |
| `scorer.rs` | 250 | Token lift/confidence/importance computation + avoid/seek guidance builder |
| `source_resolver.rs` | 555 | `SourceMap` for C source line resolution, function detection, coverage computation |
| `coverage_selector.rs` | 1,039 | Epsilon-greedy selector (v0) — module + mutation exploration using evasion scores |
| `fuzzer_selector.rs` | 891 | Evolutionary/genetic algorithm selector — tournament selection, crossover, mutation |
| `token_selector.rs` | 560 | Token-guided selector — biases module/mutation choices using avoid/seek guidance |
| `random_selector.rs` | 429 | Zero-intelligence baseline — uniform random selection for evaluation comparison |
| `token_diff.rs` | 307 | Token set comparison — set differences, Jaccard distance, mutation param distances |
| `param_space.rs` | 853 | Mutation parameter registry — `ParamDef`, `SeededRng`, sampling, perturbation, distance |

**Total: ~5,721 lines**

---

## 3. Per-File Deep Analysis

### 3.1 `mod.rs` — Core Types & Selector Trait (202 lines)

#### Purpose
Defines the shared abstractions used by all selectors and the triage pipeline. Everything in the module depends on these types.

#### Key Types

**`Selection`** — Output of every selector call:
```rust
pub struct Selection {
    pub modules: ModuleSelectionSpec,   // 7-slot module config for the build
    pub mutations: Vec<MutationSpec>,   // Ordered list of mutations to apply
    pub rationale: String,              // Human-readable explanation of choice
}
```

**`SelectorType`** — Which algorithm to use:
| Variant | Description |
|---------|------------|
| `Coverage` (default) | Epsilon-greedy over evasion scores |
| `Fuzzer` | Evolutionary genetic algorithm |
| `Token` | Token-guided (avoid/seek from triage) |
| `Random` | Uniform random (evaluation baseline) |

**`VariationStrategy`** — What axes the selector varies:
| Variant | Modules | Mutations |
|---------|---------|-----------|
| `MutationOnly` (default) | Fixed (defaults) | Varied per round |
| `Full` | Varied per round | Varied per round |

**`SearchSpace`** — Full configuration for mutation exploration:
```rust
pub struct SearchSpace {
    pub selector: SelectorType,
    pub strategy: VariationStrategy,
    pub variable_categories: Vec<String>,  // ["deconditioner"]
    pub mutation_pool: Vec<String>,        // AST mutations to explore
    pub mutation_targets: Vec<String>,     // Which modules mutations apply to
    pub fixed_mutations: Vec<String>,      // Always-applied mutations (binary.* + llvm.*)
    pub fuzzer_config: Option<FuzzerConfig>,
}
```

**Default mutation sets:**

| Set | Mutations | Purpose |
|-----|-----------|---------|
| `fixed_mutations` | `llvm.opaque_predicate`, `binary.rich_header`, `binary.import_pad`, `binary.resource_inject`, `binary.section_rename`, `binary.entropy_normalize`, `binary.string_inject`, `binary.size_pad`, `binary.debug_dir`, `binary.timestamp` | Always applied (PE normalization + LLVM obfuscation) |
| `mutation_pool` | `ast.decon_rounds`, `ast.fill_pattern`, `ast.exec_decoy`, `ast.timing_pattern`, `ast.protection_transition`, `ast.const_obfuscation`, `ast.string_xor`, `ast.benign_syscall_insert`, `ast.benign_preamble`, `ast.api_sequence_obfuscation` | One explored per round (AST-level behavioral changes) |

**`TriageGuidance`** — Output of the scoring pipeline, input to TokenSelector:
```rust
pub struct TriageGuidance {
    pub avoid_tokens: Vec<String>,  // Tokens correlated with detection (high lift)
    pub seek_tokens: Vec<String>,   // Tokens correlated with evasion (low lift)
}
```

**`Selector` trait:**
```rust
#[async_trait]
pub trait Selector: Send + Sync {
    async fn select(
        &self,
        job_id: &str,
        round_number: u32,
        search_space: &SearchSpace,
        default_modules: &ModuleSelectionSpec,
        history: &BTreeMap<u32, RoundSummary>,
        guidance: Option<&TriageGuidance>,
    ) -> Selection;
}
```
Async to allow ES queries (TokenSelector may query token history). `history` is the in-memory `BTreeMap<round_number, RoundSummary>` from `JobSession`.

---

### 3.2 `extractor.rs` — Token Extraction (635 lines)

#### Purpose
Converts execution data into normalized string tokens from three sources, combines them, indexes to ES, scores them, and produces `TriageGuidance` for the selector.

#### Token Sources

**Source 1: In-memory (`extract_round_tokens`)** — Pure, no IO
- 7 module tokens: `module:carrier=alloc_rw_rx`, `module:decoder=xor`, etc.
- N mutation tokens via `format_mutation_token()`:
  - No params: `mutation:binary.rich_header`
  - With params (sorted keys): `mutation:ast.decon_rounds:count=50:method=fixed`

**Source 2: ES telemetry (`extract_telemetry_tokens`)** — Async
Queries `storage.query_api_telemetry(run_id)` for all non-trace events, then extracts via `extract_tokens_from_docs()`:

| Event Category | Token Types | Example |
|---------------|------------|---------|
| dll (syscall hooks) | `api:<func>` | `api:NtAllocateVirtualMemory` |
| dll (syscall hooks) | `api_arg:<func>:protect=<val>` | `api_arg:NtProtectVirtualMemory:protect=R-X` |
| dll (consecutive calls) | `seq2:<prev>→<func>` | `seq2:NtAllocateVirtualMemory→NtProtectVirtualMemory` |
| kernel (image_load) | `image:<basename>` | `image:ntdll.dll` |
| etw | `etw:<provider>/<event_id>` | `etw:Microsoft-Windows-Kernel-Process/6` |
| etw | `etw_event:<event_name>` | `etw_event:ProcessStopStop` |

Key implementation details:
- `seq2:` bigrams track `prev_func` state across consecutive `payload_func` values — ETW events (no `payload_func`) don't reset the chain
- `image:` tokens extract the DLL basename via `rsplit('\\')` and lowercase it
- All tokens are deduplicated via `HashSet` in insertion order

**Source 3: ES checkpoints (`extract_checkpoint_tokens`)** — Async
Queries `storage.query_checkpoint_events(run_id)` for instrumented run checkpoints:
- `checkpoint:<name>` (lowercased), e.g., `checkpoint:antiemulation_passed`

#### Combined Pipeline (`extract_and_score`)

This is the **top-level background task**, spawned by `finalize_round()` in the JobWorker:

```
1. extract_round_tokens(summary)              → module + mutation tokens
2. extract_telemetry_tokens(baseline_run_id)   → api + seq2 + etw + image tokens
3. extract_checkpoint_tokens(instrumented_run) → checkpoint tokens
4. Combine all → index to tokens-YYYY.MM
5. Build token-round matrix from all prior summaries (filter untrustworthy)
6. scorer::compute_token_scores() → lift/confidence/importance
7. scorer::build_guidance() → TriageGuidance (avoid/seek)
```

Error handling: Each extraction step logs warnings on failure but continues with partial data. The overall function returns `Result<TriageGuidance>`.

#### Tests (12 unit tests)
Cover: mutation token formatting with sorted params, empty params, bool/number values, telemetry token extraction from mock RedEDR docs (dll/kernel/etw), deduplication, edge cases.

---

### 3.3 `scorer.rs` — Token Scoring (250 lines)

#### Purpose
Pure computation — no IO. Takes a token-round matrix and computes statistical scores that classify tokens as detection-correlated (avoid) or evasion-correlated (seek).

#### Data Model

```rust
pub struct TokenScore {
    pub token: String,
    pub lift: f64,         // P(detected|T) / P(detected)
    pub confidence: f64,   // min(1.0, n_total / 5.0)
    pub importance: f64,   // lift * confidence
    pub n_detected: u32,
    pub n_total: u32,
}
```

#### Algorithm: `compute_token_scores`

Input: `&[(Vec<String>, bool)]` — (tokens, detected) per trustworthy round.

1. Compute base detection rate: `P(detected) = total_detected / total_rounds`
2. **Guard**: if `P(detected)` is 0.0 or 1.0, lift is degenerate → return empty (can't distinguish)
3. Count per-token: `(n_total, n_detected)` across all rounds
4. For each token:
   - `P(detected|T) = n_detected / n_total`
   - `lift = P(detected|T) / P(detected)`
   - `confidence = min(1.0, n_total / 5.0)` — ramps from 0.2 (1 observation) to 1.0 (5+ observations)
   - `importance = lift * confidence`
5. Sort by importance descending

#### Algorithm: `build_guidance`

Input: Scored tokens + `lift_threshold` (default 1.5) + `min_confidence` (default 0.3)

- **Avoid tokens**: `lift > 1.5` AND `confidence > 0.3` (strongly detection-correlated)
- **Seek tokens**: `lift < 1/1.5 = 0.667` AND `confidence > 0.3` (strongly evasion-correlated)
- Both lists capped at 50 tokens

#### Example
```
Token "api_arg:VirtualProtect:RWX":
  P(detected) = 0.5, P(detected|T) = 1.0
  lift = 2.0, confidence = 0.8 (4 observations)
  importance = 1.6 → AVOID

Token "module:deconditioner=entropy_flood":
  P(detected) = 0.5, P(detected|T) = 0.0
  lift = 0.0, confidence = 0.4 (2 observations)
  importance = 0.0 → SEEK
```

#### Tests (5 unit tests)
Cover: high-lift detection-correlated token, seek-only evasion token, confidence scaling, threshold filtering, all-detected degenerate case.

---

### 3.4 `source_resolver.rs` — Source Code Resolution (555 lines)

#### Purpose
Maps trace line numbers to C source code and computes per-function coverage. Used by the Orchestrator's `compute_round_coverage` to produce `CoverageResult` from trace data.

#### Key Types

```rust
pub struct SourceMap {
    lines: Vec<String>,
    func_ranges: Vec<FuncRange>,
}

pub struct CoverageResult {
    pub total_lines: usize,
    pub total_executable: usize,
    pub executed_lines: usize,
    pub coverage_percent: f64,
    pub cutoff_line: Option<usize>,      // Highest executed line
    pub cutoff_func: Option<String>,     // Function containing cutoff
    pub functions: Vec<FunctionCoverage>,
}
```

#### SourceMap Construction

`SourceMap::new(source)` parses the assembled C source text and detects function boundaries:

1. Split source into lines
2. `detect_functions()` scans for C function signatures using heuristics:
   - Looks for patterns like `type name(params)` (extracts the identifier before `(`)
   - Rejects keywords (`if`, `for`, `while`, `return`, `sizeof`), typedefs, declarations ending with `;`
   - Tracks brace depth `{` / `}` to find function body boundaries
   - Handles opening brace on same line or next few lines
3. Each function stored as `FuncRange { name, start_line, end_line }` (1-based)

#### Line Resolution

- `resolve(line)` → `ResolvedLine { line, code, func }` — maps a 1-based line number to trimmed source code + enclosing function name
- `function_at(line)` — linear scan of `func_ranges` to find containing function

#### Coverage Computation

`compute_coverage(executed: &HashSet<usize>)`:

1. `cutoff_line` = max of all executed line numbers (identifies where execution stopped)
2. Per-function: count executable lines (skip signature line, blank, preprocessor, braces, comments) and intersection with executed set
3. Global coverage = sum of per-function stats (avoids inflating denominator with non-function lines)
4. Returns `CoverageResult` with per-function breakdown

#### Line Executability

`is_executable_line()` returns false for:
- Blank lines, `{`, `}`
- Preprocessor directives (`#include`, `#define`)
- Comments (`//`, `/*`, `*`)

#### Tests (12 unit tests)
Cover: line resolution, out-of-bounds, function detection (multiple functions, nested braces), coverage computation, executable line classification, function signature parsing edge cases.

---

### 3.5 `coverage_selector.rs` — Coverage-Driven Selector (1,039 lines)

#### Purpose
The **default selector** (v0). Epsilon-greedy exploration over module variants and mutations using evasion scores from round history. Pure logic — no ES dependency.

#### Constants

- `EPSILON = 0.3` — exploration rate
- `DECONDITIONER_VARIANTS`: `["none", "alloc_loop", "alloc_exec", "thread_alloc", "mixed_apis", "entropy_flood"]`
- `MUTATION_CATALOG`: 22 mutations across AST (10), LLVM IR (3), Binary (9) layers

#### Algorithm

**Round 1**: Always returns defaults (baseline control measurement — no mutations, default modules).

**Round 2+ (MutationOnly strategy)**:

Two-tier mutation selection:
1. **Fixed mutations** — all `search_space.fixed_mutations` are included with sampled params
2. **Explored mutation** — one from `mutation_pool` via epsilon-greedy:
   - Collect per-mutation stats from trustworthy history (only rounds with exactly 1 explored mutation)
   - **Untried phase**: explore individual mutations not yet tried
   - **Epsilon-greedy phase** (ε=0.3): 70% exploit best mean evasion score, 30% random

**Round 2+ (Full strategy)**: Both modules AND mutations varied.

Module selection (`select_modules` — shared by FuzzerSelector):
1. Collect per-variant `VariantStats { count, total_evasion_score }` from trustworthy history
2. **Untried phase**: explore variants not yet tried (random from untried set)
3. **Epsilon-greedy** (ε=0.3): 70% exploit best `mean_evasion_score()`, 30% random

**Backward compatibility**: If both `fixed_mutations` and `mutation_pool` are empty, falls back to full-catalog single-mutation exploration.

#### Trustworthiness Filter
Only rounds where `differential_category.is_trustworthy()` returns true are used for statistics. This excludes `InstrumentationArtifact` and `Flaky` rounds — preventing noise from corrupting learning.

#### Tests (12 unit tests)
Cover: round 1 baseline, default strategy, fixed+pool configuration, backward compatibility, custom pool, full mode, untried exploration, trustworthy filtering, exploitation bias, no variable categories.

---

### 3.6 `fuzzer_selector.rs` — Evolutionary Selector (891 lines)

#### Purpose
Genetic algorithm that treats mutation recipes as **evolving chromosomes**. Explores parameter spaces, mutation combinations, and learns from fitness (evasion score) via evolution.

#### Configuration (`FuzzerConfig`)

| Parameter | Default | Description |
|-----------|---------|-------------|
| `population_size` | 10 | Recipes per generation |
| `elitism` | 2 | Elite recipes kept across generations |
| `param_mutation_rate` | 0.3 | Probability of parameter perturbation |
| `structural_mutation_rate` | 0.2 | Probability of adding/removing a mutation |
| `min_pool_mutations` | 1 | Minimum mutations from pool per recipe |
| `max_pool_mutations` | 5 | Maximum mutations from pool per recipe |
| `vary_fixed_params` | true | Whether to vary params on fixed mutations |

#### Internal Data Model

```rust
struct Recipe {
    mutations: Vec<MutationSpec>,
    fitness: Option<f64>,   // evasion_score from history
    generation: u32,
    rationale: String,
}
```

#### Algorithm

**Phase 1: Seeding** (rounds 2 through `population_size + 1`):
- Generate random recipes via `random_recipe()`:
  1. Include all fixed mutations (optionally with random params)
  2. Pick random subset (min..max) from mutation pool via Fisher-Yates shuffle
  3. Sample params from `param_space::default_registry()`

**Phase 2: Evolution** (rounds > `population_size + 1`):
- `evolve_recipe()`:
  1. Reconstruct population from trustworthy history (map each round to a `Recipe` with its evasion_score as fitness)
  2. If population < 2, fall back to random
  3. Sort by fitness descending
  4. **Tournament selection**: pick 2 random candidates, keep fitter one (×2 for parent_a, parent_b)
  5. **Crossover**: fixed mutations always included (params from random parent), pool mutations = union of both parents
  6. **Parameter mutation**: with probability `param_mutation_rate`, perturb one random mutation's params (intensity=0.2)
  7. **Structural mutation**: with probability `structural_mutation_rate`, add or remove a pool mutation

**Deterministic**: Uses `SeededRng::new(job_id, round_number)` — same inputs → same recipe.

**Module selection**: Reuses `coverage_selector::select_modules()` for deconditioner selection.

#### Tests (9 unit tests)
Cover: round 1 baseline, seeding phase diversity, fixed mutation inclusion, param variation, high-fitness evolution bias, param bounds, crossover merging, elitism preservation, structural mutation, determinism.

---

### 3.7 `token_selector.rs` — Token-Guided Selector (560 lines)

#### Purpose
Uses `TriageGuidance` (avoid/seek token sets) from the scoring pipeline to bias mutation and module selection. Falls back to `CoverageSelector` when guidance is not yet available.

#### Algorithm

**Round 1**: Always baseline (same as all selectors).

**No guidance available**: Delegates to `CoverageSelector` (graceful degradation for first rounds before triage completes).

**With guidance (MutationOnly)**:
- `token_guided_mutations()`:
  1. Include all fixed mutations with sampled params
  2. Score each pool mutation by combining:
     - Historical mean evasion score (from trustworthy rounds with exactly 1 explored mutation)
     - Token penalty: if `mutation:<id>` is in avoid_tokens → score −0.5
     - Token bonus: if `mutation:<id>` is in seek_tokens → score +0.3
     - Novelty bonus: untried mutations → score +0.4
  3. Sort by score descending
  4. Epsilon-greedy (ε=0.3): 70% pick best, 30% random

**With guidance (Full)**:
- `token_guided_modules()`:
  1. Score each deconditioner variant by:
     - Historical mean evasion score
     - Token penalty/bonus from guidance (same ±0.5/+0.3 scheme)
     - Novelty bonus for untried: +0.4
  2. Epsilon-greedy (ε=0.3)
- Plus `token_guided_mutations()` as above

#### Key Difference from CoverageSelector
CoverageSelector only uses evasion scores. TokenSelector additionally uses **token-level signals** from the scoring pipeline, enabling it to:
- Avoid mutations whose tokens correlate with detection (even if the evasion score was moderate)
- Seek mutations whose tokens correlate with evasion (even if untried)

#### Tests (4 unit tests)
Cover: no-guidance delegation, avoid high-lift module, seek low-lift mutation, round 1 baseline.

---

### 3.8 `random_selector.rs` — Evaluation Baseline (429 lines)

#### Purpose
Zero-intelligence baseline for controlled evaluation. Uniformly samples mutations and modules with no learning from history or guidance. Enables measuring how much the other selectors improve over random.

#### Algorithm

**Round 1**: Baseline (same as all selectors).

**Round 2+**:
1. Include all fixed mutations with seeded params
2. Pick random subset (1–3) from mutation pool via Fisher-Yates partial shuffle
3. If `Full` strategy: random deconditioner from `DECONDITIONER_VARIANTS`

**Key properties:**
- **Deterministic**: `SeededRng::new(job_id, round_number)` — reproducible
- **Ignores history**: `_history` parameter unused
- **Ignores guidance**: `_guidance` parameter unused

#### Tests (7 unit tests)
Cover: round 1 baseline, mutation production, determinism, history independence, full strategy module variation, mutation-only module fixedness, different rounds produce different selections.

---

### 3.9 `token_diff.rs` — Token Set Comparison (307 lines)

#### Purpose
Pure functions for comparing two token sets. Used by the `CompareTokens` gRPC handler to analyze differences between rounds.

#### Key Types

```rust
pub struct TokenSetComparison {
    pub only_in_a: Vec<String>,
    pub only_in_b: Vec<String>,
    pub common: Vec<String>,
    pub mutation_comparisons: Vec<MutationTokenComparison>,
    pub jaccard_distance: f64,   // 1 - |A ∩ B| / |A ∪ B|
    pub count_a: usize,
    pub count_b: usize,
}
```

#### Algorithm: `compare_token_sets`

1. **Set operations**: compute `only_in_a`, `only_in_b`, `common` via `HashSet` difference/intersection
2. **Jaccard distance**: `1 - |intersection| / |union|` (0.0 = identical, 1.0 = disjoint)
3. **Mutation token parsing**: extracts `mutation:id:k=v` tokens from both sets, groups by `mutation_id`
4. **Per-mutation comparison**:
   - Both present: compute param distance via `param_space::MutationParamSpace::compare_params()`
   - Only in one set: distance = 1.0
5. Sort mutation comparisons by ID for stable output

#### Mutation Token Parsing

`parse_mutation_token("mutation:ast.decon_rounds:count=20:method=fixed")` →
```rust
ParsedMutationToken {
    mutation_id: "ast.decon_rounds",
    params: { "count": "20", "method": "fixed" },
    raw_token: "mutation:ast.decon_rounds:count=20:method=fixed"
}
```

#### Tests (7 unit tests)
Cover: token parsing (with/without params, non-mutation tokens), basic comparison, identical sets, disjoint sets, mutation comparison with registry, only-in-one mutation, empty sets.

---

### 3.10 `param_space.rs` — Mutation Parameter Registry (853 lines)

#### Purpose
Defines the parameter space for every mutation in the catalog. Provides sampling, perturbation, and distance computation used by all selectors and token comparison.

#### Parameter Definitions (`ParamDef`)

| Variant | Fields | Example |
|---------|--------|---------|
| `Categorical` | name, options, default | `method ∈ {"fixed", "runtime"}`, default: "fixed" |
| `IntRange` | name, min, max, default | `count ∈ [5, 500]`, default: 20 |
| `FloatRange` | name, min, max, default | `density ∈ [0.0, 1.0]`, default: 0.3 |

#### Operations per `ParamDef`

| Operation | Categorical | IntRange | FloatRange |
|-----------|------------|----------|-----------|
| `sample(rng)` | Random option | `min + rng % range` | `min + rng * (max-min)` |
| `perturb(current, rng, intensity)` | Random different option | `current ± intensity*range`, clamped | Same, clamped |
| `distance(a, b)` | 0.0 if equal, 1.0 if different | `|a-b|/(max-min)`, clamped [0,1] | Same |

#### Mutation Parameter Space (`MutationParamSpace`)

Groups `ParamDef` entries per mutation. Provides:
- `sample_params(rng)` → `Option<Value>` (JSON object)
- `compare_params(a, b)` → `MutationComparison` with per-param distances and overall mean distance
- `perturb_params(current, rng, intensity)` → `Option<Value>` (perturbed JSON object)

#### Default Registry (`default_registry`)

22 mutations registered with full parameter spaces:

| Mutation | Parameters |
|----------|-----------|
| `ast.decon_rounds` | count: [5, 500], method: {fixed, runtime} |
| `ast.fill_pattern` | pattern: {xor, nop_sled, random, zero} |
| `ast.exec_decoy` | method: {none, direct, thread} |
| `ast.timing_pattern` | min_ms: [0, 500], max_ms: [10, 2000] |
| `ast.protection_transition` | pattern: {rw_rx, rw_rwx, rw_r_rx} |
| `ast.string_xor` | xor_key: [1, 255] |
| `ast.const_obfuscation` | min_value: [2, 2] |
| `ast.benign_syscall_insert` | groups: {4 options}, count: [1, 9], density: [0.1, 1.0], target_fn: {carrier, deconditioner, antiemulation, guardrail} |
| `ast.benign_preamble` | count: [1, 3] |
| `ast.api_sequence_obfuscation` | count: [1, 3] |
| `llvm.nop_insert` | density: [0.0, 1.0] |
| `llvm.opaque_predicate` | density: [0.0, 1.0], mode: {robust} |
| `llvm.junk_block` | count: [1, 10] |
| `binary.rich_header` | donor: {notepad, calc, explorer} |
| `binary.import_pad` | count: [5, 100] |
| `binary.string_inject` | count: [5, 50] |
| `binary.size_pad` | target_kb: [64, 1024] |
| `binary.entropy_normalize` | target: [4.0, 7.5] |
| `binary.timestamp` | age_days: [30, 1825] |
| `binary.resource_inject` | (no params) |
| `binary.section_rename` | (no params) |
| `binary.debug_dir` | (no params) |

#### Seeded RNG (`SeededRng`)

Deterministic xorshift64 PRNG:
- `SeededRng::new(job_id, round_number)` — FNV-1a hash of job_id bytes + round_number
- `next_u64()` — xorshift64 step
- `next_usize(n)` — `[0, n)` via modulo
- `next_f64()` — `[0.0, 1.0)` via 53-bit extraction
- `coin(p)` — Bernoulli trial with probability `p`

Used by FuzzerSelector, TokenSelector, and RandomSelector for reproducible evolution.

#### Tests (15 unit tests)
Cover: registry completeness (all 22 mutations), sample ranges, categorical perturbation, numeric perturbation bounds, RNG determinism/divergence, param space sampling, empty params, distance computation (int/float/categorical/zero-range), param comparison (mixed, missing keys with defaults).

---

## 4. Architecture

### 4.1 Component Dependency Graph

```
                    ┌──────────────────────────────────────────┐
                    │           mod.rs                          │
                    │  Selector trait, Selection, SearchSpace,  │
                    │  SelectorType, VariationStrategy,         │
                    │  TriageGuidance                           │
                    └──────────────┬───────────────────────────┘
                                   │ used by all
         ┌──────────────┬──────────┼───────────┬──────────────────┐
         ▼              ▼          ▼           ▼                  ▼
  ┌─────────────┐ ┌───────────┐ ┌──────────┐ ┌──────────────┐ ┌───────────┐
  │ coverage_   │ │ fuzzer_   │ │ token_   │ │ random_      │ │ extractor │
  │ selector    │ │ selector  │ │ selector │ │ selector     │ │ .rs       │
  │             │ │           │ │          │ │              │ │           │
  │ ε-greedy    │ │ genetic   │ │ token-   │ │ uniform      │ │ token     │
  │ evasion     │ │ algorithm │ │ guided   │ │ random       │ │ extract + │
  │ scores      │ │ evolving  │ │ avoid/   │ │ baseline     │ │ score     │
  │             │ │ recipes   │ │ seek     │ │              │ │           │
  └──────┬──────┘ └─────┬─────┘ └─────┬────┘ └──────┬───────┘ └─────┬─────┘
         │              │             │              │               │
         │   ┌──────────┤             │              │               │
         │   │ reuses   │             │ falls back   │               │
         │   │ select_  │◄────────────┘              │               │
         │   │ modules  │                            │               │
         └───┤          │                            │               ▼
             └──────────┘                            │         ┌───────────┐
                   │                                 │         │ scorer.rs │
                   ▼                                 ▼         │           │
            ┌──────────────┐                  ┌──────────────┐ │ lift,     │
            │ param_space  │◄─────────────────│ param_space  │ │ confidence│
            │ .rs          │                  │ .rs          │ │ guidance  │
            │              │                  │              │ └─────┬─────┘
            │ ParamDef,    │                  │ SeededRng    │       │
            │ registry,    │                  │ sampling     │       │
            │ sample,      │                  │ determinism  │       │
            │ perturb,     │                  │              │       │
            │ distance     │                  └──────────────┘       │
            └──────┬───────┘                                        │
                   │                                                │
                   ▼                                                │
            ┌──────────────┐                  ┌──────────────┐      │
            │ token_diff   │◄── used by ──────│ API handler  │      │
            │ .rs          │    compare_tokens │ (job.rs)     │      │
            │              │                  └──────────────┘      │
            │ Jaccard,     │                                        │
            │ param dist,  │                  ┌──────────────┐      │
            │ set diff     │                  │ source_      │◄─────┘
            └──────────────┘                  │ resolver.rs  │  (coverage
                                              │              │   used by
                                              │ SourceMap,   │   orchestrator)
                                              │ coverage     │
                                              └──────────────┘
```

### 4.2 Selector Comparison

| Property | Coverage | Fuzzer | Token | Random |
|----------|----------|--------|-------|--------|
| **Learning** | Evasion scores | Evasion scores via evolution | Evasion scores + token signals | None |
| **Exploration** | Epsilon-greedy (ε=0.3) | GA: tournament + crossover + mutation | Token-scored epsilon-greedy | Uniform random |
| **Exploitation** | Best mean evasion | Fitness-proportionate via tournament | Best combined score (evasion + token bias) | N/A |
| **Pool mutations per round** | 1 (individual) | 1–5 (variable via GA) | 1 (individual) | 1–3 (random subset) |
| **Parameter variation** | Sampled fresh each round | Inherited + perturbed via GA | Sampled fresh each round | Sampled from seed |
| **Determinism** | Pseudo (subsec_nanos) | Full (SeededRng) | Pseudo (subsec_nanos) | Full (SeededRng) |
| **History dependency** | Yes (filters untrustworthy) | Yes (reconstructs population) | Yes + guidance | No |
| **ES dependency** | No | No | No (guidance arrives via channel) | No |

### 4.3 Data Flow: Feedback Loop

```
                          ┌─────────────────────────┐
                          │    Selector.select()     │
                          │ (CoverageSelector or     │
                          │  FuzzerSelector or       │
               ┌─────────│  TokenSelector or        │
               │          │  RandomSelector)         │
               │          └────────────┬────────────┘
               │                       │
               │ TriageGuidance        │ Selection (modules + mutations)
               │ (avoid/seek tokens)   │
               │                       ▼
        ┌──────┴──────┐         ┌──────────────┐
        │  scorer.rs  │         │  JobWorker   │
        │             │         │ produce_round│
        │ compute_    │         └──────┬───────┘
        │ token_      │                │
        │ scores() +  │                │ build + deploy + execute
        │ build_      │                │
        │ guidance()  │                ▼
        └──────┬──────┘         ┌──────────────┐
               ▲                │  VM execution │
               │                │  (worker)     │
        ┌──────┴──────┐         └──────┬───────┘
        │ extractor   │                │
        │ .rs         │                │ RunOutcome + telemetry
        │             │                │
        │ extract_    │                ▼
        │ and_score() │◄────────┌──────────────┐
        │             │         │ finalize_    │
        └─────────────┘         │ round()      │
                                │ (JobWorker)  │
                                └──────────────┘
```

### 4.4 Token Lifecycle

```
1. CREATION (extractor.rs):
   Raw data → normalized tokens → indexed to tokens-YYYY.MM

2. SCORING (scorer.rs):
   Per-round token sets + detection outcomes → lift/confidence per token

3. GUIDANCE (scorer.rs → TriageGuidance):
   High-lift tokens → avoid_tokens
   Low-lift tokens  → seek_tokens

4. CONSUMPTION (token_selector.rs):
   Guidance biases mutation/module scoring → next round's Selection

5. COMPARISON (token_diff.rs):
   Two token sets → Jaccard distance + per-mutation param distances
   (used by CompareTokens API for UI analysis)
```

---

## 5. Concurrency Model

### 5.1 Selector Invocation
Selectors are invoked synchronously within the `JobWorker::produce_round()` flow. They are `Send + Sync` (required by the `Selector` trait), enabling them to be shared across job worker tasks via `Arc<dyn Selector>`.

### 5.2 Async Triage Pipeline
`extract_and_score()` runs in a **background `tokio::spawn`** task (non-blocking). The result (`TriageGuidance`) is sent to the `JobWorker` via an `mpsc` channel. The `TokenSelector` reads guidance when available; other selectors ignore it.

### 5.3 Determinism
- **FuzzerSelector** and **RandomSelector** use `SeededRng::new(job_id, round_number)` — fully deterministic given the same inputs
- **CoverageSelector** uses `SystemTime::now().subsec_nanos()` — pseudo-random, NOT deterministic (acceptable for epsilon-greedy exploration)
- **TokenSelector** uses `SystemTime::now().subsec_nanos()` — same as CoverageSelector

### 5.4 Stateless Design
All four selectors are **stateless structs** (zero-size types). All state comes via function arguments:
- `history: &BTreeMap<u32, RoundSummary>` — from `JobSession`
- `guidance: Option<&TriageGuidance>` — from background triage task
- `search_space: &SearchSpace` — from job configuration

This makes them trivially testable and eliminates mutation of shared state.

---

## 6. Role in the Global Project

### 6.1 Position in Architecture

The triage module sits between the **dispatch layer** (produces execution results) and the **build layer** (consumes mutation recipes):

```
Dispatch (RoundSummary, telemetry) → Triage (analyze + select) → Build (next artifact)
```

It is the **decision-making brain** of the experimental loop — without it, mutation selection is random.

### 6.2 Scientific Contribution

The triage module implements the project's core scientific claims:

1. **Feature-level causality**: Tokens provide a vocabulary for *why* detections happen (which API calls, which protection arguments, which sequences triggered EDR rules)

2. **Evidence-driven mutation**: Token scoring (lift × confidence) replaces guesswork with statistical evidence about which behaviors matter

3. **Controlled experimentation**: The `Selector` trait enables comparing strategies (Coverage vs Fuzzer vs Token vs Random) under identical conditions

4. **Reproducibility**: Seeded RNG in FuzzerSelector and RandomSelector enables exact reproduction of any experimental sequence

### 6.3 Integration Points

| Component | Interface | Direction |
|-----------|-----------|-----------|
| **JobWorker** | `Selector::select()` | Triage → Dispatch (mutation choice) |
| **JobWorker** | `extract_and_score()` via `tokio::spawn` | Dispatch → Triage (background scoring) |
| **JobWorker** | `TriageGuidance` via `mpsc` channel | Triage → Dispatch (guidance delivery) |
| **Orchestrator** | `SourceMap::compute_coverage()` | Triage → Dispatch (coverage result) |
| **Storage** | `query_api_telemetry()`, `query_checkpoint_events()` | Storage → Triage (telemetry data) |
| **Storage** | `index_token_set()` | Triage → Storage (token persistence) |
| **API** | `CompareTokens` → `token_diff::compare_token_sets()` | Triage → API (analysis queries) |
| **API** | `GetRound` → round correction logic | Triage types used in API response |
| **Orchestrator** | `SelectorType::from_str_or_default()` | API → Triage (selector injection at job spawn) |

---

## 7. Summary Statistics

| Metric | Value |
|--------|-------|
| Total files | 10 |
| Total lines | ~5,721 |
| Selector implementations | 4 (Coverage, Fuzzer, Token, Random) |
| Token categories | 9 (module, mutation, api, api_arg, seq2, image, etw, etw_event, checkpoint) |
| Mutations in catalog | 22 (10 AST + 3 LLVM + 9 Binary) |
| Parameter spaces registered | 22 |
| Mutation pool (default explored) | 10 AST mutations |
| Fixed mutations (always applied) | 10 (1 LLVM + 9 Binary) |
| Unit tests | ~66 |
| Epsilon rate | 0.3 (30% exploration, 70% exploitation) |
| Confidence threshold | 5 observations for full confidence |
| Guidance cap | 50 avoid + 50 seek tokens |
