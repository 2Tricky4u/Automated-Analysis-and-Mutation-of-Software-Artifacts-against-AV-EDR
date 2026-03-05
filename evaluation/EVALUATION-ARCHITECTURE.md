# Evaluation Framework — Architecture & Module Reference

## Role in the Global Project

The evaluation crate is the **scientific measurement layer** for AutoMutate++. While the controller runs experiments (build → execute → collect telemetry → triage), the evaluation crate answers: *"Is the system actually working?"*

It computes ~40 metrics across three axes that together assess the full closed-loop pipeline:

```
                        AutoMutate++ Loop
                        ═══════════════
  ┌───────────┐    ┌───────────┐    ┌────────────────┐
  │  Selector  │───▶│  Builder   │───▶│  VM Execution   │
  │  (Input)   │    │  (Mutate)  │    │  (Detect/Evade) │
  └─────▲─────┘    └───────────┘    └───────┬────────┘
        │                                    │
        │         ┌────────────┐             │
        └─────────│  Triage    │◀────────────┘
                  │  (Oracle)  │
                  └─────┬──────┘
                        │
                  ┌─────▼──────┐
                  │ Evaluation  │  ◀── THIS CRATE
                  │ (3 axes,    │
                  │  ~40 metrics)│
                  └─────────────┘

  Input Axis ──── Are we generating diverse, valid experiments?
  Oracle Axis ─── Are detection verdicts trustworthy and attributable?
  Guidance Axis ─ Is the feedback loop actually improving evasion?
```

**Key design principle:** Every metric is a pure, stateless function from `EvalDataset → Vec<MetricResult>`. No side effects, no retained state, full offline reproducibility.

---

## Directory Structure

```
evaluation/
├── Cargo.toml                          Package metadata + feature flags
├── EVALUATION-GUIDE.md                 User guide for running evaluations
├── src/
│   ├── lib.rs                          Core abstractions: EvalMetric trait, EvalDataset, MetricResult
│   ├── helpers.rs                      Statistical utilities (entropy, Jaccard, Pearson, etc.)
│   ├── input/                          INPUT AXIS — experiment generation quality
│   │   ├── mod.rs                      Re-exports (feature-gated)
│   │   ├── expressiveness.rs           Module/mutation coverage, config uniqueness
│   │   ├── validity.rs                 Build/execution success rates
│   │   └── diversity.rs                Jaccard distance, entropy, discovery rate
│   ├── oracle/                         ORACLE AXIS — verdict trustworthiness
│   │   ├── mod.rs                      Re-exports (feature-gated)
│   │   ├── precision.rs                FP/FN proxies, trustworthy ratio
│   │   ├── soundness.rs                Static/dynamic ratio, evasion rate, blind spots
│   │   ├── attribution.rs              Token lift ranking, stability, counterfactual
│   │   └── stability.rs                Flaky rate, behavior match, config consistency
│   ├── guidance/                       GUIDANCE AXIS — feedback loop effectiveness
│   │   ├── mod.rs                      Re-exports (feature-gated)
│   │   ├── feedback_quality.rs         Coverage correlation, guidance strength
│   │   ├── search_efficiency.rs        Evasion rate, TTFE, score trajectory
│   │   ├── baseline_comparison.rs      Guided vs random delta, ablation
│   │   └── convergence.rs              Decay ratio, plateau detection, exploitation
│   ├── report/                         Output formatters
│   │   ├── json_report.rs              JSON report writer (detail + summary)
│   │   └── csv_report.rs              CSV writer with escaping
│   ├── fixtures/                       Test data generation
│   │   ├── loader.rs                   JSON load/save for EvalDataset
│   │   ├── round_factory.rs            Deterministic synthetic round builder
│   │   └── token_factory.rs            Token matrix construction (basic + enriched)
│   └── bin/
│       ├── evaluate.rs                 CLI: load dataset → run metrics → write reports
│       └── export.rs                   CLI: generate synthetic test datasets
└── tests/                              Integration tests (1 per metric, feature-gated)
    ├── common/mod.rs                   Shared test fixtures
    ├── input_expressiveness.rs
    ├── input_validity.rs
    ├── input_diversity.rs
    ├── oracle_precision.rs
    ├── oracle_soundness.rs
    ├── oracle_attribution.rs
    ├── oracle_stability.rs
    ├── guidance_feedback_quality.rs
    ├── guidance_search_efficiency.rs
    ├── guidance_baseline_comparison.rs
    └── guidance_convergence.rs
```

---

## Core Abstractions (`lib.rs`)

### `EvalDataset` — Universal Input

Every metric consumes the same dataset structure:

```rust
pub struct EvalDataset {
    pub job_id: String,
    pub rounds: Vec<RoundSummary>,              // Core experiment results
    pub selections: Vec<SelectionRecord>,        // Selector decisions (rationale text)
    pub token_matrices: Vec<TokenMatrixEntry>,   // Per-round tokens + detection outcome
    pub telemetry_tokens: Option<Vec<RoundTelemetryTokens>>,  // Optional raw telemetry
}
```

| Field | Source | Used By |
|---|---|---|
| `rounds` | Controller round aggregation | All metrics |
| `selections` | Selector rationale logs | Convergence (exploitation_ratio), Baseline (token_guidance_usage) |
| `token_matrices` | Triage token extractor | Attribution, Diversity (seq2), Feedback quality |
| `telemetry_tokens` | Raw ETW/API telemetry | Reserved for future per-category analysis |

### `EvalMetric` Trait

```rust
pub trait EvalMetric: Send + Sync {
    fn metric_id(&self) -> &str;                                    // e.g. "input.validity"
    fn evaluate(&self, dataset: &EvalDataset) -> Result<Vec<MetricResult>>;  // Pure computation
}
```

### `MetricResult` — Universal Output

```rust
pub struct MetricResult {
    pub metric_id: String,          // "input.validity.rejection_rate"
    pub axis: String,               // "input" | "oracle" | "guidance"
    pub category: String,           // "validity" | "precision" | etc.
    pub label: String,              // Human-readable description
    pub value: f64,                 // Primary numeric score
    pub details: serde_json::Value, // Structured breakdown (per-module, per-config, etc.)
    pub n: usize,                   // Sample size
}
```

### Feature Flags

| Feature | Compiles | Sub-metrics |
|---|---|---|
| `input` | `input/*` modules | ~10 |
| `oracle` | `oracle/*` modules | ~15 |
| `guidance` | `guidance/*` modules | ~13 |
| `full` | All three axes | ~38–41 total |

---

## Statistical Utilities (`helpers.rs`)

| Function | Signature | Description |
|---|---|---|
| `shannon_entropy` | `(&[usize]) → f64` | H = −Σ pᵢ log₂ pᵢ in bits |
| `normalized_entropy` | `(&[usize]) → f64` | H / log₂(n), range [0, 1] |
| `jaccard_distance` | `(&[String], &[String]) → f64` | 1 − |A∩B| / |A∪B| |
| `mean_pairwise_jaccard` | `(&[Vec<String>]) → f64` | Average all-pairs Jaccard distance |
| `pearson_correlation` | `(&[f64], &[f64]) → f64` | Standard Pearson r, range [−1, 1] |
| `config_fingerprint` | `(modules, mutations) → String` | Deterministic config identity string |
| `rolling_mean` | `(&[f64], usize) → Vec<f64>` | Sliding window averages |

---

## Input Axis — Experiment Generation Quality

Answers: *"Is the selector generating diverse, valid, expressive experiments?"*

### `input.expressiveness` (4 sub-metrics)

Measures how much of the available search space the selector actually explores.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `module_coverage` | [0, 1] | used_variants / total_known_variants | 1.0 = every module variant exercised |
| `mutation_coverage` | [0, 1] | used_mutations ∩ known / known | 1.0 = every mutation type used |
| `unique_configs` | [0, 1] | distinct_fingerprints / total_rounds | 1.0 = every round is unique |
| `category_reachability` | [0, 1] | varied_categories / 7 | 1.0 = selector varies all 7 module categories |

**Details breakdown:** Per-category coverage with variants_seen lists; per-mutation used/known.

Known variants are hardcoded from the project's module catalog (17 variants across 7 categories, 5 known AST mutations).

### `input.validity` (3 sub-metrics)

Measures how often generated artifacts actually compile and execute.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `rejection_rate` | [0, 1] | (MutationFailed + PayloadFailed) / n | 0.0 = no wasted rounds |
| `execution_rate` | [0, 1] | 1 − rejection_rate | 1.0 = all artifacts run |
| `mutation_failure_correlation` | [0, 1] | max per-mutation failure rate | Identifies which mutation causes the most failures |

**Details breakdown:** Per-mutation failure rates sorted descending — immediately surfaces problematic mutations.

### `input.diversity` (3–4 sub-metrics)

Measures whether rounds differ enough to be informative.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `mutation_jaccard` | [0, 1] | mean pairwise Jaccard of mutation sets | 1.0 = every pair is completely different |
| `module_entropy` | [0, 1] | avg normalized Shannon entropy across 7 categories | 1.0 = uniform distribution |
| `config_discovery_rate` | [0, 1] | fraction of rounds introducing a new config | Drops toward 0 when search space is exhausted |
| `seq2_uniqueness` | [0, 1] | unique seq2 tokens / total occurrences | Only if token_matrices available; measures behavioral diversity |

---

## Oracle Axis — Verdict Trustworthiness

Answers: *"Can we trust the detection outcomes? Can we attribute detections to specific behaviors?"*

### `oracle.precision` (4 sub-metrics)

Measures false positive/negative proxies from the differential protocol.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `fp_proxy_rate` | [0, 1] | InstrumentationArtifact / n | 0.0 = no trace-induced false detections |
| `fn_proxy_rate` | [0, 1] | Flaky / n | 0.0 = no unreproducible results |
| `trustworthy_ratio` | [0, 1] | (RealDetection + StaticDetection + Evasion) / n | 1.0 = all verdicts trustworthy |
| `dryrun_resolution_rate` | [0, 1] | resolved dryruns / total dryruns | Measures dry-run override effectiveness |

### `oracle.soundness` (4 sub-metrics)

Measures detection coverage depth.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `static_ratio` | [0, 1] | StaticDetection / (Static + RealDetection) | High = mostly caught by signatures (easy wins) |
| `evasion_rate` | [0, 1] | Evasion / n | Overall evasion success |
| `blind_spot_ratio` | [0, 1] | always-detected configs / multi-run configs | 1.0 = EDR catches everything regardless of config |
| `evasion_config_ratio` | [0, 1] | never-detected configs / total configs | Identifies reliably evasive configurations |

Blind spot and evasion config metrics only count trustworthy rounds and configs seen ≥2 times.

### `oracle.attribution` (1–3 sub-metrics)

Measures whether triage tokens explain detections. Uses the controller's `compute_token_scores` (lift × confidence) and `build_guidance` (thresholds: lift > 1.5, confidence > 0.3).

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `token_ranking` | [0, ∞] | top token's importance score | Higher = stronger signal; 0 = no meaningful tokens |
| `top5_stability` | [0, 1] | overlap(first_half_top5, second_half_top5) / 5 | 1.0 = rankings are stable over time |
| `counterfactual` | [−1, 1] | P(det\|token) − P(det\|¬token) | Positive = token presence causes detection |

Requires ≥4 trustworthy entries with outcome variance. Returns empty results for degenerate data.

### `oracle.stability` (4 sub-metrics)

Measures result reproducibility.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `flaky_rate` | [0, 1] | Flaky / (trustworthy + Flaky) | 0.0 = fully reproducible |
| `behavior_match_rate` | [0, 1] | behavior_match=true / n | 1.0 = baseline ↔ instrumented always agree |
| `config_consistency` | [0, 1] | 1 − (inconsistent / multi_run) | 1.0 = same config always gives same outcome |
| `score_variance` | [0, ∞] | mean σ of evasion_score per repeated config | 0.0 = scores perfectly reproducible |

---

## Guidance Axis — Feedback Loop Effectiveness

Answers: *"Is the token-driven feedback loop actually improving mutation selection?"*

### `guidance.feedback_quality` (1–3 sub-metrics)

Measures whether triage signals are informative.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `coverage_correlation` | [−1, 1] | Pearson(coverage_pct, evasion_score) | Positive = coverage predicts evasion |
| `guidance_strength` | [0, 1] | (avoid + seek tokens) / total_scored | 1.0 = all tokens have strong signal |
| `avoidance_rate` | [0, 1] | post-midpoint rounds avoiding top-5 avoid tokens / total | 1.0 = selector fully acts on guidance |

### `guidance.search_efficiency` (1–4 sub-metrics)

Measures how quickly the system finds evasions.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `evasions_per_round` | [0, 1] | Evasion / n | Higher is better |
| `time_to_first_evasion` | [0, 1] | first_evasion_round / n | 0.0 = found immediately; 1.0 = never |
| `evasions_at_n` | [0, 1] | best rate at checkpoints [5, 10, 20, 50] | Early checkpoint performance |
| `score_trajectory` | [−1, 1] | last_rolling_mean − first_rolling_mean | Positive = improving over time |

### `guidance.baseline_comparison` (1–4 sub-metrics)

Compares guided runs against a synthetic random baseline (deterministic seed=12345).

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `evasion_rate_delta` | [−1, 1] | guided_rate − random_rate | Positive = guidance helps |
| `score_delta` | [−1, 1] | guided_mean − random_mean | Positive = guidance helps |
| `mutation_ablation` | [−1, 1] | mean_score(with_mutations) − mean_score(without) | Positive = mutations contribute |
| `token_guidance_usage` | [0, 1] | selections_with_avoid_or_seek / total | 1.0 = all selections use token guidance |

### `guidance.convergence` (1–4 sub-metrics)

Measures whether the system is converging or stalling.

| Sub-metric | Range | Formula | Interpretation |
|---|---|---|---|
| `decay_ratio` | [0, 10] | second_half_evasions / first_half_evasions | >1 = improving; <1 = diminishing returns |
| `plateau_round` | [0, 1] | plateau_round / n | Earlier = faster convergence |
| `config_discovery_decay` | [0, ∞] | second_half_new / first_half_new | <1 = running out of configs |
| `exploitation_ratio` | [0, 1] | exploit keywords / (exploit + explore) | Balance between exploitation and exploration |

Exploitation ratio uses keyword matching on selector rationale text (e.g., "exploit", "best", "repeat" vs "explore", "random", "epsilon", "new").

---

## Test Data Factories (`fixtures/`)

### `RoundSequenceBuilder` — Deterministic Round Generation

Uses a seeded xorshift32 PRNG for full reproducibility.

| Method | Description | Detection Distribution |
|---|---|---|
| `random_rounds(n, seed)` | Random modules + 1–4 mutations | 40% detected, 25% evasion, 15% failed, 10% artifact, 10% flaky |
| `improvement_scenario(n)` | Linear progress toward evasion | Starts all-detected, ends all-evasion |
| `plateau_scenario(n)` | Quick gain then stall | Rapid improvement in first 25%, hovers at 0.4–0.5 |
| `all_detected(n)` | Everything detected | 100% RealDetection, scores 0.0–0.2 |
| `all_evasion(n)` | Everything evades | 100% Evasion, scores 0.7–1.0 |

### `TokenFactory` — Token Matrix Construction

| Function | Description |
|---|---|
| `build_token_matrix(rounds)` | Extract tokens from round metadata using controller's `extract_round_tokens` |
| `build_enriched_token_matrix(rounds)` | Basic extraction + synthetic API/ETW tokens per carrier/module variant |

Enriched tokens add carrier-specific patterns (e.g., peb_walk → `api:LdrGetProcedureAddress`), decoy tokens (`api:CreateProcessW`), and universal ETW provider tokens.

---

## CLI Binaries

### `evaluate` — Run Metrics

```bash
cargo run -p evaluation --features full --bin evaluate -- \
  --input eval_dataset.json \
  --json report.json \
  --csv metrics.csv \
  --summary summary.json \
  --axis oracle
```

| Flag | Default | Description |
|---|---|---|
| `--input` | `eval_dataset.json` | Input dataset (JSON) |
| `--json` | `eval_report.json` | Detailed JSON report |
| `--csv` | `eval_metrics.csv` | Tabular CSV (metric_id, axis, category, label, value, n) |
| `--summary` | — | Optional grouped-by-axis summary |
| `--axis` | all | Filter to single axis |
| `--quiet` | false | Suppress stderr, print CSV to stdout |

### `eval-export` — Generate Synthetic Datasets

```bash
cargo run -p evaluation --bin eval-export -- \
  --scenario improvement --rounds 50 --enriched --output my_test.json
```

| Flag | Default | Description |
|---|---|---|
| `--scenario` | `random` | `random`, `improvement`, `plateau`, `detected`, `evasion` |
| `--rounds` | 30 | Number of rounds |
| `--seed` | 42 | PRNG seed (random scenario only) |
| `--output` | `eval_dataset.json` | Output path |
| `--enriched` | false | Add synthetic telemetry tokens |

---

## Report Formats

### JSON Detail Report

Array of `MetricResult` objects with full `details` breakdowns:

```json
[
  {
    "metric_id": "input.expressiveness.module_coverage",
    "axis": "input",
    "category": "expressiveness",
    "label": "Module variant coverage",
    "value": 0.647,
    "details": {
      "per_category": {
        "carrier": { "used": 2, "available": 3, "coverage": 0.667, "variants_seen": ["change_rw_rx", "peb_walk"] },
        "decoder": { "used": 2, "available": 2, "coverage": 1.0, "variants_seen": ["xor", "english"] }
      }
    },
    "n": 50
  }
]
```

### JSON Summary Report

Grouped by axis with job metadata:

```json
{
  "job_id": "job-abc123",
  "generated_at": "2026-03-05T...",
  "total_metrics": 41,
  "axes": {
    "input": [ ... ],
    "oracle": [ ... ],
    "guidance": [ ... ]
  }
}
```

### CSV Report

```csv
metric_id,axis,category,label,value,n
input.expressiveness.module_coverage,input,expressiveness,Module variant coverage,0.647,50
input.expressiveness.mutation_coverage,input,expressiveness,Mutation type coverage,0.800,50
```

---

## Integration Test Suite

12 integration test files, one per metric, all feature-gated. Each test:
1. Creates a synthetic dataset via `RoundSequenceBuilder`
2. Runs the metric
3. Asserts value ranges and boundary conditions

| Test File | Key Assertions |
|---|---|
| `input_expressiveness` | module_cov > 0.3, unique > 0.5 for mixed; reachability < 0.5 for all-detected |
| `input_validity` | rejection + execution = 1.0; all-evasion has 0 rejections |
| `input_diversity` | 0 ≤ jaccard ≤ 1, 0 ≤ entropy ≤ 1; all-detected has entropy < 0.5 |
| `oracle_precision` | 0 < trustworthy ≤ 1; all-evasion: fp=0, trustworthy=1.0 |
| `oracle_soundness` | 0 ≤ evasion ≤ 1; all-detected: evasion=0; all-evasion: evasion=1 |
| `oracle_attribution` | Optional results if meaningful; empty for degenerate (all-detected) |
| `oracle_stability` | 0 ≤ flaky ≤ 1; all-evasion: flaky=0, behavior_match=1.0 |
| `guidance_feedback_quality` | −1 ≤ coverage_corr ≤ 1 |
| `guidance_search_efficiency` | 0 ≤ epr ≤ 1; all-evasion: epr=1.0, ttfe=round_1 |
| `guidance_baseline_comparison` | −1 ≤ delta ≤ 1 |
| `guidance_convergence` | improvement: decay ≥ 1.0; plateau: plateau_round < 1.0 |

---

## Design Decisions

| Decision | Rationale |
|---|---|
| Pure stateless metrics | Full offline reproducibility; no database dependency |
| Feature-gated compilation | Avoids pulling in controller deps when only testing input metrics |
| Synthetic baselines (seed=12345) | Guided vs random comparison without needing a second job run |
| Keyword-based exploitation ratio | Cheap heuristic over rationale text; no NLP dependency |
| Trustworthiness filtering | Metrics only learn from high-quality data (Real, Static, Evasion) |
| Per-config grouping (≥2 runs) | Avoids single-observation noise in consistency/blind-spot metrics |
| Token scoring via controller crate | Reuses production lift/confidence math; no reimplementation |
| JSON + CSV dual output | JSON for programmatic drill-down; CSV for plotting in pandas/R |

---

## Metric Summary Table

| # | Metric ID | Axis | Range | What It Measures |
|---|---|---|---|---|
| 1 | `input.expressiveness.module_coverage` | Input | [0,1] | Fraction of known module variants exercised |
| 2 | `input.expressiveness.mutation_coverage` | Input | [0,1] | Fraction of known mutation types used |
| 3 | `input.expressiveness.unique_configs` | Input | [0,1] | Ratio of unique configurations to total rounds |
| 4 | `input.expressiveness.category_reachability` | Input | [0,1] | Categories where selector varies choices |
| 5 | `input.validity.rejection_rate` | Input | [0,1] | Build/execution failures |
| 6 | `input.validity.execution_rate` | Input | [0,1] | Successfully executed rounds |
| 7 | `input.validity.mutation_failure_correlation` | Input | [0,1] | Worst per-mutation failure rate |
| 8 | `input.diversity.mutation_jaccard` | Input | [0,1] | Pairwise mutation set dissimilarity |
| 9 | `input.diversity.module_entropy` | Input | [0,1] | Module selection uniformity |
| 10 | `input.diversity.config_discovery_rate` | Input | [0,1] | Rate of novel configurations |
| 11 | `input.diversity.seq2_uniqueness` | Input | [0,1] | Behavioral sequence diversity |
| 12 | `oracle.precision.fp_proxy_rate` | Oracle | [0,1] | Instrumentation-caused false detections |
| 13 | `oracle.precision.fn_proxy_rate` | Oracle | [0,1] | Flaky/unreproducible results |
| 14 | `oracle.precision.trustworthy_ratio` | Oracle | [0,1] | Fraction of trustworthy verdicts |
| 15 | `oracle.precision.dryrun_resolution_rate` | Oracle | [0,1] | Dry-run override effectiveness |
| 16 | `oracle.soundness.static_ratio` | Oracle | [0,1] | Static vs dynamic detection split |
| 17 | `oracle.soundness.evasion_rate` | Oracle | [0,1] | Overall evasion success |
| 18 | `oracle.soundness.blind_spot_ratio` | Oracle | [0,1] | Configs always caught by EDR |
| 19 | `oracle.soundness.evasion_config_ratio` | Oracle | [0,1] | Configs never caught by EDR |
| 20 | `oracle.attribution.token_ranking` | Oracle | [0,∞] | Strength of top attribution signal |
| 21 | `oracle.attribution.top5_stability` | Oracle | [0,1] | Token ranking consistency over time |
| 22 | `oracle.attribution.counterfactual` | Oracle | [−1,1] | Causal impact of top token |
| 23 | `oracle.stability.flaky_rate` | Oracle | [0,1] | Result irreproducibility |
| 24 | `oracle.stability.behavior_match_rate` | Oracle | [0,1] | Baseline↔instrumented agreement |
| 25 | `oracle.stability.config_consistency` | Oracle | [0,1] | Same config → same outcome |
| 26 | `oracle.stability.score_variance` | Oracle | [0,∞] | Score spread for repeated configs |
| 27 | `guidance.feedback_quality.coverage_correlation` | Guidance | [−1,1] | Coverage predicts evasion? |
| 28 | `guidance.feedback_quality.guidance_strength` | Guidance | [0,1] | Fraction of tokens with strong signal |
| 29 | `guidance.feedback_quality.avoidance_rate` | Guidance | [0,1] | Selector acts on avoid tokens? |
| 30 | `guidance.search_efficiency.evasions_per_round` | Guidance | [0,1] | Evasion hit rate |
| 31 | `guidance.search_efficiency.time_to_first_evasion` | Guidance | [0,1] | How quickly first evasion found |
| 32 | `guidance.search_efficiency.evasions_at_n` | Guidance | [0,1] | Best early-checkpoint performance |
| 33 | `guidance.search_efficiency.score_trajectory` | Guidance | [−1,1] | Score trend direction |
| 34 | `guidance.baseline_comparison.evasion_rate_delta` | Guidance | [−1,1] | Guided vs random evasion gap |
| 35 | `guidance.baseline_comparison.score_delta` | Guidance | [−1,1] | Guided vs random score gap |
| 36 | `guidance.baseline_comparison.mutation_ablation` | Guidance | [−1,1] | Mutations help vs hurt? |
| 37 | `guidance.baseline_comparison.token_guidance_usage` | Guidance | [0,1] | Fraction of guided selections |
| 38 | `guidance.convergence.decay_ratio` | Guidance | [0,10] | Second-half vs first-half evasions |
| 39 | `guidance.convergence.plateau_round` | Guidance | [0,1] | When improvement stalls |
| 40 | `guidance.convergence.config_discovery_decay` | Guidance | [0,∞] | Search space exhaustion rate |
| 41 | `guidance.convergence.exploitation_ratio` | Guidance | [0,1] | Exploit vs explore balance |
