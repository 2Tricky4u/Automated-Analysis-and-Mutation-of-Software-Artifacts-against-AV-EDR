# Evaluation Framework Guide

The evaluation crate measures AutoMutate++'s fuzzer quality along three canonical axes — **Input**, **Oracle**, and **Guidance** — producing 41 scored metrics from round data. It operates entirely offline from JSON exports, with zero dependency on ElasticSearch or VM infrastructure.

---

## Table of Contents

1. [Quick Start](#1-quick-start)
2. [Architecture Overview](#2-architecture-overview)
3. [The EvalDataset Schema](#3-the-evaldataset-schema)
4. [Exporting Job Data to JSON](#4-exporting-job-data-to-json)
5. [Running Evaluation from JSON](#5-running-evaluation-from-json)
6. [Exporting Results to CSV](#6-exporting-results-to-csv)
7. [Complete Pipeline Examples](#7-complete-pipeline-examples)
8. [Metric Reference](#8-metric-reference)
   - [Input Axis](#81-input-axis)
   - [Oracle Axis](#82-oracle-axis)
   - [Guidance Axis](#83-guidance-axis)
9. [Interpreting Results](#9-interpreting-results)
10. [Programmatic Usage (Rust)](#10-programmatic-usage-rust)
11. [Component-Level Experiments](#11-component-level-experiments)
12. [Running Tests](#12-running-tests)
13. [End-to-End Analysis Pipeline](#13-end-to-end-analysis-pipeline)
14. [Infrastructure-Level Experiments (I1–I15)](#14-infrastructure-level-experiments-i1i15)

---

## 1. Quick Start

```bash
# Step 1: Generate a synthetic dataset (or export real job data — see Section 4)
cargo run -p evaluation --bin eval-export -- \
  --scenario improvement --rounds 50 --enriched \
  --output my_dataset.json

# Step 2: Run all 12 metrics, output JSON report + CSV
cargo run -p evaluation --features full --bin evaluate -- \
  --input my_dataset.json \
  --json eval_report.json \
  --csv eval_metrics.csv \
  --summary eval_summary.json

# Step 3: The CSV is ready for plotting in Python, R, or Excel
```

---

## 2. Architecture Overview

```
                         EvalDataset (JSON)
                              │
               ┌──────────────┼──────────────┐
               ▼              ▼              ▼
          INPUT AXIS     ORACLE AXIS   GUIDANCE AXIS
         ┌──────────┐  ┌───────────┐  ┌─────────────┐
         │Express.  │  │Precision  │  │FeedbackQual.│
         │Validity  │  │Soundness  │  │SearchEffic. │
         │Diversity │  │Attribution│  │BaselineComp.│
         └──────────┘  │Stability  │  │Convergence  │
                       └───────────┘  └─────────────┘
               │              │              │
               └──────────────┼──────────────┘
                              ▼
                     Vec<MetricResult>
                       │         │
                       ▼         ▼
                  JSON Report  CSV Report
```

Every metric implements the `EvalMetric` trait — stateless, pure computation. All metrics receive the same `EvalDataset` and produce one or more `MetricResult` values.

Feature flags control compilation:

| Flag | Compiles |
|------|----------|
| `--features input` | 3 input metrics |
| `--features oracle` | 4 oracle metrics |
| `--features guidance` | 4 guidance metrics |
| `--features full` | All 11 metrics (produces ~41 sub-metrics) |

---

## 3. The EvalDataset Schema

The `EvalDataset` is the universal input to all metrics. It is fully serializable as JSON.

```json
{
  "job_id": "job-abc123",
  "rounds": [ ... ],
  "selections": [ ... ],
  "token_matrices": [ ... ],
  "telemetry_tokens": null
}
```

### 3.1 `rounds: Vec<RoundSummary>`

Each entry is a completed round from the job. This is the primary data source for most metrics.

```json
{
  "round_id": "job-abc123-round-1",
  "round_number": 1,
  "mutations": ["ast.fill_pattern", "ast.timing_pattern"],
  "mutation_specs": [
    { "id": "ast.fill_pattern", "params": null },
    { "id": "ast.timing_pattern", "params": null }
  ],
  "modules": {
    "carrier": "alloc_rw_rx",
    "decoder": "xor",
    "antiemulation": "none",
    "deconditioner": "alloc_loop",
    "guardrail": "none",
    "virtualprotect": "standard",
    "decoy": "none"
  },
  "detected": true,
  "behavior_match": true,
  "evasion_score": 0.15,
  "differential_category": "RealDetection",
  "completed_at": { "secs_since_epoch": 1740000000, "nanos_since_epoch": 0 },
  "dry_run_exit_code": null,
  "has_dryrun": false,
  "detection_verdict": "detected",
  "coverage_percent": 0.62,
  "time_factor": 0.0
}
```

**Key fields used by metrics:**

| Field | Type | Used By |
|-------|------|---------|
| `modules` | `ModuleSelectionSpec` | Input (expressiveness, diversity), Oracle (soundness, stability) |
| `mutations` | `Vec<String>` | Input (expressiveness, validity, diversity) |
| `differential_category` | enum | Oracle (all), Guidance (search efficiency, convergence) |
| `detected` | `bool` | Oracle (stability), token scoring |
| `evasion_score` | `f64` | Guidance (feedback quality, search efficiency, convergence) |
| `coverage_percent` | `Option<f64>` | Guidance (feedback quality) |
| `behavior_match` | `bool` | Oracle (stability) |
| `has_dryrun` | `bool` | Oracle (precision) |

**`differential_category` values:**

| Value | Meaning | `is_detected()` | `is_trustworthy()` |
|-------|---------|-----------------|---------------------|
| `RealDetection` | Both baseline and instrumented detected | true | true |
| `StaticDetection` | Defender file scan before execution | true | true |
| `Evasion` | Neither baseline nor instrumented detected | false | true |
| `InstrumentationArtifact` | Only instrumented detected (trace noise) | false | false |
| `Flaky` | Only baseline detected (inconsistent) | false | false |
| `MutationFailed` | Dryrun crash — artifact broken | false | false |
| `PayloadFailed` | Dryrun crash — payload binary broken | false | false |

### 3.2 `selections: Vec<SelectionRecord>`

Records of selector decisions. Used by Guidance metrics (convergence exploitation ratio, baseline comparison).

```json
{
  "round_number": 5,
  "rationale": "Exploit best config from prior rounds",
  "modules": { "carrier": "peb_walk", "decoder": "xor", ... },
  "mutations": ["ast.fill_pattern"],
  "avoid_tokens": ["api_arg:NtProtectVirtualMemory:protect=R-X"],
  "seek_tokens": ["module:carrier=peb_walk"]
}
```

Can be empty `[]` — guidance metrics that need selections will simply skip those sub-metrics.

### 3.3 `token_matrices: Vec<TokenMatrixEntry>`

The token-round matrix: what tokens were observed in each round and whether it was detected. Used by Oracle (attribution) and Guidance (feedback quality).

```json
{
  "round_number": 1,
  "tokens": [
    "module:carrier=alloc_rw_rx",
    "module:decoder=xor",
    "mutation:ast.fill_pattern",
    "api:NtAllocateVirtualMemory",
    "api_arg:NtProtectVirtualMemory:protect=R-X",
    "seq2:NtAllocateVirtualMemory→NtProtectVirtualMemory",
    "etw:Microsoft-Windows-Kernel-Process/1"
  ],
  "detected": true,
  "trustworthy": true
}
```

**Token types:**

| Prefix | Source | Example |
|--------|--------|---------|
| `module:` | Module selection (7 categories) | `module:carrier=peb_walk` |
| `mutation:` | Applied mutations (with sorted params) | `mutation:ast.decon_rounds:count=50:method=fixed` |
| `checkpoint:` | Instrumented run checkpoint events | `checkpoint:antiemulation_passed` |
| `api:` | Syscall/lifecycle function | `api:NtAllocateVirtualMemory` |
| `api_arg:` | Memory protection arguments | `api_arg:NtProtectVirtualMemory:protect=R-X` |
| `seq2:` | Bigrams of consecutive calls | `seq2:NtAllocateVirtualMemory→NtProtectVirtualMemory` |
| `etw:` | ETW provider/event pairs | `etw:Microsoft-Windows-Kernel-Process/6` |
| `image:` | Loaded DLL basenames | `image:ntdll.dll` |

### 3.4 `telemetry_tokens: Option<Vec<RoundTelemetryTokens>>`

Optional. Per-round breakdown of telemetry tokens by category. Currently unused by metrics but reserved for future deep-dive analysis.

```json
{
  "round_number": 1,
  "api_tokens": ["api:NtAllocateVirtualMemory", "api:NtProtectVirtualMemory"],
  "seq2_tokens": ["seq2:NtAllocateVirtualMemory→NtProtectVirtualMemory"],
  "etw_tokens": ["etw:Microsoft-Windows-Kernel-Process/1"],
  "image_tokens": ["image:ntdll.dll"]
}
```

---

## 4. Exporting Job Data to JSON

### 4.1 From a Completed Job (Rust Code)

After a job completes, the `JobSession` holds all round data in `session.rounds`. Convert it to an `EvalDataset`:

```rust
use evaluation::{EvalDataset, SelectionRecord};
use evaluation::fixtures::token_factory::build_token_matrix;
use evaluation::fixtures::loader::save_dataset;
use std::path::Path;

fn export_job(session: &JobSession) -> anyhow::Result<()> {
    // 1. Collect rounds from the BTreeMap
    let rounds: Vec<RoundSummary> = session.rounds.values().cloned().collect();

    // 2. Build the token matrix from round summaries
    //    This extracts module:* and mutation:* tokens per round.
    //    For richer tokens (api:*, seq2:*, etw:*), use build_enriched_token_matrix
    //    or query ES and construct TokenMatrixEntry manually.
    let token_matrices = build_token_matrix(&rounds);

    // 3. If you tracked selector decisions, include them:
    let selections: Vec<SelectionRecord> = vec![];  // or your tracked selections

    // 4. Assemble the dataset
    let dataset = EvalDataset {
        job_id: session.id.0.clone(),
        rounds,
        selections,
        token_matrices,
        telemetry_tokens: None,
    };

    // 5. Save to JSON
    save_dataset(&dataset, Path::new("job_eval_export.json"))?;
    Ok(())
}
```

### 4.2 From ElasticSearch (Manual Export)

If you have round data in ES, query it and build the JSON manually:

```bash
# Query rounds from ES
curl -s "http://localhost:9200/rounds-*/_search?q=job_id:job-abc123&size=100" \
  | jq '[.hits.hits[]._source]' > rounds_raw.json

# Then use a script to reshape into the EvalDataset schema.
# The key fields needed per round are listed in Section 3.1.
```

### 4.3 Synthetic Data (for Testing)

Use the `eval-export` binary to generate synthetic datasets:

```bash
# Random mixed scenario (default)
cargo run -p evaluation --bin eval-export -- \
  --scenario random --rounds 50 --seed 42 --output random_50.json

# Improvement trajectory (detected → evasion over time)
cargo run -p evaluation --bin eval-export -- \
  --scenario improvement --rounds 50 --output improvement_50.json

# Plateau (quick gain then flat)
cargo run -p evaluation --bin eval-export -- \
  --scenario plateau --rounds 50 --output plateau_50.json

# Worst case (all detected)
cargo run -p evaluation --bin eval-export -- \
  --scenario detected --rounds 30 --output detected_30.json

# Best case (all evasion)
cargo run -p evaluation --bin eval-export -- \
  --scenario evasion --rounds 30 --output evasion_30.json

# With enriched telemetry tokens (api:*, seq2:*, etw:*)
cargo run -p evaluation --bin eval-export -- \
  --scenario random --rounds 50 --enriched --output enriched_50.json
```

**Options:**

| Flag | Description | Default |
|------|-------------|---------|
| `--scenario <NAME>` | `random`, `improvement`, `plateau`, `detected`, `evasion` | `random` |
| `--rounds <N>` | Number of rounds to generate | 30 |
| `--seed <N>` | RNG seed (only for `random` scenario) | 42 |
| `--output <PATH>` | Output JSON file path | `eval_dataset.json` |
| `--enriched` | Add synthetic API/ETW tokens to the token matrix | off |

---

## 5. Running Evaluation from JSON

### 5.1 CLI Tool

The `evaluate` binary loads a dataset JSON, runs all enabled metrics, and writes report files:

```bash
cargo run -p evaluation --features full --bin evaluate -- \
  --input my_dataset.json \
  --json eval_report.json \
  --csv eval_metrics.csv \
  --summary eval_summary.json
```

**Options:**

| Flag | Description | Default |
|------|-------------|---------|
| `--input <PATH>` | Input EvalDataset JSON | `eval_dataset.json` |
| `--json <PATH>` | Output JSON report (flat array of MetricResult) | `eval_report.json` |
| `--csv <PATH>` | Output CSV for plotting | `eval_metrics.csv` |
| `--summary <PATH>` | Output summary JSON grouped by axis | *(not written)* |
| `--axis <NAME>` | Only run metrics for one axis: `input`, `oracle`, `guidance` | all |
| `--quiet` | Suppress stderr output; print CSV to stdout | off |

### 5.2 Single-Axis Evaluation

```bash
# Only input metrics
cargo run -p evaluation --features full --bin evaluate -- \
  --input my_dataset.json --axis input --csv input_metrics.csv

# Only oracle metrics
cargo run -p evaluation --features full --bin evaluate -- \
  --input my_dataset.json --axis oracle --csv oracle_metrics.csv
```

### 5.3 Quiet Mode (Pipe CSV to stdout)

```bash
# Pipe CSV directly to another tool
cargo run -p evaluation --features full --bin evaluate -- \
  --input my_dataset.json --quiet > metrics.csv
```

---

## 6. Exporting Results to CSV

### 6.1 Via CLI (Recommended)

The `evaluate` binary writes CSV automatically:

```bash
cargo run -p evaluation --features full --bin evaluate -- \
  --input my_dataset.json --csv eval_metrics.csv
```

### 6.2 CSV Format

The CSV has 6 columns:

```
metric_id,axis,category,label,value,n
input.expressiveness.module_coverage,input,expressiveness,Module variant coverage (used/total across all categories),0.7059,30
input.expressiveness.mutation_coverage,input,expressiveness,Mutation pool coverage (used/available),1.0000,30
...
```

| Column | Description |
|--------|-------------|
| `metric_id` | Dotted identifier (e.g. `oracle.precision.fp_proxy_rate`) |
| `axis` | `input`, `oracle`, or `guidance` |
| `category` | Sub-category (e.g. `precision`, `convergence`) |
| `label` | Human-readable description |
| `value` | Primary numeric score |
| `n` | Sample size (number of rounds analyzed) |

### 6.3 JSON Report Format

The JSON report is a flat array of `MetricResult` objects, each containing a `details` field with the full breakdown:

```json
[
  {
    "metric_id": "input.expressiveness.module_coverage",
    "axis": "input",
    "category": "expressiveness",
    "label": "Module variant coverage (used/total across all categories)",
    "value": 0.7059,
    "details": {
      "per_category": {
        "carrier": { "used": 2, "available": 3, "coverage": 0.667, "variants_seen": ["alloc_rw_rx", "peb_walk"] },
        "decoder": { "used": 1, "available": 2, "coverage": 0.5, "variants_seen": ["xor"] }
      }
    },
    "n": 30
  }
]
```

### 6.4 Summary Report Format

The summary JSON groups results by axis:

```json
{
  "job_id": "eval-improvement-30",
  "total_metrics": 41,
  "axes": {
    "input": [ ... ],
    "oracle": [ ... ],
    "guidance": [ ... ]
  }
}
```

---

## 7. Complete Pipeline Examples

### Example 1: Evaluate a Completed Job

```bash
# Step 1: Export from job (in your controller code, call save_dataset)
# This produces: job_abc123_eval.json

# Step 2: Run evaluation
cargo run -p evaluation --features full --bin evaluate -- \
  --input job_abc123_eval.json \
  --json job_abc123_report.json \
  --csv  job_abc123_metrics.csv \
  --summary job_abc123_summary.json

# Step 3: Plot in Python
python3 -c "
import pandas as pd
import matplotlib.pyplot as plt

df = pd.read_csv('job_abc123_metrics.csv')

# Bar chart of all metrics by axis
for axis in ['input', 'oracle', 'guidance']:
    sub = df[df['axis'] == axis]
    plt.figure(figsize=(12, 4))
    plt.barh(sub['metric_id'], sub['value'])
    plt.title(f'{axis.upper()} Axis Metrics')
    plt.tight_layout()
    plt.savefig(f'{axis}_metrics.png')
    plt.close()
"
```

### Example 2: Compare Two Jobs

```bash
# Generate CSVs for both jobs
cargo run -p evaluation --features full --bin evaluate -- \
  --input job_A.json --csv job_A.csv --quiet

cargo run -p evaluation --features full --bin evaluate -- \
  --input job_B.json --csv job_B.csv --quiet

# Compare in Python
python3 -c "
import pandas as pd
a = pd.read_csv('job_A.csv').set_index('metric_id')['value']
b = pd.read_csv('job_B.csv').set_index('metric_id')['value']
delta = (b - a).dropna().sort_values()
print(delta.to_string())
"
```

### Example 3: Generate All Scenarios and Compare

```bash
for scenario in random improvement plateau detected evasion; do
  cargo run -p evaluation --bin eval-export -- \
    --scenario $scenario --rounds 50 --enriched \
    --output ${scenario}.json

  cargo run -p evaluation --features full --bin evaluate -- \
    --input ${scenario}.json \
    --csv ${scenario}_metrics.csv \
    --quiet
done

# Now you have 5 CSV files to compare across scenarios
```

---

## 8. Metric Reference

### 8.1 Input Axis

The input axis measures the quality and diversity of generated artifacts — how well the mutation engine explores the search space.

---

#### 8.1.1 Expressiveness (`input.expressiveness`)

Measures how much of the available search space the fuzzer actually exercises.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Module coverage** | `input.expressiveness.module_coverage` | Fraction of known module variants used across all 7 categories. With 3 carriers, 2 decoders, 3 antiemulations, etc. (17 total variants), this is `variants_used / 17`. | 0.0–1.0 | >0.7 | <0.3 |
| **Mutation coverage** | `input.expressiveness.mutation_coverage` | Fraction of the known AST mutation pool that was applied at least once. Pool: `ast.decon_rounds`, `ast.fill_pattern`, `ast.exec_decoy`, `ast.timing_pattern`, `ast.protection_transition`. | 0.0–1.0 | 1.0 | <0.5 |
| **Unique configs** | `input.expressiveness.unique_configs` | Ratio of distinct (modules+mutations) fingerprints to total rounds. A fingerprint is the sorted concatenation of all module values and mutation IDs. | 0.0–1.0 | >0.8 | <0.3 |
| **Category reachability** | `input.expressiveness.category_reachability` | Fraction of the 7 module categories where more than one variant was used. Measures breadth of exploration across categories. | 0.0–1.0 | >0.6 | <0.2 |

**Details breakdown:** `per_category` object showing per-category `used`, `available`, `coverage`, and `variants_seen`.

**When to worry:** Low expressiveness means the fuzzer is stuck in a small corner of the search space. Check if `SearchSpace.variable_categories` is too restrictive or if `VariationStrategy` should be `Full`.

---

#### 8.1.2 Validity (`input.validity`)

Measures whether generated artifacts actually execute rather than crashing.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Rejection rate** | `input.validity.rejection_rate` | Fraction of rounds classified as `MutationFailed` or `PayloadFailed`. These are broken artifacts that crash before doing anything useful. | 0.0–1.0 | <0.1 | >0.3 |
| **Execution rate** | `input.validity.execution_rate` | 1 - rejection_rate. Fraction of rounds that reached meaningful execution (detected or evaded). | 0.0–1.0 | >0.9 | <0.7 |
| **Mutation failure correlation** | `input.validity.mutation_failure_correlation` | Per-mutation failure rate: for each mutation, what fraction of rounds using it were broken. Sorted descending. The `value` is the highest single-mutation failure rate. | 0.0–1.0 | <0.1 | >0.3 |

**Details breakdown:** `per_mutation` array with `mutation`, `failure_rate`, `broken`, `total`.

**When to worry:** High rejection rate means mutations are producing invalid artifacts. Check the `per_mutation` breakdown to find which specific mutation causes the most failures, then fix or remove it from the pool.

---

#### 8.1.3 Diversity (`input.diversity`)

Measures how different generated artifacts are from each other.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Mutation Jaccard** | `input.diversity.mutation_jaccard` | Mean pairwise Jaccard distance between mutation sets across all rounds. 0 = identical sets, 1 = completely disjoint. | 0.0–1.0 | >0.5 | <0.2 |
| **Module entropy** | `input.diversity.module_entropy` | Mean normalized Shannon entropy across the 7 module categories. 0 = all rounds use the same variant, 1 = perfectly uniform distribution. | 0.0–1.0 | >0.6 | <0.2 |
| **Config discovery rate** | `input.diversity.config_discovery_rate` | Fraction of rounds that introduced a previously-unseen configuration fingerprint. Measures how quickly new configurations are being explored. | 0.0–1.0 | >0.5 | <0.2 |
| **Seq2 uniqueness** | `input.diversity.seq2_uniqueness` | Ratio of unique seq2 bigram tokens to total seq2 occurrences across all rounds. Only computed if token matrices contain seq2 tokens (requires `--enriched` or real telemetry). | 0.0–1.0 | >0.3 | 0.0 |

**Details breakdown:** `per_category` entropy with distribution, `cumulative_unique` configs over time.

**When to worry:** Low diversity means the selector keeps picking similar configurations. This indicates the epsilon-greedy balance is off, or the search space is inherently small.

---

### 8.2 Oracle Axis

The oracle axis measures how reliable and informative the detection verdicts are — can you trust the outcomes?

---

#### 8.2.1 Precision (`oracle.precision`)

Measures false positive/negative rates in the differential protocol.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **FP proxy rate** | `oracle.precision.fp_proxy_rate` | Fraction of rounds classified as `InstrumentationArtifact` (trace-mode detected but baseline didn't). These are false detections caused by the instrumentation, not real AV behavior. | 0.0–1.0 | <0.05 | >0.15 |
| **FN proxy rate** | `oracle.precision.fn_proxy_rate` | Fraction of rounds classified as `Flaky` (baseline detected but instrumented didn't). These are unreproducible detections. | 0.0–1.0 | <0.05 | >0.15 |
| **Trustworthy ratio** | `oracle.precision.trustworthy_ratio` | Fraction of rounds that are `RealDetection`, `Evasion`, or `StaticDetection`. These are the rounds where the differential protocol gave a clear, usable answer. | 0.0–1.0 | >0.8 | <0.5 |
| **Dryrun resolution** | `oracle.precision.dryrun_resolution_rate` | Of rounds with a dryrun, how many resolved to `MutationFailed` or `PayloadFailed`. Measures how useful dry-runs are at disambiguating crashes. | 0.0–1.0 | >0.5 | 0.0 (no dryruns) |

**When to worry:** High FP proxy rate means instrumentation is too heavy — consider reducing trace granularity. High FN proxy rate means the VM environment is inconsistent between runs. Low trustworthy ratio means too many rounds are wasted on noise.

---

#### 8.2.2 Soundness (`oracle.soundness`)

Measures detection scope — what the EDR catches and what it misses.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Static ratio** | `oracle.soundness.static_ratio` | Fraction of all detections that were static (file scan) vs dynamic (runtime). High static ratio means the EDR catches artifacts before they even execute. | 0.0–1.0 | depends | depends |
| **Evasion rate** | `oracle.soundness.evasion_rate` | Overall fraction of rounds that achieved full evasion. This is the core success metric of the fuzzer. | 0.0–1.0 | >0.3 | <0.05 |
| **Blind spot ratio** | `oracle.soundness.blind_spot_ratio` | Fraction of configurations (seen ≥2 times in trustworthy rounds) that were always detected. These are configurations the fuzzer cannot make work. | 0.0–1.0 | <0.3 | >0.7 |
| **Evasion config ratio** | `oracle.soundness.evasion_config_ratio` | Fraction of configurations that were never detected. These are the "winning" configurations that consistently evade. | 0.0–1.0 | >0.2 | 0.0 |

**When to worry:** Blind spot ratio near 1.0 means almost every configuration gets caught — the EDR is very effective against the current mutation space. Evasion config ratio > 0 with low blind spots means there are known-good configurations the selector should exploit.

---

#### 8.2.3 Attribution (`oracle.attribution`)

Measures how well the token scoring system identifies what causes detection.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Token ranking** | `oracle.attribution.token_ranking` | Top importance score from `compute_token_scores()` (lift × confidence). Higher means clearer signal about what tokens correlate with detection. | 0.0–∞ | >1.5 | <0.5 |
| **Top-5 stability** | `oracle.attribution.top5_stability` | Overlap of the 5 highest-scored tokens between the first half and second half of rounds. 1.0 = same top-5 in both halves. Measures whether token rankings are stable over time. | 0.0–1.0 | >0.6 | <0.2 |
| **Counterfactual** | `oracle.attribution.counterfactual` | Detection rate delta: `P(detected | top_token_present) - P(detected | top_token_absent)`. Positive means the top token genuinely correlates with detection. | -1.0–1.0 | >0.3 | <0.1 |

**Requires:** Non-empty `token_matrices` with a mix of detected and evasion outcomes (if all rounds have the same outcome, lift is degenerate and this metric returns no results).

**When to worry:** Low stability means token rankings are noisy — need more rounds for reliable attribution. Low counterfactual means the top-ranked token doesn't actually predict detection, which undermines the token-driven guidance loop.

---

#### 8.2.4 Stability (`oracle.stability`)

Measures reproducibility of detection outcomes.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Flaky rate** | `oracle.stability.flaky_rate` | `Flaky / (trustworthy + Flaky)`. How often does the differential protocol give inconsistent results? | 0.0–1.0 | <0.05 | >0.15 |
| **Behavior match** | `oracle.stability.behavior_match_rate` | Fraction of rounds where `behavior_match` is true (baseline and instrumented agree on detection outcome). | 0.0–1.0 | >0.9 | <0.7 |
| **Config consistency** | `oracle.stability.config_consistency` | Of configurations seen ≥2 times (trustworthy rounds only), fraction with consistent detection outcomes. 1.0 = same config always gives same result. | 0.0–1.0 | >0.9 | <0.7 |
| **Score variance** | `oracle.stability.score_variance` | Mean standard deviation of evasion scores per configuration. Lower = more consistent scoring. | 0.0–∞ | <0.05 | >0.15 |

**When to worry:** Low behavior match or config consistency means the VM environment is noisy — results are unreliable. This is the most fundamental check: if stability is low, all other metrics are suspect.

---

### 8.3 Guidance Axis

The guidance axis measures whether the feedback loop (token scoring → selector → mutation) actually improves outcomes over time.

---

#### 8.3.1 Feedback Quality (`guidance.feedback_quality`)

Measures how well the feedback signals correlate with real outcomes.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Coverage correlation** | `guidance.feedback_quality.coverage_correlation` | Pearson correlation between `coverage_percent` and `evasion_score` across rounds. Positive = more coverage → better evasion. | -1.0–1.0 | >0.3 | <0 |
| **Guidance strength** | `guidance.feedback_quality.guidance_strength` | `(avoid_tokens + seek_tokens) / total_scored_tokens`. Fraction of tokens that produce actionable guidance (strong avoid or seek signal). | 0.0–1.0 | >0.3 | <0.1 |
| **Avoidance rate** | `guidance.feedback_quality.avoidance_rate` | In rounds after the midpoint: fraction of rounds that do NOT contain any of the top-5 avoid tokens. Measures whether the selector actually acts on guidance. | 0.0–1.0 | >0.7 | <0.3 |

**Requires:** `coverage_percent` on rounds (for correlation), non-empty `token_matrices` (for guidance strength and avoidance).

**When to worry:** Negative coverage correlation means coverage is counter-productive — investigate if instrumentation is causing detection. Low avoidance rate means the selector ignores token guidance — check if `TokenSelector` is actually active.

---

#### 8.3.2 Search Efficiency (`guidance.search_efficiency`)

Measures how quickly the fuzzer finds evasions.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Evasions per round** | `guidance.search_efficiency.evasions_per_round` | `evasion_count / total_rounds`. Overall success rate. | 0.0–1.0 | >0.3 | <0.05 |
| **Time-to-first-evasion** | `guidance.search_efficiency.time_to_first_evasion` | Round number of first `Evasion` outcome. Lower is better. 0 = never found. | 0–N | <5 | >20 |
| **Evasions@N** | `guidance.search_efficiency.evasions_at_n` | Best evasion rate across checkpoint windows (N=5,10,20,50). Useful for comparing efficiency at different budget levels. | 0.0–1.0 | >0.2 | 0.0 |
| **Score trajectory** | `guidance.search_efficiency.score_trajectory` | Rolling-mean evasion score improvement: `last_window_avg - first_window_avg`. Positive = scores improving over time. | -1.0–1.0 | >0.2 | <0 |

**When to worry:** Negative score trajectory means the fuzzer is getting *worse* over time — the guidance loop may be misguided. High time-to-first-evasion with eventual evasions means the search needs more warm-up; consider starting with known-good configurations.

---

#### 8.3.3 Baseline Comparison (`guidance.baseline_comparison`)

Compares guided search against a synthetic random baseline.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Evasion rate delta** | `guidance.baseline_comparison.evasion_rate_delta` | `guided_evasion_rate - random_evasion_rate`. Positive = guided search finds more evasions than random. The random baseline uses the same number of rounds with a fixed seed. | -1.0–1.0 | >0.1 | <0 |
| **Score delta** | `guidance.baseline_comparison.score_delta` | `guided_mean_score - random_mean_score`. Positive = guided search achieves higher average evasion scores. | -1.0–1.0 | >0 | <0 |
| **Mutation ablation** | `guidance.baseline_comparison.mutation_ablation` | `score_with_mutations - score_without_mutations`. Measures the value-add of AST mutations beyond module variation alone. | -1.0–1.0 | >0 | <0 |
| **Token guidance usage** | `guidance.baseline_comparison.token_guidance_usage` | Fraction of selections that included avoid/seek tokens. Measures how often the guidance loop produced actionable advice. Only computed if selections are present. | 0.0–1.0 | >0.5 | 0.0 |

**When to worry:** Negative evasion rate delta means guided search is worse than random — the selector is actively harmful. Negative mutation ablation means mutations hurt more than they help (likely causing rejections).

---

#### 8.3.4 Convergence (`guidance.convergence`)

Measures discovery decay and whether the fuzzer plateaus.

| Sub-Metric | ID | Description | Range | Good | Bad |
|---|---|---|---|---|---|
| **Decay ratio** | `guidance.convergence.decay_ratio` | `second_half_evasions / first_half_evasions`. >1.0 = finding more evasions later (good — learning). <1.0 = diminishing returns. Clamped to 10.0 for JSON safety. | 0.0–10.0 | >1.0 | <0.5 |
| **Plateau onset** | `guidance.convergence.plateau_round` | Fraction of total rounds before the rolling evasion score stops improving (delta < 0.01). 1.0 = no plateau detected. | 0.0–1.0 | >0.7 | <0.3 |
| **Config discovery decay** | `guidance.convergence.config_discovery_decay` | `second_half_new_configs / first_half_configs`. <1.0 = running out of new configurations to try. | 0.0–∞ | >0.5 | <0.1 |
| **Exploitation ratio** | `guidance.convergence.exploitation_ratio` | From selector rationale text: fraction classified as "exploit" vs "explore" (keyword matching). 0.5 = balanced. Near 1.0 = over-exploiting. Near 0.0 = too much exploration. Only computed if selections are present. | 0.0–1.0 | 0.3–0.7 | <0.1 or >0.9 |

**When to worry:** Early plateau (<30% of rounds) means the fuzzer converged prematurely — increase epsilon or add more module variants. Decay ratio <0.5 means the fuzzer is exhausting its options in the first half and the second half is wasted. Over-exploitation (>0.9) means the fuzzer keeps repeating the same configurations instead of exploring.

---

## 9. Interpreting Results

### Decision Matrix

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| Low expressiveness + low diversity | Search space too restrictive | Set `VariationStrategy::Full`, add more `variable_categories` |
| High rejection rate | Mutations producing broken artifacts | Check `mutation_failure_correlation` details, fix or remove bad mutations |
| Low trustworthy ratio | Instrumentation too heavy or VM noise | Reduce trace mode, check VM snapshot consistency |
| High FP proxy | Instrumentation causes false detection | Use `trace_mode: "off"` for more rounds, lightweight probes |
| Low top-5 stability | Not enough data for stable attribution | Run more rounds (40+) before relying on token guidance |
| Score trajectory negative | Guidance steering in wrong direction | Audit token scores, check if avoid tokens are correct |
| Evasion rate delta < 0 | Guided worse than random | Selector is harmful — fall back to `SelectorType::Random` temporarily |
| Early plateau | Premature convergence | Increase epsilon, expand mutation pool, add module variants |

### Minimum Rounds for Meaningful Metrics

| Metric Group | Minimum Rounds | Recommended |
|---|---|---|
| Input (all) | 5 | 20+ |
| Oracle precision/stability | 10 | 30+ |
| Oracle attribution | 8 (needs mix of outcomes) | 40+ |
| Guidance efficiency | 10 | 30+ |
| Guidance convergence | 10 | 50+ |
| Baseline comparison | 20 | 50+ |

---

## 10. Programmatic Usage (Rust)

### Run All Metrics

```rust
use evaluation::{EvalDataset, run_evaluation};
use evaluation::fixtures::loader::load_dataset;

let dataset = load_dataset(Path::new("data.json"))?;
let results = run_evaluation(&dataset);

for r in &results {
    println!("{}: {:.4}", r.metric_id, r.value);
}
```

### Run a Single Metric

```rust
use evaluation::EvalMetric;
use evaluation::oracle::precision::Precision;

let metric = Precision;
let results = metric.evaluate(&dataset)?;
```

### Access Details

```rust
for r in &results {
    if r.metric_id == "input.expressiveness.module_coverage" {
        let per_cat = &r.details["per_category"];
        println!("Carrier coverage: {}", per_cat["carrier"]["coverage"]);
    }
}
```

### Export from Code

```rust
use evaluation::report::json_report::write_json_report;
use evaluation::report::csv_report::write_csv_report;

write_json_report(&results, Path::new("report.json"))?;
write_csv_report(&results, Path::new("metrics.csv"))?;
```

---

## 11. Component-Level Experiments

Beyond the 11 evaluation metrics (Section 8), the crate includes 6 **component-level academic experiments** designed for thesis figures and tables. These probe specific subsystem behaviors that the high-level metrics aggregate over.

### 11.1 Experiment Reference

| ID | Module | Thesis Section | What It Measures | Key Outputs |
|----|--------|---------------|------------------|-------------|
| **C1** | `token_sensitivity` | Triage | Actionable token count across a 5x5 grid of (lift_threshold, min_confidence) | Heatmap data for parameter sensitivity figure |
| **C3** | `token_coverage` | Triage | Token extraction completeness per category (module, mutation, api, seq2, etw, image) | Coverage table + presence heatmap (top-20 tokens x rounds) |
| **C4** | `scoring_convergence` | Triage | How quickly token rankings stabilize as rounds accumulate | Top-5 overlap curve + actionable count over rounds |
| **C5** | `counterfactual` | Triage | Per-token detection rate delta with Fisher exact test + Bonferroni correction | Forest plot data + volcano plot data |
| **B2** | `classifier_analysis` | Execution | Verdict-to-category confusion matrix from the 3-run differential protocol | Confusion matrix (verdict x category) |
| **B3** | `telemetry_completeness` | Execution | Coverage distribution across differential categories | Box plot data + histogram |

### 11.2 Running Component Experiments

```bash
# Run all 6 experiments on your dataset
cargo run -p evaluation --features full --bin component-eval -- \
  --input eval_dataset.json \
  --output component_eval_report.json \
  --csv component_eval_metrics.csv

# Run a single experiment by ID
cargo run -p evaluation --features full --bin component-eval -- \
  --input eval_dataset.json --experiment c1

# Run quietly (CSV to stdout)
cargo run -p evaluation --features full --bin component-eval -- \
  --input eval_dataset.json --quiet > component_metrics.csv
```

**Options:**

| Flag | Description | Default |
|------|-------------|---------|
| `--input <PATH>` | Input EvalDataset JSON | `eval_dataset.json` |
| `--output <PATH>` | Output JSON report (with full details for plotting) | `component_eval_report.json` |
| `--csv <PATH>` | Output CSV summary | `component_eval_metrics.csv` |
| `--experiment <ID>` | Only run one experiment: `c1`, `c3`, `c4`, `c5`, `b2`, `b3` | all |
| `--quiet` | Suppress stderr; print CSV to stdout | off |

### 11.3 Generating Thesis Figures

The JSON report from `component-eval` contains structured `details` fields designed for direct plotting. A Python script generates publication-quality PDF/PNG figures:

```bash
# Install dependencies
pip install matplotlib seaborn numpy

# Generate all figures
python evaluation/scripts/plots.py \
  --input component_eval_report.json \
  --outdir figures/
```

**Generated figures:**

| File | Experiment | Description |
|------|-----------|-------------|
| `c1_sensitivity_heatmap.pdf` | C1 | Actionable token count by (lift, confidence) |
| `c3_token_coverage.pdf` | C3 | Unique tokens per category + occurrence proportions |
| `c3_presence_heatmap.pdf` | C3 | Token presence matrix (top-20 tokens x rounds) |
| `c4_scoring_convergence.pdf` | C4 | Top-5 overlap and actionable count vs rounds included |
| `c5_forest_plot.pdf` | C5 | Detection rate delta per token with 95% CI |
| `c5_volcano_plot.pdf` | C5 | Effect size vs significance (-log10(p)) |
| `b2_confusion_matrix.pdf` | B2 | Verdict-to-category confusion matrix |
| `b3_coverage_boxplot.pdf` | B3 | Coverage distribution by differential category |
| `b3_coverage_histogram.pdf` | B3 | Overall coverage histogram |
| `component_metrics_table.tex` | All | LaTeX summary table for thesis appendix |

---

## 12. Running Tests

The evaluation crate has 42 integration tests across 13 test files. All tests use synthetic datasets from the shared fixture module — no external dependencies required.

### 12.1 Run All Tests

```bash
cargo test -p evaluation --features full
```

### 12.2 Test Organization

| Test File | Experiments Covered | Tests |
|-----------|-------------------|-------|
| `input_expressiveness.rs` | Input expressiveness metric | Module/mutation coverage, unique configs, edge cases |
| `input_validity.rs` | Input validity metric | Rejection/execution rates, mutation failure correlation |
| `input_diversity.rs` | Input diversity metric | Jaccard distance, entropy, config discovery, seq2 |
| `oracle_precision.rs` | Oracle precision metric | FP/FN proxy rates, trustworthy ratio, dryrun resolution |
| `oracle_soundness.rs` | Oracle soundness metric | Static ratio, evasion rate, blind spots |
| `oracle_attribution.rs` | Oracle attribution metric | Token ranking, top-5 stability, counterfactual |
| `oracle_stability.rs` | Oracle stability metric | Flaky rate, behavior match, config consistency |
| `guidance_feedback_quality.rs` | Guidance feedback quality | Coverage correlation, guidance strength, avoidance |
| `guidance_search_efficiency.rs` | Guidance search efficiency | Evasions/round, time-to-first, score trajectory |
| `guidance_baseline_comparison.rs` | Guidance baseline comparison | Evasion delta, score delta, mutation ablation |
| `guidance_convergence.rs` | Guidance convergence | Decay ratio, plateau onset, exploitation ratio |
| `classifier_coverage.rs` | B2 (standalone) | Exhaustive 11-branch classifier decision tree coverage |
| `mutation_impact.rs` | A2 (standalone) | 22-mutation catalog completeness, ablation table structure |

### 12.3 Run Specific Test Files

```bash
# Run tests for a single metric
cargo test -p evaluation --features full --test oracle_attribution

# Run tests for classifier coverage (B2)
cargo test -p evaluation --test classifier_coverage

# Run a single test by name
cargo test -p evaluation --features full --test guidance_convergence -- test_improvement_scenario

# Run with output (for debug info and tables printed by tests)
cargo test -p evaluation --features full -- --nocapture
```

### 12.4 Shared Test Fixtures

All integration tests use fixtures from `evaluation/tests/common/mod.rs` which provides 5 pre-built datasets:

| Fixture | Scenario | Rounds | Token Matrix | Selections |
|---------|----------|:------:|:------------:|:----------:|
| `mixed_dataset()` | Random outcomes (seed=42) | 30 | Enriched | No |
| `improvement_dataset()` | Detected -> evasion trajectory | 30 | Enriched | Yes |
| `plateau_dataset()` | Quick gain then flat | 30 | Basic | No |
| `all_detected_dataset()` | Every round detected | 20 | Basic | No |
| `all_evasion_dataset()` | Every round evades | 20 | Basic | No |

---

## 13. End-to-End Analysis Pipeline

This section walks through the complete workflow: generate data, run all evaluations, produce figures, and verify correctness.

### 13.1 Full Pipeline

```bash
# ── Step 1: Generate a synthetic dataset ──────────────────────────────
cargo run -p evaluation --bin eval-export -- \
  --scenario improvement --rounds 50 --enriched \
  --output eval_dataset.json

# ── Step 2: Run the 11 evaluation metrics ─────────────────────────────
cargo run -p evaluation --features full --bin evaluate -- \
  --input eval_dataset.json \
  --json eval_report.json \
  --csv eval_metrics.csv \
  --summary eval_summary.json

# ── Step 3: Run the 6 component-level experiments ─────────────────────
cargo run -p evaluation --features full --bin component-eval -- \
  --input eval_dataset.json \
  --output component_eval_report.json \
  --csv component_eval_metrics.csv

# ── Step 4: Generate thesis figures ───────────────────────────────────
python evaluation/scripts/plots.py \
  --input component_eval_report.json \
  --outdir figures/

# ── Step 5: Run all 42 tests to verify correctness ───────────────────
cargo test -p evaluation --features full
```

### 13.2 What Each Step Produces

| Step | Command | Outputs |
|------|---------|---------|
| 1 | `eval-export` | `eval_dataset.json` — synthetic dataset (50 rounds, enriched tokens) |
| 2 | `evaluate` | `eval_report.json` (detailed), `eval_metrics.csv` (flat), `eval_summary.json` (grouped by axis) |
| 3 | `component-eval` | `component_eval_report.json` (with plotting details), `component_eval_metrics.csv` |
| 4 | `plots.py` | 9 PDF/PNG figures + 1 LaTeX table in `figures/` |
| 5 | `cargo test` | 42 passing tests, 0 warnings |

### 13.3 Multi-Scenario Comparison

To compare how metrics behave across different outcome distributions:

```bash
for scenario in random improvement plateau detected evasion; do
  # Generate dataset
  cargo run -p evaluation --bin eval-export -- \
    --scenario $scenario --rounds 50 --enriched \
    --output ${scenario}_dataset.json

  # Run evaluation metrics
  cargo run -p evaluation --features full --bin evaluate -- \
    --input ${scenario}_dataset.json \
    --csv ${scenario}_eval.csv --quiet

  # Run component experiments
  cargo run -p evaluation --features full --bin component-eval -- \
    --input ${scenario}_dataset.json \
    --output ${scenario}_components.json \
    --csv ${scenario}_components.csv --quiet
done

# Compare in Python
python3 -c "
import pandas as pd

scenarios = ['random', 'improvement', 'plateau', 'detected', 'evasion']
frames = []
for s in scenarios:
    df = pd.read_csv(f'{s}_eval.csv')
    df['scenario'] = s
    frames.append(df)

combined = pd.concat(frames)
pivot = combined.pivot_table(index='metric_id', columns='scenario', values='value')
print(pivot.to_string(float_format='%.4f'))
"
```

### 13.4 From Real Job Data

After a live campaign completes, the same pipeline applies to real data:

```bash
# 1. Export job data from controller (see Section 4.1 for Rust code)
#    This produces: job_abc123_eval.json

# 2. Run the full analysis
cargo run -p evaluation --features full --bin evaluate -- \
  --input job_abc123_eval.json \
  --json job_abc123_report.json \
  --csv job_abc123_metrics.csv \
  --summary job_abc123_summary.json

cargo run -p evaluation --features full --bin component-eval -- \
  --input job_abc123_eval.json \
  --output job_abc123_components.json

# 3. Generate figures for thesis
python evaluation/scripts/plots.py \
  --input job_abc123_components.json \
  --outdir figures/job_abc123/

# 4. Verify test infrastructure still passes
cargo test -p evaluation --features full
```

---

## 14. Infrastructure-Level Experiments (I1–I15)

While the component experiments (Section 11) evaluate the system's campaign-level behavior, the **infrastructure experiments** evaluate each engineering contribution independently — payload encoding, mutation engines, template assembly, token extraction, and token scoring. These produce standalone benchmarks that measure correctness, performance, and effectiveness without needing any campaign data.

### 14.1 Architecture

The infrastructure evaluation mirrors the component pipeline but uses its own data type and binaries:

```
infra-bench (binary)   → exercises build + triage crate APIs with Instant::now() timing
                       → writes InfraEvalDataset JSON

infra-eval (binary)    → reads InfraEvalDataset JSON
                       → runs I1–I15 analysis modules
                       → writes infra_eval_report.json + CSV

plots.py --infra       → reads infra_eval_report.json
                       → generates thesis-quality PDF/PNG figures
```

The key difference from component experiments: **infra-bench actively calls build/triage APIs and records measurements**, whereas component-eval passively analyzes pre-existing campaign data.

### 14.2 Experiment Reference

| ID | Module | What It Measures | Feature Needed | Key Outputs |
|----|--------|-----------------|:--------------:|-------------|
| **I1** | `payload_encoding` | 4 encoding types across 8 payload sizes: entropy profiles, roundtrip correctness, size expansion, latency | `build-bench` | Grouped bar: entropy by type; expansion ratio table |
| **I2** | `ast_mutation_analysis` | 10 AST mutations: line delta, AST node delta, parse validity, transform latency | `build-bench` | Horizontal bar: per-mutation line delta |
| **I3** | `ir_mutation_analysis` | 3 IR mutations: insertion count, determinism from same seed, O2 survival (if `opt` available) | `build-bench` | Grouped bar: insertions; bloat ratio |
| **I4** | `binary_mutation_analysis` | 9 PE transforms: PE validity, size/section/import/entropy deltas | `build-bench` + PE | Heatmap: 9 mutations × 4 features |
| **I5** | `template_assembly_analysis` | 7-slot module system: marker resolution across combinations, assembly latency distribution | `build-bench` | Histogram: assembly time; marker resolution rate |
| **I6** | `instrumentation_analysis` | Weak-symbol linkage: baseline vs instrumented PE size, build time overhead per carrier | `build-bench` + toolchain | Grouped bar: size ratio per carrier |
| **I7** | `token_extraction_analysis` | Token extractor: category coverage (9 types), tokens-per-doc yield, latency, determinism | none | Stacked bar: tokens per category; latency histogram |
| **I8** | `token_scoring_validation` | Lift/confidence computation: mathematical correctness against known ground truth, degenerate input handling | none | Table: expected vs computed values per test case |
| **I9** | `input_diversity_analysis` | 10 AST mutations pairwise: structural distance, output uniqueness, parameter sensitivity | `build-bench` | Heatmap: pairwise distances; uniqueness fraction |
| **I10** | `oracle_stability_analysis` | Scoring robustness: determinism, permutation stability (Jaccard), incremental convergence, lift variance | none | Convergence line; permutation robustness score |
| **I11** | `selector_comparison_analysis` | 4 selectors (Coverage, Fuzzer, Token, Random): pool coverage, diversity, exploration rate | none | Grouped bar: coverage by selector; delta vs random |
| **I12** | `guidance_utilization_analysis` | Token/Coverage selectors: avoidance rate, seek adoption, recipe delta with vs without guidance | none | Bar: avoidance + adoption rates per selector |
| **I13** | `convergence_simulation_analysis` | 40-round simulation: phase transitions, recipe growth, diversity preservation, score plateau | none | Multi-panel trajectory: score, recipe size, diversity |
| **I14** | `line_tracing_analysis` | Line trace injection: scaling with source size, injection density, parse validity, throughput | `build-bench` | Scatter: time vs lines + regression; bar: injection density |
| **I15** | `sc_checkpoint_analysis` | INT3 checkpoint patching: 9 shellcodes × 5 counts, scaling by size/count, clamping, throughput | `build-bench` | 2×2: log-log size scaling, count scaling, clamping heatmap, throughput bar |

### 14.3 Quick Start

```bash
# Minimal run (I7, I8, I10–I13 only, no build crate needed)
cargo run -p evaluation --bin infra-bench -- \
  --experiments i7,i8,i10,i11,i12,i13 \
  --output infra_dataset.json

# Full run (all experiments — needs build crate + shellcodes for I15)
cargo run -p evaluation --features build-bench --bin infra-bench -- \
  --experiments i1,i2,i3,i5,i7,i8,i9,i10,i11,i12,i13,i14,i15 \
  --output infra_dataset.json

# Just the new instrumentation benchmarks (I14 + I15)
cargo run -p evaluation --features build-bench --bin infra-bench -- \
  --experiments i14,i15 \
  --output infra_dataset.json

# Run with REAL campaign data (I7,I8,I10-I13 use real rounds/tokens)
cargo run -p evaluation --bin infra-bench -- \
  --experiments i7,i8,i10,i11,i12,i13 \
  --dataset my_campaign.json \
  --output infra_dataset.json

# Run evaluation
cargo run -p evaluation --bin infra-eval -- \
  --input infra_dataset.json \
  --output infra_eval_report.json \
  --csv infra_eval_metrics.csv

# Generate thesis figures
python evaluation/scripts/plots.py --infra \
  --input infra_eval_report.json \
  --outdir figures/
```

When `--dataset` is provided, experiments I7, I8, I10–I13 use the real campaign's rounds, token matrices, and telemetry tokens instead of synthetic data. Other experiments (I1–I5, I9, I14, I15) exercise build-crate APIs directly and are unaffected.

### 14.4 Feature Requirements

Not all experiments require the same dependencies:

| Tier | Experiments | Cargo Command | What's Needed |
|------|------------|---------------|---------------|
| **No dependencies** | I7, I8, I10, I11, I12, I13 | `cargo run -p evaluation --bin infra-bench` | Controller crate only (always available) |
| **Build crate** | I1, I2, I3, I5, I9, I14 | `cargo run -p evaluation --features build-bench --bin infra-bench` | Build crate Rust APIs (no external toolchain) |
| **Build crate + shellcodes** | I15 | `cargo run -p evaluation --features build-bench --bin infra-bench` | Build crate + `data/shellcodes/*.bin` files |
| **Full toolchain** | I4, I6 | Not yet automated | Clang/LLVM + xwin SDK + compiled PE artifacts |

When an experiment is requested but its feature is not enabled, `infra-bench` prints a skip message and continues with the remaining experiments.

### 14.5 infra-bench Options

```bash
cargo run -p evaluation [--features build-bench] --bin infra-bench -- [OPTIONS]
```

| Flag | Description | Default |
|------|-------------|---------|
| `--experiments <IDS>` | Comma-separated experiment IDs: `i1`–`i15` | `i7,i8,i10,i11,i12,i13` (or `i1,i2,i3,i5,i7,i8,i10,i11,i12,i13,i14,i15` with `build-bench`) |
| `--output <PATH>` | Output `InfraEvalDataset` JSON | `infra_dataset.json` |
| `--dataset <PATH>` | Load real campaign `EvalDataset` JSON for I7,I8,I10–I13 | none (synthetic) |
| `--quiet` | Suppress progress output to stderr | off |

### 14.6 infra-eval Options

```bash
cargo run -p evaluation --bin infra-eval -- [OPTIONS]
```

| Flag | Description | Default |
|------|-------------|---------|
| `--input <PATH>` | Input `InfraEvalDataset` JSON | `infra_dataset.json` |
| `--output <PATH>` | Output JSON report (with full details for plotting) | `infra_eval_report.json` |
| `--csv <PATH>` | Output CSV summary | `infra_eval_metrics.csv` |
| `--experiment <ID>` | Only run one experiment: `i1`–`i15` | all |
| `--quiet` | Suppress stderr; print CSV to stdout | off |

### 14.7 Metric Reference

Each experiment produces multiple sub-metrics. All use `MetricResult` with structured `details` JSON for plotting.

**I1: Payload Encoding (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i1.payload_encoding.entropy_comparison` | Max entropy range across types | Per-type entropy by payload size |
| `infra.i1.payload_encoding.roundtrip_correctness` | Fraction correct (0.0–1.0) | Incorrect cases list |
| `infra.i1.payload_encoding.size_expansion` | Mean encoded/original ratio | Per-type expansion |
| `infra.i1.payload_encoding.latency` | Mean encode time (µs) | Per-type timing |

**I2: AST Mutations (3 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i2.ast_mutation.line_impact` | Mean absolute line delta | Per-mutation table with node deltas |
| `infra.i2.ast_mutation.parse_validity` | Fraction valid (0.0–1.0) | Invalid mutation list |
| `infra.i2.ast_mutation.transform_latency` | Mean transform time (µs) | Per-mutation timing |

**I3: IR Mutations (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i3.ir_mutation.insertion_effectiveness` | Insertions per input line | Per-mutation breakdown |
| `infra.i3.ir_mutation.o2_survival` | Fraction surviving `-O2` | Survival table |
| `infra.i3.ir_mutation.determinism` | Fraction deterministic | Count |
| `infra.i3.ir_mutation.line_bloat` | Mean output/input ratio | Per-mutation ratios |

**I4: Binary Mutations (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i4.binary_mutation.pe_validity` | Fraction valid PE | Validity table |
| `infra.i4.binary_mutation.size_impact` | Mean size ratio | Per-mutation deltas |
| `infra.i4.binary_mutation.entropy_shift` | Mean abs entropy delta | Per-mutation shifts |
| `infra.i4.binary_mutation.feature_heatmap` | Mutation count | 9×4 feature matrix |

**I5: Template Assembly (3 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i5.template_assembly.marker_resolution` | Fraction resolved (0.0–1.0) | Failed combinations |
| `infra.i5.template_assembly.combination_coverage` | Tested/576 | Coverage percent |
| `infra.i5.template_assembly.latency` | Mean assembly time (µs) | Histogram bins, all times |

**I6: Instrumentation Overhead (3 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i6.instrumentation.size_overhead` | Mean instrumented/baseline ratio | Per-carrier breakdown |
| `infra.i6.instrumentation.text_overhead` | Size ratio range | Min/max across carriers |
| `infra.i6.instrumentation.build_time_overhead` | Mean time ratio | Per-carrier timing |

**I7: Token Extraction (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i7.token_extraction.category_coverage` | Active/9 categories | Category table |
| `infra.i7.token_extraction.tokens_per_doc` | Mean tokens per doc | Per-run breakdown |
| `infra.i7.token_extraction.latency_distribution` | Mean extraction time (µs) | p50/p95/p99, all times |
| `infra.i7.token_extraction.determinism` | Fraction deterministic | Count |

**I8: Token Scoring (2 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i8.token_scoring.lift_accuracy` | 1 − max error (0.0–1.0) | Per-case expected vs computed |
| `infra.i8.token_scoring.guidance_correctness` | Fraction correct (0.0–1.0) | Incorrect cases |

**I9: Input Diversity (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i9.input_diversity.pairwise_distance` | Mean normalized distance between mutation outputs | Per-pair heatmap with min/max |
| `infra.i9.input_diversity.output_uniqueness` | Fraction of pairs producing distinct output (0.0–1.0) | Differ count |
| `infra.i9.input_diversity.param_sensitivity` | Fraction of mutations where params change output | Mutations with variants |
| `infra.i9.input_diversity.encoding_entropy_spread` | Entropy range across encoding types (bits) | Max − min entropy |

**I10: Oracle Stability (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i10.oracle_stability.determinism` | 1.0 if all repeated scoring runs identical | Binary pass/fail |
| `infra.i10.oracle_stability.permutation_robustness` | Mean top-5 Jaccard similarity across 10 permutations | Per-permutation scores |
| `infra.i10.oracle_stability.incremental_convergence` | Fraction of snapshots with Jaccard > 0.8 vs full | Round-by-round convergence |
| `infra.i10.oracle_stability.lift_variance` | Lift stability: 1/(1 + mean variance) | Per-token lift variance |

**I11: Selector Comparison (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i11.selector_comparison.coverage_by_selector` | Mean mutation pool coverage | Per-selector breakdown |
| `infra.i11.selector_comparison.diversity_by_selector` | Mean pairwise Jaccard distance of selections | Per-selector diversity |
| `infra.i11.selector_comparison.exploration_rate` | Mean exploration fraction | Per-selector rates |
| `infra.i11.selector_comparison.guided_vs_random_delta` | Coverage delta vs Random baseline | Per-selector improvement |

**I12: Guidance Utilization (3 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i12.guidance_utilization.avoidance_rate` | Mean fraction of rounds avoiding avoid-tokens | Per-selector rates |
| `infra.i12.guidance_utilization.seek_adoption_rate` | Mean fraction of rounds adopting seek-tokens | Per-selector rates |
| `infra.i12.guidance_utilization.recipe_delta` | Mean Jaccard distance (with vs without guidance) | Per-selector delta |

**I13: Convergence Simulation (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i13.convergence_simulation.phase_transition_round` | Round of Accumulation phase entry (fraction of total) | Phase transition list |
| `infra.i13.convergence_simulation.recipe_growth_rate` | Recipe size growth (mutations/round) during accumulation | Linear regression slope |
| `infra.i13.convergence_simulation.diversity_preservation` | Minimum diversity during accumulation | Diversity trajectory |
| `infra.i13.convergence_simulation.score_plateau_round` | Round where best score plateaus (fraction of total) | Score trajectory |

**I14: Line Tracing Overhead (4 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i14.line_tracing.throughput` | Mean source throughput (chars/µs) | Per-source breakdown with stddev |
| `infra.i14.line_tracing.injection_density` | Mean trace calls per source line | Per-source traces + deferred counts |
| `infra.i14.line_tracing.scaling` | Latency scaling coefficient (µs/line) | Slope + R² from linear regression |
| `infra.i14.line_tracing.validity` | Fraction of outputs that parse as valid C | Invalid source list |

**I15: Shellcode Checkpoint Patching (5 metrics)**

| Metric ID | Value | Details |
|-----------|-------|---------|
| `infra.i15.sc_checkpoint.throughput_by_size` | Mean patch throughput (bytes/µs) | Per-file breakdown |
| `infra.i15.sc_checkpoint.scaling_by_size` | Patch time scaling with size (µs/byte, count=5) | Slope + R² from linear regression |
| `infra.i15.sc_checkpoint.scaling_by_checkpoints` | Patch time scaling with count (µs/checkpoint) | Slope + R² for mid-size shellcode |
| `infra.i15.sc_checkpoint.clamping_rate` | Fraction where actual < requested checkpoints | Clamped cases list |
| `infra.i15.sc_checkpoint.boundary_correctness` | Fraction with correct instruction boundaries | Failure list |

### 14.8 Generated Figures

```bash
python evaluation/scripts/plots.py --infra \
  --input infra_eval_report.json --outdir figures/
```

| File | Experiment | Description |
|------|-----------|-------------|
| `i1_encoding_entropy.pdf` | I1 | Grouped bar: Shannon entropy by encoding type × payload size |
| `i1_encoding_expansion.pdf` | I1 | Grouped bar: mean/max size expansion per encoding type |
| `i2_ast_mutation_impact.pdf` | I2 | Horizontal bar: line delta per AST mutation |
| `i3_ir_mutation_analysis.pdf` | I3 | Grouped bar: insertions + line bloat per IR mutation |
| `i4_binary_mutation_heatmap.pdf` | I4 | Heatmap: 9 mutations × 4 features (normalized) |
| `i5_template_assembly.pdf` | I5 | Histogram: assembly time distribution |
| `i6_instrumentation_overhead.pdf` | I6 | Grouped bar: baseline vs instrumented size per carrier |
| `i7_token_extraction.pdf` | I7 | Stacked bar: tokens per category + latency histogram |
| `i8_scoring_validation.pdf` | I8 | Table figure: expected vs computed lift per test case |
| `i9_input_diversity.pdf` | I9 | Heatmap: pairwise mutation distances; output uniqueness |
| `i10_oracle_stability.pdf` | I10 | Multi-panel: determinism, permutation Jaccard, convergence |
| `i11_selector_comparison.pdf` | I11 | Grouped bar: coverage + diversity per selector |
| `i12_guidance_utilization.pdf` | I12 | Bar: avoidance + seek rates per selector |
| `i13_convergence_sim.pdf` | I13 | Multi-panel trajectory: score, recipe size, diversity, phases |
| `i14_line_tracing.pdf` | I14 | 1×2: scatter time vs lines + regression; grouped bar: injection density |
| `i15_sc_checkpoint.pdf` | I15 | 2×2: log-log size scaling, count scaling, throughput heatmap, bar |
| `infra_metrics_table.tex` | All | LaTeX summary table for thesis appendix |

### 14.9 The InfraEvalDataset Schema

The infrastructure pipeline uses `InfraEvalDataset` (parallel to `EvalDataset`). Each field is an optional vector of result structs populated by `infra-bench`:

```rust
pub struct InfraEvalDataset {
    pub payload_encoding:       Option<Vec<PayloadEncodingResult>>,       // I1
    pub ast_mutation:            Option<Vec<AstMutationResult>>,          // I2
    pub ir_mutation:             Option<Vec<IrMutationResult>>,           // I3
    pub binary_mutation:         Option<Vec<BinaryMutationResult>>,       // I4
    pub template_assembly:      Option<Vec<TemplateAssemblyResult>>,      // I5
    pub instrumentation:        Option<Vec<InstrumentationResult>>,       // I6
    pub token_extraction:       Option<Vec<TokenExtractionResult>>,       // I7
    pub token_scoring:          Option<Vec<TokenScoringResult>>,          // I8
    pub input_diversity:        Option<Vec<InputDiversityResult>>,        // I9
    pub oracle_stability:       Option<Vec<OracleStabilityResult>>,       // I10
    pub selector_comparison:    Option<Vec<SelectorComparisonResult>>,    // I11
    pub guidance_utilization:   Option<Vec<GuidanceUtilizationResult>>,   // I12
    pub convergence_simulation: Option<Vec<ConvergenceSimulationResult>>, // I13
    pub line_tracing:           Option<Vec<LineTracingResult>>,           // I14
    pub sc_checkpoint:          Option<Vec<ScCheckpointResult>>,          // I15
}
```

Fields are `None` for experiments that were not run. Analysis modules gracefully skip missing data and return empty results.

### 14.10 Programmatic Usage

```rust
use evaluation::{InfraEvalDataset, InfraMetric, all_infra_metrics, run_infra_evaluation};

// Load dataset
let json = std::fs::read_to_string("infra_dataset.json")?;
let dataset: InfraEvalDataset = serde_json::from_str(&json)?;

// Run all infrastructure metrics
let results = run_infra_evaluation(&dataset);

// Or run individual metrics (e.g., just I14 line tracing)
let metrics = all_infra_metrics();
for metric in &metrics {
    if metric.metric_id().starts_with("infra.i14") {
        let metric_results = metric.evaluate(&dataset)?;
        for r in metric_results {
            println!("{}: {:.4}", r.metric_id, r.value);
        }
    }
}
```

### 14.11 Example Output

```
$ cargo run -p evaluation --features build-bench --bin infra-bench -- \
    --experiments i1,i2,i3,i5,i7,i8,i10,i11,i12,i13,i14,i15 \
    --output infra_dataset.json
Infrastructure benchmark runner
Experiments: ["i1", "i2", "i3", "i5", "i7", "i8", "i10", "i11", "i12", "i13", "i14", "i15"]

Running I1: Payload Encoding...
  xor size=  256 encoded=   256 entropy=1.066 time=6µs
  ...
Running I8: Token Scoring Validation...
  perfect_correlation: exp=2.000 got=2.000 err=0.000000 ok=true
  ...
Running I10: Oracle Stability...
  deterministic=true perm_jaccard=0.800 lift_var=0.000123
Running I11: Selector Comparison...
  Coverage   coverage=0.80 unique_sets=15 mean_size=2.3 explore=0.30
  Fuzzer     coverage=0.70 unique_sets=20 mean_size=3.1 explore=0.00
  Token      coverage=0.90 unique_sets=18 mean_size=2.5 explore=0.27
  Random     coverage=1.00 unique_sets=30 mean_size=2.0 explore=1.00
Running I14: Line Tracing Overhead...
  reference_c           lines=   70 traces=  35 deferred=  3 mean=150µs valid=true
  synthetic_1000        lines= 1000 traces= 500 deferred= 50 mean=2100µs valid=true
Running I15: Shellcode Checkpoint Patching...
  calc64.bin               size=    272 count= 5 actual= 5 mean=      25.0µs throughput=10.9 bytes/µs
  NimPlant.bin             size= 878592 count= 5 actual= 5 mean=   12000.0µs throughput=73.2 bytes/µs
  ...
Wrote infra_dataset.json

$ cargo run -p evaluation --bin infra-eval -- --input infra_dataset.json
  infra.i1.payload_encoding.entropy_comparison       1.8099  (n=32)
  infra.i1.payload_encoding.roundtrip_correctness    1.0000  (n=32)
  infra.i2.ast_mutation.parse_validity               1.0000  (n=10)
  infra.i3.ir_mutation.determinism                   1.0000  (n=4)
  infra.i5.template_assembly.marker_resolution       1.0000  (n=36)
  infra.i7.token_extraction.determinism              1.0000  (n=5)
  infra.i8.token_scoring.lift_accuracy               1.0000  (n=7)
  infra.i8.token_scoring.guidance_correctness        1.0000  (n=7)
  infra.i10.oracle_stability.determinism             1.0000  (n=1)
  infra.i10.oracle_stability.permutation_robustness  0.8000  (n=1)
  infra.i11.selector_comparison.coverage_by_selector 0.8500  (n=4)
  infra.i12.guidance_utilization.avoidance_rate       0.8500  (n=2)
  infra.i13.convergence_simulation.phase_transition   0.2500  (n=1)
  infra.i14.line_tracing.throughput                  12.3456  (n=5)
  infra.i14.line_tracing.validity                     1.0000  (n=5)
  infra.i15.sc_checkpoint.throughput_by_size          42.1234  (n=45)
  infra.i15.sc_checkpoint.boundary_correctness        1.0000  (n=45)
  INFRASTRUCTURE EVALUATION: 41 metrics from 15 experiments
```
