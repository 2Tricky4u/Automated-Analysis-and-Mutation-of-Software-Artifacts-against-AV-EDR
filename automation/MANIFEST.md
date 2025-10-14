# AutoMutate++ Automation Package Manifest

Complete infrastructure automation for Windows EDR testing environment.

## Package Contents

### Configuration Files

| File | Purpose | Edit Required |
|------|---------|---------------|
| `config.yaml` | Master configuration (workers, network, services) | ✅ YES |
| `templates/controller.toml` | Controller service configuration template | Optional |
| `templates/worker.toml` | Worker agent configuration template | Optional |
| `templates/docker-compose.yml` | Elasticsearch + Kibana container config | Optional |
| `templates/autounattend.xml` | Unattended Windows installation | Optional |

### Setup Scripts (One-Time)

| Script | Purpose | Duration | Requires Admin |
|--------|---------|----------|----------------|
| `setup-all.ps1` | **Master orchestrator** - runs all setup steps | 30-60 min | ✅ YES |
| `scripts/01-host-setup.ps1` | Enable Hyper-V, WSL2, network isolation | 5-10 min | ✅ YES |
| `scripts/02-wsl-bootstrap.sh` | Install Rust, Elasticsearch, build Controller | 15-20 min | ❌ NO |
| `scripts/03-create-worker-vm.ps1` | Create Gen2 VM with TPM + Secure Boot | 2-5 min | ✅ YES |
| `scripts/04-vm-init.ps1` | Configure worker (inside VM or PowerShell Direct) | 20-30 min | ✅ YES |
| `scripts/05-create-baseline.ps1` | Create golden image snapshot | 1-2 min | ✅ YES |

### Operational Scripts (Daily Use)

| Script | Purpose | Duration | Requires Admin |
|--------|---------|----------|----------------|
| `scripts/start-environment.ps1` | Start all services (WSL2, ES, Kibana, Workers) | 1-2 min | ✅ YES |
| `scripts/stop-environment.ps1` | Stop all services gracefully | 30-60 sec | ✅ YES |
| `scripts/revert-worker.ps1` | Fast snapshot restore to baseline | ~10 sec | ✅ YES |
| `scripts/validate-environment.ps1` | Comprehensive health checks | 30-60 sec | ❌ NO |
| `scripts/update-baseline.ps1` | Update golden image after modifications | 2-5 min | ✅ YES |

### Maintenance Scripts

| Script | Purpose | Duration | Requires Admin |
|--------|---------|----------|----------------|
| `scripts/backup-checkpoints.ps1` | Export all worker snapshots to external storage | 10-30 min | ✅ YES |
| `scripts/restore-checkpoints.ps1` | Import workers from backup archive | 10-30 min | ✅ YES |
| `scripts/generate-certs.ps1` | Generate mTLS certificates for secure communication | 10-30 sec | ❌ NO |

### Documentation

| File | Purpose |
|------|---------|
| `README.md` | Complete setup guide (500+ lines) |
| `QUICK_REFERENCE.md` | Command cheat sheet for daily operations |
| `MANIFEST.md` | This file - package inventory |

## File Tree

```
automation/
├── config.yaml                        # Master configuration
├── setup-all.ps1                      # One-command setup
├── README.md                          # Full documentation
├── QUICK_REFERENCE.md                 # Command reference
├── MANIFEST.md                        # This file
│
├── scripts/                           # PowerShell & Bash scripts
│   ├── 01-host-setup.ps1             # Hyper-V + WSL2 + network
│   ├── 02-wsl-bootstrap.sh           # Controller bootstrap
│   ├── 03-create-worker-vm.ps1       # VM creation
│   ├── 04-vm-init.ps1                # Worker initialization
│   ├── 05-create-baseline.ps1        # Snapshot creation
│   ├── start-environment.ps1         # Start all services
│   ├── stop-environment.ps1          # Stop all services
│   ├── revert-worker.ps1             # Fast snapshot restore
│   ├── validate-environment.ps1      # Health checks
│   ├── update-baseline.ps1           # Update snapshot
│   ├── backup-checkpoints.ps1        # Export snapshots
│   ├── restore-checkpoints.ps1       # Import snapshots
│   └── generate-certs.ps1            # mTLS certificate generation
│
├── templates/                         # Configuration templates
│   ├── controller.toml               # Controller config
│   ├── worker.toml                   # Worker config
│   ├── docker-compose.yml            # Elasticsearch + Kibana
│   └── autounattend.xml              # Windows unattended install
│
├── logs/                              # Execution logs (created at runtime)
│   └── setup-YYYYMMDD-HHMMSS.log
│
└── certs/                             # mTLS certificates (created by generate-certs.ps1)
    ├── ca.crt, ca.key
    ├── controller.crt, controller.key
    └── <worker>.crt, <worker>.key
```

## Script Dependencies

### Dependency Graph

```
setup-all.ps1
├── 01-host-setup.ps1
├── 02-wsl-bootstrap.sh
├── 03-create-worker-vm.ps1 (per worker)
├── 04-vm-init.ps1 (per worker, manual)
└── 05-create-baseline.ps1 (per worker, manual)

Daily Operations:
start-environment.ps1 → (no dependencies)
stop-environment.ps1 → (no dependencies)
revert-worker.ps1 → requires baseline snapshot
validate-environment.ps1 → (no dependencies)

Maintenance:
update-baseline.ps1 → requires existing VM
backup-checkpoints.ps1 → requires existing snapshots
restore-checkpoints.ps1 → requires backup archive
generate-certs.ps1 → requires OpenSSL
```

### External Dependencies

| Dependency | Purpose | Installation |
|------------|---------|--------------|
| **Hyper-V** | Virtual machine hypervisor | Windows feature (auto-enabled) |
| **WSL2** | Linux subsystem for Controller | Windows feature (auto-enabled) |
| **Docker** | Container runtime for Elasticsearch | WSL2 Docker Desktop |
| **Rust** | Compile Controller/Worker binaries | Auto-installed (rustup) |
| **protoc** | Protobuf compiler for gRPC | Auto-installed (25.1) |
| **OpenSSL** | Certificate generation (optional) | Manual: `choco install openssl` |

## Configuration Reference

### config.yaml Structure

```yaml
network:
  switch_name: "IsolationSwitch"       # Internal Hyper-V switch name
  subnet: "192.168.200.0/24"           # Isolated subnet
  host_ip: "192.168.200.1"             # Host gateway IP

controller:
  grpc_port: 50051                     # Controller gRPC port
  elasticsearch_port: 9200             # Elasticsearch API
  kibana_port: 5601                    # Kibana web UI

workers:
  - name: "win-worker-01"              # VM name (must be unique)
    os: "windows11"                    # "windows10" or "windows11"
    ip: "192.168.200.100"              # Static IP in subnet
    cpu_count: 2                       # vCPU cores
    memory_gb: 6                       # RAM (6GB min for Win11)
    disk_gb: 80                        # VHDX size
    iso_path: "C:\\ISOs\\Win11.iso"   # Windows installation media

storage:
  vhd_root: "C:\\Hyper-V\\VMs"        # VHDX storage location
```

## Usage Patterns

### Pattern 1: Initial Setup (Fresh Environment)

```powershell
# 1. Edit config.yaml with worker details
# 2. Run master setup
.\setup-all.ps1

# 3. Install Windows on each worker (manual GUI)
# 4. Initialize each worker
Invoke-Command -VMName "win-worker-01" -FilePath ".\scripts\04-vm-init.ps1" -ArgumentList "192.168.200.100", "win-worker-01"

# 5. Create baselines
.\scripts\05-create-baseline.ps1 -WorkerName "win-worker-01"

# 6. Validate
.\scripts\validate-environment.ps1
```

### Pattern 2: Daily Operations

```powershell
# Morning: Start environment
.\scripts\start-environment.ps1

# Run experiments...

# After each experiment: Revert workers
.\scripts\revert-worker.ps1 -WorkerName "win-worker-01"

# Evening: Stop environment
.\scripts\stop-environment.ps1
```

### Pattern 3: Add New Worker

```powershell
# 1. Edit config.yaml, add worker entry
# 2. Create VM
.\scripts\03-create-worker-vm.ps1 -WorkerName "win-worker-05" -Os "windows11" -IsoPath "C:\ISOs\Win11.iso" -StaticIP "192.168.200.104"

# 3. Install Windows (manual)
# 4. Initialize
Invoke-Command -VMName "win-worker-05" -FilePath ".\scripts\04-vm-init.ps1" -ArgumentList "192.168.200.104", "win-worker-05"

# 5. Copy binaries, create baseline
.\scripts\05-create-baseline.ps1 -WorkerName "win-worker-05"
```

### Pattern 4: Disaster Recovery

```powershell
# Create backup (weekly)
.\scripts\backup-checkpoints.ps1 -BackupPath "D:\Backups\AutoMutate"

# Restore from backup (if needed)
.\scripts\restore-checkpoints.ps1 -BackupPath "D:\Backups\AutoMutate\automutate-backup-20250114-153000"
```

## Script Exit Codes

| Code | Meaning | Action |
|------|---------|--------|
| 0 | Success | Continue |
| 1 | General error | Check logs |
| 3010 | Reboot required | Reboot, re-run script |

## Log Locations

| Component | Log Path |
|-----------|----------|
| Setup | `automation\logs\setup-YYYYMMDD-HHMMSS.log` |
| Controller | `/var/log/automutate/controller.log` (WSL2) |
| Worker | `C:\AutoMutate\logs\worker.log` (VM) |
| Elasticsearch | `docker-compose logs elasticsearch` (WSL2) |
| Kibana | `docker-compose logs kibana` (WSL2) |

## Validation Checklist

After setup, validate with `.\scripts\validate-environment.ps1`:

- [ ] Hyper-V enabled
- [ ] WSL2 Ubuntu installed and running
- [ ] IsolationSwitch created (192.168.200.0/24)
- [ ] Host IP configured (192.168.200.1)
- [ ] Port forwarding configured (50051, 9200, 5601)
- [ ] Firewall rules created
- [ ] Elasticsearch responding (http://localhost:9200)
- [ ] Kibana responding (http://localhost:5601)
- [ ] Worker VMs created
- [ ] Worker VMs have TPM enabled
- [ ] Baseline checkpoints exist
- [ ] Workers can reach Controller (Test-NetConnection)

## Security Considerations

1. **Network Isolation**: Workers on internal switch only (no internet)
2. **No Enhanced Session**: Disabled to prevent clipboard/drive sharing
3. **Baseline Revert**: Always revert after runs (never re-use dirty VM)
4. **mTLS Optional**: Generate certificates for production use
5. **Lab-Only Passwords**: Change default passwords in autounattend.xml
6. **Snapshot Backups**: Weekly backups to external storage

## Performance Benchmarks

| Operation | Duration | Notes |
|-----------|----------|-------|
| Initial setup (complete) | 30-60 min | Includes downloads |
| Host setup | 5-10 min | May require reboot |
| WSL2 bootstrap | 15-20 min | Rust + Elasticsearch |
| Create worker VM | 2-5 min | VHDX creation |
| Worker initialization | 20-30 min | Rust + protoc + VS Build Tools |
| Create baseline | 1-2 min | Snapshot creation |
| Start environment | 1-2 min | All services |
| Stop environment | 30-60 sec | Graceful shutdown |
| **Revert worker** | **~10 sec** | **Fast snapshot restore** |
| Backup checkpoints | 10-30 min | Depends on worker count |
| Restore checkpoints | 10-30 min | Depends on worker count |

## Disk Space Requirements

| Component | Size | Notes |
|-----------|------|-------|
| Windows ISO | 5-6 GB | Per OS version |
| Worker VHDX (Win10) | 20-40 GB | After install + baseline |
| Worker VHDX (Win11) | 25-50 GB | After install + baseline |
| Baseline snapshot | 10-20 GB | Per worker (differential) |
| Elasticsearch data | 10-100 GB | Grows with experiments |
| WSL2 virtual disk | 10-20 GB | Controller + tools |
| **Total (3 workers)** | **200-400 GB** | Recommended: 500 GB free |

## Version History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2025-01-14 | Initial release - complete automation package |

## Support & Troubleshooting

1. **Check logs**: `automation\logs\setup-*.log`
2. **Validate environment**: `.\scripts\validate-environment.ps1`
3. **Review troubleshooting**: See README.md Section 8
4. **Quick reference**: See QUICK_REFERENCE.md

## Integration with Project

This automation package integrates with:

- **controller/**: Rust modules (scheduler, selector, mutator, triage-engine)
- **worker/**: Rust modules (agent, harness)
- **telemetry/**: Collector + RedEDR integration
- **build/**: Emitter (deterministic builds with trace flags)
- **CLAUDE.md**: Project instructions + telemetry strategy
- **REDEDR_FUZZER_INTEGRATION.md**: RedEDR integration guide

## Next Steps

After completing automation setup:

1. Build Worker binaries: `cargo build --release -p worker-agent -p worker-harness`
2. Copy binaries to workers: `Copy-VMFile ...`
3. Deploy RedEDR to workers (see REDEDR_FUZZER_INTEGRATION.md)
4. Configure trace flags in build/emitter (see CLAUDE.md Section 5)
5. Run first experiment and validate telemetry flow
6. Create Kibana dashboards for visualization

---

**Package Created**: 2025-01-14
**Compatibility**: Windows 10 22H2+, Windows 11 23H2+
**License**: Project-specific (see main repository)
**Maintainer**: AutoMutate++ Project
