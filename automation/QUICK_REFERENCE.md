# AutoMutate++ Quick Reference Guide

## Initial Setup (One-Time)

```powershell
# 1. Download Windows ISOs to C:\ISOs\

# 2. Edit config.yaml with worker details

# 3. Run complete setup (as Administrator)
cd automation
.\setup-all.ps1

# 4. Validate installation
.\scripts\validate-environment.ps1
```

## Daily Operations

### Start Environment
```powershell
.\scripts\start-environment.ps1
# Starts: WSL2 → Elasticsearch/Kibana → Controller → Workers
```

### Stop Environment
```powershell
.\scripts\stop-environment.ps1
# Stops: Workers → Controller → Elasticsearch/Kibana
```

### Revert Worker to Baseline
```powershell
.\scripts\revert-worker.ps1 -WorkerName "win-worker-01"
# Fast snapshot restore (~10 seconds)
```

## Worker Management

### Create New Worker
```powershell
.\scripts\03-create-worker-vm.ps1 `
    -WorkerName "win-worker-03" `
    -Os "windows11" `
    -IsoPath "C:\ISOs\Win11_24H2.iso" `
    -StaticIP "192.168.200.102"
```

### Initialize Worker (after Windows install)
```powershell
Invoke-Command -VMName "win-worker-03" `
    -FilePath ".\scripts\04-vm-init.ps1" `
    -ArgumentList "192.168.200.102", "win-worker-03"
```

### Create Baseline Checkpoint
```powershell
.\scripts\05-create-baseline.ps1 -WorkerName "win-worker-03"
```

### Update Baseline (after modifications)
```powershell
.\scripts\update-baseline.ps1 -WorkerName "win-worker-01"
```

## Backup & Restore

### Backup All Checkpoints
```powershell
.\scripts\backup-checkpoints.ps1 -BackupPath "D:\Backups\AutoMutate"
```

### Restore from Backup
```powershell
.\scripts\restore-checkpoints.ps1 -BackupPath "D:\Backups\AutoMutate\automutate-backup-20250114-153000"
```

## Security (Optional)

### Generate mTLS Certificates
```powershell
.\scripts\generate-certs.ps1
# Creates CA + Controller + Worker certificates in automation\certs\
```

## Troubleshooting

### Check Environment Health
```powershell
.\scripts\validate-environment.ps1
# Comprehensive health checks for all components
```

### View Elasticsearch Status
```powershell
# From host
curl http://localhost:9200
curl http://localhost:9200/_cat/indices?v

# From WSL2
wsl -d Ubuntu bash -c "curl http://localhost:9200"
```

### View Docker Services
```bash
# Inside WSL2
cd ~/automutate/automation
docker-compose ps
docker-compose logs elasticsearch
docker-compose logs kibana
```

### Test Worker Connectivity
```powershell
# From host to worker
Test-NetConnection 192.168.200.100 -Port 3389

# From worker to controller (inside worker VM)
Test-NetConnection 192.168.200.1 -Port 50051
```

### Reset WSL2
```powershell
wsl --shutdown
wsl --unregister Ubuntu
wsl --install -d Ubuntu
# Then re-run: .\scripts\02-wsl-bootstrap.sh
```

### Check Port Forwarding
```powershell
netsh interface portproxy show v4tov4
# Should show: 192.168.200.1:50051 → 127.0.0.1:50051
```

### Fix Elasticsearch Memory Lock
```bash
# Inside WSL2
sudo sysctl -w vm.max_map_count=262144
echo "vm.max_map_count=262144" | sudo tee -a /etc/sysctl.conf
```

## Network Reference

| Host | IP | Purpose |
|------|-------|---------|
| Host vEthernet | 192.168.200.1 | Controller gateway |
| win-worker-01 | 192.168.200.100 | Worker VM |
| win-worker-02 | 192.168.200.101 | Worker VM |
| ... | 192.168.200.102-199 | Additional workers |

| Service | Port | Access |
|---------|------|--------|
| Controller gRPC | 50051 | Workers → Controller |
| Elasticsearch | 9200 | All components |
| Kibana | 5601 | Browser (host) |
| Worker Agent | 50052 | Controller → Worker |
| RedEDR API | 8080 | Worker Agent → RedEDR |

## Key File Locations

### Host (Windows)
```
C:\ISOs\                                   # Windows installation ISOs
automation\config.yaml                     # Worker/network configuration
automation\logs\                           # Setup logs
automation\certs\                          # mTLS certificates (if generated)
```

### WSL2 (Controller)
```
~/automutate/config/controller.toml        # Controller configuration
~/automutate/target/release/               # Compiled binaries
~/automutate/automation/                   # Docker Compose files
/var/log/automutate/                       # Controller logs
```

### Worker VMs (Windows)
```
C:\AutoMutate\                             # Worker root
C:\AutoMutate\worker.toml                  # Worker configuration
C:\AutoMutate\worker-agent.exe             # Worker Agent binary
C:\AutoMutate\runs\                        # Experiment runs
C:\AutoMutate\logs\                        # Worker logs
C:\RedEDR\Data\                            # RedEDR telemetry output
```

## Common Command Patterns

### Run Experiment (example workflow)
```powershell
# 1. Ensure environment is running
.\scripts\start-environment.ps1

# 2. Submit artifact via Controller CLI (inside WSL2)
wsl -d Ubuntu bash -c "cd ~/automutate && ./target/release/controller-cli submit --artifact /path/to/artifact.exe --worker win-worker-01"

# 3. Monitor in Kibana
# Open browser: http://localhost:5601

# 4. Revert worker after run
.\scripts\revert-worker.ps1 -WorkerName "win-worker-01"
```

### Add New Worker to Existing Environment
```powershell
# 1. Edit config.yaml, add new worker entry

# 2. Create VM
.\scripts\03-create-worker-vm.ps1 -WorkerName "win-worker-04" -Os "windows11" -IsoPath "C:\ISOs\Win11.iso" -StaticIP "192.168.200.103"

# 3. Install Windows (manual GUI)

# 4. Initialize worker
Invoke-Command -VMName "win-worker-04" -FilePath ".\scripts\04-vm-init.ps1" -ArgumentList "192.168.200.103", "win-worker-04"

# 5. Copy worker binaries (from WSL2 build)
Copy-VMFile -Name "win-worker-04" -SourcePath "\\wsl$\Ubuntu\home\<user>\automutate\target\release\worker-agent.exe" -DestinationPath "C:\AutoMutate\worker-agent.exe" -FileSource Host

# 6. Create baseline
.\scripts\05-create-baseline.ps1 -WorkerName "win-worker-04"

# 7. Validate
.\scripts\validate-environment.ps1
```

### View Logs
```powershell
# Setup logs
Get-Content automation\logs\setup-*.log | Select-Object -Last 50

# Controller logs (WSL2)
wsl -d Ubuntu bash -c "tail -f /var/log/automutate/controller.log"

# Worker logs (inside VM)
Get-Content C:\AutoMutate\logs\worker.log | Select-Object -Last 50

# Docker logs
wsl -d Ubuntu bash -c "cd ~/automutate/automation && docker-compose logs -f"
```

## Performance Tips

1. **SSD Required**: Use NVMe SSD for VHD storage
2. **RAM Allocation**: Give WSL2 at least 8GB (Controller + Elasticsearch)
3. **CPU Cores**: Assign 2 cores per worker VM
4. **Parallel Workers**: Max 4-6 workers on 32GB host
5. **Snapshot Size**: Baseline checkpoints ~20-40GB each
6. **Elasticsearch Heap**: 4GB for production loads (docker-compose.yml)

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 3010 | Reboot required (Windows features) |

## Quick Links

- **Kibana**: http://localhost:5601
- **Elasticsearch**: http://localhost:9200
- **Elasticsearch Indices**: http://localhost:9200/_cat/indices?v
- **Full README**: [README.md](README.md)
- **Project Instructions**: [../CLAUDE.md](../CLAUDE.md)
- **RedEDR Integration**: [../REDEDR_FUZZER_INTEGRATION.md](../REDEDR_FUZZER_INTEGRATION.md)

## Emergency Recovery

### Complete Environment Reset
```powershell
# 1. Stop everything
.\scripts\stop-environment.ps1 -Force -StopWSL

# 2. Remove all workers (careful!)
Get-VM | Where-Object { $_.Name -like "win-worker-*" } | Remove-VM -Force

# 3. Remove WSL2 Ubuntu
wsl --unregister Ubuntu

# 4. Remove network switch
Remove-VMSwitch -Name "IsolationSwitch" -Force

# 5. Re-run setup
.\setup-all.ps1
```

### Quick Worker Reset (without full revert)
```powershell
# Stop worker
Stop-VM -Name "win-worker-01" -TurnOff

# Restore snapshot
Restore-VMSnapshot -VMName "win-worker-01" -Name "win-worker-01-baseline" -Confirm:$false

# Start worker
Start-VM -Name "win-worker-01"
```

---

**Last Updated**: 2025-01-14
**Version**: 1.0
**For**: AutoMutate++ EDR Triage Framework
