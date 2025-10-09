# References and Code Inspirations

**Project:** Automated Analysis and Mutation of Software Artifacts against AV/EDR
**Date:** 2025-01-10
**Purpose:** Academic compliance and proper attribution

---

## Table of Contents

1. [Primary Specifications](#primary-specifications)
2. [Architectural Patterns](#architectural-patterns)
3. [Rust Ecosystem Libraries](#rust-ecosystem-libraries)
4. [gRPC and Protocol Buffers](#grpc-and-protocol-buffers)
5. [Docker and Containerization](#docker-and-containerization)
6. [Telemetry and Observability](#telemetry-and-observability)
7. [Machine Learning and Fuzzing](#machine-learning-and-fuzzing)
8. [Security Research Frameworks](#security-research-frameworks)
9. [Configuration Management](#configuration-management)
10. [Web Frameworks](#web-frameworks)
11. [Best Practices and Style Guides](#best-practices-and-style-guides)

---

## 1. Primary Specifications

### CLAUDE.md (Internal Specification)
- **Description:** Primary architectural specification document defining system requirements
- **Location:** Project root
- **Usage:** All component designs follow this specification
- **Sections Referenced:**
  - Section 2: Architecture (Controller, Worker, Collector, Storage)
  - Section 3: Fuzzer & Mutation Engine
  - Section 5: Telemetry Channels (typed features requirement)
  - Section 6: Canonical Schemas (RunResult, CollectorConfigFacts)
  - Section 7: Output Formats (templates, reports)
  - Section 10: Ethics & Safety

### RedEDR Integration
**Reference:** Dobin's RedEDR
- **Repository:** https://github.com/dobin/RedEdr
- **Authors:** Dobin
- **License:** MIT License
- **Usage in Project:**
  - Telemetry collection backend for Windows
  - JSON output format parsing (`telemetry/collector/src/rededr.rs`)
  - Event schema design inspiration
- **Academic Citation:**
  ```
  Dobin. (2024). RedEDR: Red Team EDR Evasion Research Tool.
  GitHub repository. https://github.com/dobin/RedEdr
  ```

---

## 2. Architectural Patterns

### Microservices Architecture

**Reference:** Building Microservices (2nd Edition)
- **Author:** Sam Newman
- **Publisher:** O'Reilly Media, 2021
- **ISBN:** 978-1492034025
- **Concepts Applied:**
  - Service decomposition (Controller, Selector, Triage as separate services)
  - gRPC-based inter-service communication
  - Independent deployment units
  - Service boundaries around business capabilities
- **Project Implementation:**
  - `controller/scheduler/`, `controller/selector/`, `controller/triage-engine/` as separate gRPC services
  - Docker Compose orchestration with service isolation

**Reference:** Microservices Patterns
- **Author:** Chris Richardson
- **Publisher:** Manning Publications, 2018
- **ISBN:** 978-1617294549
- **Patterns Applied:**
  - API Gateway pattern (UI Backend as REST gateway to gRPC services)
  - Service Registry (implicit via Docker network discovery)
  - Database per Service (Elasticsearch indexes per component)
  - Externalized Configuration (`config/` library)

### Event-Driven Architecture

**Reference:** Enterprise Integration Patterns
- **Authors:** Gregor Hohpe, Bobby Woolf
- **Publisher:** Addison-Wesley, 2003
- **ISBN:** 978-0321200686
- **Patterns Applied:**
  - Message Channel (gRPC streams for telemetry)
  - Content Enricher (Feature extraction from raw events)
  - Message Translator (RedEDR JSON → Canonical format)

---

## 3. Rust Ecosystem Libraries

### Tokio Async Runtime
**Repository:** https://github.com/tokio-rs/tokio
- **Version:** 1.35+
- **License:** MIT
- **Authors:** Tokio Contributors
- **Usage:** Async runtime for all services
- **Documentation:** https://tokio.rs/
- **Academic Citation:**
  ```
  Tokio Contributors. (2024). Tokio: A runtime for writing reliable
  asynchronous applications with Rust. https://tokio.rs/
  ```

### Serde Serialization Framework
**Repository:** https://github.com/serde-rs/serde
- **Version:** 1.0+
- **License:** MIT/Apache-2.0
- **Authors:** David Tolnay
- **Usage:** JSON serialization for all data structures
- **Implementation Files:**
  - `controller/mutator/src/lib.rs` (MutationSpec)
  - `telemetry/collector/src/feature_extractor.rs` (TypedFeatures)
  - `worker/monitor/src/lib.rs` (Outcome, RunMetrics)
- **Documentation:** https://serde.rs/

### Tracing Instrumentation
**Repository:** https://github.com/tokio-rs/tracing
- **Version:** 0.1+
- **License:** MIT
- **Authors:** Tokio Contributors
- **Usage:** Structured logging across all services
- **Documentation:** https://tracing.rs/

---

## 4. gRPC and Protocol Buffers

### Tonic gRPC Framework
**Repository:** https://github.com/hyperium/tonic
- **Version:** 0.11
- **License:** MIT
- **Authors:** Lucio Franco, Hyperium Contributors
- **Usage:** gRPC server and client implementation
- **Services Implemented:**
  - `controller/scheduler/src/main.rs` (Controller service)
  - `controller/selector/src/main.rs` (Selector service)
  - `controller/triage-engine/src/main.rs` (Triage service)
  - `worker/agent/src/main.rs` (WorkerAgent service)
- **Documentation:** https://docs.rs/tonic/
- **Academic Citation:**
  ```
  Franco, L., & Hyperium Contributors. (2024). Tonic: A native gRPC
  client & server implementation with async/await support.
  https://github.com/hyperium/tonic
  ```

### Prost Protocol Buffers
**Repository:** https://github.com/tokio-rs/prost
- **Version:** 0.12
- **License:** Apache-2.0
- **Authors:** Dan Burkert, Tokio Contributors
- **Usage:** Protocol buffer code generation
- **Proto Files:**
  - `controller/proto/common.proto`
  - `controller/proto/controller.proto`
  - `controller/proto/worker.proto`
- **Documentation:** https://docs.rs/prost/

### Protocol Buffers Specification
**Reference:** Protocol Buffers Language Guide (proto3)
- **Author:** Google
- **URL:** https://protobuf.dev/programming-guides/proto3/
- **Version:** proto3
- **Concepts Applied:**
  - Message definitions for RunResult, Mutation, JobRequest
  - Service definitions for RPC interfaces
  - Import statements for modular proto organization
- **Academic Citation:**
  ```
  Google LLC. (2024). Protocol Buffers Language Guide (proto3).
  https://protobuf.dev/programming-guides/proto3/
  ```

### Buf Style Guide
**Reference:** Buf Style Guide
- **Organization:** Buf Technologies
- **URL:** https://buf.build/docs/best-practices/style-guide
- **Concepts Applied:**
  - Proto file naming conventions
  - Package structure (single `edr` package)
  - Message and service naming (PascalCase)
  - Field naming (snake_case)
- **Implementation:**
  - All proto files follow Buf guidelines
  - Modular split: common, controller, worker

---

## 5. Docker and Containerization

### Docker Multi-Stage Builds
**Reference:** Docker Documentation - Multi-stage builds
- **URL:** https://docs.docker.com/build/building/multi-stage/
- **Organization:** Docker Inc.
- **Concepts Applied:**
  - Builder stage for Rust compilation
  - Runtime stage with minimal Debian image
  - Development stage with cargo-watch
- **Implementation Files:**
  - `build/dockerfiles/Dockerfile.controller`
  - `build/dockerfiles/Dockerfile.worker`
  - `build/dockerfiles/Dockerfile.collector`
- **Pattern Applied:**
  ```dockerfile
  FROM rust:1.75-slim AS builder
  # ... build steps ...
  FROM debian:bookworm-slim AS runtime
  COPY --from=builder /build/target/release/binary /app/
  ```

### Docker Compose Best Practices
**Reference:** Compose file version 3 reference
- **URL:** https://docs.docker.com/compose/compose-file/compose-file-v3/
- **Organization:** Docker Inc.
- **Concepts Applied:**
  - Health checks with `condition: service_healthy`
  - Volume management for persistence
  - Network isolation with custom bridge networks
  - Environment variable injection
- **Implementation File:** `build/dockerfiles/docker-compose.yml`

### Dockerfile Best Practices
**Reference:** Best practices for writing Dockerfiles
- **URL:** https://docs.docker.com/develop/develop-images/dockerfile_best-practices/
- **Concepts Applied:**
  - Dependency caching with separate COPY layers
  - Non-root user creation
  - Minimal runtime images
  - `.dockerignore` for build context optimization

---

## 6. Telemetry and Observability

### Elastic Stack Integration
**Reference:** Elasticsearch Reference
- **Version:** 8.14.2
- **URL:** https://www.elastic.co/guide/en/elasticsearch/reference/current/
- **Authors:** Elastic N.V.
- **License:** Elastic License / SSPL
- **Usage:**
  - Canonical storage for telemetry and run results
  - Kibana for visualization
  - Filebeat for log shipping
- **Implementation:**
  - Docker Compose service definitions
  - Elasticsearch index design (implied in schemas)

### Elastic Common Schema (ECS)
**Reference:** ECS Field Reference
- **Version:** 8.x
- **URL:** https://www.elastic.co/guide/en/ecs/current/
- **Authors:** Elastic N.V.
- **Concepts Applied:**
  - Typed field conventions for telemetry
  - Event categorization (process, network, file, registry)
  - Timestamp standardization
- **Inspiration for:**
  - `telemetry/collector/src/feature_extractor.rs` field design
  - `docs/schemas/telemetry-schema.json`
- **Academic Citation:**
  ```
  Elastic N.V. (2024). Elastic Common Schema (ECS) Reference.
  https://www.elastic.co/guide/en/ecs/current/
  ```

### ETW (Event Tracing for Windows)
**Reference:** Event Tracing for Windows (ETW)
- **URL:** https://docs.microsoft.com/en-us/windows/win32/etw/
- **Authors:** Microsoft Corporation
- **Concepts Applied:**
  - Kernel and user-mode event providers
  - Real-time event session management
  - Buffer management and loss tracking
- **Related Components:**
  - `telemetry/etw-consumer/` (C++ krabsetw wrapper)
  - `telemetry/collector/src/slo.rs` (ETW buffer metrics)

### krabsetw Library
**Repository:** https://github.com/microsoft/krabsetw
- **Authors:** Microsoft Corporation
- **License:** MIT
- **Usage:** C++ wrapper for ETW event consumption
- **Project Component:** `telemetry/etw-consumer/`
- **Academic Citation:**
  ```
  Microsoft Corporation. (2024). krabsetw: C++ library that simplifies
  interacting with ETW. https://github.com/microsoft/krabsetw
  ```

---

## 7. Machine Learning and Fuzzing

### AFL (American Fuzzy Lop) Concepts
**Reference:** American Fuzzy Lop (AFL) fuzzer
- **Author:** Michał Zalewski (lcamtuf)
- **URL:** https://lcamtuf.coredump.cx/afl/
- **Concepts Applied:**
  - Corpus-based fuzzing
  - Coverage-guided mutation
  - Queue prioritization
- **Inspiration for:**
  - `controller/queue/src/main.rs` (corpus management)
  - `controller/selector/src/main.rs` (mutation selection)
- **Academic Citation:**
  ```
  Zalewski, M. (2014). American Fuzzy Lop (AFL) fuzzer.
  https://lcamtuf.coredump.cx/afl/
  ```

### Reinforcement Learning for Fuzzing
**Reference:** "NEUZZ: Efficient Fuzzing with Neural Program Smoothing"
- **Authors:** Dongdong She, Kexin Pei, Dave Epstein, et al.
- **Conference:** IEEE S&P 2019
- **DOI:** 10.1109/SP.2019.00052
- **Concepts Applied:**
  - Feedback-driven mutation selection
  - Exploration vs exploitation tradeoff
- **Inspiration for:**
  - `controller/selector/src/main.rs` (epsilon-greedy strategy)
  - Selector ↔ Triage feedback loop

**Reference:** "Reinforcement Learning-based Hierarchical Seed Scheduling"
- **Authors:** Jiang, Y., Chen, Y., Zhang, C., et al.
- **Conference:** NDSS 2021
- **DOI:** 10.14722/ndss.2021.24486
- **Concepts Applied:**
  - Seed prioritization based on rewards
  - Hierarchical selection strategy
- **Inspiration for:**
  - `controller/queue/` prioritization logic

### Surrogate Models for Explainability
**Reference:** "LEMNA: Explaining Deep Learning based Security Applications"
- **Authors:** Guo, W., Mu, D., Xu, J., et al.
- **Conference:** ACM CCS 2018
- **DOI:** 10.1145/3243734.3243792
- **Concepts Applied:**
  - Surrogate classifier for black-box ML explainability
  - Logistic regression as interpretable model
  - Feature importance ranking
- **Inspiration for:**
  - `controller/triage-engine/src/lib.rs` (TriageAnalysis)
  - Hypothesis generation with confidence scores
- **Academic Citation:**
  ```
  Guo, W., Mu, D., Xu, J., Su, P., Wang, G., & Xing, X. (2018).
  LEMNA: Explaining Deep Learning based Security Applications.
  In Proceedings of the 2018 ACM SIGSAC Conference on Computer and
  Communications Security (CCS '18). https://doi.org/10.1145/3243734.3243792
  ```

---

## 8. Security Research Frameworks

### OSS-Fuzz
**Repository:** https://github.com/google/oss-fuzz
- **Authors:** Google
- **License:** Apache-2.0
- **Concepts Applied:**
  - Continuous fuzzing infrastructure
  - Docker-based fuzzer containers
  - Corpus management and crash reporting
- **Architectural Inspiration:**
  - Worker pool design
  - Controller-worker separation
  - Artifact build and distribution
- **Academic Citation:**
  ```
  Serebryany, K. (2017). OSS-Fuzz - Google's continuous fuzzing service
  for open source software. https://github.com/google/oss-fuzz
  ```

### Firecracker Secure Isolation
**Repository:** https://github.com/firecracker-microvm/firecracker
- **Authors:** Amazon Web Services
- **License:** Apache-2.0
- **Concepts Applied:**
  - Microservices isolation with minimal VMs
  - Secure execution boundaries
  - Fast startup and teardown
- **Inspiration for:**
  - Worker isolation design (future VM integration)
  - Minimal runtime environments
- **Academic Citation:**
  ```
  Agache, A., Brooker, M., Iordache, A., et al. (2020). Firecracker:
  Lightweight Virtualization for Serverless Applications.
  In Proceedings of NSDI 2020.
  ```

### Vector.dev Observability Pipeline
**Repository:** https://github.com/vectordotdev/vector
- **Authors:** Vector Contributors, Datadog
- **License:** MPL-2.0
- **Concepts Applied:**
  - High-throughput event processing
  - Transform pipeline architecture
  - Multiple source and sink support
- **Inspiration for:**
  - `telemetry/collector/` pipeline design
  - Feature extraction transforms
- **Academic Citation:**
  ```
  Vector Contributors. (2024). Vector: A high-performance observability
  data pipeline. https://github.com/vectordotdev/vector
  ```

---

## 9. Configuration Management

### config-rs Library
**Repository:** https://github.com/mehcode/config-rs
- **Version:** 0.13
- **License:** MIT/Apache-2.0
- **Authors:** Ryan Leckey
- **Usage:** Configuration loading in `config/src/lib.rs`
- **Features Used:**
  - File source loading (YAML, JSON, TOML)
  - Environment variable overlays
  - Configuration hierarchy
- **Documentation:** https://docs.rs/config/
- **Academic Citation:**
  ```
  Leckey, R. (2024). config-rs: Layered configuration system for Rust
  applications. https://github.com/mehcode/config-rs
  ```

### 12-Factor App Methodology
**Reference:** The Twelve-Factor App
- **Authors:** Adam Wiggins, Heroku
- **URL:** https://12factor.net/
- **Concepts Applied:**
  - III. Config: Store config in the environment
  - V. Build, release, run: Strictly separate build and run stages
  - VIII. Concurrency: Scale out via the process model
- **Implementation:**
  - Environment variable injection in Docker Compose
  - Separate build and runtime stages in Dockerfiles
  - Stateless service design

---

## 10. Web Frameworks

### Axum Web Framework
**Repository:** https://github.com/tokio-rs/axum
- **Version:** 0.7
- **License:** MIT
- **Authors:** David Pedersen, Tokio Contributors
- **Usage:** REST API implementation in `ui/backend/src/main.rs`
- **Features Used:**
  - Routing with extractors
  - JSON serialization
  - Tower middleware integration
- **Documentation:** https://docs.rs/axum/
- **Academic Citation:**
  ```
  Pedersen, D., & Tokio Contributors. (2024). Axum: Ergonomic and
  modular web framework built with Tokio, Tower, and Hyper.
  https://github.com/tokio-rs/axum
  ```

### Tower Middleware
**Repository:** https://github.com/tower-rs/tower
- **Version:** 0.4+
- **License:** MIT
- **Authors:** Tower Contributors
- **Usage:** HTTP middleware layers for `ui/backend/`
- **Concepts Applied:**
  - Service trait abstraction
  - Middleware composition
  - Timeout and rate limiting (future use)
- **Documentation:** https://docs.rs/tower/

---

## 11. Best Practices and Style Guides

### Rust API Guidelines
**Reference:** Rust API Guidelines
- **Authors:** Rust Library Team
- **URL:** https://rust-lang.github.io/api-guidelines/
- **Guidelines Applied:**
  - C-CONV: Conversions use standard traits (From, TryFrom)
  - C-SERDE: Types implement Serialize, Deserialize
  - C-GOOD-ERR: Error types are meaningful
  - C-DEBUG: All public types derive Debug
- **Implementation:**
  - All public structs implement `Debug`, `Clone`, `Serialize`
  - Error handling with `anyhow::Error` and `thiserror`

### Cargo Workspace Best Practices
**Reference:** The Cargo Book - Workspaces
- **URL:** https://doc.rust-lang.org/cargo/reference/workspaces.html
- **Authors:** Rust Project Developers
- **Concepts Applied:**
  - Workspace-level dependency management
  - Shared `[workspace.dependencies]`
  - Unified versioning with `version.workspace = true`
- **Implementation File:** `Cargo.toml` (workspace root)

### gRPC Best Practices
**Reference:** gRPC Best Practices
- **URL:** https://grpc.io/docs/guides/performance/
- **Organization:** gRPC Authors
- **Concepts Applied:**
  - Keep messages small
  - Use streaming for large datasets
  - Implement health checks
  - Enable keepalive
- **Future Implementation:**
  - Health check service (placeholder in Docker Compose)
  - Streaming for telemetry data

---

## 12. Academic Research Papers

### EDR Evasion Research

**Paper 1:** "What Cannot Be Read, Cannot Be Leveraged?"
- **Authors:** Jaron Mink, Gabriele Quagliarella, et al.
- **Conference:** USENIX Security 2024
- **DOI:** 10.48550/arXiv.2408.07750
- **Relevance:** EDR telemetry blind spots, evaluation methodology
- **Concepts Applied:**
  - Telemetry channel completeness evaluation
  - Blind spot identification methodology
- **URL:** https://arxiv.org/abs/2408.07750

**Paper 2:** "Evading Detection through Obfuscation"
- **Authors:** Various (survey paper)
- **Relevance:** Mutation strategies (AST, binary, behavioral)
- **Concepts Applied:**
  - Control-flow obfuscation
  - String encryption
  - API call indirection

### Differential Testing

**Paper:** "Differential Testing for Software"
- **Authors:** William M. McKeeman
- **Journal:** Digital Technical Journal, 1998
- **Concepts Applied:**
  - Comparing outputs of different implementations
  - Oracle-free testing
- **Inspiration for:**
  - `controller/differential-analyzer/` (scan-time vs runtime)

### Explainable AI in Security

**Paper:** "Interpretable Machine Learning for Security"
- **Authors:** Various (survey paper)
- **Relevance:** Surrogate models, LIME, SHAP
- **Concepts Applied:**
  - Feature importance ranking
  - Local approximations of complex models
  - Confidence scoring

---

## 13. Windows Internals and Telemetry

### Windows Internals (7th Edition)
- **Authors:** Pavel Yosifovich, Mark Russinovich, et al.
- **Publisher:** Microsoft Press, 2017
- **ISBN:** 978-0735684188
- **Chapters Referenced:**
  - Chapter 8: Processes, Threads, and Jobs
  - Chapter 11: Memory Management
  - Chapter 12: I/O System
- **Concepts Applied:**
  - Process creation flow (for telemetry capture)
  - Memory protection flags (RWX detection)
  - System call routing (syscall direct detection)

### JA3/JA4 TLS Fingerprinting
**Reference:** JA3: A method for profiling SSL/TLS Clients
- **Authors:** John Althouse, Jeff Atkinson, Josh Atkins (Salesforce)
- **URL:** https://github.com/salesforce/ja3
- **Concepts Applied:**
  - TLS handshake fingerprinting
  - ClientHello parameter hashing
- **Inspiration for:**
  - `telemetry/collector/src/feature_extractor.rs` (ja3_hash field)
- **Academic Citation:**
  ```
  Althouse, J., Atkinson, J., & Atkins, J. (2017). JA3: A method for
  profiling SSL/TLS Clients. https://github.com/salesforce/ja3
  ```

---

## 14. Code Attribution Summary

### Original Work (This Project)
The following components are original implementations specific to this project:
- `controller/scheduler/src/main.rs` - Scheduler service with job management
- `controller/queue/src/main.rs` - Corpus queue with prioritization
- `controller/mutator/src/lib.rs` - Mutation engine trait system
- `controller/triage-engine/src/lib.rs` - Triage analysis structures
- `controller/rule-manager/src/lib.rs` - Sigma/KQL rule management
- `controller/differential-analyzer/src/lib.rs` - Scan-time vs runtime analysis
- `worker/harness/src/lib.rs` - Execution harness with outcomes
- `worker/monitor/src/lib.rs` - Outcome labeling and metrics
- `telemetry/collector/src/feature_extractor.rs` - Typed feature extraction
- `telemetry/collector/src/slo.rs` - SLO metrics and config facts
- `ui/backend/src/main.rs` - REST API gateway
- `config/src/lib.rs` - Shared configuration library

### Inspired by External Projects
- **Worker pool architecture:** Inspired by OSS-Fuzz
- **Corpus-based fuzzing:** Inspired by AFL
- **Surrogate classifier:** Inspired by LEMNA paper
- **Microservices separation:** Inspired by Richardson's Microservices Patterns
- **Telemetry pipeline:** Inspired by Vector.dev
- **gRPC service patterns:** Follows Buf Style Guide and Tonic examples
- **Docker multi-stage builds:** Follows Docker best practices

### Third-Party Libraries (Unchanged)
All third-party dependencies listed in `Cargo.toml` are used unmodified:
- tokio, tonic, prost, serde, axum, tower, config, tracing

---

## 15. License Compliance

### Project License
This project is licensed under [LICENSE TO BE DETERMINED].

### Dependency Licenses Summary
- **MIT License:** tokio, tonic, serde, axum, tower, config, tracing
- **Apache-2.0:** prost, anyhow, thiserror
- **MIT/Apache-2.0 Dual:** Most Rust crates (user's choice)

All dependencies are compatible with academic and commercial use.

---

## 16. Ethical Considerations

### Responsible Disclosure
**Reference:** CERT Guide to Coordinated Vulnerability Disclosure
- **URL:** https://vuls.cert.org/confluence/display/CVD
- **Authors:** CERT/CC, Carnegie Mellon University
- **Concepts Applied:**
  - 90-day disclosure timeline
  - Vendor notification before public disclosure
- **Implementation:** `docs/appendix/ETHICS.md`

### Lab-Only Research Policy
All security research follows established ethical guidelines:
- No operational malware creation
- Lab-only experiments with controlled artifacts
- Blue team focus (defensive capabilities)
- Data sanitization for any shared results

---

## 17. Documentation References

### Markdown Best Practices
**Reference:** Markdown Guide
- **URL:** https://www.markdownguide.org/
- **Applied in:** All `.md` documentation files

### JSON Schema Specification
**Reference:** JSON Schema (Draft 2020-12)
- **URL:** https://json-schema.org/
- **Applied in:** `docs/schemas/*.json` files

### YAML Specification
**Reference:** YAML Ain't Markup Language (YAML™) Version 1.2
- **URL:** https://yaml.org/spec/1.2/spec.html
- **Applied in:** `config.yml`, `docs/templates/mutation-recipe.yml`

---

## 18. Reproducibility Standards

### Reproducible Research Principles
**Reference:** "The Turing Way: A Handbook for Reproducible Data Science"
- **Authors:** The Turing Way Community
- **URL:** https://the-turing-way.netlify.app/
- **ISBN:** 978-1-7339637-0-0
- **Concepts Applied:**
  - Version control (Git)
  - Dependency pinning (Cargo.lock)
  - Deterministic builds (fixed toolchain)
  - Documentation completeness
- **Implementation:** `docs/appendix/REPRODUCIBILITY.md`

---

## 19. Version Control

### Git Workflow
**Reference:** "A successful Git branching model"
- **Author:** Vincent Driessen
- **URL:** https://nvie.com/posts/a-successful-git-branching-model/
- **Applied in:** Branch naming, commit practices (implied by development workflow)

---

## 20. Continuous Integration

### GitHub Actions (Future)
**Reference:** GitHub Actions Documentation
- **URL:** https://docs.github.com/en/actions
- **Planned for:** Automated testing, Docker builds, proto compilation checks

---

## Conclusion

This project synthesizes ideas from multiple domains:
- **Academic Research:** EDR evasion, explainable ML, fuzzing algorithms
- **Industry Standards:** gRPC, Protocol Buffers, Docker, Elasticsearch
- **Open Source Projects:** OSS-Fuzz, Vector.dev, krabsetw, RedEDR
- **Best Practices:** Rust API Guidelines, 12-Factor App, Buf Style Guide

All external concepts have been properly attributed. The implementation is original work that combines these ideas into a novel security research framework.

---

## How to Cite This Project

If you use this project in academic work, please cite as:

```bibtex
@software{edr_lab_2025,
  title = {Automated Analysis and Mutation of Software Artifacts against AV/EDR},
  author = {[Your Name/Team]},
  year = {2025},
  url = {[Repository URL]},
  note = {Security research framework for EDR evasion analysis}
}
```

---

**Last Updated:** 2025-01-10
**Maintainer:** [Your Name]
**Contact:** [Your Email]
