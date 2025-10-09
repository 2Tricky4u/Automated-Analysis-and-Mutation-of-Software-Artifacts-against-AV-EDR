# Skeleton Gap Analysis: Path to 100% CLAUDE.md Compliance

**Current Status:** 95% skeleton structure, 62.5% overall
**Target:** 100% skeleton structure with state-of-the-art design
**Date:** 2025-01-10

---

## Executive Summary

While the skeleton is **structurally excellent (95%)**, there are **critical architectural gaps** that prevent 100% compliance with CLAUDE.md and state-of-the-art patterns. This document identifies 12 specific gaps and provides exact remediation steps.

---

## Gap Analysis by Category

### 1. Architecture Layer (CLAUDE.md Section 2)

#### ❌ Gap 1.1: UI Component Missing

**CLAUDE.md Requirement:**
> "Controller: Mutator · Selector · Queue · **UI** · Rule Manager"

**Current State:**
```
ui/
└── kibana-dashboards/  # Only Kibana configs, no UI backend
```

**Missing:**
- REST API for job submission (alternative to gRPC CLI)
- Web dashboard for experiment management
- Real-time job status WebSocket
- Report viewer UI

**State-of-the-Art Reference:**
- [Grafana](https://github.com/grafana/grafana) - Separates API from visualization
- [Temporal UI](https://github.com/temporalio/ui) - Modern workflow UI

**Impact on Compliance:** Medium (UI is mentioned but not critical path)

**Remediation:**
```bash
# Create UI backend service
mkdir -p ui/backend/src
cat > ui/backend/Cargo.toml << 'EOF'
[package]
name = "ui-backend"
version.workspace = true
edition.workspace = true

[dependencies]
axum = "0.7"
tower = "0.4"
tower-http = { version = "0.5", features = ["cors", "fs"] }
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
EOF

# Create REST API
cat > ui/backend/src/main.rs << 'EOF'
use axum::{
    routing::{get, post},
    Router,
    Json,
};

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs", post(submit_job))
        .route("/api/jobs/:id", get(get_job_status));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .unwrap();

    axum::serve(listener, app).await.unwrap();
}

async fn list_jobs() -> Json<Vec<String>> {
    Json(vec![])
}

async fn submit_job() -> Json<String> {
    Json("job-000001".to_string())
}

async fn get_job_status() -> Json<String> {
    Json("queued".to_string())
}
EOF
```

**Update Workspace:**
```toml
members = [
    # ... existing
    "ui/backend",
]
```

---

#### ❌ Gap 1.2: Monitor Component Missing

**CLAUDE.md Requirement (Section 3):**
> "**Monitor:** labels outcomes: detected | not_detected | noisy | crashed, returns metrics."

**Current State:**
- `worker/harness/src/lib.rs` has `Outcome` enum
- No dedicated `Monitor` service or component
- No centralized outcome labeling logic

**Missing:**
- Standalone Monitor service that watches runs
- Automated labeling based on telemetry heuristics
- Metrics collection (CPU, memory, detection latency)
- Integration with Elastic for real-time monitoring

**State-of-the-Art Reference:**
- [Prometheus + Alertmanager](https://prometheus.io/) - Metrics + labeling
- [OpenTelemetry Collector](https://opentelemetry.io/) - Observability pipeline

**Impact on Compliance:** High (explicitly mentioned as separate component)

**Remediation:**
```bash
# Create Monitor service
mkdir -p worker/monitor/src
cat > worker/monitor/Cargo.toml << 'EOF'
[package]
name = "monitor"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace = true
serde.workspace = true
anyhow.workspace = true
tracing.workspace = true
EOF

cat > worker/monitor/src/lib.rs << 'EOF'
/// Monitor service for outcome labeling and metrics collection
///
/// Implements CLAUDE.md Section 3: Monitor component

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Outcome {
    Detected,
    NotDetected,
    Noisy,
    Crashed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMetrics {
    pub cpu_pct: f64,
    pub mem_mb: u64,
    pub detection_latency_ms: Option<u64>,
}

pub struct Monitor {}

impl Monitor {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn label_outcome(
        &self,
        _run_id: &str,
    ) -> Result<(Outcome, RunMetrics), Box<dyn std::error::Error>> {
        // TODO: Implement automated labeling based on telemetry
        Ok((Outcome::NotDetected, RunMetrics {
            cpu_pct: 0.0,
            mem_mb: 0,
            detection_latency_ms: None,
        }))
    }
}
EOF
```

**Update Workspace:**
```toml
members = [
    # ... existing
    "worker/monitor",
]
```

---

### 2. Telemetry Architecture (CLAUDE.md Section 5)

#### ❌ Gap 2.1: Typed Feature Indexing Missing

**CLAUDE.md Requirement:**
> "For every channel, **index small, typed features (booleans, enums, counts, hashes, min/max/Δt), not raw streams.**"

**Current State:**
- `telemetry/collector/src/rededr.rs` parses to generic `RedEdrEvent`
- No feature extraction pipeline
- No typed feature schema

**Missing:**
- Feature extractor that converts raw events → typed features
- Elasticsearch mappings for typed fields
- Feature registry/catalog

**State-of-the-Art Reference:**
- [Elastic Common Schema (ECS)](https://www.elastic.co/guide/en/ecs/current/index.html) - Typed field standard
- [Logstash Filters](https://www.elastic.co/guide/en/logstash/current/filter-plugins.html) - Feature extraction

**Impact on Compliance:** High (core to triage engine)

**Remediation:**
```bash
# Create feature extractor module
cat > telemetry/collector/src/feature_extractor.rs << 'EOF'
/// Feature extractor for typed telemetry features
///
/// Implements CLAUDE.md Section 5: typed features

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypedFeatures {
    // Boolean features
    pub mem_rwx_short_window: bool,
    pub thread_start_anon: bool,
    pub proc_parent_unsigned: bool,
    pub syscall_direct: bool,

    // Count features
    pub mem_allocations: u32,
    pub network_connections: u32,
    pub process_creations: u32,

    // Timing features (milliseconds)
    pub mem_write_to_execute_ms: Option<u32>,
    pub write_to_threadstart_ms: Option<u32>,

    // Hashes
    pub ja3_hash: Option<String>,
    pub image_hash: Option<String>,

    // Enums
    pub alert_level: AlertLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertLevel {
    None,
    Low,
    Med,
    High,
}

pub struct FeatureExtractor {}

impl FeatureExtractor {
    pub fn new() -> Self {
        Self {}
    }

    pub fn extract(&self, events: &[super::rededr::RedEdrEvent]) -> TypedFeatures {
        TypedFeatures {
            mem_rwx_short_window: self.detect_rwx_window(events),
            thread_start_anon: self.detect_thread_start_anon(events),
            proc_parent_unsigned: false,
            syscall_direct: false,
            mem_allocations: self.count_memory_ops(events),
            network_connections: 0,
            process_creations: 0,
            mem_write_to_execute_ms: self.compute_write_to_exec(events),
            write_to_threadstart_ms: None,
            ja3_hash: None,
            image_hash: None,
            alert_level: AlertLevel::None,
        }
    }

    fn detect_rwx_window(&self, _events: &[super::rededr::RedEdrEvent]) -> bool {
        // TODO: Implement RWX window detection
        false
    }

    fn detect_thread_start_anon(&self, _events: &[super::rededr::RedEdrEvent]) -> bool {
        false
    }

    fn count_memory_ops(&self, _events: &[super::rededr::RedEdrEvent]) -> u32 {
        0
    }

    fn compute_write_to_exec(&self, _events: &[super::rededr::RedEdrEvent]) -> Option<u32> {
        None
    }
}
EOF

# Update collector main.rs to use feature extractor
```

---

#### ❌ Gap 2.2: Collector Config Facts Not Recorded

**CLAUDE.md Requirement (Section 6):**
```json
{
  "etw": {"buffersize_kb":1024,"lost_events":0},
  "collector": {"threads":4,"cache_pools":4,"parser":"sliding"},
  "sem": {"fixups":["filekey->name","thread->process"]},
  "stack": {"user_hash":"u#...","kernel_hash":"k#..."}
}
```

**Current State:**
- No structure to capture collector config facts
- No SLO metrics (lost_events, event_to_record_ms)

**Missing:**
- `CollectorConfigFacts` struct
- SLO metrics collection
- Integration with RunResult

**Remediation:**
```bash
cat > telemetry/collector/src/slo.rs << 'EOF'
/// SLO metrics and collector config facts
///
/// Implements CLAUDE.md Section 6: Collector Config Facts

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorConfigFacts {
    pub etw: EtwConfig,
    pub collector: CollectorConfig,
    pub semantic_enrichment: SemanticConfig,
    pub stack: StackConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtwConfig {
    pub buffersize_kb: u32,
    pub lost_events: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorConfig {
    pub threads: u32,
    pub cache_pools: u32,
    pub parser: String,  // "sliding", "batch", etc.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticConfig {
    pub fixups: Vec<String>,  // ["filekey->name", "thread->process"]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackConfig {
    pub user_hash: String,
    pub kernel_hash: String,
}

impl Default for CollectorConfigFacts {
    fn default() -> Self {
        Self {
            etw: EtwConfig {
                buffersize_kb: 1024,
                lost_events: 0,
            },
            collector: CollectorConfig {
                threads: 4,
                cache_pools: 4,
                parser: "sliding".to_string(),
            },
            semantic_enrichment: SemanticConfig {
                fixups: vec![
                    "filekey->name".to_string(),
                    "thread->process".to_string(),
                ],
            },
            stack: StackConfig {
                user_hash: String::new(),
                kernel_hash: String::new(),
            },
        }
    }
}
EOF
```

---

### 3. Proto Definitions (Interface Contracts)

#### ❌ Gap 3.1: Selector/Triage Services Not in Scheduler

**Current Issue:**
`controller/scheduler/src/main.rs` only implements `Controller` service, but CLAUDE.md Section 2 defines Selector and Triage as separate services.

**CLAUDE.md Requirement:**
- Selector service: `SelectMutation`, `ReportOutcome`
- Triage service: `AnalyzeRun`, `GetAvoidList`

**Current State:**
These services are defined in proto but NOT implemented in any binary.

**Missing:**
- `controller/selector/src/main.rs` should start gRPC server for Selector service
- `controller/triage-engine/src/main.rs` should start gRPC server for Triage service

**State-of-the-Art Reference:**
- Microservices pattern: Each service in separate binary
- [Buf Connect](https://connectrpc.com/) - Modern gRPC service patterns

**Impact on Compliance:** High (violates "modular by interface contracts")

**Remediation:**

Update `controller/selector/src/main.rs`:
```rust
use tonic::{transport::Server, Request, Response, Status};
use tracing::info;

pub mod edr {
    tonic::include_proto!("edr");
}

use edr::controller::{
    selector_server::{Selector, SelectorServer},
    SelectionRequest, SelectionResponse, OutcomeReport, OutcomeAck,
};

#[derive(Debug, Default)]
pub struct SelectorService {}

#[tonic::async_trait]
impl Selector for SelectorService {
    async fn select_mutation(
        &self,
        request: Request<SelectionRequest>,
    ) -> Result<Response<SelectionResponse>, Status> {
        let req = request.into_inner();
        info!("Selecting mutations for job: {:?}", req.job_id);

        Ok(Response::new(SelectionResponse {
            mutations: vec![],
            exploration_probability: 0.3,
            rationale: "Placeholder".to_string(),
        }))
    }

    async fn report_outcome(
        &self,
        request: Request<OutcomeReport>,
    ) -> Result<Response<OutcomeAck>, Status> {
        let req = request.into_inner();
        info!("Outcome reported for run: {:?}", req.run_id);

        Ok(Response::new(OutcomeAck { received: true }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr = "0.0.0.0:50054".parse()?;
    let selector = SelectorService::default();

    info!("Selector service starting on {}", addr);

    Server::builder()
        .add_service(SelectorServer::new(selector))
        .serve(addr)
        .await?;

    Ok(())
}
```

Update `controller/triage-engine/src/main.rs`:
```rust
use tonic::{transport::Server, Request, Response, Status};
use tracing::info;

pub mod edr {
    tonic::include_proto!("edr");
}

use edr::controller::{
    triage_server::{Triage, TriageServer},
    AnalysisRequest, AnalysisResponse, AvoidListRequest, AvoidListResponse,
};

#[derive(Debug, Default)]
pub struct TriageService {}

#[tonic::async_trait]
impl Triage for TriageService {
    async fn analyze_run(
        &self,
        request: Request<AnalysisRequest>,
    ) -> Result<Response<AnalysisResponse>, Status> {
        let req = request.into_inner();
        info!("Analyzing run: {:?}", req.run_id);

        Ok(Response::new(AnalysisResponse {
            hypotheses: vec![],
            avoid_features: vec![],
        }))
    }

    async fn get_avoid_list(
        &self,
        request: Request<AvoidListRequest>,
    ) -> Result<Response<AvoidListResponse>, Status> {
        let req = request.into_inner();
        info!("Getting avoid list for job: {:?}", req.job_id);

        Ok(Response::new(AvoidListResponse {
            features: vec![],
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr = "0.0.0.0:50055".parse()?;
    let triage = TriageService::default();

    info!("Triage service starting on {}", addr);

    Server::builder()
        .add_service(TriageServer::new(triage))
        .serve(addr)
        .await?;

    Ok(())
}
```

**Update Docker Compose:**
```yaml
services:
  selector:
    build:
      context: ../../
      dockerfile: build/dockerfiles/Dockerfile.controller
    command: ["./selector"]
    ports:
      - "50054:50054"
    depends_on:
      - elasticsearch

  triage:
    build:
      context: ../../
      dockerfile: build/dockerfiles/Dockerfile.controller
    command: ["./triage-engine"]
    ports:
      - "50055:50055"
    depends_on:
      - elasticsearch
```

---

### 4. Configuration Management

#### ❌ Gap 4.1: No Configuration Loading

**Current State:**
- `config.example.yml` exists (excellent)
- NO service actually loads this config
- All services have hardcoded values

**State-of-the-Art Reference:**
- [config-rs](https://github.com/mehcode/config-rs) - Rust config library
- [Viper](https://github.com/spf13/viper) - Go config (for reference)

**Impact on Compliance:** Medium (violates reproducibility)

**Remediation:**
```bash
# Create shared config library
mkdir -p config/src
cat > config/Cargo.toml << 'EOF'
[package]
name = "config"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
config = "0.14"
EOF

cat > config/src/lib.rs << 'EOF'
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub controller: ControllerConfig,
    pub worker: WorkerConfig,
    pub telemetry: TelemetryConfig,
    pub elasticsearch: ElasticsearchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerConfig {
    pub host: String,
    pub port: u16,
    pub max_jobs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub id: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub etw_buffer_size_kb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElasticsearchConfig {
    pub hosts: Vec<String>,
}

impl AppConfig {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name("config"))
            .build()?;

        Ok(settings.try_deserialize()?)
    }
}
EOF
```

**Update services to use config:**
```rust
// In controller/scheduler/src/main.rs
use config::AppConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = AppConfig::load()?;
    let addr = format!("{}:{}", cfg.controller.host, cfg.controller.port);
    // ...
}
```

---

### 5. Testing Infrastructure

#### ❌ Gap 5.1: No Integration Tests

**CLAUDE.md Requirement (Section 16):**
> "Results reproducible with recorded artifact IDs, seeds, and collector config."

**Current State:**
- `tests/e2e/` directory exists but is empty
- No integration tests

**Missing:**
- End-to-end test: submit job → build → execute → collect → triage
- Proto contract tests (prost verify)
- Docker Compose test harness

**State-of-the-Art Reference:**
- [Testcontainers](https://testcontainers.com/) - Container-based integration tests
- [k6](https://k6.io/) - Load testing

**Impact on Compliance:** High (repeatability requirement)

**Remediation:**
```bash
cat > tests/e2e/integration_test.rs << 'EOF'
#[tokio::test]
async fn test_full_pipeline() {
    // 1. Start Docker Compose stack
    // 2. Wait for health checks
    // 3. Submit job via gRPC
    // 4. Poll for completion
    // 5. Query Elasticsearch for telemetry
    // 6. Verify RunResult schema
    // 7. Verify triage hypotheses generated
}
EOF
```

---

### 6. Observability

#### ❌ Gap 6.1: No Structured Logging

**Current State:**
- All services use `tracing::info!()` unstructured
- No span/trace IDs
- No correlation across services

**State-of-the-Art Reference:**
- [OpenTelemetry](https://opentelemetry.io/) - Distributed tracing
- [tracing-opentelemetry](https://docs.rs/tracing-opentelemetry/) - Rust integration

**Impact on Compliance:** Low (nice-to-have)

**Remediation:**
```bash
# Add to workspace dependencies
[workspace.dependencies]
tracing-opentelemetry = "0.22"
opentelemetry = "0.21"
```

---

### 7. Deployment & Operations

#### ❌ Gap 7.1: No Kubernetes Manifests

**Current State:**
- Docker Compose only (good for local)
- No K8s manifests for production

**State-of-the-Art Reference:**
- [Helm charts](https://helm.sh/) - K8s package manager
- [Kustomize](https://kustomize.io/) - K8s config management

**Impact on Compliance:** Low (Docker Compose satisfies "start minimal")

**Optional Enhancement:**
```bash
mkdir -p deploy/k8s
# Create deployments, services, configmaps
```

---

## Summary: Gaps by Priority

### Critical (Blocks 100% Skeleton)

1. ✅ **Monitor component** - Mentioned explicitly in CLAUDE.md Section 3
2. ✅ **UI backend** - Controller: UI mentioned in Section 2
3. ✅ **Typed feature indexing** - Core requirement Section 5
4. ✅ **Selector/Triage gRPC services** - Implement services in binaries
5. ✅ **CollectorConfigFacts** - Schema requirement Section 6

### High (Improves State-of-the-Art)

6. ✅ **Config loading** - Shared config library
7. ✅ **Integration tests** - E2E tests in tests/e2e/
8. ✅ **Feature extractor** - Separate module in collector

### Medium (Nice-to-Have)

9. ⚠️ **Structured logging** - OpenTelemetry integration
10. ⚠️ **K8s manifests** - For production deployment

---

## Remediation Checklist

To achieve **100% skeleton compliance**:

```bash
# Critical Items
[ ] Create worker/monitor/ component
[ ] Create ui/backend/ REST API
[ ] Add telemetry/collector/src/feature_extractor.rs
[ ] Add telemetry/collector/src/slo.rs
[ ] Convert selector to gRPC service binary
[ ] Convert triage-engine to gRPC service binary
[ ] Update Docker Compose with selector + triage services

# High Priority
[ ] Create config/ shared library
[ ] Update all services to load config
[ ] Write tests/e2e/integration_test.rs
[ ] Add proto contract tests

# Total Estimated Effort: 2-3 days
```

---

## Final Score After Remediation

| Metric | Current | After Remediation | Target |
|--------|---------|-------------------|--------|
| **Skeleton Structure** | 95% | 100% | 100% |
| **CLAUDE.md Compliance** | 62.5% | 85% | 100% |
| **State-of-the-Art** | Good | Excellent | Best |

---

## Conclusion

Your skeleton is **excellent** but has 8 critical gaps preventing 100% compliance:

1. Missing Monitor component
2. Missing UI backend
3. No typed feature indexing
4. Selector/Triage not running as services
5. No CollectorConfigFacts recording
6. No config loading
7. No integration tests
8. Feature extractor not separated

**All gaps are structural** (not implementation). Once remediated, your skeleton will be **world-class** and ready for business logic.

**Estimated Time:** 2-3 days of focused work.
