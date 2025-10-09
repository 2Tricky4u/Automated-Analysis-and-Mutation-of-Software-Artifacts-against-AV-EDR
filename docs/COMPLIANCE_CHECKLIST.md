# CLAUDE.md Compliance Checklist

**Last Updated:** 2025-01-10
**Project Version:** 0.1.0

## Architecture (Section 2) ✅

- [x] Controller with modular components
  - [x] Scheduler (`controller/scheduler/`)
  - [x] Mutator (`controller/mutator/`)
  - [x] Selector (`controller/selector/`)
  - [x] Queue (`controller/queue/`)
  - [x] Triage Engine (`controller/triage-engine/`)
  - [x] Rule Manager (`controller/rule-manager/`)
- [x] Build/Emitter (`build/emitter/`)
- [x] Worker pool (`worker/agent/`)
  - [x] Harness (`worker/harness/`)
  - [x] IPC library (`worker/harness-ipc/`)
- [x] Collector (`telemetry/collector/`)
- [x] Storage (Elastic + Kibana via Docker Compose)
- [x] Modular by interface contracts (gRPC proto split)

**Status:** ✅ Skeleton Complete (95%)

---

## Fuzzer & Mutation Engine (Section 3) ⚠️

- [x] Corpus structure (`controller/mutator/src/corpus/`)
- [ ] AST transforms (placeholder)
  - [ ] Control-flow jitter
  - [ ] Opaque predicates
  - [ ] Import reshaping
- [ ] IR transforms (LLVM integration pending)
- [ ] Binary transforms (placeholder)
- [ ] Behavioral mutations (placeholder)
- [x] Selector with feedback (`controller/selector/`)
- [x] Queue with prioritization (`controller/queue/`)

**Status:** ⚠️ Structure Ready (40%)

---

## Differential Analysis (Section 4) 🔴

- [x] Component created (`controller/differential-analyzer/`)
- [ ] Scan-time vs runtime comparison logic
- [ ] Token → likelihood mapping
- [ ] Confidence scoring

**Status:** 🔴 Not Implemented (10%)

---

## Telemetry Channels (Section 5) ⚠️

### Baseline (Implemented)
- [x] ETW consumer (C++ krabsetw)
- [x] Process/thread/image events
- [x] Network events
- [x] File operations
- [x] Registry modifications

### RedEDR Integration
- [x] Collector structure (`telemetry/collector/`)
- [ ] JSON parser implementation
- [ ] Elasticsearch shipper
- [ ] Real-time file watching

### Missing Channels (18+ from CLAUDE.md)
- [ ] GPU/Compute ETW
- [ ] COM Activation ETW
- [ ] BITS Client ETW
- [ ] TLS/DNS fingerprints (JA3/JA4)
- [ ] Prefetch/Amcache diffs
- [ ] Process Mitigation Policy snapshots
- [ ] Syscall-route profiling
- [ ] RW/RX micro-timings
- [ ] WMI Activity ETW
- [ ] USN Journal
- [ ] Named-Pipe topology
- [ ] PowerShell ScriptBlock logging

**Status:** ⚠️ Partial (30%)

---

## Canonical Schemas (Section 6) ✅

- [x] RunResult canonical format (`docs/schemas/runresult-canonical.json`)
- [x] Telemetry event schema (`docs/schemas/telemetry-schema.json`)
- [x] Job schema (`docs/schemas/job-schema.json`)
- [x] Proto definitions (modular split)

**Status:** ✅ Complete (100%)

---

## Output Formats (Section 7) ✅

- [x] Experiment plan template (`docs/templates/experiment-plan.md`)
- [x] Mutation recipe DSL (`docs/templates/mutation-recipe.yml`)
- [x] Triage hypothesis report (`docs/templates/triage-report.md`)
- [ ] LaTeX output generator (not required for MVP)
- [ ] Sigma/KQL rule generator (placeholder in `rule-manager`)

**Status:** ✅ Templates Complete (90%)

---

## Checklists (Section 8) ⚠️

### Lab & Collector
- [x] Docker Compose for Elastic Stack
- [ ] Windows VM images (user responsibility)
- [x] ETW consumer (existing)
- [x] Collector structure (placeholder)
- [ ] SLO metrics collection (not implemented)

### Triage & Rules
- [x] Triage engine structure
- [ ] Surrogate classifier implementation
- [x] Kibana dashboards (configs exist)
- [ ] Rule export/import (placeholder)
- [ ] Hypothesis text generator (placeholder)

### Basic Fuzzer Loop
- [x] Mutator framework (structure)
- [x] Queue (structure)
- [ ] End-to-end run (not yet integrated)
- [ ] Deterministic seeds (structure ready)

**Status:** ⚠️ Partial (50%)

---

## Evaluation Metrics (Section 9) 🔴

- [ ] Explainability accuracy
- [ ] Evasion rate
- [ ] Transformation cost
- [ ] Differential mapping quality
- [ ] Repeatability tracking

**Status:** 🔴 Not Implemented (0%)

---

## Ethics & Safety (Section 10) ✅

- [x] Lab-only policy documented (`docs/appendix/ETHICS.md`)
- [x] Data sanitization guidelines
- [x] Responsible disclosure process
- [x] No operational payloads (design principle)
- [x] Blue team focus (documented in CLAUDE.md)

**Status:** ✅ Complete (100%)

---

## Documentation (Section 14) ✅

- [x] Architecture overview (`docs/ARCHITECTURE.md`)
- [x] Experiment plan template
- [x] Mutation recipe template
- [x] Triage report template
- [x] Ethics appendix
- [x] Reproducibility guidelines (`docs/appendix/REPRODUCIBILITY.md`)

**Status:** ✅ Complete (100%)

---

## Infrastructure ✅

### Docker Compose
- [x] Elasticsearch + Kibana
- [x] Controller service
- [x] Worker services (2x)
- [x] Collector service
- [x] Filebeat
- [x] Multi-stage Dockerfiles
- [x] Development overrides

### gRPC
- [x] Modular proto files (common, controller, worker)
- [x] Service definitions
- [x] Canonical types (RunResult, Mutation, etc.)

**Status:** ✅ Complete (100%)

---

## Overall Compliance Score

| Category | Score | Weight | Weighted |
|----------|-------|--------|----------|
| Architecture | 95% | 20% | 19.0% |
| Fuzzer & Mutation | 40% | 15% | 6.0% |
| Differential Analysis | 10% | 10% | 1.0% |
| Telemetry | 30% | 15% | 4.5% |
| Schemas | 100% | 10% | 10.0% |
| Output Formats | 90% | 5% | 4.5% |
| Checklists | 50% | 5% | 2.5% |
| Metrics | 0% | 5% | 0.0% |
| Ethics | 100% | 5% | 5.0% |
| Documentation | 100% | 5% | 5.0% |
| Infrastructure | 100% | 5% | 5.0% |
| **TOTAL** | | **100%** | **62.5%** |

---

## Critical Path to 100%

### Phase 1: Implement Core Logic (Weeks 1-2)
1. Mutator engine with real AST/binary transforms
2. Triage engine with surrogate classifier
3. Selector with feedback loop
4. Queue with corpus management

### Phase 2: RedEDR Integration (Week 3)
1. Collector JSON parser
2. Elasticsearch bulk shipper
3. Real-time file watching
4. Worker harness integration

### Phase 3: Differential Analysis (Week 4)
1. Scan-time API integration
2. Runtime comparison logic
3. Token mapping with confidence

### Phase 4: Validation & Metrics (Week 5)
1. End-to-end integration test
2. Evaluation metrics collection
3. Repeatability verification
4. Performance benchmarks

---

## Known Limitations

1. **Windows-only telemetry:** RedEDR requires Windows
2. **No GUI:** Command-line only (Kibana for visualization)
3. **Manual VM setup:** Docker runs control plane only
4. **Placeholder implementations:** Many services are stubs

---

## Next Steps

1. Run `cargo build --workspace` to verify compilation
2. Start infrastructure: `cd build/dockerfiles && docker-compose up -d`
3. Implement mutator engine (see guide Phase 3)
4. Implement triage engine (see guide Phase 3)
5. Test end-to-end with simple artifact
