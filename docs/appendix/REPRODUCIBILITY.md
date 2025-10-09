# Reproducibility Guidelines

## Overview

Per CLAUDE.md Section 16, all experiments must be **fully reproducible** with recorded artifact IDs, seeds, and collector configuration.

## Deterministic Builds

### Toolchain Pinning

All builds must use pinned toolchain versions:

```dockerfile
# Dockerfile example
FROM rust:1.75-slim AS builder  # Pin exact version

# Pin protoc version
RUN wget https://github.com/protocolbuffers/protobuf/releases/download/v25.0/protoc-25.0-linux-x86_64.zip
```

### Build Flags

Record all build flags in run metadata:

```json
{
  "build_config": {
    "toolchain": "rust-1.75",
    "target": "x86_64-pc-windows-gnu",
    "optimization": "release",
    "features": ["mutation-ast", "mutation-binary"],
    "env": {
      "RUSTFLAGS": "-C target-cpu=native"
    }
  }
}
```

### Artifact Verification

Every build produces:
- **SHA-256 hash:** Stored in `artifact_id` field
- **Build log:** Capture stdout/stderr
- **Source snapshot:** Commit hash or tarball

## Deterministic Seeds

### Seed Storage

All randomness must be seeded:

```rust
use rand::SeedableRng;

let seed = [0x12, 0x34, ...];  // From mutation recipe
let mut rng = rand::rngs::StdRng::from_seed(seed);
```

### Seed Propagation

Seeds flow through pipeline:

```
Mutation Recipe (seed) → Mutator (RNG) → Artifact → Run (trace)
```

### Verification

Re-run with same seed must produce:
- Identical artifact hash
- Identical mutation transformations
- Identical random decisions (jitter, timing, etc.)

## Telemetry Collection

### Collector Configuration

Record all collector settings in `RunResult`:

```json
{
  "collector_config": {
    "etw": {
      "buffersize_kb": 1024,
      "lost_events": 0
    },
    "rededr": {
      "enable_etw": true,
      "enable_etwti": false,
      "enable_kernel": true
    },
    "collector": {
      "threads": 4,
      "cache_pools": 4,
      "parser": "sliding"
    },
    "semantic_enrichment": {
      "fixups": ["filekey->name", "thread->process"]
    }
  }
}
```

### SLO Metrics

Track data quality:

- **lost_events:** Must be 0 for reproducible runs
- **event_to_record_ms_p95:** Latency from event to Elastic
- **buffer_overflows:** Count per provider

### Telemetry Snapshot

For critical experiments, export raw telemetry:

```bash
# Export Elasticsearch data
curl -X POST "localhost:9200/rededr-*/_search?scroll=1m" \
  -H 'Content-Type: application/json' \
  -d '{"query":{"term":{"run_id":"abc-123"}}}' \
  | jq -r '.hits.hits[]._source' > run-abc-123-telemetry.ndjson
```

## Experiment Metadata

### Canonical RunResult

Every run produces (CLAUDE.md Section 6):

```json
{
  "run_id": "550e8400-e29b-41d4-a716-446655440000",
  "artifact_id": "sha256:abcd1234...",
  "worker_id": "worker-01",
  "mutations": [
    {"id": "ast.import_reshape", "params": {"delay_load": true}},
    {"id": "beh.preamble.fs", "params": {"fs_ops": 3}}
  ],
  "start_ts": "2025-01-10T12:00:00.000Z",
  "end_ts": "2025-01-10T12:05:23.456Z",
  "status": "detected",
  "labels": {
    "telemetry_seen": true,
    "alert_level": "high",
    "blocked": false,
    "detection_latency_ms": 1234
  },
  "perf": {
    "cpu_pct": 3.1,
    "mem_mb": 42,
    "event_to_record_ms_p95": 180
  },
  "notes": "Detected by signature at syscall layer"
}
```

### VM State

Record VM configuration:

```json
{
  "vm_config": {
    "snapshot": "defender-baseline-v2.3",
    "snapshot_hash": "sha256:feed...",
    "os_version": "Windows 10 22H2 (19045.3803)",
    "defender_version": "1.401.155.0",
    "memory_mb": 4096,
    "cpu_cores": 2
  }
}
```

## Verification Procedure

### Repeatability Test

To verify reproducibility:

1. **Same Seed, Same Worker:**
   ```bash
   run_experiment --seed=0x1234 --worker=worker-01
   run_experiment --seed=0x1234 --worker=worker-01
   # Compare: artifact_id, status, detection_latency_ms
   ```

2. **Cross-Worker Validation:**
   ```bash
   run_experiment --seed=0x1234 --worker=worker-01
   run_experiment --seed=0x1234 --worker=worker-02
   # Expect: same artifact_id, similar telemetry
   ```

3. **Temporal Stability:**
   ```bash
   run_experiment --seed=0x1234 --date=2025-01-10
   run_experiment --seed=0x1234 --date=2025-02-10
   # Track: signature updates, detection drift
   ```

### Acceptance Criteria

A run is **reproducible** if:
- ✅ Artifact hash matches (100% identical)
- ✅ Status matches (detected/not_detected)
- ✅ Detection latency within ±10%
- ✅ Telemetry event count within ±5%
- ✅ No collector errors (lost_events=0)

## Archival

### Dataset Export

For publication:

```bash
# Export full dataset
./scripts/export_dataset.sh \
  --experiment-id=exp-2025-001 \
  --sanitize=true \
  --format=ndjson \
  --output=dataset-exp-2025-001.tar.gz
```

Contents:
- `runresults.ndjson`: All RunResult objects
- `telemetry/`: Sanitized telemetry data
- `artifacts/`: SHA-256 hashes and metadata (not binaries)
- `config/`: Collector configs, seeds, VM snapshots
- `README.md`: Reproduction instructions

### Long-Term Storage

- **Hot storage:** Last 90 days in Elastic
- **Cold storage:** Export to S3/MinIO with ILM
- **Archival:** Compress with Zstandard, verify checksums

## Troubleshooting

### Non-Reproducible Results

Common causes:

1. **Different collector config:** Check `buffersize_kb`, `threads`
2. **VM drift:** Snapshot corrupted, verify hash
3. **Network timing:** Disable if not under test
4. **Non-seeded RNG:** Audit code for `rand::thread_rng()`

### Data Loss

If `lost_events > 0`:
- Increase `buffersize_kb` to 2048 or 4096
- Reduce event volume (disable verbose providers)
- Flag run as non-reproducible

---

**Last Updated:** [Date]
**Version:** 1.0
