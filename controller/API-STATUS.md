# Controller Public API Summary

## Currently Exposed gRPC Endpoints (Controller Service)

| Endpoint | Status | Description |
|----------|--------|-------------|
| `Ping` | ✅ Implemented | Health check |
| `ScheduleJob` | ✅ Implemented | Submit mutation job |
| `BuildArtifact` | ✅ Implemented | Build from template/mutations |
| `DeployArtifact` | ✅ Implemented | Stream artifact to worker |
| `ListWorkers` | ✅ Implemented | List registered workers |
| `StreamTelemetry` | ✅ Implemented | Client streaming to ES |
| `ReportStatus` | ✅ Implemented | Worker status reports |
| `SubmitTriage` | ✅ Implemented | Accept triage results |
| `GetJobStatus` | ✅ Implemented | ES query for job status |
| `GetJobProgress` | ✅ Implemented | ES query for job + rounds |
| `StopJob` | ✅ Implemented | Stop job via pool shutdown |
| `GetRound` | ✅ Implemented | ES query for round details |
| `CompareRuns` | ✅ Implemented | ES query for run comparison |
| `QueryResults` | ⚠️ Stub | Needs ES query |
| `GetWorker` | ✅ **NEW** | Get worker by ID |
| `GetAvailableWorkers` | ✅ **NEW** | Filter available workers by OS/caps |
| `GetWorkerMetadata` | ✅ **NEW** | Enhanced worker info (health, tools) |
| `GetPoolMetrics` | ✅ **NEW** | Pool utilization metrics |
| `GetOrchestratorStatus` | ✅ **NEW** | Queue depth, pool/worker counts |

## Priority 1: Completed ✅

All ES queries implemented:
- `GetJobStatus` - Queries `jobs-*` index for job document
- `GetJobProgress` - Queries jobs + rounds indices
- `StopJob` - Finds pool running job, calls `pool.shutdown()`
- `GetRound` - Queries `rounds-*` index for round details
- `CompareRuns` - Queries `runs-*` for two runs, computes differences

## Priority 2: Completed ✅

New monitoring endpoints added to proto and implemented:
- `GetWorker` - Get specific worker by ID
- `GetAvailableWorkers` - Filter workers by status/OS/capabilities
- `GetWorkerMetadata` - Enhanced info (health status, connected_at, tools)
- `GetPoolMetrics` - Per-pool metrics (runs dispatched/completed, queue size)
- `GetOrchestratorStatus` - System-wide status (pools, workers, active jobs)

## TargetManager Functions Exposed ✅

```rust
// Now exposed via gRPC:
pub fn get(&self, id: &str) -> Option<Target>      // via GetWorker
pub fn list_all(&self) -> Vec<Target>              // via ListWorkers
pub fn get_available(&self) -> Vec<String>         // via GetAvailableWorkers
// Enhanced info via GetWorkerMetadata
```

## Defined But NOT Implemented Services

| Service | RPCs | Status |
|---------|------|--------|
| `Selector` | `SelectMutation`, `ReportOutcome` | Defined in proto, not in scheduler |
| `Triage` | `AnalyzeRun`, `GetAvoidList` | Defined in proto, separate crate |

## Summary

| Category | Total | Implemented | Stub/Missing |
|----------|-------|-------------|--------------|
| Controller Service | 19 | 18 | 1 |
| Selector Service | 2 | 0 | 2 |
| Triage Service | 2 | 0 | 2 |
| **Total** | **23** | **18 (78%)** | **5 (22%)** |

## New Proto Messages Added

```protobuf
// Worker queries
message GetWorkerRequest { string worker_id = 1; }
message GetWorkerResponse { WorkerInfo worker = 1; bool found = 2; }

message GetAvailableWorkersRequest {
  string target_os = 1;
  repeated string required_capabilities = 2;
}
message GetAvailableWorkersResponse {
  repeated WorkerInfo workers = 1;
  int32 total_available = 2;
}

message GetWorkerMetadataRequest { string worker_id = 1; }
message GetWorkerMetadataResponse { repeated WorkerMetadataEntry workers = 1; }
message WorkerMetadataEntry {
  string worker_id, address, status, os_version;
  repeated string capabilities;
  map<string, string> metadata;
  ToolVersions tools;
  int64 last_seen_seconds_ago;
  bool healthy;
  string current_job_id;
  int64 connected_at;
}

// Pool metrics
message GetPoolMetricsRequest { string pool_id = 1; }
message GetPoolMetricsResponse { repeated PoolMetricsEntry pools = 1; }
message PoolMetricsEntry {
  string pool_id;
  uint64 total_runs_dispatched, total_runs_completed;
  uint64 total_rounds_completed, total_jobs_completed;
  uint32 current_queue_size, worker_count;
  string current_job_id;
}

// Orchestrator status
message GetOrchestratorStatusRequest {}
message GetOrchestratorStatusResponse {
  uint32 pending_jobs, active_pools, total_workers;
  uint32 available_workers, busy_workers;
  repeated string pool_ids;
  repeated ActiveJobEntry active_jobs;
}
```
