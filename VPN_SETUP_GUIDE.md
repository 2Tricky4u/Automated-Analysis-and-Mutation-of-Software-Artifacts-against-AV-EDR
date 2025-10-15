# VPN/External RPC Setup Guide

This guide explains how to configure AutoMutate++ to send telemetry to external systems (Cortex, Microsoft Defender for Endpoint) over a VPN connection.

---

## Overview

By default, AutoMutate++ runs in an **isolated network** (IsolationSwitch: 192.168.200.0/24) with no internet access. Workers can only communicate with the Controller and local Elasticsearch.

**With VPN support**, workers can additionally send telemetry to:
- **Cortex**: Prometheus-compatible TSDB for centralized metrics (Prometheus remote write protocol)
- **Microsoft Defender for Endpoint (MDE)**: Custom detection events via Security Center API
- **Custom HTTP**: Any HTTP/HTTPS endpoint (e.g., SIEM, custom collector)

---

## Architecture Changes

### Before (Isolated Network)
```
┌─────────────────────────────────────────────────────────────┐
│ Host OS (Windows 11)                                        │
│                                                              │
│ ┌──────────────┐                                            │
│ │ WSL2         │                                            │
│ │ Controller   │ ◄──── gRPC (50051) ────┐                  │
│ │ Elasticsearch│ ◄──── HTTP (9200)  ────┼─────┐            │
│ └──────────────┘                         │     │            │
│                                          │     │            │
│ ┌────────────────────────────────────────┴─────┴──────────┐│
│ │ Hyper-V IsolationSwitch (192.168.200.0/24)             ││
│ │                                                          ││
│ │  ┌─────────────────┐  ┌─────────────────┐              ││
│ │  │ Worker VM 1     │  │ Worker VM 2     │              ││
│ │  │ 192.168.200.100 │  │ 192.168.200.110 │              ││
│ │  │ ✅ Can reach    │  │ ✅ Can reach    │              ││
│ │  │    Controller   │  │    Controller   │              ││
│ │  │ ❌ No Internet  │  │ ❌ No Internet  │              ││
│ │  └─────────────────┘  └─────────────────┘              ││
│ └──────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
```

### After (VPN-Enabled)
```
┌─────────────────────────────────────────────────────────────────────────────┐
│ Host OS (Windows 11)                                                        │
│                                                                              │
│ ┌──────────────┐                                                            │
│ │ WSL2         │                                                            │
│ │ Controller   │ ◄──── gRPC/TLS (50051) ────┐                              │
│ │ Elasticsearch│ ◄──── HTTP (9200)  ─────────┼─────┐                       │
│ └──────────────┘                             │     │                       │
│                                              │     │                       │
│ ┌────────────────────────────────────────────┴─────┴──────────┐            │
│ │ Hyper-V vSwitch (External/NAT + VPN Gateway)                │            │
│ │                                                              │            │
│ │  ┌─────────────────┐  ┌─────────────────┐                  │            │
│ │  │ Worker VM 1     │  │ Worker VM 2     │                  │            │
│ │  │ 192.168.200.100 │  │ 192.168.200.110 │                  │            │
│ │  │ ✅ Controller   │  │ ✅ Controller   │                  │            │
│ │  │ ✅ VPN Gateway  │  │ ✅ VPN Gateway  │                  │            │
│ │  │    (10.0.0.1)   │  │    (10.0.0.1)   │                  │            │
│ │  └─────────────────┘  └─────────────────┘                  │            │
│ │         │                     │                             │            │
│ │         │                     │                             │            │
│ │         └──────────┬──────────┘                             │            │
│ └────────────────────┼──────────────────────────────────────┘            │
│                      │                                                     │
└──────────────────────┼─────────────────────────────────────────────────────┘
                       │ VPN Tunnel
                       ▼
┌──────────────────────────────────────────────────────────────┐
│ External Network (VPN/RPC)                                   │
│                                                              │
│  ┌───────────────┐  ┌─────────────────┐  ┌──────────────┐  │
│  │ Cortex        │  │ MDE Security    │  │ Custom HTTP  │  │
│  │ (Prometheus)  │  │ Center API      │  │ Endpoint     │  │
│  │ :9009/write   │  │ api.security... │  │ :8443/events │  │
│  └───────────────┘  └─────────────────┘  └──────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

---

## Configuration Steps

### 1. Enable External Network Support

Edit `automation/config.yaml`:

```yaml
network:
  switch_name: "AutoMutateSwitch"  # Change from IsolationSwitch to External/NAT
  subnet: "192.168.200.0/24"
  host_ip: "192.168.200.1"
  gateway: "192.168.200.1"
  dns: "192.168.200.1"

  # Enable VPN connectivity
  allow_external: true              # ✅ SET TO TRUE
  vpn_gateway: "10.0.0.1"          # VPN gateway IP (replace with yours)
  external_dns: "8.8.8.8"           # DNS for external resolution
```

**Note**: You may need to:
- Change `switch_name` to an **External** or **NAT** type Hyper-V switch (instead of "Internal")
- Or set `bridge_mode: true` + `bridge_adapter: "Ethernet"` to bridge to your host's VPN adapter

---

### 2. Configure Worker Firewall Whitelist

Edit `automation/templates/worker.toml` on each worker:

```toml
[security]
disable_network = false
block_internet = true               # Keep true (block general internet)
allow_controller_only = false        # ✅ SET TO FALSE (allow external IPs)

# Whitelist VPN endpoints
allowed_ips = [
    "192.168.200.1",      # Controller (always allowed)
    "10.0.0.0/8",         # VPN network range (adjust to your VPN)
    "172.16.50.100",      # Cortex server IP (example)
    "20.190.160.100",     # MDE API endpoint (example - use actual IPs)
]
```

---

### 3. Enable External Telemetry Exporters

#### Option A: Cortex (Prometheus Remote Write)

Edit `automation/templates/worker.toml`:

```toml
[telemetry.external]
enabled = true

[telemetry.external.cortex]
enabled = true
endpoint = "https://cortex.vpn.example.com:9009/api/v1/push"
bearer_token = "your-api-token-here"

# OR use mTLS (recommended for production)
use_mtls = true
tls_cert_path = "C:\\AutoMutate\\certs\\cortex-client.crt"
tls_key_path = "C:\\AutoMutate\\certs\\cortex-client.key"
tls_ca_path = "C:\\AutoMutate\\certs\\cortex-ca.crt"

batch_size = 1000
flush_interval_secs = 10
retry_attempts = 3
timeout_secs = 30
```

**How to get credentials**:
1. Access your Cortex/Prometheus instance
2. Generate an API token (if using Grafana Cloud Prometheus)
3. Or set up mTLS certificates (recommended for self-hosted)

#### Option B: Microsoft Defender for Endpoint (MDE)

Edit `automation/templates/worker.toml`:

```toml
[telemetry.external.mde]
enabled = true
endpoint = "https://api.securitycenter.microsoft.com"
tenant_id = "your-azure-ad-tenant-id"
client_id = "your-app-registration-client-id"
client_secret = "your-app-registration-secret"

batch_size = 100
flush_interval_secs = 30
retry_attempts = 3
timeout_secs = 60
```

**How to get credentials**:
1. Go to Azure Portal → Azure Active Directory → App registrations
2. Create a new app registration (e.g., "AutoMutate-Telemetry")
3. Grant API permissions: `SecurityEvents.ReadWrite.All` (for custom detections)
4. Create a client secret
5. Copy `tenant_id`, `client_id`, `client_secret` to config

#### Option C: Custom HTTP Endpoint

Edit `automation/templates/worker.toml`:

```toml
[telemetry.external.custom_http]
enabled = true
endpoint = "https://custom-collector.vpn:8443/events"
method = "POST"
headers = {
    "Content-Type" = "application/json",
    "Authorization" = "Bearer YOUR_TOKEN_HERE"
}
batch_size = 500
flush_interval_secs = 15
retry_attempts = 3
timeout_secs = 30
```

---

### 4. Enable Controller TLS (Recommended)

If workers connect over VPN, enable TLS for gRPC:

Edit `automation/templates/controller.toml`:

```toml
[server]
bind_address = "0.0.0.0:50051"

# Enable TLS
tls_enabled = true
tls_cert_path = "/home/user/automutate/certs/controller.crt"
tls_key_path = "/home/user/automutate/certs/controller.key"

# Enable mTLS (require worker certificates)
require_client_cert = true
client_ca_cert_path = "/home/user/automutate/certs/ca.crt"
```

Update worker config to trust controller CA:

```toml
[controller]
controller_address = "192.168.200.1:50051"
tls_enabled = true
tls_ca_cert_path = "C:\\AutoMutate\\certs\\ca.crt"
```

---

## Certificate Generation (for mTLS)

### Generate Self-Signed CA and Certificates

```powershell
# On host OS (PowerShell)
mkdir certs
cd certs

# 1. Generate CA
openssl genrsa -out ca.key 4096
openssl req -new -x509 -days 365 -key ca.key -out ca.crt \
    -subj "/C=US/O=AutoMutate/CN=AutoMutate-CA"

# 2. Generate Controller certificate
openssl genrsa -out controller.key 2048
openssl req -new -key controller.key -out controller.csr \
    -subj "/C=US/O=AutoMutate/CN=controller"
openssl x509 -req -in controller.csr -CA ca.crt -CAkey ca.key \
    -CAcreateserial -out controller.crt -days 365

# 3. Generate Worker certificate
openssl genrsa -out worker.key 2048
openssl req -new -key worker.key -out worker.csr \
    -subj "/C=US/O=AutoMutate/CN=worker"
openssl x509 -req -in worker.csr -CA ca.crt -CAkey ca.key \
    -CAcreateserial -out worker.crt -days 365

# 4. Generate Cortex client certificate (if using mTLS)
openssl genrsa -out cortex-client.key 2048
openssl req -new -key cortex-client.key -out cortex-client.csr \
    -subj "/C=US/O=AutoMutate/CN=cortex-client"
openssl x509 -req -in cortex-client.csr -CA ca.crt -CAkey ca.key \
    -CAcreateserial -out cortex-client.crt -days 365
```

### Deploy Certificates

**Controller (WSL2)**:
```bash
mkdir -p ~/automutate/certs
cp certs/controller.{crt,key} ~/automutate/certs/
cp certs/ca.crt ~/automutate/certs/
chmod 600 ~/automutate/certs/*.key
```

**Workers (Windows VMs)**:
```powershell
# Copy to each worker VM
mkdir C:\AutoMutate\certs
cp certs\worker.crt C:\AutoMutate\certs\
cp certs\worker.key C:\AutoMutate\certs\
cp certs\ca.crt C:\AutoMutate\certs\
cp certs\cortex-client.{crt,key} C:\AutoMutate\certs\  # If using Cortex mTLS
```

---

## Testing VPN Connectivity

### 1. Test from Worker VM

```powershell
# Test VPN gateway reachability
ping 10.0.0.1

# Test Cortex endpoint
Test-NetConnection cortex.vpn.example.com -Port 9009

# Test MDE API endpoint
Invoke-WebRequest -Uri "https://api.securitycenter.microsoft.com" -Method HEAD
```

### 2. Test Telemetry Export

```powershell
# On worker VM, check logs
Get-Content C:\AutoMutate\logs\worker.log | Select-String "cortex\|mde\|external"

# Expected output:
# [INFO] Cortex exporter initialized: https://cortex.vpn.example.com:9009/api/v1/push
# [INFO] MDE exporter initialized: https://api.securitycenter.microsoft.com
# [INFO] Flushed 142 events to Cortex (batch_id=abc123)
```

### 3. Verify Data in External Systems

**Cortex/Prometheus**:
```promql
# Query in Grafana/Prometheus
automutate_etw_event{run_id="run-123"}
```

**MDE**:
1. Go to Microsoft 365 Defender portal
2. Advanced Hunting → Custom Detections
3. Filter by `Category == "AutoMutateTest"`

---

## Firewall Configuration

### Host OS (Windows Firewall)

If using NAT or bridge mode, ensure host firewall allows VPN traffic:

```powershell
# Allow VPN traffic on vEthernet adapter
New-NetFirewallRule -DisplayName "AutoMutate-VPN-Outbound" `
    -Direction Outbound `
    -InterfaceAlias "vEthernet (AutoMutateSwitch)" `
    -Action Allow

# Allow VPN DNS
New-NetFirewallRule -DisplayName "AutoMutate-DNS-Outbound" `
    -Direction Outbound `
    -Protocol UDP `
    -LocalPort 53 `
    -Action Allow
```

### Worker VMs (Windows Firewall)

Workers automatically configure firewall based on `security.allowed_ips`:

```powershell
# Applied by automation/scripts/04-vm-init.ps1

# Allow Controller
New-NetFirewallRule -DisplayName "Allow-Controller" `
    -RemoteAddress "192.168.200.1" `
    -Action Allow

# Allow VPN network
New-NetFirewallRule -DisplayName "Allow-VPN-Network" `
    -RemoteAddress "10.0.0.0/8" `
    -Action Allow

# Allow Cortex
New-NetFirewallRule -DisplayName "Allow-Cortex" `
    -RemoteAddress "172.16.50.100" `
    -Action Allow

# Block all other outbound (default-deny)
Set-NetFirewallProfile -DefaultOutboundAction Block
```

---

## Performance Considerations

### Telemetry Volume

External telemetry adds overhead:

| Exporter | Events/sec | Bandwidth | Latency |
|----------|------------|-----------|---------|
| Cortex   | ~1000      | ~10 KB/s  | +5-20ms |
| MDE      | ~100       | ~50 KB/s  | +50-200ms (OAuth) |
| Custom   | ~500       | ~20 KB/s  | +10-50ms |

**Recommendations**:
- Use **batching** (flush every 10-30s) to reduce API calls
- Enable **compression** (Snappy for Cortex, gzip for HTTP)
- Set `batch_size` based on event rate:
  - High-volume (>10K events/run): `batch_size=1000`
  - Low-volume (<1K events/run): `batch_size=100`

### Network Bandwidth

VPN adds ~10-50ms latency. For real-time detection testing:
- Keep local Elasticsearch as primary store (no VPN latency)
- Use external exporters for **centralized aggregation** only
- Consider **post-processing** (export after run completes)

---

## Troubleshooting

### Workers Can't Reach VPN Gateway

**Symptom**: `ping 10.0.0.1` fails from worker VM

**Fixes**:
1. Check Hyper-V switch type:
   ```powershell
   Get-VMSwitch "AutoMutateSwitch" | Select-Object SwitchType
   # Should be "External" or "NAT", not "Internal"
   ```

2. Enable IP forwarding on host:
   ```powershell
   Set-NetIPInterface -InterfaceAlias "vEthernet (AutoMutateSwitch)" -Forwarding Enabled
   ```

3. Check host VPN is active:
   ```powershell
   Get-NetAdapter | Where-Object Status -eq "Up" | Select-Object Name, InterfaceDescription
   ```

### Cortex Export Fails

**Symptom**: `Cortex write failed: 401 Unauthorized`

**Fixes**:
1. Verify bearer token is correct
2. Check token expiry (regenerate if expired)
3. Enable debug logging:
   ```toml
   [logging]
   level = "debug"
   ```

4. Test with curl:
   ```bash
   curl -X POST https://cortex.vpn:9009/api/v1/push \
     -H "Authorization: Bearer YOUR_TOKEN" \
     -H "Content-Type: application/x-protobuf" \
     --data-binary "@test-data.pb"
   ```

### MDE Export Fails

**Symptom**: `MDE token acquisition failed: 401 Unauthorized`

**Fixes**:
1. Verify `tenant_id`, `client_id`, `client_secret` are correct
2. Check app registration has `SecurityEvents.ReadWrite.All` permission
3. Grant admin consent in Azure Portal
4. Test OAuth flow manually:
   ```bash
   curl -X POST https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token \
     -d "client_id={client_id}" \
     -d "client_secret={client_secret}" \
     -d "scope=https://api.securitycenter.microsoft.com/.default" \
     -d "grant_type=client_credentials"
   ```

### TLS Handshake Failures

**Symptom**: `gRPC error: tls handshake failed`

**Fixes**:
1. Verify certificate paths are correct
2. Check certificate validity:
   ```bash
   openssl x509 -in controller.crt -noout -dates
   ```

3. Verify CA certificate matches:
   ```bash
   openssl verify -CAfile ca.crt controller.crt
   ```

4. Check certificate CN matches hostname:
   ```bash
   openssl x509 -in controller.crt -noout -subject
   # Should match controller hostname or IP
   ```

---

## Security Best Practices

1. **Always use TLS/mTLS** when connecting over VPN
2. **Rotate credentials** every 90 days (client secrets, API tokens)
3. **Use certificate auth** for MDE (more secure than client secret)
4. **Whitelist only required IPs** (don't use `0.0.0.0/0`)
5. **Monitor failed auth attempts** in external system logs
6. **Separate VPN credentials per environment** (dev/staging/prod)
7. **Use firewall rules** to enforce egress restrictions
8. **Audit external telemetry access** (who can query Cortex/MDE data)

---

## Architecture Validation

After setup, verify end-to-end flow:

```powershell
# 1. Start environment
cd automation
.\scripts\start-environment.ps1

# 2. Submit a test job
# (via Controller UI or gRPC)

# 3. Check worker logs
ssh worker-01 "tail -f C:\AutoMutate\logs\worker.log"

# Expected output:
# [INFO] Run started: run-abc123
# [INFO] ETW collector started
# [INFO] Cortex exporter: buffered 42 events
# [INFO] Cortex exporter: flushed 42 events (200 OK)
# [INFO] MDE exporter: buffered 8 events
# [INFO] MDE exporter: flushed 8 events (202 Accepted)
# [INFO] Run completed: status=not_detected

# 4. Verify data in external systems
# - Grafana: Query automutate_etw_event{run_id="run-abc123"}
# - MDE Portal: Check Custom Detections for "AutoMutateTest" category
```

---

## Example Configuration (Complete)

### automation/config.yaml
```yaml
network:
  switch_name: "AutoMutateExternal"
  subnet: "192.168.200.0/24"
  host_ip: "192.168.200.1"
  gateway: "192.168.200.1"
  dns: "8.8.8.8"
  allow_external: true
  vpn_gateway: "10.0.0.1"
  external_dns: "8.8.8.8"

external_telemetry:
  cortex:
    enabled: true
    endpoint: "https://cortex.internal:9009/api/v1/push"
    bearer_token: "{{ env.CORTEX_TOKEN }}"
    batch_size: 1000
    flush_interval_secs: 10

  mde:
    enabled: true
    tenant_id: "{{ env.AZURE_TENANT_ID }}"
    client_id: "{{ env.AZURE_CLIENT_ID }}"
    client_secret: "{{ env.AZURE_CLIENT_SECRET }}"
    batch_size: 100
    flush_interval_secs: 30

security:
  enable_mtls: true
  mtls_ca_path: "./certs/ca.crt"
  mtls_cert_path: "./certs/client.crt"
  mtls_key_path: "./certs/client.key"
```

### automation/templates/worker.toml
```toml
[controller]
controller_address = "192.168.200.1:50051"
tls_enabled = true
tls_ca_cert_path = "C:\\AutoMutate\\certs\\ca.crt"

[security]
disable_network = false
block_internet = true
allow_controller_only = false
allowed_ips = [
    "192.168.200.1",
    "10.0.0.0/8",
    "172.16.50.100",
]

[telemetry.external]
enabled = true

[telemetry.external.cortex]
enabled = true
endpoint = "https://cortex.internal:9009/api/v1/push"
bearer_token = "{{ env.CORTEX_TOKEN }}"
batch_size = 1000
flush_interval_secs = 10

[telemetry.external.mde]
enabled = true
tenant_id = "{{ env.AZURE_TENANT_ID }}"
client_id = "{{ env.AZURE_CLIENT_ID }}"
client_secret = "{{ env.AZURE_CLIENT_SECRET }}"
```

---

## Next Steps

1. **Test in staging**: Set up VPN in a non-production environment first
2. **Monitor bandwidth**: Use `Get-NetAdapterStatistics` to track VPN traffic
3. **Tune batch sizes**: Adjust based on event volume and latency tolerance
4. **Set up dashboards**: Create Grafana dashboards for exported metrics
5. **Configure alerts**: Set up alerts in Cortex/MDE for detection anomalies

---

## References

- [Prometheus Remote Write Spec](https://prometheus.io/docs/concepts/remote_write_spec/)
- [Cortex Documentation](https://cortexmetrics.io/docs/)
- [Microsoft Defender for Endpoint API](https://learn.microsoft.com/en-us/microsoft-365/security/defender-endpoint/apis-intro)
- [Hyper-V Networking Guide](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/user-guide/setup-nat-network)
- [OpenSSL Certificate Generation](https://www.openssl.org/docs/man1.1.1/man1/openssl-req.html)

---

**Last Updated**: 2025-01-15
**Status**: ✅ Ready for Testing
