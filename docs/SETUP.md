# Setup Guide

## Windows Host Setup

### 1. Enable Hyper-V

```powershell
# Run as Administrator
Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All
```

### 2. Create Virtual Machines

#### Baseline VM
- Windows 10/11 Pro
- 4GB RAM, 2 vCPU
- 50GB disk
- No antivirus installed

#### Defender VM
- Windows 10/11 Pro
- 4GB RAM, 2 vCPU
- 50GB disk
- Windows Defender enabled

### 3. Install WSL2

```powershell
wsl --install -d Ubuntu
wsl --set-default-version 2
```

### 4. Configure Networking

Create a Hyper-V virtual switch:
```powershell
New-VMSwitch -Name "EDR-Lab" -SwitchType Internal
```

## WSL2 Ubuntu Setup

### 1. Install Docker

```bash
# Update packages
sudo apt-get update
sudo apt-get install -y ca-certificates curl gnupg

# Add Docker GPG key
sudo install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
sudo chmod a+r /etc/apt/keyrings/docker.gpg

# Add Docker repository
echo \
  "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu \
  $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
  sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

# Install Docker
sudo apt-get update
sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin

# Add user to docker group
sudo usermod -aG docker $USER
```

### 2. Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### 3. Install Protobuf

```bash
sudo apt-get install -y protobuf-compiler libprotobuf-dev
```

### 4. Clone Repository

```bash
git clone https://github.com/2Tricky4u/Automated-Analysis-and-Mutation-of-Software-Artifacts-against-AV-EDR.git
cd Automated-Analysis-and-Mutation-of-Software-Artifacts-against-AV-EDR
```

### 5. Build Services

```bash
cargo build --release
```

## Windows VM Setup

### 1. Build ETW Consumer

On each Windows VM:

```powershell
# Install Visual Studio Build Tools or Visual Studio
# Install CMake

cd telemetry\etw-consumer
mkdir build
cd build
cmake ..
cmake --build . --config Release
```

### 2. Install krabsetw (Optional)

For full ETW functionality:

```powershell
# Clone krabsetw
git clone https://github.com/microsoft/krabsetw.git third_party/krabsetw

# Rebuild ETW consumer
cd telemetry\etw-consumer\build
cmake ..
cmake --build . --config Release
```

### 3. Configure Shared Folder

Create a shared folder between host and VM for telemetry data:

```powershell
# On host
New-SmbShare -Name "EDRLogs" -Path "C:\EDRLogs" -FullAccess "Everyone"

# On VM
net use Z: \\host\EDRLogs
```

## Starting the Lab

### 1. Start Elastic Stack (WSL2)

```bash
cd build/dockerfiles
docker-compose up -d

# Wait for services to start
docker-compose ps

# Check Elasticsearch
curl http://localhost:9200

# Check Kibana (wait 1-2 minutes)
curl http://localhost:5601
```

### 2. Start Controller (WSL2)

```bash
cargo run --release -p scheduler
```

### 3. Start Worker (WSL2)

```bash
export WORKER_ID=worker-01
cargo run --release -p worker-agent
```

### 4. Start ETW Consumer (Windows VMs)

On both Baseline and Defender VMs:

```powershell
cd telemetry\etw-consumer\build\Release
.\etw_consumer.exe C:\EDRLogs\etw_events.csv
```

### 5. Import Kibana Dashboards

```bash
# From WSL2
cd ui/kibana-dashboards

# Import dashboard (requires Kibana to be running)
curl -X POST "localhost:5601/api/saved_objects/_import" \
  -H "kbn-xsrf: true" \
  --form file=@edr-dashboard.ndjson
```

## Verification

### 1. Test Controller

```bash
cargo run -p triage-client
```

### 2. Test Worker

```bash
grpcurl -plaintext -d '{"worker_id":"worker-01"}' \
  localhost:50052 edr.WorkerAgent/HealthCheck
```

### 3. Check Elasticsearch

```bash
# List indices
curl http://localhost:9200/_cat/indices?v

# Search telemetry
curl http://localhost:9200/edr-telemetry-*/_search?pretty
```

### 4. View Kibana Dashboard

Open browser to `http://localhost:5601` and navigate to Dashboard.

## Troubleshooting

### Docker Not Starting

```bash
sudo service docker start
sudo systemctl enable docker
```

### Port Already in Use

```bash
# Check what's using the port
sudo lsof -i :50051
sudo lsof -i :9200

# Kill process or change port in configuration
```

### Elasticsearch Won't Start

Check logs:
```bash
docker-compose logs elasticsearch
```

Common issues:
- Increase vm.max_map_count: `sudo sysctl -w vm.max_map_count=262144`
- Insufficient memory: Increase Docker memory limit

### ETW Consumer Not Capturing Events

- Run as Administrator on Windows
- Check Windows Event Log service is running
- Verify ETW providers are available: `logman query providers`

### Filebeat Not Sending Data

```bash
# Check Filebeat logs
docker-compose logs filebeat

# Verify Filebeat can reach Elasticsearch
docker-compose exec filebeat curl http://elasticsearch:9200
```

## Network Configuration

### Firewall Rules (Windows)

Allow gRPC ports:
```powershell
New-NetFirewallRule -DisplayName "EDR Controller" -Direction Inbound -Protocol TCP -LocalPort 50051 -Action Allow
New-NetFirewallRule -DisplayName "EDR Worker" -Direction Inbound -Protocol TCP -LocalPort 50052 -Action Allow
```

### WSL2 Port Forwarding

```powershell
# Forward ports from Windows to WSL2
netsh interface portproxy add v4tov4 listenport=5601 listenaddress=0.0.0.0 connectport=5601 connectaddress=<WSL2_IP>
netsh interface portproxy add v4tov4 listenport=9200 listenaddress=0.0.0.0 connectport=9200 connectaddress=<WSL2_IP>
```

Get WSL2 IP:
```bash
ip addr show eth0 | grep inet | awk '{print $2}' | cut -d'/' -f1
```

## Security Considerations

1. **Isolate the Lab**: Use a separate network segment
2. **No Internet Access**: Configure VMs without internet access
3. **Snapshots**: Take VM snapshots before running samples
4. **Monitoring**: Monitor all network traffic
5. **Cleanup**: Reset VMs after each analysis

## Next Steps

- Read [ARCHITECTURE.md](ARCHITECTURE.md) for system design
- Review [Job Schema](schemas/job-schema.json) for job definitions
- Test with benign samples first
- Configure additional mutation strategies
- Customize Kibana dashboards
