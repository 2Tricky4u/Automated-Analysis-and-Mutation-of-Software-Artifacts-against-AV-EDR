# Usage Examples

## Example 1: Simple Analysis Job

### Schedule a Job

```bash
# Using grpcurl
grpcurl -plaintext -d '{
  "name": "test-notepad",
  "artifact_type": "exe",
  "source": "/samples/benign/notepad.exe",
  "mutation_strategies": [],
  "priority": 1
}' localhost:50051 edr.Controller/ScheduleJob
```

**Response:**
```json
{
  "job_id": "job-000001",
  "accepted": true,
  "message": "Job job-000001 scheduled successfully",
  "estimated_duration_seconds": "300"
}
```

### Check Job Status

```bash
grpcurl -plaintext -d '{
  "job_id": "job-000001"
}' localhost:50051 edr.Controller/GetJobStatus
```

**Response:**
```json
{
  "job_id": "job-000001",
  "status": "running",
  "progress_percent": 50,
  "current_phase": "executing",
  "logs": [
    "Job job-000001 created",
    "Building artifact...",
    "Running sample..."
  ]
}
```

## Example 2: Mutated Artifact Analysis

### Schedule with Mutations

```bash
grpcurl -plaintext -d '{
  "name": "mutated-payload",
  "artifact_type": "exe",
  "source": "/samples/payload.rs",
  "mutation_strategies": [
    "string_obfuscation",
    "api_hashing",
    "control_flow_flattening"
  ],
  "priority": 5
}' localhost:50051 edr.Controller/ScheduleJob
```

### Worker Executes Build

The worker agent will:
1. Receive build request
2. Apply mutations
3. Compile artifact
4. Return build result

## Example 3: Worker Health Check

```bash
grpcurl -plaintext -d '{
  "worker_id": "worker-01"
}' localhost:50052 edr.WorkerAgent/HealthCheck
```

**Response:**
```json
{
  "worker_id": "worker-01",
  "healthy": true,
  "cpu_percent": 25,
  "memory_percent": 40,
  "active_jobs": 2
}
```

## Example 4: Triage Submission

### Submit Detection Results

```rust
use tonic::Request;
use edr::{controller_client::ControllerClient, TriageRequest};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ControllerClient::connect("http://localhost:50051").await?;
    
    let mut iocs = HashMap::new();
    iocs.insert("file".to_string(), "C:\\Windows\\Temp\\payload.exe".to_string());
    iocs.insert("registry".to_string(), "HKLM\\Software\\Startup".to_string());
    
    let request = Request::new(TriageRequest {
        job_id: "job-000001".to_string(),
        detected: true,
        av_product: "Windows Defender".to_string(),
        detection_type: "behavioral".to_string(),
        iocs,
    });
    
    let response = client.submit_triage(request).await?;
    println!("Triage ID: {}", response.into_inner().triage_id);
    
    Ok(())
}
```

## Example 5: Query Analysis Results

```bash
grpcurl -plaintext -d '{
  "job_ids": ["job-000001", "job-000002"],
  "filters": {
    "detected": "true"
  }
}' localhost:50051 edr.Controller/QueryResults
```

## Example 6: ETW Consumer with Custom Provider

### Add DNS Query Tracking

```cpp
// In etw_consumer.cpp
krabs::provider<> dns_provider(L"Microsoft-Windows-DNS-Client");
dns_provider.any(0x8000000000000000); // All events

dns_provider.add_on_event_callback([this](const EVENT_RECORD& record, 
                                           const krabs::trace_context& context) {
    krabs::schema schema(record, context.schema_locator);
    krabs::parser parser(schema);
    
    try {
        auto query_name = parser.parse<std::wstring>(L"QueryName");
        auto query_type = parser.parse<uint32_t>(L"QueryType");
        
        std::wcout << L"DNS Query: " << query_name 
                   << L" (Type: " << query_type << L")" << std::endl;
        
        this->write_event("dns", 0, query_name);
    } catch (...) {
        // Skip unparseable events
    }
});

trace.enable(dns_provider);
```

## Example 7: Kibana Query for Detected Artifacts

### KQL Query in Kibana

```kql
# All detected artifacts
etw.event_type:process AND edr.detected:true

# Process creation events
etw.event_type:process_create

# Network connections to suspicious IPs
etw.event_type:network AND (etw.dest_ip:10.0.0.* OR etw.dest_ip:192.168.*)

# File operations in system directories
etw.event_type:file_create AND etw.path:*System32*

# High-priority jobs
edr.priority:>=5
```

### Elasticsearch Query

```bash
curl -X POST "localhost:9200/edr-telemetry-*/_search?pretty" \
  -H 'Content-Type: application/json' -d'
{
  "query": {
    "bool": {
      "must": [
        { "term": { "etw.event_type.keyword": "process" } },
        { "range": { "@timestamp": { "gte": "now-1h" } } }
      ]
    }
  },
  "aggs": {
    "processes": {
      "terms": {
        "field": "etw.details.keyword",
        "size": 10
      }
    }
  }
}
'
```

## Example 8: Filebeat Custom Processor

### Add Process Name Extraction

```yaml
# In filebeat.yml
filebeat.inputs:
  - type: log
    paths:
      - /logs/etw_events.csv
    processors:
      - dissect:
          tokenizer: "%{timestamp},%{event_type},%{process_id},%{details}"
          field: "message"
          target_prefix: "etw"
      
      # Extract process name from details
      - dissect:
          tokenizer: "%{process_name}.exe"
          field: "etw.details"
          target_prefix: "process"
          ignore_failure: true
      
      # Add threat level based on process
      - script:
          lang: javascript
          source: >
            function process(event) {
              var suspicious = ["cmd", "powershell", "rundll32"];
              var proc = event.Get("process.process_name");
              if (proc && suspicious.includes(proc.toLowerCase())) {
                event.Put("threat.level", "high");
              }
            }
```

## Example 9: Docker Compose with Custom Configuration

### Custom Environment Variables

```yaml
# In docker-compose.yml
services:
  worker:
    environment:
      - WORKER_ID=worker-defender
      - LOG_LEVEL=debug
      - MAX_JOBS=5
      - TIMEOUT_SECONDS=600
```

### Volume Mounts for Artifacts

```yaml
  worker:
    volumes:
      - ./artifacts:/artifacts
      - ./logs:/logs
```

## Example 10: Programmatic Job Management

### Python Client Example

```python
import grpc
import edr_pb2
import edr_pb2_grpc

def schedule_batch_jobs(samples):
    channel = grpc.insecure_channel('localhost:50051')
    stub = edr_pb2_grpc.ControllerStub(channel)
    
    jobs = []
    for sample in samples:
        request = edr_pb2.JobRequest(
            name=sample['name'],
            artifact_type='exe',
            source=sample['path'],
            mutation_strategies=['string_obfuscation'],
            priority=sample.get('priority', 1)
        )
        
        response = stub.ScheduleJob(request)
        jobs.append(response.job_id)
        print(f"Scheduled: {response.job_id}")
    
    return jobs

def monitor_jobs(job_ids):
    channel = grpc.insecure_channel('localhost:50051')
    stub = edr_pb2_grpc.ControllerStub(channel)
    
    while job_ids:
        completed = []
        for job_id in job_ids:
            request = edr_pb2.JobStatusRequest(job_id=job_id)
            response = stub.GetJobStatus(request)
            
            print(f"{job_id}: {response.status} ({response.progress_percent}%)")
            
            if response.status in ['completed', 'failed']:
                completed.append(job_id)
        
        for job_id in completed:
            job_ids.remove(job_id)
        
        if job_ids:
            time.sleep(5)

# Usage
samples = [
    {'name': 'sample1', 'path': '/samples/s1.exe', 'priority': 1},
    {'name': 'sample2', 'path': '/samples/s2.exe', 'priority': 2},
]

jobs = schedule_batch_jobs(samples)
monitor_jobs(jobs)
```

## Example 11: Continuous Analysis Pipeline

### Shell Script for Batch Processing

```bash
#!/bin/bash

SAMPLES_DIR="/samples"
RESULTS_DIR="/results"

# Process all samples in directory
for sample in "$SAMPLES_DIR"/*.exe; do
    filename=$(basename "$sample")
    echo "Processing: $filename"
    
    # Schedule job
    job_id=$(grpcurl -plaintext -d "{
        \"name\": \"$filename\",
        \"artifact_type\": \"exe\",
        \"source\": \"$sample\",
        \"mutation_strategies\": [\"string_obfuscation\"],
        \"priority\": 1
    }" localhost:50051 edr.Controller/ScheduleJob | jq -r '.job_id')
    
    echo "Job ID: $job_id"
    
    # Wait for completion
    while true; do
        status=$(grpcurl -plaintext -d "{\"job_id\": \"$job_id\"}" \
                 localhost:50051 edr.Controller/GetJobStatus | jq -r '.status')
        
        if [ "$status" = "completed" ] || [ "$status" = "failed" ]; then
            break
        fi
        
        sleep 10
    done
    
    echo "$filename -> $status"
done
```

## Example 12: Real-time Dashboard Updates

### WebSocket Telemetry Stream (Concept)

```javascript
// Frontend JavaScript
const ws = new WebSocket('ws://localhost:8080/telemetry');

ws.onmessage = (event) => {
    const telemetry = JSON.parse(event.data);
    
    // Update live chart
    updateProcessChart(telemetry.process_count);
    updateNetworkChart(telemetry.network_events);
    
    // Alert on detection
    if (telemetry.detected) {
        showAlert(`Artifact detected: ${telemetry.job_id}`);
    }
};
```

## Troubleshooting Examples

### Debug Worker Connection

```bash
# Test worker connectivity
grpcurl -plaintext localhost:50052 list

# Test controller connectivity
grpcurl -plaintext localhost:50051 list

# Describe service
grpcurl -plaintext localhost:50051 describe edr.Controller
```

### Check Elasticsearch Data

```bash
# View all indices
curl http://localhost:9200/_cat/indices?v

# Get mapping
curl http://localhost:9200/edr-telemetry-*/_mapping?pretty

# Sample data
curl http://localhost:9200/edr-telemetry-*/_search?size=1&pretty
```

### Filebeat Debugging

```bash
# Test configuration
docker-compose exec filebeat filebeat test config -e

# Test output connection
docker-compose exec filebeat filebeat test output -e

# Check registry
docker-compose exec filebeat cat /usr/share/filebeat/data/registry/filebeat/log.json | jq
```

## Best Practices

1. **Start Simple**: Begin with benign samples
2. **Monitor Resources**: Watch CPU/memory usage
3. **Log Everything**: Enable debug logging initially
4. **Batch Processing**: Use scripts for multiple samples
5. **Regular Cleanup**: Clear old indices and logs
6. **Snapshot VMs**: Take VM snapshots before analysis
7. **Isolate Network**: Ensure VMs can't reach internet
8. **Version Control**: Track mutation configurations
9. **Document Results**: Keep analysis notes
10. **Test Incrementally**: Validate each component
