# AutoMutate++ Automation Package - Quick Start Index

**New to this package? Start here!**

## 📖 Documentation Roadmap

Choose your path based on your role and needs:

### 🚀 I want to get started quickly
→ **Start here**: [QUICK_REFERENCE.md](QUICK_REFERENCE.md)
- Command cheat sheet
- Common workflows
- Emergency recovery

### 📚 I need complete setup instructions
→ **Start here**: [README.md](README.md)
- Full setup guide (580+ lines)
- Step-by-step manual installation
- Troubleshooting section (9 common issues)

### 🔍 I want to understand what's in this package
→ **Start here**: [MANIFEST.md](MANIFEST.md)
- Complete file inventory
- Script dependencies
- Usage patterns

### ✅ I want to verify completion status
→ **Start here**: [COMPLETION_SUMMARY.md](COMPLETION_SUMMARY.md)
- What was built
- Performance benchmarks
- Known limitations

## 🎯 Quick Start (3 Steps)

```powershell
# 1. Edit configuration
notepad automation\config.yaml

# 2. Run automated setup (as Administrator)
cd automation
.\setup-all.ps1

# 3. Validate installation
.\scripts\validate-environment.ps1
```

**Duration**: 30-60 minutes (includes downloads + Windows installation)

## 📁 File Structure

```
automation/
├── README.md                    ← Full setup guide
├── QUICK_REFERENCE.md           ← Command cheat sheet
├── MANIFEST.md                  ← Package inventory
├── COMPLETION_SUMMARY.md        ← Technical overview
├── INDEX.md                     ← This file
├── config.yaml                  ← Master configuration
├── setup-all.ps1                ← One-command setup
│
├── scripts/                     ← Automation scripts
│   ├── 01-host-setup.ps1       ← Hyper-V + WSL2 + network
│   ├── 02-wsl-bootstrap.sh     ← Controller bootstrap
│   ├── 03-create-worker-vm.ps1 ← VM creation
│   ├── 04-vm-init.ps1          ← Worker initialization
│   ├── 05-create-baseline.ps1  ← Snapshot creation
│   ├── start-environment.ps1   ← Start services
│   ├── stop-environment.ps1    ← Stop services
│   ├── revert-worker.ps1       ← Fast restore (~10 sec)
│   ├── validate-environment.ps1← Health checks
│   ├── update-baseline.ps1     ← Update snapshot
│   ├── backup-checkpoints.ps1  ← Export snapshots
│   ├── restore-checkpoints.ps1 ← Import snapshots
│   └── generate-certs.ps1      ← mTLS certificates
│
└── templates/                   ← Configuration templates
    ├── controller.toml         ← Controller config
    ├── worker.toml             ← Worker config
    ├── docker-compose.yml      ← Elasticsearch + Kibana
    └── autounattend.xml        ← Unattended Windows install
```

## 🎓 Learning Path

### Beginner (First-Time User)
1. Read [README.md](README.md) - Architecture Overview section
2. Follow [README.md](README.md) - Quick Start section
3. Bookmark [QUICK_REFERENCE.md](QUICK_REFERENCE.md) for daily use

### Intermediate (Customization)
1. Review [MANIFEST.md](MANIFEST.md) - Configuration Reference
2. Edit `config.yaml` for your environment
3. Read [templates/controller.toml](templates/controller.toml) for options
4. Read [templates/worker.toml](templates/worker.toml) for options

### Advanced (Troubleshooting)
1. Review [README.md](README.md) - Troubleshooting section
2. Read [COMPLETION_SUMMARY.md](COMPLETION_SUMMARY.md) - Known Limitations
3. Check script inline comments for customization points
4. Review logs in `automation\logs\`

## 📋 Common Tasks

### Initial Setup
```powershell
.\setup-all.ps1
```
→ See [README.md](README.md#quick-start) for details

### Daily Operations
```powershell
.\scripts\start-environment.ps1        # Start
.\scripts\revert-worker.ps1 -WorkerName "win-worker-01"  # Revert
.\scripts\stop-environment.ps1         # Stop
```
→ See [QUICK_REFERENCE.md](QUICK_REFERENCE.md#daily-operations) for more

### Health Checks
```powershell
.\scripts\validate-environment.ps1
```
→ See [README.md](README.md#troubleshooting) if checks fail

### Backup & Restore
```powershell
.\scripts\backup-checkpoints.ps1 -BackupPath "D:\Backups"
.\scripts\restore-checkpoints.ps1 -BackupPath "D:\Backups\automutate-backup-20250114-153000"
```
→ See [QUICK_REFERENCE.md](QUICK_REFERENCE.md#backup--restore) for details

## 🔧 Prerequisites

### Required
- ✅ Windows 10/11 Pro or Enterprise (Hyper-V support)
- ✅ 16GB RAM minimum (32GB recommended)
- ✅ 200GB free disk space (500GB recommended)
- ✅ Administrator privileges

### Downloads Needed
- 📥 Windows 10 ISO: https://www.microsoft.com/software-download/windows10
- 📥 Windows 11 ISO: https://www.microsoft.com/software-download/windows11

## 🛠️ Script Reference

| Script | Purpose | Admin Required |
|--------|---------|----------------|
| `setup-all.ps1` | Complete automated setup | ✅ YES |
| `validate-environment.ps1` | Health checks (8 sections) | ❌ NO |
| `start-environment.ps1` | Start all services | ✅ YES |
| `stop-environment.ps1` | Stop all services | ✅ YES |
| `revert-worker.ps1` | Fast snapshot restore (~10 sec) | ✅ YES |

→ Full script reference: [MANIFEST.md](MANIFEST.md#script-dependencies)

## 📊 What Gets Created

- **WSL2 Ubuntu**: Controller runtime (Elasticsearch, Kibana, Rust binaries)
- **IsolationSwitch**: Internal Hyper-V network (192.168.200.0/24)
- **Worker VMs**: Windows 10/11 with TPM + Secure Boot
- **Baseline Snapshots**: Golden images for fast revert
- **Configuration Files**: controller.toml, worker.toml
- **Logs**: Setup and runtime logs

→ Architecture diagram: [README.md](README.md#architecture-overview)

## 🔒 Security Features

- ✅ Network isolation (internal switch only)
- ✅ TPM 2.0 for Windows 11 workers
- ✅ Secure Boot (MS UEFI CA template)
- ✅ Minimal firewall rules (only required ports)
- ✅ Optional mTLS certificates (generate-certs.ps1)

→ Security checklist: [README.md](README.md#security-hardening)

## ⚡ Performance Expectations

| Operation | Duration |
|-----------|----------|
| Initial setup (complete) | 30-60 min |
| Start environment | 1-2 min |
| Stop environment | 30-60 sec |
| **Revert worker** | **~10 sec** ⚡ |

→ Full benchmarks: [COMPLETION_SUMMARY.md](COMPLETION_SUMMARY.md#performance-characteristics)

## 🆘 Help & Support

### Troubleshooting Steps
1. Run health check: `.\scripts\validate-environment.ps1`
2. Check logs: `Get-Content automation\logs\setup-*.log`
3. Review common issues: [README.md](README.md#troubleshooting)
4. Search documentation: Use Ctrl+F in README.md

### Common Issues
- **Hyper-V won't enable** → [README.md](README.md#issue-hyper-v-wont-enable)
- **WSL2 won't start** → [README.md](README.md#issue-wsl2-ubuntu-wont-start)
- **Worker can't reach Controller** → [README.md](README.md#issue-worker-cant-reach-controller)
- **Elasticsearch won't start** → [README.md](README.md#issue-elasticsearch-wont-start)

## 🔗 Integration with Project

This automation package integrates with:

- **controller/** - Rust modules (scheduler, selector, mutator, triage-engine)
- **worker/** - Rust modules (agent, harness)
- **telemetry/** - Collector + RedEDR integration
- **CLAUDE.md** - Project instructions + telemetry strategy
- **REDEDR_FUZZER_INTEGRATION.md** - RedEDR integration guide

→ Integration details: [COMPLETION_SUMMARY.md](COMPLETION_SUMMARY.md#integration-with-project)

## 📝 Next Steps After Setup

1. ✅ Run `validate-environment.ps1` (verify all green ✓)
2. 📦 Build Worker binaries: `cargo build --release -p worker-agent`
3. 📂 Copy binaries to workers: `Copy-VMFile ...`
4. 🔧 Deploy RedEDR to workers (see REDEDR_FUZZER_INTEGRATION.md)
5. 🧪 Run first experiment and validate telemetry flow
6. 📊 Create Kibana dashboards for visualization

## 🎉 Quick Wins

After setup, you can:

- ✅ Start/stop entire environment in 1-2 minutes
- ✅ Revert worker to baseline in ~10 seconds (fast experimentation)
- ✅ Access Kibana dashboards at http://localhost:5601
- ✅ Query Elasticsearch directly at http://localhost:9200
- ✅ Backup all snapshots with one command
- ✅ Add new workers without disrupting existing ones

## 📚 Documentation Map

```
For setup instructions    → README.md (580 lines)
For daily commands        → QUICK_REFERENCE.md (cheat sheet)
For package contents      → MANIFEST.md (inventory)
For technical details     → COMPLETION_SUMMARY.md (overview)
For navigation           → INDEX.md (this file)
```

## 🚦 Status Indicators

Throughout the documentation, look for:

- ✅ **Green checkmark**: Feature implemented
- ⚠️ **Yellow warning**: Optional/manual step required
- ❌ **Red X**: Not implemented/not supported
- ⚡ **Lightning**: Performance highlight
- 📥 **Download**: External resource needed

---

**Need help?** Start with [README.md](README.md) or [QUICK_REFERENCE.md](QUICK_REFERENCE.md)

**Ready to begin?** Run `.\setup-all.ps1` (as Administrator)

**Want details?** See [MANIFEST.md](MANIFEST.md) or [COMPLETION_SUMMARY.md](COMPLETION_SUMMARY.md)
