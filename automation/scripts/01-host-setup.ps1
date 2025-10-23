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
    elseif ($_ -match '^\s+(\w+):\s*"?([^#]+?)"?\s*(?:#.*)?$' -and $section) {
        $config[$section][$matches[1]] = $matches[2].Trim().Trim('"')
    }
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

# 10) VM-to-VM Isolation (optional, based on config)
$EnableVmIsolation = $false
if ($config.security.ContainsKey('enable_vm_isolation')) {
    try {
        $EnableVmIsolation = [bool]::Parse($config.security.enable_vm_isolation)
    } catch {
        Write-Warn "Invalid value for enable_vm_isolation: $($config.security.enable_vm_isolation), defaulting to false"
    }
}

if ($EnableVmIsolation) {
    Write-Info "`nConfiguring VM-to-VM isolation (security.enable_vm_isolation=true)..."

    # Get adapter
    $adapter = Get-NetAdapter | Where-Object { $_.Name -like "*$SwitchName*" }
    if (-not $adapter) {
        Write-Warn "Could not find IsolationSwitch adapter for VM isolation"
    } else {
        $adapterAlias = $adapter.Name
        $subnetRange = "$($config.network.subnet.Split('/')[0].Split('.')[0..2] -join '.').0-$($config.network.subnet.Split('/')[0].Split('.')[0..2] -join '.').255"

        # Remove old rules if exist
        Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Isolation-Block-Inbound" -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Isolation-Block-Outbound" -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host" -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Outbound" -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Inbound" -ErrorAction SilentlyContinue

        # Create isolation rules
        # NOTE: Windows Firewall evaluates Allow rules before Block rules when they have the same specificity
        # So we create Allow for host first, then Block for subnet (which will not match host)

        # 1. CRITICAL: Allow traffic TO/FROM host (must be created first)
        New-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Outbound" -Direction Outbound -Action Allow `
            -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $HostIP -Profile Any -Enabled True | Out-Null

        New-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Inbound" -Direction Inbound -Action Allow `
            -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $HostIP -Profile Any -Enabled True | Out-Null
        Write-Success "VM-to-Host traffic explicitly allowed ($HostIP)"

        # 2. Block inbound FROM other VMs (but not from host due to rule above)
        New-NetFirewallRule -DisplayName "AutoMutate-VM-Isolation-Block-Inbound" -Direction Inbound -Action Block `
            -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $subnetRange -Profile Any -Enabled True | Out-Null
        Write-Success "VM-to-VM inbound traffic blocked"

        # 3. Block outbound TO other VMs (but not to host due to rule above)
        New-NetFirewallRule -DisplayName "AutoMutate-VM-Isolation-Block-Outbound" -Direction Outbound -Action Block `
            -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $subnetRange -Profile Any -Enabled True | Out-Null
        Write-Success "VM-to-VM outbound traffic blocked"

        Write-Info "VM-to-VM isolation ENABLED: VMs can only communicate with host ($HostIP)"
    }
} else {
    Write-Info "`nVM-to-VM isolation: DISABLED (default)"
    Write-Info "VMs can communicate with each other (useful for lateral movement testing)"
    Write-Info "To enable isolation: Set security.enable_vm_isolation=true in config.yaml"
    Write-Info "Or use: .\scripts\toggle-vm-isolation.ps1 -Action Enable"
}

# 11) Security notes
Write-Info "`n+================================================================+"
Write-Info "Security Enhancements Available:"
Write-Info "+================================================================+"
Write-Info ""
Write-Info "1. Egress Filtering (whitelist-based traffic control):"
Write-Info "   .\scripts\manage-egress-filter.ps1 -Action Enable"
Write-Info "   Default whitelist: DNS (53), HTTP (80), HTTPS (443), NTP (123)"
Write-Info ""
Write-Info "2. Internet Kill Switch (air-gap VMs):"
Write-Info "   .\scripts\toggle-vm-internet.ps1 -Action Disable"
Write-Info "   Add -KillConnections to terminate existing sessions"
Write-Info ""
Write-Info "3. VM-to-VM Isolation (prevent lateral movement):"
Write-Info "   .\scripts\toggle-vm-isolation.ps1 -Action Enable"
Write-Info "   Current: $(if ($EnableVmIsolation) { 'ENABLED' } else { 'DISABLED' })"
Write-Info ""
Write-Info "Recommended for malware testing:"
Write-Info "   1. .\scripts\toggle-vm-isolation.ps1 -Action Enable"
Write-Info "   2. .\scripts\toggle-vm-internet.ps1 -Action Disable -KillConnections"

Write-Success "`nHost setup complete"
Write-Success "`nNext steps:"
Write-Success "  1. Run: wsl -- bash ./scripts/02-wsl-bootstrap.sh"
Write-Success "  2. Validate: .\scripts\validate-environment.ps1"
Write-Success "  3. Check security: .\scripts\toggle-vm-internet.ps1 -Action Status"
exit 0
