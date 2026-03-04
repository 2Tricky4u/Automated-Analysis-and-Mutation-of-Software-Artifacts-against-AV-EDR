# Automation Scripts

The `automation/` folder contains PowerShell and Bash scripts that automate the full infrastructure setup for AutoMutate++. The scripts support two deployment modes:

- **Local Hyper-V** — Create and configure Gen2 VMs on a single Windows 11 host with an internal network switch, WSL2 controller, and Elasticsearch/Kibana in Docker. This is the default lab setup.
- **Remote VM** — Deploy the worker agent to any reachable Windows machine (cloud VMs, physical hosts) via SSH or WMI/RPC. Workers self-register with the controller at startup.

Both modes share the same `config.yaml` master configuration and TOML config generation pipeline.

---

## Folder Structure

```
automation/
├── config.yaml                     # Master configuration (network, workers, services)
├── docker-compose.yml              # Elasticsearch + Kibana containers
├── setup-all.ps1                   # Master orchestrator (runs everything in order)
│
├── templates/                      # Configuration & install templates
│   ├── controller.toml             # Controller TOML with {{TOKEN}} placeholders
│   ├── worker.toml                 # Worker TOML with {{TOKEN}} placeholders
│   ├── docker-compose.yml          # Docker Compose template for ES + Kibana
│   ├── autounattend.xml            # Windows unattended install answer file
│   ├── autounattend-manual.xml     # Manual variant
│   └── autounattend.vfd            # Floppy disk image for OOBE
│
├── scripts/                        # All operational scripts
│   ├── 01-host-setup.ps1           # Enable Hyper-V, WSL2, networking
│   ├── 02-wsl-bootstrap.sh         # Install Rust, Docker, ES/Kibana in WSL2
│   ├── 03-create-worker-vm.ps1     # Create Hyper-V Gen2 VMs
│   ├── 04-vm-init.ps1              # Inside-VM initialization (10-step)
│   ├── 05-create-baseline.ps1      # Create Hyper-V checkpoint snapshots
│   ├── initialize-worker.ps1       # Bridge: copy build package → run 04-vm-init
│   ├── generate-configs.ps1        # YAML → TOML config generator
│   ├── start-environment.ps1       # Start all services + VMs
│   ├── stop-environment.ps1        # Stop all services + VMs
│   ├── validate-environment.ps1    # Health check suite (12 checks)
│   ├── start-rededr-system.ps1     # Launch RedEDR as SYSTEM scheduled task
│   ├── toggle-vm-internet.ps1      # NAT kill switch (air-gap mode)
│   ├── manage-egress-filter.ps1    # Firewall whitelist management
│   ├── toggle-vm-isolation.ps1     # VM-to-VM isolation
│   ├── wsl-keepalive-daemon.ps1    # Prevent WSL2 auto-shutdown
│   ├── build-modular.sh            # gRPC CLI for modular artifact builds
│   ├── modules/
│   │   └── AutoMutateConfig.psm1   # Shared config parsing module
│   └── workers/
│       ├── deploy-remote-worker.ps1 # Deploy agent to remote VM (SSH/WMI)
│       └── list-workers.ps1         # Query registered workers via gRPC
│
└── generated/                      # Output of generate-configs.ps1
    ├── controller.toml
    ├── win10-worker-1.toml
    └── win10-worker-2.toml
```

---

## Setup Flow

The full local setup is orchestrated by `setup-all.ps1`, which calls the numbered scripts in order:

```
setup-all.ps1 (run as Administrator)
│
├─ generate-configs.ps1          config.yaml → generated/*.toml
│
├─ 01-host-setup.ps1             Enable Hyper-V, WSL2, VirtualMachinePlatform
│   ├─ Create IsolationSwitch (internal, 10.200.200.0/24)
│   ├─ Configure host IP (10.200.200.1)
│   ├─ Enable IP forwarding (WSL2 ↔ VM routing)
│   ├─ Create NAT for VM internet access
│   ├─ Configure firewall (gRPC 50051, ES 9200, Kibana 5601)
│   └─ Optional: VM isolation, egress filtering
│
├─ 02-wsl-bootstrap.sh           (runs inside WSL2 Ubuntu)
│   ├─ Install Docker Engine + Compose
│   ├─ Install Rust (stable + nightly + llvm-tools)
│   ├─ Install protoc 25.1 + grpcurl 1.8.6
│   ├─ Generate docker-compose.yml from config
│   ├─ Start Elasticsearch 8.11.0 + Kibana 8.11.0
│   └─ Create systemd service (auto-start on WSL boot)
│
├─ 03-create-worker-vm.ps1       (per worker from config.yaml)
│   ├─ Download Windows ISO from Google Drive (if needed)
│   ├─ SHA256 hash verification
│   ├─ Create VHDX (dynamic, 64-80 GB)
│   ├─ Create Gen2 VM (UEFI)
│   ├─ Enable TPM 2.0
│   ├─ Configure Secure Boot (Off for Win10, On for Win11)
│   └─ Attach ISO, set DVD boot
│
├─ [MANUAL] Install Windows on each VM
│
├─ initialize-worker.ps1         (per worker, uses PowerShell Direct)
│   ├─ Disable Secure Boot (requires VM off)
│   ├─ Package build files + RedEDR zip
│   ├─ Copy to VM via Hyper-V integration
│   └─ Execute 04-vm-init.ps1 inside VM
│
├─ 04-vm-init.ps1                (runs inside worker VM, 10 steps)
│   ├─ [0] Disable Defender for automation dirs
│   ├─ [1] System config (hostname, UTC, UAC off, execution policy)
│   ├─ [2] Static IP + DNS (8.8.8.8 primary, gateway 10.200.200.1)
│   ├─ [3] Disable Windows telemetry + Cortana
│   ├─ [4] Keep Defender enabled (baseline)
│   ├─ [5] Install Chocolatey, Rust, protoc, VC++ runtime, VS 2022 Build Tools
│   ├─ [6] Create C:\AutoMutate\{artifacts,logs,harness}
│   ├─ [7] Extract RedEDR, trust ELAM driver certificate, configure firewall
│   ├─ [8] Enable testsigning + kernel debug (bcdedit)
│   ├─ [9] Enable ALL audit policies for maximum ETW telemetry
│   └─ [10] Install RedEDR drivers + register PPL service
│
├─ 05-create-baseline.ps1        (per worker)
│   ├─ Stop VM gracefully
│   ├─ Create Hyper-V checkpoint (clean snapshot)
│   └─ Start VM
│
└─ [READY] start-environment.ps1
```

---

## Configuration

### `config.yaml` (Master)

Single source of truth for the entire infrastructure:

```yaml
network:
  switch_name: IsolationSwitch
  subnet: 10.200.200.0/24
  host_ip: 10.200.200.1

controller:
  grpc_port: 50051
  elasticsearch_port: 9200
  kibana_port: 5601

workers:
  windows10:
    count: 2
    name_prefix: win10-worker
    ip_start: 10.200.200.100
    cpu_count: 2
    memory_gb: 4
    disk_gb: 64

scheduler:
  max_concurrent_runs_per_worker: 1
  default_timeout_seconds: 300
```

### Config Generation Pipeline

```
config.yaml + templates/*.toml → generate-configs.ps1 → generated/*.toml
```

`generate-configs.ps1` expands `{{TOKEN}}` placeholders in templates with values from `config.yaml`. Generates one `controller.toml` and one `<hostname>.toml` per worker.

### `AutoMutateConfig.psm1` (Shared Module)

PowerShell module imported by all scripts. Provides:

| Function | Purpose |
|----------|---------|
| `Read-AutoMutateConfig` | Parse `config.yaml` → hashtable |
| `Get-AutoMutateWorkers` | Generate worker list from templates (name, OS, IP, CPU, RAM, disk) |
| `Get-AutoMutateWorkerByName` | Lookup single worker by name |
| `Get-AutoMutateWorkerNames` | Get worker names only |
| `Get-IncrementedIP` | Calculate worker IP from base + offset |

---

## Script Reference

### Host & Controller Setup

| Script | Runs On | Purpose |
|--------|---------|---------|
| `01-host-setup.ps1` | Windows host | Enable Hyper-V + WSL2, create network switch, NAT, firewall rules, port forwarding |
| `02-wsl-bootstrap.sh` | WSL2 Ubuntu | Install Rust + Docker + protoc + grpcurl, start ES/Kibana, create systemd service |

### VM Provisioning

| Script | Runs On | Purpose |
|--------|---------|---------|
| `03-create-worker-vm.ps1` | Windows host | Create Hyper-V Gen2 VM with TPM, VHDX, ISO. Downloads Win ISOs from Google Drive |
| `initialize-worker.ps1` | Windows host | Bridge script: packages build files, copies to VM via PowerShell Direct, runs 04-vm-init |
| `04-vm-init.ps1` | Inside VM | 10-step initialization: networking, dev tools (Rust, VS2022, protoc), RedEDR, audit policies, driver certs |
| `05-create-baseline.ps1` | Windows host | Create clean Hyper-V checkpoint for fast revert between mutation rounds |

### Remote Deployment

| Script | Runs On | Purpose |
|--------|---------|---------|
| `deploy-remote-worker.ps1` | Windows host | Deploy worker agent to any remote Windows VM via SSH (primary) or WMI/RPC (fallback). Handles SSH key generation, binary build, config generation, `worker-init.ps1` execution on remote |
| `list-workers.ps1` | Any | Query controller via gRPC `ListWorkers`, display in table/JSON/CSV. Filter by status or capability |

### Environment Management

| Script | Runs On | Purpose |
|--------|---------|---------|
| `start-environment.ps1` | Windows host | Start WSL2 + Docker + ES/Kibana + all worker VMs |
| `stop-environment.ps1` | Windows host | Stop all worker VMs + Docker containers + WSL keepalive |
| `validate-environment.ps1` | Windows host | 12-point health check: Windows features, WSL, network switch, ES, Kibana, VMs, storage, firewall |
| `wsl-keepalive-daemon.ps1` | Windows host | Prevent WSL2 auto-shutdown (Windows terminates idle WSL instances) |

### Security & Isolation

| Script | Runs On | Purpose |
|--------|---------|---------|
| `toggle-vm-internet.ps1` | Windows host | Enable/disable NAT (air-gap mode). VMs keep host access but lose internet |
| `manage-egress-filter.ps1` | Windows host | Default-deny firewall + whitelist (DNS 53, HTTP 80, HTTPS 443, NTP 123). Custom rules via `-AddRule` |
| `toggle-vm-isolation.ps1` | Windows host | Block VM-to-VM traffic. Allow only VM ↔ host (10.200.200.1) |

### Build & Telemetry

| Script | Runs On | Purpose |
|--------|---------|---------|
| `build-modular.sh` | WSL2 | gRPC CLI for `BuildArtifact` RPC. Select modules (carrier, decoder, anti-emulation, etc.), specify payload + encoding |
| `start-rededr-system.ps1` | Inside VM | Start RedEDR as `NT AUTHORITY\SYSTEM` via scheduled task. Modes: `etw` (ETW + ETW-TI) or `hooking` (kernel + APC injection). Auto-restart on failure |

---

## Network Topology

### Local Mode (Hyper-V)

All components on a single Windows 11 host. VMs communicate through an internal Hyper-V switch. The host controls all network access (NAT, egress filtering, VM isolation).

```
┌──────────────────────────────────────────────────────────────┐
│  Windows 11 Host                                             │
│                                                              │
│  ┌─────────────────────┐    ┌──────────────────────────────┐ │
│  │  WSL2 Ubuntu        │    │  IsolationSwitch (internal)  │ │
│  │                     │    │  10.200.200.0/24             │ │
│  │  Controller gRPC    │    │                              │ │
│  │  :50051             │    │  ┌──────────┐ ┌──────────┐  │ │
│  │                     │    │  │ Win10 VM │ │ Win10 VM │  │ │
│  │  Elasticsearch      │    │  │ .100     │ │ .101     │  │ │
│  │  :9200              │    │  │ Agent    │ │ Agent    │  │ │
│  │                     │    │  │ RedEDR   │ │ RedEDR   │  │ │
│  │  Kibana             │    │  └──────────┘ └──────────┘  │ │
│  │  :5601              │    │                              │ │
│  └─────────────────────┘    │  Host: 10.200.200.1         │ │
│                              │  NAT → internet (optional)  │ │
│  Port forwarding:            └──────────────────────────────┘ │
│  host:9200 → WSL:9200                                        │
│  host:5601 → WSL:5601                                        │
│  IP forwarding: WSL ↔ VMs                                    │
│                                                              │
│  Security controls (host-enforced):                          │
│  ├─ toggle-vm-internet.ps1   NAT kill switch                 │
│  ├─ manage-egress-filter.ps1 default-deny + whitelist        │
│  └─ toggle-vm-isolation.ps1  block VM-to-VM traffic          │
└──────────────────────────────────────────────────────────────┘
```

### Remote Mode (VPN / External VMs)

Controller stays on the local host (WSL2). Workers run on remote machines (Azure, AWS, bare metal, remote Hyper-V hosts) reachable over a VPN or routable network. The controller **dials workers** (Phase 1 architecture) — workers listen on `:50052`, the controller connects to them.

```
┌─────────────────────────────────────┐
│  Windows 11 Host (controller)       │
│                                     │
│  ┌───────────────────────────┐      │
│  │  WSL2 Ubuntu              │      │
│  │                           │      │
│  │  Controller gRPC :50051   │      │      VPN / Routable Network
│  │  Elasticsearch  :9200     │      │     ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─
│  │  Kibana         :5601     │      │    │
│  └───────────────────────────┘      │
│                                     │    │  Controller dials workers
│  deploy-remote-worker.ps1 ─── SSH ──┼──────────────┐
│                                     │    │          │
│  Controller connects via gRPC ──────┼──────────┐   │
│                                     │    │     │   │
└─────────────────────────────────────┘         │   │
                                           │     │   │
       ┌─────────────────────────────────────────┼───┼──────────────────┐
       │                                   │     │   │                  │
       │  ┌──────────────────────────┐     │  ┌──┼───┼───────────────┐  │
       │  │  Remote Worker 1         │        │  │   │               │  │
       │  │  172.21.107.15           │     │  │  ▼   ▼               │  │
       │  │                          │        │  Agent :50052        │  │
       │  │  Agent listens :50052  ◄─┼─ gRPC  │  ├─ RunSample       │  │
       │  │  RedEDR :8081 (local)    │     │  │  ├─ SendArtifact    │  │
       │  │  Defender (real EDR)     │        │  ├─ GetTelemetry     │  │
       │  │                          │     │  │  └─ HealthCheck     │  │
       │  │  SSH :22 ◄───────────────┼─deploy  │                    │  │
       │  │  SCP: agent + config     │     │  │  RedEDR :8081       │  │
       │  └──────────────────────────┘        │  Defender            │  │
       │                                   │  └─────────────────────┘  │
       │  ┌──────────────────────────┐        ┌─────────────────────┐  │
       │  │  Remote Worker 2         │     │  │  Remote Worker 2    │  │
       │  │  172.21.107.17           │        │  Agent :50052       │  │
       │  │  (same layout)           │     │  │  RedEDR + Defender  │  │
       │  └──────────────────────────┘        └─────────────────────┘  │
       │                                   │                           │
       │         Remote Network                                        │
       └─────────────────────────────────────────────────────────────── ┘
```

**Key differences from local mode:**

| Aspect | Local (Hyper-V) | Remote (VPN) |
|--------|----------------|--------------|
| Connection direction | Controller dials `10.200.200.x:50052` | Controller dials `<vpn_ip>:50052` |
| Deployment | PowerShell Direct (no network) | SSH + SCP over VPN |
| NAT / egress control | Host-enforced via IsolationSwitch | **Not enforced** — remote VM manages its own firewall |
| VM-to-VM isolation | Host firewall rules on switch | **Not enforced** — VMs may be on same remote LAN |
| ES access | Port-proxy `10.200.200.1:9200 → WSL` | Controller pushes to ES locally; workers don't contact ES directly |
| Telemetry path | Controller pulls from `10.200.200.x:50052` | Controller pulls from `<vpn_ip>:50052` (over VPN tunnel) |
| Artifact transfer | gRPC `SendArtifact` over local switch | gRPC `SendArtifact` over VPN (bandwidth-dependent) |
| Security controls | `toggle-vm-internet.ps1`, `manage-egress-filter.ps1`, `toggle-vm-isolation.ps1` | Worker-side only: `block_internet`, `allow_controller_only` in worker.toml (Windows Firewall on remote VM) |
| SSH keys | `automation/ssh-keys/<worker-id>/id_ed25519` | Same — generated by `deploy-remote-worker.ps1` |
| Checkpoint/revert | Hyper-V snapshots (`05-create-baseline.ps1`) | **Not available** — no Hyper-V control over remote VMs |

**Traffic flows (remote mode):**

```
Deployment (one-time):
  Host ── SSH/SCP ──► Remote VM
  Transfers: worker-agent.exe, worker.toml, worker-init.ps1, RedEdr.zip

Runtime (per mutation round):
  Controller ── gRPC ──► Worker:50052  SendArtifact (stream .exe chunks)
  Controller ── gRPC ──► Worker:50052  RunSample (execute + collect)
  Controller ── gRPC ──► Worker:50052  GetTelemetry (pull ETW/logs/coverage)
  Controller ── gRPC ──► Worker:50052  HealthCheck (periodic)

  Controller ── HTTP ──► localhost:9200  Store telemetry in Elasticsearch
  Browser   ── HTTP ──► localhost:5601  Kibana dashboards
```

**Worker security (remote mode):**

The worker.toml `[security]` section is the only enforcement point for remote workers. These settings are applied by the worker agent on the remote VM's own Windows Firewall:

```toml
[security]
block_internet = true          # Block outbound internet (disable for VPN setups
                                # where the worker needs VPN connectivity)
allow_controller_only = true   # Only accept inbound from controller IP
```

> **Note:** For remote workers behind a VPN, you may need `block_internet = false` if the VPN tunnel itself requires internet access. The `allow_controller_only` flag remains useful — it restricts inbound connections to the controller's VPN IP only.

---

## Remote Deployment Flow

For VMs not hosted locally (Azure, AWS, bare-metal):

```
deploy-remote-worker.ps1 -RemoteHost <IP> -Username <user>
│
├─ [1] Build worker-agent.exe (cargo build --release)
├─ [2] Generate minimal worker.toml (auto-increment worker ID)
├─ [3] SSH key generation + installation (ed25519, no passphrase)
├─ [4] SCP: worker-agent.exe → C:\AutoMutate\
├─ [5] SCP: worker.toml → C:\AutoMutate\
├─ [6] SCP: worker-init.ps1 + RedEdr.zip → C:\AutoMutate\
├─ [7] Execute worker-init.ps1 as SYSTEM (via scheduled task)
├─ [8] Create directories + start worker-agent.exe
└─ Worker self-registers with controller on startup
```

No pre-configuration needed on the remote VM beyond SSH access and admin credentials.

---

## VM Initialization Details (`04-vm-init.ps1`)

This script runs inside the worker VM and performs a 10-step setup:

| Step | Action | Details |
|------|--------|---------|
| 0 | Defender exclusions | Add path exclusions for `C:\AutoMutate`, `C:\RedEdr`; exclude build tool processes |
| 1 | System config | Set hostname, UTC timezone, `RemoteSigned` execution policy, disable UAC, disable auto-restart |
| 2 | Network | Static IP + DNS (8.8.8.8, gateway 10.200.200.1), retry DNS up to 30s |
| 3 | Privacy | Disable Windows telemetry + Cortana |
| 4 | Defender | Keep enabled for baseline (real EDR behavior) |
| 5 | Dev tools | Chocolatey, Rust (stable), protoc 25.1, VC++ runtime, VS 2022 Build Tools (C++ workload + Windows SDK) |
| 6 | Directories | Create `C:\AutoMutate\{artifacts,logs,harness}` |
| 7 | RedEDR | Extract from zip, verify signature, trust ELAM driver certificate (Root + TrustedPublisher), firewall rule for web UI, enable SMB from controller |
| 8 | Boot config | `bcdedit /set testsigning on`, `bcdedit -debug on`, disable HVCI/VBS |
| 9 | Audit policy | Enable ALL audit categories (success + failure), enable command-line logging (4688), PowerShell script block + module + transcription logging |
| 10 | Drivers | Install RedEDR `.inf` drivers via `pnputil`, register ETW-TI PPL service |

Reboot required after completion. Creates desktop shortcut for `Start-RedEDR-SYSTEM.ps1`.

---

## Quick Reference

```powershell
# Full setup (run once, as Administrator)
.\setup-all.ps1

# Day-to-day operations
.\scripts\start-environment.ps1          # Start everything
.\scripts\stop-environment.ps1           # Stop everything
.\scripts\validate-environment.ps1       # Health check

# Security controls
.\scripts\toggle-vm-internet.ps1 -Action Disable    # Air-gap VMs
.\scripts\toggle-vm-internet.ps1 -Action Enable     # Restore internet
.\scripts\toggle-vm-isolation.ps1 -Action Enable     # Block VM-to-VM

# Remote workers
.\scripts\workers\deploy-remote-worker.ps1 -RemoteHost 20.1.2.3 -Username admin
.\scripts\workers\list-workers.ps1

# Build artifacts (from WSL2)
./scripts/build-modular.sh --carrier peb_walk --decoder xor --antiemulation timeraw
```
