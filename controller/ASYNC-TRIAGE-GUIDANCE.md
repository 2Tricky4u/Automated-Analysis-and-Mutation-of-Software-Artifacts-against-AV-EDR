# Async Triage Guidance — Future Implementation Guide

## Context

The **CoverageSelector** (v0, implemented) closes the feedback loop using round-level signals only (`evasion_score` + `DifferentialCategory`). It varies the `deconditioner` module via epsilon-greedy exploration/exploitation on in-memory history.

This document describes the **next step**: async token-level triage guidance that runs in the background and feeds into the selector without blocking round production.

---

## Core Principle: Never Block Round Production

```
Round 7:  produced with guidance=None      <-- doesn't wait
Round 8:  produced with guidance=None      <-- still doesn't wait
            ... triage for rounds 1-6 completes ...
Round 9:  produced with guidance=Some(...)  <-- uses whatever is ready
Round 10: produced with guidance=Some(...)  <-- same guidance, or updated
            ... triage for rounds 7-8 completes ...
Round 11: produced with guidance=Some(updated...)
```

The pipeline stays **non-sequential**. Rounds keep flowing at build speed (~30-120s each). Triage extraction (~2-5s) runs in the background and pushes updates when ready.

---

## Architecture

```
                       JobWorker select! loop
                      ┌──────────────────────────────────────┐
                      │                                      │
  result_rx ─────────>│  on_result()                         │
                      │       │                              │
                      │       v                              │
                      │  finalize_round()                    │
                      │       │                              │
                      │       ├─ record_round_summary()      │
                      │       │   (selector reads from here) │
                      │       │                              │
                      │       └─ tokio::spawn(               │
                      │            extract_triage(...)        │
                      │               │                      │
                      │               └──> guidance_tx.send() │
                      │                         │            │
  guidance_rx ───────>│  latest_guidance = Some(...)          │
                      │                                      │
  check_interval ───> │  produce_round()                     │
                      │       │                              │
                      │       v                              │
                      │  selector.select(                    │
                      │      ...,                            │
                      │      history,                        │
                      │      self.latest_guidance.as_ref()   │
                      │  )                                   │
                      └──────────────────────────────────────┘
```

---

## What Exists (v0)

| Component | Status | Location |
|-----------|--------|----------|
| `Selector` trait with `guidance: Option<&TriageGuidance>` | Implemented | `triage/mod.rs` |
| `TriageGuidance { avoid_tokens, seek_tokens }` type | Defined (unused) | `triage/mod.rs` |
| `CoverageSelector` (ignores guidance, uses evasion_score) | Implemented | `triage/coverage_selector.rs` |
| `JobWorker.selector: Arc<dyn Selector>` | Wired | `dispatch/job_worker.rs` |
| `produce_round()` calls `selector.select(..., None)` | Wired | `dispatch/job_worker.rs:281` |
| `finalize_round()` records summary to `job.rounds` | Wired | `dispatch/job_worker.rs` |
| `modules` field on `RoundSpec`, `RoundSummary`, ES docs | Stored | `dispatch/types.rs`, `storage/rounds.rs` |

---

## What To Add

### 1. Guidance channel on JobWorker

```rust
// dispatch/job_worker.rs

pub struct JobWorker {
    // ... existing fields ...

    /// Latest triage guidance (updated asynchronously)
    latest_guidance: Option<TriageGuidance>,

    /// Receives guidance updates from background triage tasks
    guidance_rx: mpsc::Receiver<TriageGuidance>,

    /// Sender cloned into each background triage task
    guidance_tx: mpsc::Sender<TriageGuidance>,
}
```

In the constructor:
```rust
let (guidance_tx, guidance_rx) = mpsc::channel(16);
// ...
Self {
    // ...
    latest_guidance: None,
    guidance_rx,
    guidance_tx,
}
```

### 2. New arm in the select! loop

```rust
// dispatch/job_worker.rs — inside run()

loop {
    tokio::select! {
        biased;

        _ = self.shutdown_token.cancelled() => { ... }
        _ = pool_shutdown.cancelled() => { ... }

        // Receive results from VMs
        Some(result) = self.result_rx.recv() => {
            self.on_result(result).await;
        }

        // NEW: Receive triage guidance (non-blocking update)
        Some(guidance) = self.guidance_rx.recv() => {
            info!(
                "[JobWorker:{}] Triage guidance received: {} avoid, {} seek tokens",
                self.job.id,
                guidance.avoid_tokens.len(),
                guidance.seek_tokens.len()
            );
            self.latest_guidance = Some(guidance);
        }

        _ = check_interval.tick() => {
            if self.can_produce_round()
                && let Err(e) = self.produce_round().await { ... }
            if self.is_job_complete() { ... }
        }
    }
}
```

### 3. Pass guidance in produce_round()

```rust
// dispatch/job_worker.rs — produce_round()

// Changes from:
let selection = self.selector.select(
    &self.job.id.0,
    round_num,
    &self.job.search_space,
    &self.job.build_spec.modules,
    &self.job.rounds,
    None,  // <-- was always None
).await;

// To:
let selection = self.selector.select(
    &self.job.id.0,
    round_num,
    &self.job.search_space,
    &self.job.build_spec.modules,
    &self.job.rounds,
    self.latest_guidance.as_ref(),  // <-- uses whatever is available
).await;
```

### 4. Spawn triage extraction in finalize_round()

```rust
// dispatch/job_worker.rs — finalize_round(), after record_round_summary()

// Spawn async triage extraction (non-blocking)
let guidance_tx = self.guidance_tx.clone();
let storage = self.storage.clone();  // needs Arc<EsStorage> on JobWorker
let job_id = self.job.id.0.clone();
let round_id = round_id.0.clone();
let baseline_run_id = baseline_run_id.0.clone();
let instrumented_run_id = instrumented_run_id.0.clone();
let summary_clone = summary.clone();

tokio::spawn(async move {
    match extract_and_score(&storage, &job_id, &round_id,
        &baseline_run_id, &instrumented_run_id, &summary_clone).await
    {
        Ok(guidance) => {
            let _ = guidance_tx.send(guidance).await;
        }
        Err(e) => {
            warn!("[Triage:{}] Extraction failed (non-fatal): {}", job_id, e);
            // Round production continues unaffected
        }
    }
});
```

### 5. Token extraction function (new module)

```rust
// triage/extractor.rs (new file)

use crate::storage::EsStorage;
use super::TriageGuidance;

/// Extract tokens from ES telemetry, compute lift, return guidance.
///
/// This runs in a background task — it can take 2-5 seconds without
/// blocking round production.
pub async fn extract_and_score(
    storage: &EsStorage,
    job_id: &str,
    round_id: &str,
    baseline_run_id: &str,
    instrumented_run_id: &str,
    summary: &RoundSummary,
) -> anyhow::Result<TriageGuidance> {
    // 1. Query ES telemetry-* for this round's runs
    //    Filter: job_id + run_id, exists(payload_func)
    //    Sort: timestamp asc

    // 2. Parse tokens from telemetry events:
    //    - api:<func>          from payload_func
    //    - etw:<prov>/<id>     from payload_provider + event_type
    //    - seq2:<a>-><b>       from consecutive payload_func values
    //    - module:<cat>=<val>  from summary.modules

    // 3. Index token set to tokens-* for this round

    // 4. Query tokens-* for ALL rounds of this job
    //    Aggregate: by token -> avg(detected), count

    // 5. Compute lift per token:
    //    lift(T) = P(detected|T) / P(detected)
    //    confidence(T) = min(1.0, count(T) / 5)

    // 6. Build guidance:
    //    avoid = tokens where lift > 1.5 AND confidence > 0.3
    //    seek  = tokens where lift < 0.5 AND confidence > 0.3

    Ok(TriageGuidance {
        avoid_tokens,
        seek_tokens,
    })
}
```

### 6. TokenSelector (or upgrade CoverageSelector)

Two options:

**Option A: Upgrade CoverageSelector** to use guidance when available:
```rust
// In CoverageSelector::select():
if let Some(guidance) = guidance {
    // Score variants by token overlap with avoid/seek sets
    // Penalize variants whose expected tokens overlap with avoid_tokens
    // Bonus for variants whose expected tokens overlap with seek_tokens
    // Fall back to evasion_score when guidance is absent
}
```

**Option B: New TokenSelector** implementing the same trait:
```rust
pub struct TokenSelector;

#[async_trait]
impl Selector for TokenSelector {
    async fn select(..., guidance: Option<&TriageGuidance>) -> Selection {
        match guidance {
            None => {
                // Delegate to CoverageSelector behavior
                CoverageSelector::new().select(..., None).await
            }
            Some(g) => {
                // Token-aware selection:
                // For each variant, compute expected_tokens()
                // Score = -|expected ∩ avoid| + |expected ∩ seek| + novelty_bonus
                // Epsilon-greedy on the scored list
            }
        }
    }
}
```

**Recommendation:** Option A (upgrade CoverageSelector). Keeps one selector, simpler. The trait/architecture already supports swapping via `Arc<dyn Selector>` if a fundamentally different strategy is needed later.

---

## Staleness Is Acceptable

| Round produced | Guidance available from | Staleness |
|----------------|------------------------|-----------|
| Round 1 | None | N/A (baseline) |
| Round 2 | None | Triage for R1 still running |
| Round 3 | Rounds 1 | 1 round stale |
| Round 4 | Rounds 1-2 | 1 round stale |
| Round N | Rounds 1..(N-2) | Typically 1-2 rounds stale |

Why this is fine:
- Build + execution = 30-120s per round
- Token extraction + scoring = 2-5s
- By round N+2, tokens from round N are always available
- First rounds use exploration anyway (CoverageSelector covers untried variants)
- The selector gracefully degrades: `None` guidance = coverage-only, which already works

---

## Data Flow Summary

```
finalize_round(round N)
    │
    ├─ record_round_summary()     ← immediate, in-memory
    │   (CoverageSelector uses this for evasion_score-based selection)
    │
    └─ tokio::spawn(extract_and_score())   ← background, 2-5s
        │
        ├─ Query ES telemetry-* for round N
        ├─ Parse tokens (api, etw, seq2, module)
        ├─ Index to tokens-*
        ├─ Query tokens-* for ALL job rounds
        ├─ Compute lift/confidence per token
        ├─ Build avoid/seek sets
        └─ guidance_tx.send(TriageGuidance)
                │
                ▼
        guidance_rx arm in select! loop
                │
                ▼
        self.latest_guidance = Some(guidance)
                │
                ▼
        produce_round(round N+2 or later)
            selector.select(..., self.latest_guidance.as_ref())
```

---

## Files To Change (When Implementing)

| # | File | Change |
|---|------|--------|
| 1 | `triage/mod.rs` | Remove `#[allow(dead_code)]` from `TriageGuidance` |
| 2 | `triage/extractor.rs` | **NEW**: `extract_and_score()` function |
| 3 | `triage/coverage_selector.rs` | Handle `Some(guidance)` in `select()` |
| 4 | `dispatch/job_worker.rs` | Add `latest_guidance`, `guidance_rx/tx`, new `select!` arm, spawn in `finalize_round()` |
| 5 | `storage/mod.rs` | Add `query_telemetry_for_round()`, `index_token_set()`, `query_token_scores()` |
| 6 | `storage/templates.rs` | Add `tokens-*` index template |
| 7 | `dispatch/orchestrator.rs` | Pass `Arc<EsStorage>` to JobWorker (for triage extraction) |

**No changes to:** `build/` crate, `worker/agent/`, `proto/`, `config/`, `Selector` trait signature.

---

## Testing Strategy

### Unit tests (no ES)
- CoverageSelector with `Some(TriageGuidance)` — verify avoid tokens reduce variant scores
- CoverageSelector with empty avoid/seek — same as `None` behavior
- Token parsing from mock telemetry events

### Integration tests (need ES)
- Index mock telemetry, run `extract_and_score()`, verify `TriageGuidance` output
- Full loop: 3 rounds with mock ES data, verify guidance propagates and affects selection

### Manual verification
```bash
# Watch guidance flow in logs
RUST_LOG=info cargo run -p scheduler 2>&1 | grep -E "Selector|Triage guidance"

# Check token extraction results
curl -s 'localhost:9200/tokens-*/_search?size=5' \
  | jq '.hits.hits[]._source | {job_id, round_id, token_count, tokens}'

# Check lift scores
curl -s 'localhost:9200/tokens-*/_search' -d '{
  "size": 0,
  "aggs": {
    "by_token": {
      "terms": {"field": "tokens.keyword", "size": 20},
      "aggs": {"det_rate": {"avg": {"field": "detected"}}}
    }
  }
}' | jq '.aggregations.by_token.buckets[] | {token: .key, det_rate: .det_rate.value, count: .doc_count}'
```