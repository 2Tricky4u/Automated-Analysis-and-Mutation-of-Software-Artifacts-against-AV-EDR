# EDR Lab Architecture

## Overview

This is a hybrid EDR lab designed for automated analysis and mutation of software artifacts against AV/EDR systems.

## Architecture Components

### Host System (Windows with Hyper-V)
- **Baseline VM**: Clean Windows environment for baseline testing
- **Defender VM**: Windows with Windows Defender enabled for detection testing
- **WSL2 Ubuntu**: Runs Docker containers for Elastic Stack and services

### Services

#### Controller (Rust gRPC)
Located in `/controller`

- **Scheduler** (`controller/scheduler`): Orchestrates analysis jobs
  - Manages job queue
  - Tracks job status
  - Coordinates with workers
  - Port: 50051

- **Triage Client** (`controller/triage-client`): Submits analysis results
  - Reports detection status
  - Submits IOCs
  - Queries results

#### Worker (Rust gRPC)
Located in `/worker`

- **Worker Agent** (`worker/agent`): Executes analysis tasks
  - Builds artifacts with mutations
  - Runs samples in sandbox
  - Collects telemetry
  - Accessible via SSH and gRPC
  - Port: 50052

- **Harness IPC** (`worker/harness-ipc`): Inter-process communication library
  - Message passing between harness and samples
  - Status reporting

#### Build System
Located in `/build`

- **Emitter** (`build/emitter`): Artifact builder
  - Compiles source code
  - Applies mutation strategies
  - Generates artifacts

- **Dockerfiles** (`build/dockerfiles`): Container definitions
  - Controller container
  - Worker container
  - Docker Compose orchestration

### Telemetry Pipeline

#### ETW Consumer (C++ with krabsetw)
Located in `/telemetry/etw-consumer`

- Captures Windows ETW events:
  - Process creation/termination
  - Network connections
  - File operations
  - Registry modifications
- Outputs CSV format for Filebeat ingestion

#### Filebeat
Located in `/telemetry/beats-config`

- Collects ETW consumer output
- Forwards to Elasticsearch
- Configuration: `filebeat.yml`

#### Elastic Stack
- **Elasticsearch**: Stores telemetry data
  - Index: `edr-telemetry-*`
  - Index: `edr-logs-*`
  - Port: 9200

- **Kibana**: Visualizes analysis results
  - Port: 5601
  - Dashboards: `/ui/kibana-dashboards`

## Data Flow

1. **Build Phase**:
   ```
   Source Code → Emitter → Mutated Artifact
   ```

2. **Execution Phase**:
   ```
   Worker Agent → Run Sample → ETW Consumer → CSV Output
   ```

3. **Collection Phase**:
   ```
   ETW CSV → Filebeat → Elasticsearch → Kibana Dashboard
   ```

4. **Analysis Loop**:
   ```
   Controller schedules job → Worker builds → Worker runs → 
   ETW captures → Filebeat ingests → Triage client reports → 
   Results visualized in Kibana
   ```

## Network Architecture

```
Windows Host (Hyper-V)
├── Baseline VM
│   └── ETW Consumer
├── Defender VM
│   └── ETW Consumer
└── WSL2 Ubuntu
    └── Docker Network (edr-network)
        ├── Elasticsearch (9200, 9300)
        ├── Kibana (5601)
        ├── Filebeat
        ├── Controller (50051)
        └── Worker (50052)
```

## Communication Protocols

- **gRPC**: Controller ↔ Worker, Triage Client ↔ Controller
- **HTTP**: Filebeat → Elasticsearch, Kibana → Elasticsearch
- **SSH**: Host → Worker (for management)
- **File System**: ETW Consumer → Filebeat (CSV files)

## Proto Definitions

Located in `/controller/proto/edr.proto`

Key services:
- `WorkerAgent`: Build execution and sample running
- `Controller`: Job scheduling and result queries

## Schemas

Located in `/docs/schemas`

- `job-schema.json`: Analysis job definition
- `telemetry-schema.json`: ETW event structure

## Deployment

### Local Development
```bash
# Build Rust services
cargo build --release

# Start infrastructure
cd build/dockerfiles
docker-compose up -d
```

### Running ETW Consumer (Windows)
```bash
cd telemetry/etw-consumer
mkdir build && cd build
cmake ..
cmake --build .
./etw_consumer
```

## Mutation Strategies

Available mutation techniques:
- String obfuscation
- API hashing
- Control flow flattening
- Junk code insertion
- Direct syscalls

## Future Enhancements

- [ ] Multi-worker support with load balancing
- [ ] Advanced mutation engine with ML-based selection
- [ ] Real-time detection correlation
- [ ] Automated report generation
- [ ] Integration with threat intelligence feeds
