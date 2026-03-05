# AutoMutate++ — Global System Architecture

A unified architecture overview synthesizing all components of the AutoMutate++ EDR evaluation framework. This document describes the *what* and *why* at the system level; per-component implementation details live in the referenced documents.

---

## 1. Introduction and Research Objective

Endpoint Detection and Response (EDR) systems employ layered detection mechanisms — static file analysis, behavioral monitoring via ETW, memory scanning, and machine-learning classifiers — making manual evaluation of *why* an artifact is detected a slow, opaque process. A researcher modifying one aspect of an artifact (e.g., removing a suspicious API call) cannot easily isolate whether that change, or a correlated side-effect, caused the detection outcome to change.

AutoMutate++ addresses this by implementing a **closed experimental loop**: mutate an artifact, execute it under monitoring, collect telemetry, extract normalized tokens from the telemetry, score tokens by correlation with detection, and use those scores to guide the next round of mutations. The system automates the entire cycle — from mutation selection through execution and telemetry analysis — producing explainable, evidence-driven hypotheses about which observable behaviors trigger EDR detections.

All experiments run in isolated lab VMs. No operational payloads are produced; the system imitates and varies suspicious behavior patterns so that blue-team researchers can understand EDR decision boundaries.

---

## 2. System Overview

### Summary Statistics

| Metric | Value |
|--------|-------|
| Controller | 15,674 LOC, 38 files, 5 modules |
| Build crate | 22 mutations, 7 trace modes, 3-layer pipeline |
| Worker agent | 5,926 LOC, 24 files, 10-phase execution pipeline |
| Proto definitions | 1,160 lines, 121 messages, 37 RPCs, 5 services |
| Automation scripts | 20+ PowerShell/Bash scripts, 2 deployment modes |
| Language | Rust (tokio async), C (runtime/templates), PowerShell (infra) |

### Technology Stack

| Layer | Technology |
|-------|-----------|
| Orchestration runtime | Rust + Tokio (async, multi-threaded) |
| Inter-component RPC | gRPC via tonic + prost (Protobuf 3) |
| Artifact compilation | Clang/LLVM 17 + LLD + xwin SDK (Linux → Windows PE cross-compilation) |
| AST analysis | tree-sitter (C/C++ grammar) |
| PE manipulation | goblin (Rust crate for PE parsing/writing) |
| Telemetry storage | ElasticSearch 8.11 + Kibana 8.11 |
| VM platform | Hyper-V Gen2 (local) or any reachable Windows host (remote) |
| ETW collection | RedEDR (kernel ETW + ETW-TI + optional hooking) |
| Configuration | TOML (per-component) generated from a master YAML |

### Document Map

| Abbrev. | Document | Scope |
|---------|----------|-------|
| [AUTO] | `automation/AUTOMATION.md` | Lab infrastructure, VM provisioning, network topology |
| [BUILD] | `build/BUILD-SYSTEM-ARCHITECTURE.md` | Artifact factory — compilation, mutation engines, runtime libraries |
| [CTRL] | `controller/CONTROLLER-ARCHITECTURE-2.md` | Central orchestration — job lifecycle, dispatch, triage, storage |
| [PROTO] | `proto/PROTO-DEEP-ANALYSIS.md` | gRPC communication contract — all messages, services, streaming |
| [WORKER] | `worker/agent/WORKER-AGENT-ARCHITECTURE-2.md` | Execution daemon — monitoring, classification, telemetry collection |

---

## 3. Global Architecture Diagram

```
┌──────────────────────────────────────────────────────────────────────────┐
│  UI / CLI                                                                │
│  Client-only gRPC consumer                                               │
│  (controller.proto — client stubs, no server)                            │
└───────────────────────────────┬──────────────────────────────────────────┘
                                │ gRPC (controller.proto)
                                ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  CONTROLLER                                             15,674 LOC      │
│  ┌──────────┐ ┌───────────┐ ┌──────────┐ ┌─────────┐ ┌──────────┐     │
│  │  api/    │ │ dispatch/ │ │ triage/  │ │storage/ │ │   vm/    │     │
│  │ 25 RPCs  │ │Orchestr.  │ │Extractor │ │6 ES idx │ │TargetMgr │     │
│  │ thin     │ │JobWorker  │ │Scorer    │ │Write:   │ │DashMap   │     │
│  │ handlers │ │RunPool    │ │4 Select. │ │ typed   │ │Bidi strm │     │
│  │          │ │VMExecutor │ │22 mutat. │ │Read:    │ │Heartbeat │     │
│  │          │ │           │ │          │ │ raw JSON│ │Reconnect │     │
│  └──────────┘ └───────────┘ └──────────┘ └─────────┘ └──────────┘     │
│  ▲ Commands/queries (top-down)          Telemetry/outcomes (bottom-up) │
└──────────────────┬──────────────────────────┬────────────────────────────┘
                   │ BuildInput               │ gRPC (worker.proto)
                   ▼                          ▼
┌────────────────────────────┐  ┌─────────────────────────────────────────┐
│  BUILD CRATE               │  │  WORKER AGENT (per VM)     5,926 LOC   │
│  (library, no server)      │  │  ┌─────────┐ ┌───────────┐ ┌────────┐ │
│                            │  │  │  api/   │ │execution/ │ │telemetry│ │
│  Template assembler        │  │  │ 7 RPCs  │ │10-phase   │ │6 sources│ │
│  AST mutator (tree-sitter) │  │  │ thin    │ │pipeline   │ │RedEDR   │ │
│  IR mutator (LLVM text)    │  │  │ adapter │ │7 verdicts │ │trace    │ │
│  Binary mutator (PE)       │  │  │         │ │RAII guard │ │coverage │ │
│  Runtime libraries (C)     │  │  └─────────┘ └───────────┘ └────────┘ │
│  Payload encoder           │  │  ┌──────────┐ ┌──────────┐            │
│  Line tracer               │  │  │ session/ │ │  infra/  │            │
│                            │  │  │ bidi     │ │ process  │            │
│  22 mutations across       │  │  │ stream   │ │ system   │            │
│  3 layers (AST/IR/Binary)  │  │  └──────────┘ └──────────┘            │
└────────────────────────────┘  └─────────────────────────────────────────┘
                                                    │
                                                    ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  INFRASTRUCTURE                                                 [AUTO]  │
│  Hyper-V Gen2 VMs │ IsolationSwitch 10.200.200.0/24                     │
│  WSL2 Ubuntu (controller + ES + Kibana)                                  │
│  RedEDR (ETW + ETW-TI kernel tracing)                                   │
│  Windows Defender / MDE / Cortex XDR (real EDR under test)               │
└──────────────────────────────────────────────────────────────────────────┘
```

**Data flow directions:** Commands, job specifications, and artifacts flow **top-down** (UI → Controller → Build → Worker). Telemetry, detection outcomes, and triage feedback flow **bottom-up** (Worker → Controller → Triage → Selector → next round).

> **Detailed references:** [CTRL] Section 4, [WORKER] Section 2, [AUTO] Network Topology

---

## 4. The Closed Experimental Loop

The core innovation is a feedback loop that replaces manual trial-and-error with evidence-driven mutation selection. Each iteration (a "round") passes through 11 stages:

### Pipeline Stages

| # | Stage | Component | Input | Output |
|---|-------|-----------|-------|--------|
| 1 | **Schedule** | Controller `api/` | `JobRequest` (payload, modules, config) | `JobSession` queued |
| 2 | **Select** | Controller `triage/` (Selector) | Round history + `TriageGuidance` | `Selection` (modules + mutations) |
| 3 | **Build** | Build crate | `BuildInput::ModularTemplate` | 2 `BuiltArtifact`s (baseline + instrumented) |
| 4 | **Static scan** | Controller `dispatch/` | Baseline PE | Pass / `StaticDetection` (short-circuit) |
| 5 | **Deploy** | Controller `vm/` → Worker `api/` | PE binary chunks | `TransferAck` |
| 6 | **Execute** | Worker `execution/` | `SampleRequest` (×3 runs) | Exit code + checkpoints |
| 7 | **Collect** | Worker `telemetry/` | 6 raw sources | `Vec<TelemetryData>` |
| 8 | **Aggregate** | Controller `dispatch/` | 2–3 `RunOutcome`s | `DifferentialCategory` + evasion score |
| 9 | **Extract** | Controller `triage/` (Extractor) | Telemetry + modules + mutations | Token set (9 categories) |
| 10 | **Score** | Controller `triage/` (Scorer) | Token sets + outcome history | Lift × confidence per token |
| 11 | **Guide** | Controller `triage/` → Selector | Avoid/seek token lists | Next round's mutation constraints |

### Circular Flow

```
         ┌─── SELECT ◄──── GUIDE ◄──── SCORE ◄──┐
         │                                        │
         ▼                                        │
       BUILD ──► SCAN ──► DEPLOY ──► EXECUTE     │
                                       │          │
                                       ▼          │
                                    COLLECT ──► EXTRACT
                                       │
                                       ▼
                                   AGGREGATE
```

After each round completes, triage results feed back to the selector, closing the loop. The system converges on mutations that shift detection-correlated tokens toward evasion while maintaining artifact functionality.

> **Detailed references:** [CTRL] Section 6 (round lifecycle), [BUILD] Role in the Global Project, [WORKER] Section 2

---

## 5. Component Summaries

### 5.1 Controller — Central Orchestration

The controller is the single Rust binary that implements the entire experimental loop from job submission to triage feedback. It receives campaign requests, coordinates build and execution, persists results to ElasticSearch, and drives the token-driven mutation selector.

| Metric | Value |
|--------|-------|
| LOC | 15,674 |
| Files | 38 |
| Modules | 5 (`api`, `dispatch`, `storage`, `triage`, `vm`) |
| gRPC RPCs served | 25 |
| ES index families | 6 (jobs, rounds, runs, telemetry, tokens, artifacts) |
| Selector strategies | 4 (Coverage, Fuzzer, Token, Random) |

| Module | Lines | Role |
|--------|------:|------|
| `api/` | 2,146 | gRPC boundary — proto ↔ domain translation, zero business logic |
| `dispatch/` | 4,938 | Experiment loop — Orchestrator, JobWorker, RunPool, VMExecutor |
| `triage/` | 5,721 | Intelligence — token extraction, scoring, 4 selector strategies |
| `storage/` | 1,788 | Persistence — ES reads/writes, schema templates, 6 index families |
| `vm/` | 1,081 | Transport — DashMap-backed target manager, bidi streams, heartbeat |

> **Detailed reference:** [CTRL] Sections 2–5

### 5.2 Build Crate — Artifact Factory

The build crate is the only component that produces PE artifacts. It receives a `BuildInput` specifying module selection, payload, encoding, mutations, and trace mode, then applies transformations at three layers before outputting a Windows executable.

| Metric | Value |
|--------|-------|
| Mutation count | 22 (10 AST + 3 LLVM IR + 9 Binary) |
| Trace modes | 7 (Off, Api, BB, ApiPlusBB, Lines, LinesAroundBB, All) |
| Build paths | 2 (Standard Clang/LLD, MSVC-compatible clang-cl/link.exe) |
| Runtime libraries | 3 (minimal, instrumentation, sc-checkpoint) |
| Template modules | 7 slots (carrier, decoder, antiemulation, guardrail, virtualprotect, decoy, deconditioner) |

| Pipeline Phase | Engine | Scope |
|----------------|--------|-------|
| Source (AST) | tree-sitter C parser | Structural transforms, string encoding, API insertion |
| Intermediate (IR) | LLVM IR text manipulation | NOP insertion, opaque predicates, junk blocks |
| Post-link (Binary) | goblin PE parsing | Rich header, imports, resources, entropy, sections |

> **Detailed reference:** [BUILD] Sections: builder.rs, transform/

### 5.3 Worker Agent — Execution Daemon

The worker agent runs on each Windows VM, receives pre-built PE artifacts, executes them under full behavioral monitoring, classifies the outcome, collects telemetry from 6 sources, and returns everything to the controller.

| Metric | Value |
|--------|-------|
| LOC | 5,926 |
| Files | 24 |
| Modules | 7 (`api`, `execution`, `session`, `telemetry`, `infra`, `capabilities`, `constants`) |
| Execution phases | 10 (full run) / 4 (dryrun) |
| Detection verdicts | 7 (Evasion, Detected, Ambiguous, Stalled, InfraError, MutationFailed, Anomaly) |
| Telemetry sources | 6 (RedEDR events, trace JSONL, binary trace, BB coverage, checkpoints, metrics) |
| gRPC RPCs | 7 (WorkerAgent service) |

| Module | Lines | Role |
|--------|------:|------|
| `execution/` | 2,272 | Core pipeline — spawn, monitor, classify, collect |
| `telemetry/` | 1,935 | Data collection from 6 sources, deduplication, packaging |
| `api/` | 558 | gRPC thin adapters — zero business logic |
| `session/` | 495 | Bidirectional stream lifecycle |
| `capabilities.rs` | 330 | Startup self-detection (RedEDR, Defender, MDE, Cortex) |
| `infra/` | 137 | OS abstraction — process, filesystem, metrics |

> **Detailed reference:** [WORKER] Sections 4–7

### 5.4 Proto Definitions — Communication Contract

The three `.proto` files define the entire gRPC contract between all components. `common.proto` holds shared domain types (identity, telemetry, execution), `controller.proto` defines the controller's inbound API, and `worker.proto` defines the worker's inbound API.

| Metric | common.proto | controller.proto | worker.proto | **Total** |
|--------|:-----------:|:---------------:|:-----------:|:---------:|
| Messages | 30 | 77 | 14 | **121** |
| Services | 0 | 3 | 2 | **5** |
| RPCs | 0 | 28 | 9 | **37** |
| Lines | 269 | 732 | 159 | **1,160** |

| Service | Host | Callers | RPCs |
|---------|------|---------|-----:|
| `Controller` | Controller | UI, CLI, Workers | 24 |
| `Selector` | Controller | Internal (dispatch) | 2 |
| `Triage` | Controller | Internal (planned) | 2 |
| `WorkerAgent` | Worker VM | Controller | 7 |
| `Harness` | Worker VM | Controller | 2 |

> **Detailed reference:** [PROTO] Sections 2–5

### 5.5 Automation — Lab Infrastructure

The automation layer provisions the physical infrastructure: Hyper-V VMs, networking, WSL2 controller environment, ElasticSearch/Kibana, and RedEDR installation. It supports two deployment modes with a single `config.yaml` as source of truth.

| Metric | Value |
|--------|-------|
| Deployment modes | 2 (Local Hyper-V, Remote SSH/VPN) |
| VM initialization steps | 10 (networking, dev tools, RedEDR, audit policies, drivers) |
| Health checks | 12 (Windows features, WSL, network, ES, Kibana, VMs, storage, firewall) |
| Security controls | 3 (NAT kill switch, egress filtering, VM isolation) |
| Config pipeline | `config.yaml` + `templates/*.toml` → `generated/*.toml` |

| Script Category | Key Scripts |
|-----------------|-------------|
| Host setup | `01-host-setup.ps1` (Hyper-V, networking), `02-wsl-bootstrap.sh` (Rust, Docker, ES) |
| VM provisioning | `03-create-worker-vm.ps1`, `04-vm-init.ps1` (10-step), `05-create-baseline.ps1` |
| Remote deployment | `deploy-remote-worker.ps1` (SSH/SCP), `list-workers.ps1` (gRPC query) |
| Operations | `start-environment.ps1`, `stop-environment.ps1`, `validate-environment.ps1` |

> **Detailed reference:** [AUTO] Setup Flow, VM Initialization Details

---

## 6. Data Flow: Job Submission to Triage Feedback

The following sequence traces one complete round through all components, naming the protobuf message types exchanged at each boundary:

```
 UI/CLI            Controller                  Build Crate         Worker Agent
   │                   │                           │                     │
   │  JobRequest       │                           │                     │
   ├──────────────────►│                           │                     │
   │  JobResponse      │                           │                     │
   │◄──────────────────┤                           │                     │
   │                   │                           │                     │
   │            Orchestrator spawns JobWorker       │                     │
   │                   │                           │                     │
   │            Selector.select()                   │                     │
   │            → Selection{modules, mutations}     │                     │
   │                   │                           │                     │
   │                   │  BuildInput (baseline)    │                     │
   │                   ├──────────────────────────►│                     │
   │                   │  BuiltArtifact            │                     │
   │                   │◄──────────────────────────┤                     │
   │                   │                           │                     │
   │                   │  BuildInput (instrumented) │                     │
   │                   ├──────────────────────────►│                     │
   │                   │  BuiltArtifact            │                     │
   │                   │◄──────────────────────────┤                     │
   │                   │                           │                     │
   │            Static Defender scan (if detected → skip to triage)      │
   │                   │                                                 │
   │                   │  ArtifactChunkBatch (stream)                    │
   │                   ├────────────────────────────────────────────────►│
   │                   │  Ack                                            │
   │                   │◄────────────────────────────────────────────────┤
   │                   │                                                 │
   │                   │  RunSampleCommand (baseline, trace=off)         │
   │                   ├────────────────────────────────────────────────►│
   │                   │         [Execute → Monitor → Classify]          │
   │                   │  SampleResponse + TelemetryBatch                │
   │                   │◄────────────────────────────────────────────────┤
   │                   │                                                 │
   │                   │  RunSampleCommand (instrumented, trace=lines)   │
   │                   ├────────────────────────────────────────────────►│
   │                   │         [Execute → Monitor → Classify]          │
   │                   │  SampleResponse + TelemetryBatch                │
   │                   │◄────────────────────────────────────────────────┤
   │                   │                                                 │
   │                   │  RunSampleCommand (dryrun, is_dryrun=true)      │
   │                   ├────────────────────────────────────────────────►│
   │                   │  SampleResponse (exit code only)                │
   │                   │◄────────────────────────────────────────────────┤
   │                   │                                                 │
   │            RoundAgg.finalize()                                      │
   │            → DifferentialCategory + evasion_score                   │
   │                   │                                                 │
   │            index_round_and_runs() → ElasticSearch                   │
   │                   │                                                 │
   │            extract_and_score() (async background)                   │
   │            → Token set → ES                                         │
   │            → Lift × confidence scoring                              │
   │            → TriageGuidance (avoid/seek tokens)                     │
   │            → guidance_tx → JobWorker                                │
   │                   │                                                 │
   │            should_continue()?                                       │
   │            → Yes: next round with updated guidance                  │
   │            → No: emit JobCompleted                                  │
   │                   │                                                 │
```

### Three Run Types per Round

| Run | Trace Mode | RedEDR | Purpose |
|-----|-----------|--------|---------|
| **Baseline** | `off` | Active | Ground-truth EDR behavior without instrumentation overhead |
| **Instrumented** | `lines` | Active | Full execution trace — localize truncation point |
| **Dryrun** | N/A | Inactive | Clean-VM exit code — confirm artifact functionality |

> **Detailed references:** [CTRL] Section 6, [PROTO] Section 6.1 Pipeline Stage Mapping, [WORKER] Section 6

---

## 7. Two-Run Differential Protocol

Each mutation round executes the **same artifact** (same bytes, same SHA-256) under two trace modes on the same VM, then adds a third dryrun on a clean VM. The differential comparison isolates real EDR detections from instrumentation artifacts and dryrun failures.

### Interpretation Matrix

| Baseline (Run B) | Instrumented (Run A) | Dryrun | Category | Use for learning? |
|:-:|:-:|:-:|---|:-:|
| Detected | Detected | Clean exit | `RealDetection` | Yes |
| Clean | Detected | Clean exit | `InstrumentationArtifact` | No |
| Detected | Clean | Clean exit | `Flaky` | No |
| Clean | Clean | Clean exit | `Evasion` | Yes |
| — | — | Crash/error | `MutationFailed` | No (artifact broken) |
| — | — | Crash (late) | `PayloadFailed` | No (payload broken) |
| File scan hit | — | — | `StaticDetection` | Yes (short-circuit) |

Only `RealDetection`, `Evasion`, and `StaticDetection` outcomes feed into the token scoring loop. `InstrumentationArtifact` and `Flaky` results are discarded to prevent the feedback loop from learning spurious correlations.

### Build Determinism Requirements

The protocol requires identical artifacts across runs:

- **Seeded RNG:** IR mutations use a deterministic LCG; payload encoding uses fixed XOR keys
- **Reproducible PE timestamps:** Clang flag `-Wl,/Brepro` produces deterministic headers
- **Pinned toolchain:** Clang/LLVM version, xwin SDK version, and Rust nightly are fixed per campaign

> **Detailed references:** [CTRL] Section 5.2 (differential categories), [BUILD] Key Design Decisions §7, [WORKER] Section 6.3 (dryrun path)

---

## 8. Mutation Architecture

### Three-Layer Mutation Pipeline

| Layer | Phase | Engine | Mutations | Scope |
|-------|-------|--------|:---------:|-------|
| **AST** (Source) | Pre-compilation | tree-sitter C parser | 10 | Structural transforms, string encoding, API call insertion |
| **IR** (Intermediate) | Post-C-compile, pre-link | LLVM IR text manipulation | 3 | NOP insertion, opaque predicates, dead code blocks |
| **Binary** (PE) | Post-link | goblin PE parsing | 9 | Rich header, imports, resources, sections, entropy, timestamps |

Each layer has different trade-offs: AST mutations are the most expressive and human-readable but are subject to compiler optimization; IR mutations survive optimization but are harder to reason about; binary mutations operate on the final artifact and directly shift static feature vectors.

### Selector Strategies

| Selector | Algorithm | Signal | Role |
|----------|-----------|--------|------|
| **CoverageSelector** (default) | Epsilon-greedy (epsilon=0.3) | Evasion scores from round history | Exploitation with exploration |
| **FuzzerSelector** | Genetic algorithm (tournament, crossover) | Evasion fitness | Evolutionary search |
| **TokenSelector** | Token-biased epsilon-greedy | Evasion scores + avoid/seek tokens | Token-driven exploitation |
| **RandomSelector** | Uniform random | None | Evaluation baseline |

### Fixed vs Explored Mutations

| Category | Count | Varied by Selector? | Purpose |
|----------|:-----:|:-------------------:|---------|
| **Fixed** (binary + 1 IR) | 10 | No — always applied | PE normalization (Rich header, imports, sections, resources, timestamps, entropy) |
| **Explored** (AST) | 10 | Yes — 1 selected per round | Behavioral changes (deconditioner rounds, fill patterns, protection transitions, timing, preambles, syscall insertion, string obfuscation, constant obfuscation) |

> **Detailed references:** [BUILD] transform/ subsections, [CTRL] Section 5.4 (selector strategies)

---

## 9. Triage Token System

Raw telemetry is converted into **normalized triage tokens** — deterministic, comparable identifiers that abstract over specific telemetry formats. Tokens are the unit of learning: the system scores tokens by correlation with detection and uses those scores to guide mutation selection.

### Token Categories

| # | Category | Example | Source | Extractor |
|---|----------|---------|--------|-----------|
| 1 | Module | `module:carrier=peb_walk` | Build input | In-memory |
| 2 | Mutation | `mutation:ast.string_xor:xor_key=0xBB` | Build input | In-memory |
| 3 | API call | `api:VirtualProtect` | RedEDR ETW | ES telemetry query |
| 4 | API argument | `api_arg:VirtualProtect:flProtect=RWX` | RedEDR ETW | ES telemetry query |
| 5 | Sequence (2-gram) | `seq2:VirtualAlloc→memcpy` | RedEDR ETW | ES telemetry query |
| 6 | Image load | `image:ntdll.dll` | RedEDR ETW | ES telemetry query |
| 7 | ETW provider | `etw:Microsoft-Windows-Kernel-Process` | RedEDR ETW | ES telemetry query |
| 8 | ETW event | `etw_event:ProcessStart/1` | RedEDR ETW | ES telemetry query |
| 9 | Checkpoint | `checkpoint:Launching` | Artifact runtime | ES checkpoint query |

### Scoring Formulas

For each token **T** across accumulated rounds:

```
P(detected)     = total_detected_rounds / total_rounds
P(detected | T) = rounds_where(T ∈ tokens AND detected) / rounds_where(T ∈ tokens)

lift(T)         = P(detected | T) / P(detected)
confidence(T)   = min(1.0, n_observations / 5)
importance(T)   = lift(T) × confidence(T)
```

### Avoid / Seek Thresholds

| Category | Condition | Action |
|----------|-----------|--------|
| **Avoid** | `lift > 1.5 AND confidence > 0.3` | Selector penalizes mutations that produce this token |
| **Seek** | `lift < 0.667 AND confidence > 0.3` | Selector favors mutations that produce this token |

### Feedback Flow

```
TriageGuidance { avoid_tokens, seek_tokens }
    → guidance_tx channel
    → JobWorker.guidance_rx
    → Selector.select(history, guidance)
    → Selection { modules, mutations }  // next round
```

> **Detailed references:** [CTRL] Section 5.4 (triage pipeline), CLAUDE.md Sections 6–8

---

## 10. Communication Protocol

### Service Map

| Service | Host | Caller(s) | RPCs | Proto File |
|---------|------|-----------|:----:|------------|
| `Controller` | Controller (WSL2) | UI, CLI, Workers | 24 | `controller.proto` |
| `Selector` | Controller (WSL2) | Internal dispatch | 2 | `controller.proto` |
| `Triage` | Controller (WSL2) | Internal (planned) | 2 | `controller.proto` |
| `WorkerAgent` | Worker VM | Controller | 7 | `worker.proto` |
| `Harness` | Worker VM | Controller | 2 | `worker.proto` |

### Phase 1 vs Phase 2 Communication

| Aspect | Phase 1 (Unary RPCs) | Phase 2 (Bidi Stream) |
|--------|---------------------|----------------------|
| Connection | New TCP per RPC | Single persistent TCP |
| Message flow | Request-response | Full-duplex multiplexed |
| Commands | `RunSample`, `SendArtifact`, `GetTelemetry` | `ControllerMessage` envelope (6 variants) |
| Results | `SampleResponse` return value | `WorkerMessage` envelope (6 variants) |
| Heartbeat | Periodic `HealthCheck` RPC | In-band `Heartbeat` message |
| Status updates | None during execution | Real-time `ExecutionStatusReport` |
| Artifact transfer | `SendArtifact` client-streaming | `ArtifactChunkBatch` in-band |

Both phases coexist in the codebase and share the same execution engine (`execution_lock`, `execute_run()`, classifier). The bidi stream (Phase 2) is the primary path used by the controller's `VMExecutor`.

> **Detailed references:** [PROTO] Sections 3–4, [WORKER] Section 5

---

## 11. Infrastructure and Network Topology

### Local Lab (Hyper-V)

```
┌─────────────────────────────────────────────────────────────────────┐
│  Windows 11 Host                                                     │
│                                                                      │
│  ┌────────────────────┐       ┌──────────────────────────────────┐  │
│  │  WSL2 Ubuntu       │       │  IsolationSwitch (internal)      │  │
│  │                    │       │  10.200.200.0/24                 │  │
│  │  Controller :50051 │◄─────►│                                  │  │
│  │  Elastic    :9200  │       │  ┌────────────┐ ┌────────────┐  │  │
│  │  Kibana     :5601  │       │  │ Win10 VM   │ │ Win10 VM   │  │  │
│  │                    │       │  │ .100       │ │ .101       │  │  │
│  └────────────────────┘       │  │ Agent      │ │ Agent      │  │  │
│                               │  │ RedEDR     │ │ RedEDR     │  │  │
│  Security controls:           │  │ Defender   │ │ Defender   │  │  │
│  ├─ NAT kill switch           │  └────────────┘ └────────────┘  │  │
│  ├─ Egress filtering          │  Host: 10.200.200.1             │  │
│  └─ VM-to-VM isolation        └──────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

**Local mode:** All components on a single Windows 11 host. Controller runs in WSL2; worker VMs connect through a Hyper-V internal switch. The host enforces all network security (NAT, egress filtering, VM isolation).

**Remote mode:** Controller stays in WSL2. Workers run on remote machines (Azure, bare metal, VPN-connected hosts). Deployment via SSH/SCP (`deploy-remote-worker.ps1`). Workers listen on `:50052`; the controller dials them. No host-level network enforcement — worker-side `worker.toml` firewall rules are the only security boundary.

> **Detailed reference:** [AUTO] Network Topology, Remote Deployment Flow

---

## 12. Key Cross-Cutting Design Decisions

Eight design decisions span multiple components:

**1. Weak symbol linkage for conditional instrumentation** — `minimal_runtime.c` declares telemetry flush functions as `__attribute__((weak)) extern`. When `instrumentation_runtime.o` is linked (instrumented build), the real implementations win; when absent (baseline build), they resolve to NULL and are safely skipped. This allows a single runtime object to serve both build modes without conditional compilation.
> [BUILD] Key Design Decisions §1, [WORKER] Section 6 (runtime linking)

**2. Aggressive flush strategy (death-bed telemetry)** — Every trace event is flushed immediately; coverage bitmaps are written every 50 BB executions. EDR can kill the process at any unpredictable moment — without incremental flush, all telemetry after the last write would be lost. The exact line of death is captured.
> [BUILD] instrumentation_runtime.c, [WORKER] Section 7.2

**3. Direct syscall process exit** — Artifacts terminate via a direct `syscall` instruction to `NtTerminateProcess`, bypassing all usermode hooks (including RedEDR detours). Syscall numbers are resolved dynamically from `ntdll.dll` stub bytes at runtime. This prevents hooked exits from deadlocking or corrupting telemetry timing.
> [BUILD] minimal_runtime.c

**4. Deterministic builds for differential protocol** — Same inputs must produce byte-identical artifacts across runs. Achieved via: seeded LCG for IR mutations, fixed XOR keys for encoding, `-Wl,/Brepro` for reproducible PE timestamps, and pinned toolchain versions.
> [BUILD] Key Design Decisions §7, Section 7 (differential protocol requirements)

**5. Three-layer mutation architecture** — AST mutations operate on C source (structural, human-readable), IR mutations on LLVM IR text (control-flow, survives optimization), binary mutations on PE bytes (static features, post-link). Each layer has different persistence through the compilation pipeline.
> [BUILD] transform/ subsections

**6. OS-sharded run pool with capability filtering** — The controller's `RunPool` uses per-OS `DashMap` queues with `Notify` signals. `VMExecutor` takes runs matching the VM's OS and capabilities. This prevents a Windows 10 artifact from being dispatched to a Windows 11 VM (or vice versa), and enables capability-based routing (e.g., dryrun-only VMs).
> [CTRL] Section 5.2 (RunPool)

**7. Token-driven mutation selection** — Instead of purely heuristic mutation, the selector receives `TriageGuidance` containing avoid/seek token lists derived from detection-correlated scoring. Mutations that produce high-lift tokens are penalized; mutations that produce low-lift tokens are favored.
> [CTRL] Section 5.4 (Selector), CLAUDE.md Section 8

**8. Single-execution lock on worker** — Each worker VM allows exactly one artifact execution at a time, enforced by an `Arc<Mutex<ExecutionState>>` shared across both gRPC entry paths (unary and bidi stream). A second `RunSample` while one is in progress returns `resource_exhausted`. This prevents telemetry contamination between concurrent runs.
> [WORKER] Section 8 (shared state model)

---

## 13. Detection Outcomes and Evaluation Model

### Three-Axis Evaluation

| Axis | Question | Method |
|------|----------|--------|
| **Input** | What observable behaviors does the artifact produce? | Triage tokens extracted from telemetry |
| **Oracle** | Was the artifact detected? | Worker classifier (7 verdicts) + differential protocol |
| **Guidance** | Which behaviors correlate with detection? | Token scoring (lift × confidence) → avoid/seek |

### Status Matrix

| Capability | Status | Notes |
|------------|:------:|-------|
| Artifact compilation (AST/IR/Binary mutations) | Complete | 22 mutations across 3 layers |
| Template assembly (modular loader) | Complete | 7-slot module system |
| Instrumented + baseline builds | Complete | Weak-symbol conditional compilation |
| VM execution + 10-phase pipeline | Complete | 7 verdicts, 6 telemetry sources |
| Two-run differential protocol | Complete | 7 differential categories |
| ElasticSearch persistence | Complete | 6 index families, typed writes |
| Token extraction (9 categories) | Complete | In-memory + ES telemetry queries |
| Token scoring (lift × confidence) | Complete | Incremental, per-round updates |
| 4 selector strategies | Complete | Coverage, Fuzzer, Token, Random |
| Triage → Selector feedback loop | Complete | avoid/seek token lists via channel |
| Hypothesis report generation | Partial | Framework exists; UI rendering planned |
| Coverage-guided BB-level analysis | Partial | AFL bitmap collected; targeted analysis not integrated |
| Temporal tokens (`dt:write→protect_ms<20`) | Planned | Schema defined in CLAUDE.md |
| Truncation tokens (`trunc:loader.c:143`) | Planned | Line tracer captures data; token extractor not yet wired |

---

## 14. Implementation Status

| Stage | Status | Notes |
|-------|:------:|-------|
| Infrastructure automation | Complete | Hyper-V + remote modes, 10-step VM init, health checks |
| Proto contract (gRPC) | Complete | 121 messages, 37 RPCs, 5 services |
| Build crate (artifact factory) | Complete | 22 mutations, 7 trace modes, 2 compiler modes |
| Controller orchestration | Complete | Job lifecycle, dispatch, storage, VM management |
| Worker agent execution | Complete | 10-phase pipeline, 7 verdicts, 6 telemetry sources |
| Token extraction + scoring | Complete | 9 categories, lift × confidence |
| Mutation selection (4 strategies) | Complete | Coverage, Fuzzer, Token, Random |
| Feedback loop (triage → selector) | Complete | TriageGuidance channel, avoid/seek tokens |
| Differential protocol | Complete | 7 categories, baseline-consistent filtering |
| Trace compressor (gzip) | Partial | Implemented but not integrated (3 blockers) |
| Temporal/truncation tokens | Planned | Schema defined, extractors not yet wired |
| UI dashboard (Kibana + web) | Planned | ES data available; visualization layer pending |
| Multi-EDR comparison | Planned | Worker capability detection supports MDE/Cortex; no multi-EDR campaign logic |

The boundary between design and implementation: the closed loop (select → build → execute → collect → extract → score → guide) is fully operational. What remains are refinements to token categories (temporal, truncation), visualization, and multi-EDR campaign orchestration.

---

*This document synthesizes the 5 per-component architecture documents: [AUTO], [BUILD], [CTRL], [PROTO], [WORKER]. For implementation details, follow the cross-references to the specific sections cited above.*
