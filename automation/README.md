
# AutoMutate++ Environment Automation

Complete infrastructure automation for the AutoMutate++ EDR Triage Framework.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    Windows Host (Hyper-V)                       │
│                                                                 │
│  ┌─────────────────────┐         ┌──────────────────────────┐  │
│  │   WSL2 Ubuntu       │         │  IsolationSwitch         │  │
│  │  (Controller)       │         │  192.168.200.0/24        │  │
│  │                     │         │                          │  │
│  │  • Elasticsearch    │◄────────┤  Host IP: .1             │  │
│  │  • Kibana           │ Port    │  Worker IPs: .100-.199   │  │
│  │  • Controller       │ Proxy   │                          │  │
│  │  • Rust Toolchain   │         │  ┌────────────────────┐  │  │
│  └─────────────────────┘         │  │ win-worker-01      │  │  │
│                                  │  │ (Windows 10/11)    │  │  │
│         gRPC: 50051 ◄────────────┼──┤ • Agent            │  │  │
│         ES:   9200               │  │ • Harness          │  │  │
│         Kibana: 5601             │  │ • RedEDR           │  │  │
│                                  │  │ • Telemetry        │  │  │
│                                  │  └────────────────────┘  │  │
│                                  │                          │  │
│                                  │  ┌────────────────────┐  │  │
│                                  │  │ win-worker-02      │  │  │
│                                  │  │ (Windows 11 only)  │  │  │
│                                  └──┤ • Same config      │  │  │
│                                     └────────────────────┘  │  │
└─────────────────────────────────────────────────────────────────┘
```

## Features

- ✅ **Multi-OS Support**: Windows 10 22H2 and Windows 11 23H2/24H2
- ✅ **TPM + Secure Boot**: Full Gen2 VM with TPM 2.0 (Windows 11 ready)
- ✅ **Network Isolation**: Internal switch with no Internet access by default
- ✅ **Reproducible Builds**: Deterministic Rust toolchains, pinned dependencies
- ✅ **Snapshot Management**: Automatic baseline checkpoints with revert automation
- ✅ **Idempotent Scripts**: Safe to re-run, handles existing resources
- ✅ **Logging & Validation**: Comprehensive error checking with color-coded output
- ✅ **Integration**: Pre-configured for project structure (controller/, worker/, telemetry/)

## Prerequisites

- Windows 10/11 Pro or Enterprise (Hyper-V support)
- 16GB RAM minimum (32GB recommended for multiple workers)
- 100GB free disk space
- Administrator privileges
- Internet connection (host only, for initial downloads)

## Quick Start

### 1. Download Windows ISOs

```powershell
# Windows 10 22H2 (for maximum compatibility)
https://www.microsoft.com/software-download/windows10

# Windows 11 24H2 (latest)
https://www.microsoft.com/software-download/windows11
```

Place ISOs in `C:\ISOs\` (or edit paths in config).

### 2. Configure Environment

Edit `automation\config.yaml`:

```yaml
workers:
  - name: win-worker-01
    os: windows11  # or windows10
    ip: 192.168.200.100
    iso_path: "C:\\ISOs\\Win11_24H2_English_x64.iso"

  - name: win-worker-02
    os: windows11
    ip: 192.168.200.101
    iso_path: "C:\\ISOs\\Win11_24H2_English_x64.iso"
```

### 3. Run Automated Setup

```powershell
# Run as Administrator
cd automation
.\setup-all.ps1

# This will:
# 1. Enable Hyper-V + WSL2
# 2. Create network isolation
# 3. Bootstrap WSL2 controller
# 4. Create worker VMs
# 5. Configure all components
# 6. Create baseline checkpoints
```

### 4. Verify Installation

```powershell
.\validate-environment.ps1

# Expected output:
# ✓ Hyper-V enabled
# ✓ WSL2 Ubuntu installed
# ✓ IsolationSwitch created (192.168.200.0/24)
# ✓ Elasticsearch running (9200)
# ✓ Kibana running (5601)
# ✓ Controller compiled
# ✓ Workers: win-worker-01 (baseline checkpoint exists)
# ✓ Workers: win-worker-02 (baseline checkpoint exists)
```

## Manual Installation (Step-by-Step)

If you prefer manual control or need to troubleshoot:

### Step 1: Host Setup

```powershell
cd automation
.\scripts\01-host-setup.ps1
```

**What it does:**
- Enables Hyper-V, WSL2, VirtualMachinePlatform features
- Creates IsolationSwitch (internal network)
- Configures host IP (192.168.200.1) and firewall
- Sets up port forwarding (gRPC, Elasticsearch, Kibana)
- Installs WSL2 Ubuntu distribution

**Requires reboot:** If prompted, reboot and re-run the script.

### Step 2: WSL2 Bootstrap

```powershell
# From Windows host, run:
wsl -d Ubuntu bash -c "cd /mnt/c/Users/$env:USERNAME/RustroverProjects/Automated-Analysis-and-Mutation-of-Software-Artifacts-against-AV-EDR/automation && ./scripts/02-wsl-bootstrap.sh"
```

**What it does:**
- Installs Rust stable + nightly toolchains
- Installs protoc 25.1 (latest)
- Configures Docker (Elasticsearch + Kibana)
- Builds controller binaries
- Sets up systemd services (optional)

**Duration:** ~10-15 minutes (downloads dependencies)

### Step 3: Create Worker VMs

```powershell
.\scripts\03-create-worker-vm.ps1 -WorkerName "win-worker-01" -Os "windows11" -IsoPath "C:\ISOs\Win11_24H2.iso" -StaticIP "192.168.200.100"
```

**What it does:**
- Creates Gen2 VM with TPM 2.0 + Secure Boot
- Attaches to IsolationSwitch
- Mounts installation ISO
- Configures firmware for Windows 11 compatibility

**Repeat for each worker** (win-worker-02, win-worker-03, etc.)

### Step 4: Install Windows (Manual GUI)

1. Start VM in Hyper-V Manager
2. Install Windows from ISO (use **Windows 11 Pro** for TPM)
3. **During install:**
   - Skip Microsoft account (use local account: `worker-admin`)
   - Disable telemetry, Cortana, location services
   - **Do NOT enable Enhanced Session Mode**
4. Complete setup and reach desktop

### Step 5: Configure Worker VM

```powershell
# Option A: Via PowerShell Direct (no network needed)
Invoke-Command -VMName "win-worker-01" -FilePath ".\scripts\04-vm-init.ps1" -ArgumentList "192.168.200.100", "win-worker-01"

# Option B: Copy script into VM and run manually
# (Open VM console, copy via clipboard, run as Admin)
```

**What it does:**
- Sets static IP on IsolationSwitch adapter
- Disables Windows Update auto-restart
- Installs Chocolatey package manager
- Installs Rust + protoc + Visual Studio Build Tools
- Configures Windows Defender exclusions (for testing)
- Prepares for baseline snapshot

**Duration:** ~20 minutes (includes VS Build Tools download)

### Step 6: Build Worker Binaries

```powershell
# Inside WSL2, build Worker Agent + Harness
wsl -d Ubuntu bash -c "cd ~/automutate && cargo build --release -p worker-agent -p worker-harness"

# Copy binaries to VM (via shared network folder or manual copy)
Copy-VMFile -Name "win-worker-01" -SourcePath "\\wsl$\Ubuntu\home\<user>\automutate\target\release\worker-agent.exe" -DestinationPath "C:\AutoMutate\worker-agent.exe" -FileSource Host
```

### Step 7: Create Baseline Checkpoint

```powershell
.\scripts\05-create-baseline.ps1 -WorkerName "win-worker-01"
```

**What it does:**
- Shuts down VM gracefully
- Creates snapshot named `<worker>-baseline`
- Validates snapshot integrity
- Restarts VM

**Critical:** This is your golden image. All experiments will revert to this state.

## Daily Operations

### Start Environment

```powershell
.\scripts\start-environment.ps1

# Starts:
# - WSL2 (if not running)
# - Elasticsearch + Kibana (docker-compose)
# - Controller services
# - All Worker VMs
```

### Stop Environment

```powershell
.\scripts\stop-environment.ps1

# Stops:
# - Worker VMs (graceful shutdown)
# - Controller services
# - Elasticsearch + Kibana
# - WSL2 (optional)
```

### Revert Worker to Baseline

```powershell
.\scripts\revert-worker.ps1 -WorkerName "win-worker-01"

# Fast snapshot restore (~10 seconds)
# Use after each experiment run
```

### Update Worker Configuration

```powershell
# Modify worker, then update checkpoint
.\scripts\update-baseline.ps1 -WorkerName "win-worker-01"

# Captures current VM state as new baseline
# Old baseline renamed to <worker>-baseline-<timestamp>
```

## Network Configuration

### Subnet Layout

```
192.168.200.0/24 (IsolationSwitch - Internal)
├─ 192.168.200.1        Host (vEthernet adapter)
├─ 192.168.200.100-109  Worker VMs (Windows 10)
├─ 192.168.200.110-119  Worker VMs (Windows 11)
└─ 192.168.200.200-254  Reserved (future use)
```

### Port Forwarding (Host → WSL2)

| Service | Host Port | WSL2 Port | Purpose |
|---------|-----------|-----------|---------|
| gRPC Controller | 50051 | 50051 | Worker ↔ Controller |
| Elasticsearch | 9200 | 9200 | Data storage |
| Kibana | 5601 | 5601 | Web UI |
| Triage API | 8080 | 8080 | Triage engine |

**Firewall rules:** Automatically configured in `01-host-setup.ps1`

### Test Connectivity

```powershell
# From host
Test-NetConnection 192.168.200.100 -Port 50052  # Worker gRPC
curl http://localhost:9200                       # Elasticsearch

# From Worker VM (PowerShell)
Test-NetConnection 192.168.200.1 -Port 50051    # Controller gRPC
curl http://192.168.200.1:9200                   # Elasticsearch
```

## Integration with Project Structure

This automation integrates with the existing AutoMutate++ architecture:

```
RustroverProjects/Automated-Analysis-and-Mutation-of-Software-Artifacts-against-AV-EDR/
├── automation/                    # ← NEW: Infrastructure scripts
│   ├── config.yaml               # Worker/network configuration
│   ├── setup-all.ps1             # One-command setup
│   ├── validate-environment.ps1  # Health checks
│   ├── scripts/
│   │   ├── 01-host-setup.ps1
│   │   ├── 02-wsl-bootstrap.sh
│   │   ├── 03-create-worker-vm.ps1
│   │   ├── 04-vm-init.ps1
│   │   ├── 05-create-baseline.ps1
│   │   ├── start-environment.ps1
│   │   ├── stop-environment.ps1
│   │   ├── revert-worker.ps1
│   │   └── update-baseline.ps1
│   └── templates/
│       ├── docker-compose.yml    # Elasticsearch + Kibana
│       ├── controller.service    # Systemd unit (optional)
│       └── autounattend.xml      # Unattended Windows install (optional)
│
├── controller/                    # Existing: Controller modules
├── worker/                        # Existing: Worker agent + harness
├── telemetry/                     # Existing: Collector
├── build/                         # Existing: Emitter
├── config/                        # Existing: TOML configs
└── CLAUDE.md                      # Existing: Project instructions
```

### Configuration Files

**`config/worker.toml`** (auto-generated):
```toml
[worker]
worker_id = "win-worker-01"
controller_address = "192.168.200.1:50051"
heartbeat_interval_secs = 30

[rededr]
api_url = "http://localhost:8080"
data_dir = "C:\\RedEDR\\Data"
timeout_ms = 5000

[harness]
timeout_seconds = 300
max_memory_mb = 2048
```

**`config/controller.toml`** (auto-generated):
```toml
[server]
bind_address = "0.0.0.0:50051"
max_connections = 100

[elasticsearch]
url = "http://localhost:9200"
index_prefix = "etw-"
bulk_size = 100
bulk_timeout_ms = 5000

[triage]
model_path = "./models/classifier.onnx"
confidence_threshold = 0.7
```

## Troubleshooting

### Issue: Hyper-V Won't Enable

**Symptoms:** `Enable-WindowsOptionalFeature` fails
**Solution:**
```powershell
# Check virtualization support
systeminfo | findstr /C:"Hyper-V"

# Enable in BIOS/UEFI:
# - Intel VT-x / AMD-V
# - Intel VT-d / AMD IOMMU
# - Disable Hyper-V in Windows Features, reboot, re-enable
```

### Issue: WSL2 Ubuntu Won't Start

**Symptoms:** `wsl -d Ubuntu` hangs or errors
**Solution:**
```powershell
# Reset WSL
wsl --shutdown
wsl --unregister Ubuntu
wsl --install -d Ubuntu

# Re-run 02-wsl-bootstrap.sh
```

### Issue: Worker Can't Reach Controller

**Symptoms:** `Test-NetConnection 192.168.200.1 -Port 50051` fails
**Solution:**
```powershell
# Check port forwarding
netsh interface portproxy show v4tov4

# Should show:
# Listen on 192.168.200.1:50051 → connect to 127.0.0.1:50051

# Recreate if missing:
netsh interface portproxy add v4tov4 listenaddress=192.168.200.1 listenport=50051 connectaddress=127.0.0.1 connectport=50051

# Check firewall
Get-NetFirewallRule -DisplayName "*Isolation*" | Format-List
```

### Issue: Elasticsearch Won't Start

**Symptoms:** `curl http://localhost:9200` connection refused
**Solution:**
```bash
# Inside WSL2
cd ~/automutate/automation
docker-compose ps

# Check logs
docker-compose logs elasticsearch

# Common fix: Increase vm.max_map_count
sudo sysctl -w vm.max_map_count=262144
echo "vm.max_map_count=262144" | sudo tee -a /etc/sysctl.conf
```

### Issue: TPM Won't Enable

**Symptoms:** `Enable-VMTPM` fails with "Not supported"
**Solution:**
```powershell
# Check host TPM
Get-Tpm

# If no TPM: Windows 11 requires TPM 2.0 hardware or virtual TPM
# Workaround: Use Windows 10 workers, or enable TPM in BIOS

# Enable TPM for VM (if host supports)
Set-VMSecurity -VMName "win-worker-01" -VirtualizationBasedSecurityOptOut $false
Enable-VMTPM -VMName "win-worker-01"
```

### Issue: Secure Boot Fails

**Symptoms:** VM won't boot Windows 11 ISO
**Solution:**
```powershell
# Set correct Secure Boot template
Set-VMFirmware -VMName "win-worker-01" -EnableSecureBoot On -SecureBootTemplate "MicrosoftUEFICertificateAuthority"

# Verify
Get-VMFirmware -VMName "win-worker-01" | Select-Object SecureBoot, SecureBootTemplate
```

## Advanced Configuration

### Unattended Windows Installation

For fully automated installs, use `autounattend.xml`:

```powershell
# Generate autounattend.xml for Worker
.\scripts\generate-autounattend.ps1 -WorkerName "win-worker-01" -AdminPassword "SecureP@ss123"

# Mount autounattend.xml to VM floppy
Set-VMFloppyDiskDrive -VMName "win-worker-01" -Path ".\templates\autounattend.xml"
```

**Security note:** Auto-install with pre-set password is lab-only. Use secure passwords or cert-based auth for production.

### Multi-Worker Parallel Provisioning

```powershell
# Create 5 workers in parallel
1..5 | ForEach-Object -Parallel {
    $name = "win-worker-$('{0:D2}' -f $_)"
    $ip = "192.168.200.$(100 + $_)"
    & ".\scripts\03-create-worker-vm.ps1" -WorkerName $name -Os "windows11" -IsoPath "C:\ISOs\Win11.iso" -StaticIP $ip
} -ThrottleLimit 5
```

### Remote Management (PowerShell Direct)

```powershell
# Execute commands without network
Invoke-Command -VMName "win-worker-01" -ScriptBlock {
    Get-Service worker-agent
    Get-Process harness -ErrorAction SilentlyContinue
}

# Copy files
Copy-VMFile -Name "win-worker-01" -SourcePath ".\artifact.exe" -DestinationPath "C:\Artifacts\test.exe" -FileSource Host
```

## Security Hardening

### Baseline Checklist

- [ ] **Network Isolation**: Workers on IsolationSwitch only (no external NIC)
- [ ] **No Enhanced Session**: Disabled in Hyper-V settings
- [ ] **No Shared Folders**: No host drive mounts in VMs
- [ ] **Minimal Services**: Disable Windows Update, Telemetry, Cortana
- [ ] **Defender Exclusions**: Only for C:\AutoMutate\ (testing only)
- [ ] **mTLS**: Enable for gRPC in production (cert generation script provided)
- [ ] **Snapshot Revert**: Always revert after runs (never re-use dirty VM)

### Generate mTLS Certificates

```powershell
.\scripts\generate-certs.ps1

# Creates:
# - ca.crt (root CA)
# - controller.crt + controller.key (Controller)
# - worker-01.crt + worker-01.key (Worker 01)
# - worker-02.crt + worker-02.key (Worker 02)

# Deploy to VMs and update config files
```

## Performance Tuning

### Recommended VM Resources

| Component | CPU | RAM | Disk | Notes |
|-----------|-----|-----|------|-------|
| WSL2 (Controller) | 4 cores | 8GB | 20GB | Elasticsearch needs 2-4GB |
| Worker (Win10) | 2 cores | 4GB | 64GB | Minimum for RedEDR |
| Worker (Win11) | 2 cores | 6GB | 80GB | TPM overhead |

### Host Requirements

- **Minimum:** 16GB RAM, 4-core CPU, 200GB SSD
- **Recommended:** 32GB RAM, 8-core CPU, 500GB NVMe SSD
- **Optimal:** 64GB RAM, 12+ core CPU, 1TB NVMe SSD

### Elasticsearch Tuning

```yaml
# docker-compose.yml
environment:
  - "ES_JAVA_OPTS=-Xms4g -Xmx4g"  # Use 4GB heap (50% of container RAM)
  - bootstrap.memory_lock=true
```

## Backup and Recovery

### Backup Baseline Checkpoints

```powershell
.\scripts\backup-checkpoints.ps1 -Destination "D:\Backups\AutoMutate"

# Creates compressed archive of all baseline snapshots
# AutoMutate-Baseline-20250114-1530.zip (~10-20GB compressed)
```

### Restore from Backup

```powershell
.\scripts\restore-checkpoints.ps1 -BackupPath "D:\Backups\AutoMutate-Baseline-20250114-1530.zip"
```

## References

- **Project Documentation**: [../CLAUDE.md](../CLAUDE.md)
- **RedEDR Integration**: [../REDEDR_FUZZER_INTEGRATION.md](../REDEDR_FUZZER_INTEGRATION.md)
- **Architecture**: [../ARCHITECTURE.md](../ARCHITECTURE.md)
- **Hyper-V Docs**: https://learn.microsoft.com/virtualization/hyper-v-on-windows/
- **WSL2 Docs**: https://learn.microsoft.com/windows/wsl/
- **Elasticsearch**: https://www.elastic.co/guide/elasticsearch/reference/current/index.html

## Support

For issues:
1. Check [Troubleshooting](#troubleshooting) section
2. Validate environment: `.\validate-environment.ps1`
3. Review logs: `automation\logs\<date>.log`
4. Open GitHub issue with logs attached
