<#
.SYNOPSIS
    Host configuration: Hyper-V, WSL2, network isolation

.DESCRIPTION
    Enables Windows features, creates IsolationSwitch, configures firewall and port forwarding

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
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

# Load config
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$SwitchName = $config.network.switch_name
$HostIP = $config.network.host_ip
$Subnet = $config.network.subnet -replace '/.*', ''
$Prefix = ($config.network.subnet -split '/')[1]
$GrpcPort = $config.controller.grpc_port
$EsPort = $config.controller.elasticsearch_port
$KibanaPort = $config.controller.kibana_port

Write-Info "Configuring host with switch: $SwitchName, IP: $HostIP/$Prefix"

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
    Write-Warn "Reboot required (features or IP routing enabled)"
    Write-Info "After reboot, re-run this script to complete setup"
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

# Create .wslconfig to ensure proper networking mode
$wslConfigPath = "$env:USERPROFILE\.wslconfig"
$wslConfigContent = @"
[wsl2]
# Use NAT networking mode (default, ensures internet access)
networkingMode=nat

# DNS tunneling (resolves DNS through Windows)
dnsTunneling=true

# Windows firewall integration
firewall=true

# Automatic proxy detection
autoProxy=true

# Memory and processor limits (adjust based on host resources)
memory=8GB
processors=4
"@

if (-not (Test-Path $wslConfigPath)) {
    $wslConfigContent | Out-File -FilePath $wslConfigPath -Encoding utf8
    Write-Success "Created .wslconfig with NAT networking"
} else {
    Write-Info ".wslconfig already exists at $wslConfigPath"
    Write-Info "Verify it contains: networkingMode=nat, dnsTunneling=true"
}

# Ensure WSL's vEthernet adapter has proper firewall rules
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

    # Allow inbound for DNS and DHCP
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

# 6) Configure VM internet access via IP Forwarding (WSL-safe)
# CRITICAL: Windows only supports ONE NetNat properly. WSL2 uses its own implicit NAT.
# Creating a second NAT (IsolationNAT) breaks WSL2's internet access.
# Solution: Use IP forwarding + routing instead of NAT

Write-Info "Configuring VM internet access (WSL-safe IP forwarding)..."

# Always check for existing NAT conflicts (cleanup from previous runs)
$allNats = Get-NetNat -ErrorAction SilentlyContinue
$isolationNat = $allNats | Where-Object { $_.Name -eq "IsolationNAT" }
$wslNat = $allNats | Where-Object { $_.InternalIPInterfaceAddressPrefix -like "172.*" }

if ($isolationNat) {
    Write-Warn "Found conflicting IsolationNAT from previous run - removing..."
    Remove-NetNat -Name "IsolationNAT" -Confirm:$false
    Write-Success "Removed IsolationNAT (preserves WSL connectivity)"
}

if ($wslNat) {
    Write-Info "Detected WSL NAT: $($wslNat.InternalIPInterfaceAddressPrefix)"
    Write-Success "WSL NAT preserved (no conflict)"
}

# Enable IP forwarding on internal switch adapter
Write-Info "Enabling IP forwarding on vEthernet ($SwitchName)..."
$adapterAlias = "vEthernet ($SwitchName)"

# Enable forwarding at interface level
Set-NetIPInterface -InterfaceAlias $adapterAlias -Forwarding Enabled -ErrorAction SilentlyContinue
Write-Success "IP forwarding enabled on $adapterAlias"

# Enable global IP routing (required for forwarding to work)
$routingKey = "HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters"
$currentRouting = (Get-ItemProperty -Path $routingKey -Name "IPEnableRouter" -ErrorAction SilentlyContinue).IPEnableRouter

if ($currentRouting -ne 1) {
    Set-ItemProperty -Path $routingKey -Name "IPEnableRouter" -Value 1 -Type DWord
    Write-Info "Enabled global IP routing (registry: IPEnableRouter=1)"
    Write-Warn "Reboot required for routing to take full effect"
    $script:rebootNeeded = $true
} else {
    Write-Success "Global IP routing already enabled"
}

# Create firewall rules for IP forwarding (requires both Inbound + Outbound)
# Windows Firewall doesn't have a "Forward" direction - we simulate it with:
#   1. Inbound: Allow traffic FROM VMs arriving at internal switch
#   2. Outbound: Allow traffic FROM internal switch going to internet

$vmInboundRule = "AutoMutate-VM-Inbound"
if (-not (Get-NetFirewallRule -DisplayName $vmInboundRule -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $vmInboundRule -Direction Inbound -Action Allow `
        -InterfaceAlias $adapterAlias -RemoteAddress $config.network.subnet -Profile Any | Out-Null
    Write-Success "Created inbound rule: $vmInboundRule (allows VM traffic to host)"
} else {
    Write-Success "Inbound rule already exists: $vmInboundRule"
}

$vmOutboundRule = "AutoMutate-VM-Outbound"
if (-not (Get-NetFirewallRule -DisplayName $vmOutboundRule -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $vmOutboundRule -Direction Outbound -Action Allow `
        -InterfaceAlias $adapterAlias -Profile Any | Out-Null
    Write-Success "Created outbound rule: $vmOutboundRule (allows forwarded traffic to internet)"
} else {
    Write-Success "Outbound rule already exists: $vmOutboundRule"
}

Write-Success "VM internet access configured via IP forwarding"
Write-Info "VMs use gateway: $HostIP (routes through host's default gateway)"
Write-Info "WSL NAT preserved - no conflicts"
# 7) Firewall rules
$ports = @($GrpcPort, $EsPort, $KibanaPort)
foreach ($port in $ports) {
    $ruleName = "AutoMutate-Allow-$port"
    if (-not (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Action Allow `
            -Protocol TCP -LocalPort $port -LocalAddress $HostIP | Out-Null
        Write-Success "Firewall rule: $ruleName"
    }
}

# 8) Port proxy (Host IP -> WSL2 localhost)
foreach ($port in $ports) {
    netsh interface portproxy delete v4tov4 listenaddress=$HostIP listenport=$port 2>$null | Out-Null
    netsh interface portproxy add v4tov4 listenaddress=$HostIP listenport=$port `
        connectaddress=127.0.0.1 connectport=$port | Out-Null
    Write-Success "Port proxy: ${HostIP}:$port -> 127.0.0.1:$port"
}

# 9) Configure NAT for VM internet access
Write-Info "Configuring NAT for VM internet access..."
$natName = "AutoMutateVMNAT"
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

Write-Info "VMs will have internet access via NAT"
Write-Info "To disable VM internet: .\scripts\toggle-vm-internet.ps1 -Action Disable"
Write-Info "To enable VM internet:  .\scripts\toggle-vm-internet.ps1 -Action Enable"

Write-Success "Host setup complete"
exit 0
