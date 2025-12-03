# Automated Analysis and Mutation of Software Artifacts against AV/EDR

A comprehensive hybrid EDR lab for automated analysis and mutation testing of software artifacts against antivirus and endpoint detection and response systems.

## 🏗️ Architecture

This lab implements a build → run → collect → visualize loop for testing EDR detection capabilities given an artifact:

- **Windows Workers (Hyper-V)**: Windows VMs (baseline and Windows Defender/Cortex)
- **WSL2 Ubuntu**: Controller with Docker containers running Elastic Stack
- **Rust gRPC Services**: Controller and Worker agents
- **Telemetry Pipeline**: RedEDR → Elasticsearch → Kibana


## 📁 Project Structure

```
.
├── controller/           # Controller services (Rust)
│   ├── proto/           # gRPC protocol definitions
│   ├── scheduler/       # Job scheduling service
│   └── triage-client/   # Triage reporting client
├── worker/              # Worker services (Rust)
│   ├── agent/          # Worker agent (SSH + gRPC)
│   └── harness-ipc/    # Inter-process communication
├── build/               # Build system
│   ├── dockerfiles/    # Container definitions
│   └── emitter/        # Artifact builder with mutations
├── telemetry/           # Telemetry collection
│   ├── etw-consumer/   # C++ ETW consumer (krabsetw)
│   └── beats-config/   # Filebeat configuration
├── ui/                  # User interface
│   └── kibana-dashboards/ # Kibana visualizations
└── docs/                # Documentation
    └── schemas/        # JSON schemas for jobs and telemetry
```

### Service Endpoints

- **Controller gRPC**: `localhost:50051`
- **Worker Agent gRPC**: `localhost:50052`
- **Elasticsearch**: `http://localhost:9200`
- **Kibana**: `http://localhost:5601`

## 🔧 Usage

### Scheduling an Analysis Job

```bash
# Run the triage client
cargo run -p triage-client

# Or use grpcurl
grpcurl -plaintext -d '{
  "name": "test-malware",
  "artifact_type": "exe",
  "source": "/samples/test.exe",
  "mutation_strategies": ["string_obfuscation"],
  "priority": 1
}' localhost:50051 edr.Controller/ScheduleJob
```

### Checking Job Status

```bash
grpcurl -plaintext -d '{
  "job_id": "job-000001"
}' localhost:50051 edr.Controller/GetJobStatus
```

### Running ETW Consumer

```powershell
# On Windows VM
.\etw_consumer.exe C:\logs\etw_events.csv
```

### Viewing Results in Kibana

1. Open `http://localhost:5601`
2. Navigate to **Dashboard**
3. Import dashboards from `ui/kibana-dashboards/edr-dashboard.ndjson`
4. View real-time telemetry and detection results

## 🧬 Mutation Strategies

Mutation techniques idea:
- **String Obfuscation**: Encrypt/encode strings at compile time
- **API Hashing**: Replace API names with hash-based lookups
- **Control Flow Flattening**: Obscure program control flow
- **Junk Code Insertion**: Add non-functional code
- **Direct Syscalls**: Bypass API hooks via syscalls

## 📊 Telemetry Events

The ETW consumer could captures:
- Process creation/termination
- Thread creation
- Image (DLL) loading
- Network connections (TCP/IP)
- File operations
- Registry modifications

## 🔬 Data Flow

1. **Build Phase**: Controller schedules job → Worker builds artifact with mutations
2. **Execution Phase**: Worker runs artifact in sandbox → ETW captures events
3. **Collection Phase**: Filebeat ingests ETW logs → Elasticsearch stores data
4. **Visualization**: Kibana displays analysis results and detection statistics

## 📋 Schemas

- [Job Schema](docs/schemas/job-schema.json): Analysis job definition
- [Telemetry Schema](docs/schemas/telemetry-schema.json): ETW event structure

## 🛠️ Development

### Running Tests

```bash
# Run all tests
cargo test --workspace

# Run specific package tests
cargo test -p harness-ipc
```

### Building Individual Services

```bash
# Controller
cargo build -p scheduler

# Worker
cargo build -p worker-agent

# Build system
cargo build -p emitter

# Triage client
cargo build -p triage-client
```

## 📝 License

This project is for educational and research purposes only. Use responsibly and in compliance with applicable laws.

## 🎯 Roadmap

- [ ] Multi-worker support with load balancing
- [ ] Advanced mutation engine with ML-based selection
- [ ] Real-time detection correlation
- [ ] Automated report generation
- [ ] Integration with threat intelligence feeds
- [ ] Support for additional artifact types (PowerShell, .NET)

## 🔗 References

- [krabsetw](https://github.com/microsoft/krabsetw) - ETW library for C++
- [Elastic Stack](https://www.elastic.co/elastic-stack) - Search and analytics
- [gRPC](https://grpc.io/) - High-performance RPC framework
- [Tonic](https://github.com/hyperium/tonic) - Rust gRPC implementation