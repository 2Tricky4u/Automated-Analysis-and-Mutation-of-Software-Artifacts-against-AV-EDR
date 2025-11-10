# Quick Test Commands Reference

Copy-paste commands for rapid testing of hybrid telemetry system.

---

## Setup (One-Time)

### Generate Configs
```powershell
cd automation/scripts
.\generate-configs.ps1
copy ..\generated\win10-worker-01.toml C:\AutoMutate\worker.toml
```

### Build
```bash
# Controller (WSL2/Linux)
cd controller/scheduler && cargo build --release

# Worker (Windows VM)
cd worker/agent && cargo build --release
```

---

## Daily Testing

### Start Services

**Terminal 1 - Controller**:
```bash
cd controller/scheduler && cargo run
```

**Terminal 2 - Worker (Windows VM)**:
```powershell
cd worker/agent && cargo run
```

---

## Connectivity Tests

### Ping Controller
```bash
grpcurl -d '{"message":"test"}' -plaintext localhost:50051 edr.controller.Controller/Ping
```

### Ping Worker
```bash
grpcurl -d '{"message":"test"}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/Ping
```

### List Services
```bash
# Controller
grpcurl -plaintext localhost:50051 list

# Worker
grpcurl -plaintext 10.200.200.11:50052 list
```

### Check RedEDR
```powershell
curl http://{worker-ip}:8081/api/stats
```

---

## Execution Tests

### Test 1: Notepad (Timeout)
```bash
grpcurl -d '{
  "job_id": "notepad-test",
  "artifact_path": "C:\\Windows\\System32\\notepad.exe",
  "timeout_seconds": 5,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample
```

### Test 2: Quick Exit (Batch File)
```powershell
# Create artifact first
echo @echo off > C:\quick-exit.bat
echo timeout /t 3 /nobreak >> C:\quick-exit.bat
echo exit 0 >> C:\quick-exit.bat
```

```bash
grpcurl -d '{
  "job_id": "quick-exit",
  "artifact_path": "C:\\quick-exit.bat",
  "timeout_seconds": 30,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample
```

### Test 3: PowerShell Script
```powershell
# Create artifact first
@"
Write-Host 'Testing telemetry'
Get-Process | Select-Object -First 5
Start-Sleep -Seconds 2
"@ > C:\test-script.ps1
```

```bash
grpcurl -d '{
  "job_id": "ps-test",
  "artifact_path": "powershell.exe -File C:\\test-script.ps1",
  "timeout_seconds": 10,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample
```

### Test 4: Stuck Process (Detects Stuck After 9s)
```powershell
# Create artifact first
echo "Start-Sleep -Seconds 600" > C:\stuck.ps1
```

```bash
grpcurl -d '{
  "job_id": "stuck-test",
  "artifact_path": "powershell.exe -File C:\\stuck.ps1",
  "timeout_seconds": 30,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample
```

### Test 5: File Operations (Many Events)
```powershell
# Create artifact first
@"
for ($i=1; $i -le 100; $i++) {
    New-Item -Path "C:\temp\test-$i.txt" -ItemType File -Force | Out-Null
    Set-Content -Path "C:\temp\test-$i.txt" -Value "Test $i"
    Remove-Item -Path "C:\temp\test-$i.txt" -Force
}
"@ > C:\file-ops.ps1
```

```bash
grpcurl -d '{
  "job_id": "file-ops-test",
  "artifact_path": "powershell.exe -File C:\\file-ops.ps1",
  "timeout_seconds": 30,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample
```

---

## Controller Management Tests

### Schedule Job
```bash
grpcurl -d '{
  "name": "Test Job",
  "artifact_type": "exe",
  "source": "manual",
  "mutation_strategies": ["baseline"],
  "priority": 1
}' -plaintext localhost:50051 edr.controller.Controller/ScheduleJob
```

### Get Job Status
```bash
grpcurl -d '{"job_id": "job-000001"}' -plaintext localhost:50051 edr.controller.Controller/GetJobStatus
```

### Submit Triage
```bash
grpcurl -d '{
  "job_id": "job-000001",
  "detected": true,
  "av_product": "Windows Defender",
  "detection_type": "heuristic",
  "iocs": {"hash": "abc123", "behavior": "process_injection"}
}' -plaintext localhost:50051 edr.controller.Controller/SubmitTriage
```

---

## RedEDR Manual Tests

### Check Stats
```powershell
curl http://localhost:8081/api/stats
```

### Get Events
```powershell
curl http://localhost:8081/api/logs/rededr
```

### Start Tracing
```powershell
curl -X POST http://localhost:8081/api/trace/start -H "Content-Type: application/json" -d '{"trace":["notepad.exe"]}'
```

### Reset
```powershell
curl -X POST http://localhost:8081/api/trace/reset
```

---

## Debugging Commands

### Worker Logs (Follow)
```powershell
# In worker directory
$env:RUST_LOG="debug"
cargo run
```

### Controller Logs (Follow)
```bash
# In controller directory
RUST_LOG=debug cargo run
```

### Check Config
```powershell
# Worker
type C:\AutoMutate\worker.toml

# Controller
cat automation/generated/controller.toml
```

### Network Connectivity
```bash
# From controller to worker
ping 10.200.200.11
nc -zv 10.200.200.11 50052

# From worker to controller
ping 10.200.200.1
Test-NetConnection -ComputerName 10.200.200.1 -Port 50051
```

### Process Check
```powershell
# Worker processes
Get-Process | Where-Object {$_.ProcessName -like "*worker*"}

# Check if RedEDR is running
Get-Process | Where-Object {$_.ProcessName -like "*RedEDR*"}
Get-Service RedEDR
```

---

## Expected Output Examples

### Successful Execution (Worker)
```
INFO  Starting sample execution: job_id=test-001, artifact=C:\Windows\System32\notepad.exe
INFO  RedEDR tracing started for artifact: notepad.exe
INFO  Artifact process spawned: pid=5432
INFO  Monitor: Process started: pid=5432
INFO  Monitor: pid=5432, events=15, cpu=0%, mem=0MB, elapsed=3s
WARN  Process timed out after 5s, attempting to kill
INFO  Execution completed in 5.00s
INFO  Collecting telemetry events from RedEDR...
INFO  Collected 23 telemetry events
```

### Successful Execution (Controller)
```
INFO  [WORKER: 10.200.200.11 (win10-worker-01)] [PID: 5432] [JOB: test-001] [RUN: uuid] [STATUS: STARTED] [ARTIFACT: notepad.exe] Process started: pid=5432
INFO  [WORKER: 10.200.200.11 (win10-worker-01)] [PID: 5432] [JOB: test-001] [RUN: uuid] [STATUS: HEARTBEAT] [ARTIFACT: notepad.exe] pid=5432, events=15, cpu=0%, mem=0MB, elapsed=3s
```

### Successful gRPC Response
```json
{
  "job_id": "test-001",
  "success": false,
  "exit_code": -1,
  "output": "Execution timed out after 5s",
  "telemetry_ids": [
    "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
  ]
}
```

### Stuck Detection (Controller)
```
WARN  [WORKER: 10.200.200.11 (win10-worker-01)] [PID: 5432] [JOB: stuck-test] [RUN: uuid] [STATUS: STUCK] [ARTIFACT: powershell.exe] pid=5432, events=12, cpu=0%, mem=0MB, elapsed=15s [STUCK?]
```

---

## Error Scenarios

### Invalid Artifact Path
```bash
grpcurl -d '{
  "job_id": "error-test",
  "artifact_path": "C:\\does-not-exist.exe",
  "timeout_seconds": 5,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample
```

**Expected**:
```
ERROR:
  Code: Internal
  Message: Failed to spawn process: The system cannot find the file specified. (os error 2)
```

### RedEDR Offline
```powershell
# Stop RedEDR first
Stop-Process -Name RedEDR -Force
```

```bash
grpcurl -d '{
  "job_id": "rededr-offline",
  "artifact_path": "C:\\Windows\\System32\\notepad.exe",
  "timeout_seconds": 5,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample
```

**Expected**:
```
ERROR:
  Code: Internal
  Message: Failed to start RedEDR tracing: ...
```

---

## Health Checks

### Worker Health
```bash
grpcurl -d '{}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/HealthCheck
```

**Expected**:
```json
{
  "worker_id": "win10-worker-01",
  "healthy": true,
  "cpu_percent": 25,
  "memory_percent": 40,
  "active_jobs": 0
}
```

---

## One-Liner Smoke Test

Run everything in sequence to verify basic functionality:

```bash
# Test connectivity
grpcurl -plaintext localhost:50051 edr.controller.Controller/Ping -d '{"message":"test"}' && \
grpcurl -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/Ping -d '{"message":"test"}' && \

# Run sample
grpcurl -d '{"job_id":"smoke-test","artifact_path":"C:\\Windows\\System32\\notepad.exe","timeout_seconds":5,"enable_etw":true}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample && \

# Check health
grpcurl -d '{}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/HealthCheck

# If all pass: ✅ System is working!
```

---

## Performance Benchmarks

### Measure Execution Time
```bash
time grpcurl -d '{
  "job_id": "perf-test",
  "artifact_path": "powershell.exe -Command Start-Sleep -Seconds 10",
  "timeout_seconds": 15,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample

# Expected: ~10-11 seconds (artifact execution + overhead)
```

### Count Status Updates
```bash
# Run 30s execution
grpcurl -d '{
  "job_id": "status-count",
  "artifact_path": "powershell.exe -Command Start-Sleep -Seconds 30",
  "timeout_seconds": 35,
  "enable_etw": true
}' -plaintext 10.200.200.11:50052 edr.worker.WorkerAgent/RunSample

# On controller terminal, count STATUS messages:
# Expected: ~11 updates (1 STARTED + 9 HEARTBEAT + 1 COMPLETED)
```

---

**Quick Tip**: Save this file to your desktop for instant access during testing!
