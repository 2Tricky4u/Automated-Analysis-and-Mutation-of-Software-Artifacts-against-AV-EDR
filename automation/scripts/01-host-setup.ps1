<#
.SYNOPSIS
    Host configuration: Hyper-V, WSL2, network isolation

.DESCRIPTION
    Enables Windows features, creates IsolationSwitch, configures firewall and port forwarding
    - Keeps WSL2 internet working (uses WSL's own NAT)
    - Does NOT enable global IP routing
    - VM NAT is optional/toggleable (no conflict with WSL)
    - WSL services stay bound to 127.0.0.1; exposure via host port-proxy only

.PARAMETER ConfigPath
    Path to config.yaml

.EXAMPLE
    .\01-host-setup.ps1 -ConfigPath "..\config.yaml"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$ConfigPath
)

$ErrorActionPreference = "Stop"

# Color output
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

# Load config
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$SwitchName = $config.network.switch_name
$HostIP     = $config.network.host_ip
$Subnet     = $config.network.subnet -replace '/.*', ''
$Prefix     = ($config.network.subnet -split '/')[1]
$GrpcPort   = $config.controller.grpc_port
$EsPort     = $config.controller.elasticsearch_port
$KibanaPort = $config.controller.kibana_port

# Optional/toggleable NAT for VMs (set to "true" in config.yaml to enable)
# Example in config.yaml:
# network:
#   enable_nat: "false"
$EnableVmNat = $false
if ($config.network.ContainsKey('enable_nat')) {
    $EnableVmNat = [bool]::Parse($config.network.enable_nat)
}

Write-Info "Configuring host with switch: $SwitchName, IP: $HostIP/$Prefix"

# Guard: avoid subnets that can collide with WSL/HNS (commonly 172.16.0.0/12)
if ($config.network.subnet -like '172.*') {
    throw "config.network.subnet '$($config.network.subnet)' must not be in 172.16.0.0/12. Use e.g. 10.200.200.0/24"
}

# 1) Enable features
Write-Info "Enabling Hyper-V, WSL2, VirtualMachinePlatform..."
$rebootNeeded = $false

$features = @(
    "Microsoft-Hyper-V-All",
    "Microsoft-Windows-Subsystem-Linux",
    "VirtualMachinePlatform"
)

foreach ($feature in $features) {
    $state = (Get-WindowsOptionalFeature -Online -FeatureName $feature -ErrorAction SilentlyContinue).State
    if ($state -ne "Enabled") {
        Write-Info "Enabling $feature..."
        Enable-WindowsOptionalFeature -Online -FeatureName $feature -NoRestart -All | Out-Null
        $rebootNeeded = $true
    } else {
        Write-Success "$feature already enabled"
    }
}

if ($rebootNeeded) {
    Write-Warn  "Reboot required (Windows features changed)"
    Write-Info  "After reboot, re-run this script to complete setup"
    exit 3010
}

# 2) Install WSL2 + Ubuntu
Write-Info "Installing WSL2 Ubuntu..."
if (-not (wsl -l -q 2>$null | Select-String "Ubuntu")) {
    wsl --install -d Ubuntu --no-launch
    Write-Success "WSL2 Ubuntu installed"
} else {
    Write-Success "WSL2 Ubuntu already installed"
}

# Set default to WSL 2
wsl --set-default-version 2 2>$null

# 3) Configure WSL2 networking
Write-Info "Configuring WSL2 networking for internet access..."

# Create .wslconfig to ensure proper networking mode (simple, no extra toggles)
$wslConfigPath = "$env:USERPROFILE\.wslconfig"
$wslConfigContent = @"
[wsl2]
networkingMode=nat
autoProxy=true
memory=8GB
processors=4
"@

if (-not (Test-Path $wslConfigPath)) {
    $wslConfigContent | Out-File -FilePath $wslConfigPath -Encoding utf8
    Write-Success "Created .wslconfig with NAT networking"
} else {
    Write-Info ".wslconfig already exists at $wslConfigPath"
    Write-Info "Verify it contains: networkingMode=nat"
}

# Ensure WSL's vEthernet adapter has proper firewall rules (allow host<->WSL basics)
Write-Info "Configuring firewall for WSL network adapter..."
$wslAdapter = Get-NetAdapter | Where-Object { $_.Name -like "*WSL*" } | Select-Object -First 1

if ($wslAdapter) {
    Write-Success "Found WSL adapter: $($wslAdapter.Name)"

    # Allow outbound traffic from WSL adapter
    $wslOutboundRule = "WSL-Internet-Outbound"
    if (-not (Get-NetFirewallRule -DisplayName $wslOutboundRule -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName $wslOutboundRule -Direction Outbound -Action Allow `
            -InterfaceAlias $wslAdapter.Name -Profile Any | Out-Null
        Write-Success "Created firewall rule: $wslOutboundRule"
    } else {
        Write-Success "Firewall rule already exists: $wslOutboundRule"
    }

    # Allow inbound for DNS and DHCP to the adapter (needed by WSL HNS plumbing)
    $wslInboundRule = "WSL-Network-Services"
    if (-not (Get-NetFirewallRule -DisplayName $wslInboundRule -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName $wslInboundRule -Direction Inbound -Action Allow `
            -InterfaceAlias $wslAdapter.Name -Protocol UDP -LocalPort 53,67,68 -Profile Any | Out-Null
        Write-Success "Created firewall rule: $wslInboundRule"
    } else {
        Write-Success "Firewall rule already exists: $wslInboundRule"
    }
} else {
    Write-Warn "WSL vEthernet adapter not found yet (will be created on first WSL launch)"
    Write-Info "After first WSL launch, firewall rules will be auto-created on next run"
}

# Restart WSL to apply .wslconfig changes
Write-Info "Restarting WSL to apply network configuration..."
wsl --shutdown 2>$null
Start-Sleep -Seconds 3
Write-Success "WSL2 networking configured"

# 4) Create Internal switch
if (-not (Get-VMSwitch -Name $SwitchName -ErrorAction SilentlyContinue)) {
    New-VMSwitch -Name $SwitchName -SwitchType Internal | Out-Null
    Write-Success "Created internal switch: $SwitchName"
} else {
    Write-Success "Switch $SwitchName exists"
}

# 5) Assign host IP
$adapter = Get-NetAdapter | Where-Object { $_.Name -like "*$SwitchName*" } | Select-Object -First 1
if (-not $adapter) {
    Write-Err "vEthernet adapter for $SwitchName not found"
    exit 1
}

$existingIp = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $HostIP -ErrorAction SilentlyContinue
if (-not $existingIp) {
    New-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $HostIP -PrefixLength $Prefix | Out-Null
    Write-Success "Configured $($adapter.Name) -> $HostIP/$Prefix"
} else {
    Write-Success "Host IP $HostIP already assigned"
}

# =====================================================================
# REMOVED: Step 6 (IP forwarding + IPEnableRouter + 'forwarding' FW rules)
# Rationale: Keep WSL's HNS NAT untouched and avoid routing conflicts.
# =====================================================================

# 7) Firewall rules (host listens only on IsolationSwitch IP for selected ports)
$ports = @($GrpcPort, $EsPort, $KibanaPort)
foreach ($port in $ports) {
    $ruleName = "AutoMutate-Allow-$port"
    if (-not (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Action Allow `
            -Protocol TCP -LocalPort $port -LocalAddress $HostIP | Out-Null
        Write-Success "Firewall rule: $ruleName"
    } else {
        Write-Success "Firewall rule exists: $ruleName"
    }
}

# 8) Port proxy (Host IP on IsolationSwitch -> WSL2 localhost:ports)
foreach ($port in $ports) {
    netsh interface portproxy delete v4tov4 listenaddress=$HostIP listenport=$port 2>$null | Out-Null
    netsh interface portproxy add v4tov4 listenaddress=$HostIP listenport=$port `
        connectaddress=127.0.0.1 connectport=$port | Out-Null
    Write-Success "Port proxy: ${HostIP}:$port -> 127.0.0.1:$port"
}

# 9) Configure NAT for VM internet access (optional/toggleable and WSL-safe)
Write-Info "Configuring NAT for VM internet access (optional)..."
$natName = "AutoMutateVMNAT"

# Detect HNS/WSL NAT (typically present if WSL is installed)
$existingNats = Get-NetNat -ErrorAction SilentlyContinue
$hasHnsNat   = $existingNats | Where-Object { $_.Name -match 'HNS' -or $_.Name -match 'WSL' }

if ($EnableVmNat -eq $false) {
    Write-Info "VM NAT creation is disabled by config (network.enable_nat=false)."
    Write-Info "You can enable internet for VMs later via your toggle script."
} elseif ($hasHnsNat) {
    # OK to have both if subnets don't overlap; we already guarded against 172.*
    $existingNat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
    if ($existingNat) {
        if ($existingNat.InternalIPInterfaceAddressPrefix -eq $config.network.subnet) {
            Write-Success "NAT already configured for $($config.network.subnet)"
        } else {
            Write-Warn "NAT exists but for wrong subnet: $($existingNat.InternalIPInterfaceAddressPrefix)"
            Write-Warn "Removing and recreating NAT..."
            Remove-NetNat -Name $natName -Confirm:$false
            New-NetNat -Name $natName -InternalIPInterfaceAddressPrefix $config.network.subnet | Out-Null
            Write-Success "NAT recreated for $($config.network.subnet)"
        }
    } else {
        New-NetNat -Name $natName -InternalIPInterfaceAddressPrefix $config.network.subnet | Out-Null
        Write-Success "NAT created for $($config.network.subnet)"
    }
} else {
    # No HNS NAT found (unlikely on WSL hosts) — still proceed if enabled
    $existingNat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
    if ($existingNat) {
        if ($existingNat.InternalIPInterfaceAddressPrefix -eq $config.network.subnet) {
            Write-Success "NAT already configured for $($config.network.subnet)"
        } else {
            Write-Warn "NAT exists but for wrong subnet: $($existingNat.InternalIPInterfaceAddressPrefix)"
            Write-Warn "Removing and recreating NAT..."
            Remove-NetNat -Name $natName -Confirm:$false
            New-NetNat -Name $natName -InternalIPInterfaceAddressPrefix $config.network.subnet | Out-Null
            Write-Success "NAT recreated for $($config.network.subnet)"
        }
    } else {
        New-NetNat -Name $natName -InternalIPInterfaceAddressPrefix $config.network.subnet | Out-Null
        Write-Success "NAT created for $($config.network.subnet)"
    }
}

Write-Info "WSL remains isolated (services bound to 127.0.0.1). Exposure is via host port-proxy on $HostIP only."
Write-Info "VMs get internet only when NAT is enabled and subnet is non-overlapping (e.g., 10.200.200.0/24)."

Write-Success "Host setup complete"
exit 0
