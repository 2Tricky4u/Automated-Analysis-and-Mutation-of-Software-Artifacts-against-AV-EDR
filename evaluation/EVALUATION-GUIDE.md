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
| `mutation:` | Applied mutations | `mutation:ast.fill_pattern` |
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
