# Telemetry System

## Overview

The telemetry system captures Windows ETW (Event Tracing for Windows) events and forwards them to Elasticsearch for analysis and visualization.

## Architecture

```
Windows VM
    └── ETW Consumer (C++)
            ↓ (CSV file)
        Filebeat
            ↓ (HTTP)
        Elasticsearch
            ↓ (Query)
        Kibana
```

## Components

### ETW Consumer (`etw-consumer/`)

C++ application using krabsetw library to capture Windows ETW events.

**Captured Events:**
- Process creation/termination (Microsoft-Windows-Kernel-Process)
- Network connections (Microsoft-Windows-TCPIP)
- File operations
- Registry modifications
- Image (DLL) loading

**Output Format:**
CSV file with columns:
```
timestamp,event_type,process_id,details
```

**Build:**
```bash
cd etw-consumer
mkdir build && cd build
cmake ..
cmake --build . --config Release
```

**Run:**
```bash
# Windows (Administrator required)
.\etw_consumer.exe C:\logs\etw_events.csv

# With custom output
.\etw_consumer.exe Z:\shared\events.csv
```

### Filebeat (`beats-config/`)

Elastic Beat for log shipping and processing.

**Configuration:** `beats-config/filebeat.yml`

**Features:**
- Reads CSV output from ETW consumer
- Parses CSV with dissect processor
- Forwards to Elasticsearch
- Adds metadata (host, docker, cloud)

**Indices:**
- `edr-telemetry-*`: ETW events
- `edr-logs-*`: Application logs

**Testing:**
```bash
# Validate configuration
docker-compose exec filebeat filebeat test config

# Test output
docker-compose exec filebeat filebeat test output
```

## Data Flow

1. **Collection**: ETW Consumer captures Windows events
2. **Storage**: Events written to CSV file
3. **Shipping**: Filebeat reads CSV and parses events
4. **Indexing**: Elasticsearch stores events in indices
5. **Visualization**: Kibana queries and displays data

## Event Schema

### Process Events
```json
{
  "@timestamp": "2024-01-01T12:00:00.000Z",
  "etw": {
    "event_type": "process",
    "process_id": "1234",
    "details": "notepad.exe"
  },
  "fields": {
    "source": "etw",
    "pipeline": "edr-telemetry"
  }
}
```

### Network Events
```json
{
  "@timestamp": "2024-01-01T12:00:00.000Z",
  "etw": {
    "event_type": "network",
    "process_id": "1234",
    "details": "TCP/IP Event"
  }
}
```

## Configuration

### ETW Consumer

Modify `etw_consumer.cpp` to:
- Add more ETW providers
- Change output format
- Filter events
- Add custom processing

**Example Providers:**
```cpp
// DNS queries
krabs::provider<> dns_provider(L"Microsoft-Windows-DNS-Client");

// Registry operations
krabs::provider<> reg_provider(L"Microsoft-Windows-Kernel-Registry");

// File system
krabs::provider<> fs_provider(L"Microsoft-Windows-Kernel-File");
```

### Filebeat

Modify `filebeat.yml` to:
- Change input paths
- Add processors
- Adjust output settings
- Configure filtering

**Custom Processor Example:**
```yaml
processors:
  - drop_event:
      when:
        equals:
          etw.event_type: "ignore"
  - add_fields:
      target: edr
      fields:
        vm_name: "baseline"
        environment: "test"
```

## Troubleshooting

### ETW Consumer Issues

**Problem**: No events captured
- **Solution**: Run as Administrator
- **Check**: `logman query providers` to list available providers

**Problem**: Access denied
- **Solution**: Ensure user has "Performance Log Users" group membership

**Problem**: Events missing
- **Solution**: Check ETW provider keywords and event IDs

### Filebeat Issues

**Problem**: Not reading CSV file
- **Check**: File permissions and path
- **Solution**: Mount shared volume correctly

**Problem**: Events not in Elasticsearch
- **Check**: `docker-compose logs filebeat`
- **Solution**: Verify Elasticsearch connectivity

**Problem**: Parse errors
- **Check**: CSV format matches dissect tokenizer
- **Solution**: Adjust tokenizer in `filebeat.yml`

### Elasticsearch Issues

**Problem**: Index not created
- **Check**: Template settings in `filebeat.yml`
- **Solution**: Manually create index template

**Problem**: Too much data
- **Solution**: Configure ILM (Index Lifecycle Management)

## Performance Tuning

### ETW Consumer

```cpp
// Buffer size (default 64KB)
trace.set_buffer_size(1024);  // 1MB

// Flush timer (default 1 second)
trace.set_flush_timer(500);   // 500ms
```

### Filebeat

```yaml
# Batch size
output.elasticsearch:
  bulk_max_size: 1000
  
# Buffer settings
filebeat.inputs:
  - type: log
    close_inactive: 5m
    scan_frequency: 10s
```

## Security Considerations

1. **ETW Access**: Requires administrator privileges
2. **Data Sensitivity**: ETW data may contain sensitive information
3. **Storage**: CSV files should be on encrypted volumes
4. **Transport**: Use TLS for Filebeat → Elasticsearch
5. **Access Control**: Implement Elasticsearch security

## Advanced Usage

### Real-time Processing

Stream events directly to gRPC service:

```cpp
// In etw_consumer.cpp
void on_event(const EVENT_RECORD& record) {
    // Send via gRPC instead of CSV
    grpc_client->StreamTelemetry(event_data);
}
```

### Event Filtering

Filter events by process:

```cpp
provider.add_on_event_callback([](const EVENT_RECORD& record, ...) {
    auto image = parser.parse<std::wstring>(L"ImageFileName");
    if (image.find(L"malware") != std::wstring::npos) {
        write_event(record);
    }
});
```

### Custom Enrichment

Add process context:

```cpp
#include <windows.h>
#include <psapi.h>

void enrich_event(uint32_t pid) {
    HANDLE hProcess = OpenProcess(PROCESS_QUERY_INFORMATION, FALSE, pid);
    if (hProcess) {
        WCHAR path[MAX_PATH];
        GetModuleFileNameExW(hProcess, NULL, path, MAX_PATH);
        // Add to event data
        CloseHandle(hProcess);
    }
}
```

## Monitoring

### Check ETW Consumer

```powershell
# Check if running
Get-Process etw_consumer

# Check output file
Get-Content C:\logs\etw_events.csv -Tail 10

# Monitor in real-time
Get-Content C:\logs\etw_events.csv -Wait
```

### Check Filebeat

```bash
# View recent events
docker-compose exec filebeat tail -f /var/log/filebeat/filebeat

# Check registry (tracks file position)
docker-compose exec filebeat cat /usr/share/filebeat/data/registry/filebeat/log.json
```

### Query Elasticsearch

```bash
# Count events
curl -X GET "localhost:9200/edr-telemetry-*/_count?pretty"

# Recent events
curl -X GET "localhost:9200/edr-telemetry-*/_search?pretty" -H 'Content-Type: application/json' -d'
{
  "query": { "match_all": {} },
  "sort": [ { "@timestamp": "desc" } ],
  "size": 10
}
'

# Search by event type
curl -X GET "localhost:9200/edr-telemetry-*/_search?pretty" -H 'Content-Type: application/json' -d'
{
  "query": {
    "term": { "etw.event_type.keyword": "process" }
  }
}
'
```

## References

- [krabsetw Documentation](https://github.com/microsoft/krabsetw)
- [ETW Providers Reference](https://docs.microsoft.com/en-us/windows/win32/etw/about-event-tracing)
- [Filebeat Documentation](https://www.elastic.co/guide/en/beats/filebeat/current/index.html)
- [Elasticsearch Reference](https://www.elastic.co/guide/en/elasticsearch/reference/current/index.html)
