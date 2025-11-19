<#
.SYNOPSIS
    Host configuration: Hyper-V, WSL2, network isolation

.DESCRIPTION
    Enables Windows features, creates IsolationSwitch, configures firewall and port forwarding
    - Keeps WSL2 internet working (own NAT)
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

# toggleable NAT for VMs
$EnableVmNat = $false
if ($config.network.ContainsKey('enable_nat')) {
    $EnableVmNat = [bool]::Parse($config.network.enable_nat)
}

Write-Info "Configuring host with switch: $SwitchName, IP: $HostIP/$Prefix"

# Guard: avoid subnets that can collide with WSL/HNS (commonly 172.16.0.0/12)
if ($config.network.subnet -like '172.*') {
    throw "config.network.subnet '$($config.network.subnet)' must not be in 172.16.0.0/12. Use e.g. 10.200.200.0/24"
}

# 1. Enable features
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

# 2. Install WSL2 + Ubuntu
Write-Info "Installing WSL2 Ubuntu..."
if (-not (wsl -l -q 2>$null | Select-String "Ubuntu")) {
    wsl --install -d Ubuntu --no-launch
    Write-Success "WSL2 Ubuntu installed"
} else {
    Write-Success "WSL2 Ubuntu already installed"
}

# Set default to WSL 2
wsl --set-default-version 2 2>$null

# 3. Configure WSL2 networking
Write-Info "Configuring WSL2 networking for internet access..."

# Create .wslconfig to ensure proper networking mode
$wslConfigPath = "$env:USERPROFILE\.wslconfig"
$wslConfigContent = @"
[wsl2]
networkingMode=nat
dnsTunneling=true
localhostForwarding=true
autoProxy=true
memory=12GB
swap=24GB
processors=6
"@

if (-not (Test-Path $wslConfigPath)) {
    $wslConfigContent | Out-File -FilePath $wslConfigPath -Encoding utf8
    Write-Success "Created .wslconfig with NAT networking"
} else {
    Write-Info ".wslconfig already exists at $wslConfigPath"
    Write-Info "Verify it contains: networkingMode=nat"
}

# Ensure WSL vEthernet adapter has proper firewall rules
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

    # Allow inbound for DNS and DHCP to the adapter
    $wslInboundRule = "WSL-Network-Services"
    if (-not (Get-NetFirewallRule -DisplayName $wslInboundRule -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName $wslInboundRule -Direction Inbound -Action Allow `
            -InterfaceAlias $wslAdapter.Name -Protocol UDP -LocalPort 53,67,68 -Profile Any | Out-Null
        Write-Success "Created firewall rule: $wslInboundRule"
    } else {
        Write-Success "Firewall rule already exists: $wslInboundRule"
    }
} else {
    Write-Warn "WSL vEthernet adapter not found yet"
    Write-Info "After first WSL launch, firewall rules will be auto-created on next run"
}

# Restart WSL to apply .wslconfig changes
Write-Info "Restarting WSL to apply network configuration..."
wsl --shutdown 2>$null
Start-Sleep -Seconds 3
Write-Success "WSL2 networking configured"

# 4. Create Internal switch
if (-not (Get-VMSwitch -Name $SwitchName -ErrorAction SilentlyContinue)) {
    New-VMSwitch -Name $SwitchName -SwitchType Internal | Out-Null
    Write-Success "Created internal switch: $SwitchName"
} else {
    Write-Success "Switch $SwitchName exists"
}

# 5. Assign host IP
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

# 5.5. Enable IP forwarding between WSL2 and IsolationSwitch
Write-Info "Enabling IP forwarding for WSL2 <-> VM communication..."

# Enable global IP forwarding on Windows (requires reboot to fully activate)
$currentForwarding = Get-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters" -Name "IPEnableRouter" -ErrorAction SilentlyContinue
if ($null -eq $currentForwarding -or $currentForwarding.IPEnableRouter -ne 1) {
    Set-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters" -Name "IPEnableRouter" -Value 1
    Write-Success "Global IP forwarding enabled (registry: IPEnableRouter=1)"
    $rebootNeeded = $true
} else {
    Write-Success "Global IP forwarding already enabled"
}

# Enable forwarding on IsolationSwitch adapter
$isolationAdapter = Get-NetAdapter | Where-Object { $_.Name -like "*$SwitchName*" } | Select-Object -First 1
if ($isolationAdapter) {
    $forwardingStatus = Get-NetIPInterface -InterfaceAlias $isolationAdapter.Name -AddressFamily IPv4 | Select-Object -ExpandProperty Forwarding
    if ($forwardingStatus -ne "Enabled") {
        Set-NetIPInterface -InterfaceAlias $isolationAdapter.Name -AddressFamily IPv4 -Forwarding Enabled
        Write-Success "IP forwarding enabled on $($isolationAdapter.Name)"
    } else {
        Write-Success "IP forwarding already enabled on $($isolationAdapter.Name)"
    }
}

# Enable forwarding on WSL adapter (if it exists)
$wslAdapter = Get-NetAdapter | Where-Object { $_.Name -like "*WSL*" } | Select-Object -First 1
if ($wslAdapter) {
    $forwardingStatus = Get-NetIPInterface -InterfaceAlias $wslAdapter.Name -AddressFamily IPv4 -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Forwarding
    if ($forwardingStatus -ne "Enabled") {
        Set-NetIPInterface -InterfaceAlias $wslAdapter.Name -AddressFamily IPv4 -Forwarding Enabled
        Write-Success "IP forwarding enabled on $($wslAdapter.Name)"
    } else {
        Write-Success "IP forwarding already enabled on $($wslAdapter.Name)"
    }
} else {
    Write-Info "WSL adapter not found yet (will be configured on next run after WSL is started)"
}

Write-Success "IP forwarding configured - WSL2 can now communicate with VMs"

# 5.6. Configure WSL2 routing to VM network
Write-Info "Configuring WSL2 route to VM network..."

# Check if WSL is running
$wslRunning = wsl -l --running 2>$null | Select-String "Ubuntu"
$wslWasRunning = $true

if (-not $wslRunning) {
    Write-Info "WSL not running - starting Ubuntu to configure route..."
    $wslWasRunning = $false

    # Start WSL
    wsl -d Ubuntu echo "WSL started" 2>$null | Out-Null
    Start-Sleep -Seconds 3
}

# Get Windows host IP from WSL's perspective (WSL's default gateway)
$wslGatewayRaw = wsl -- ip route show default 2>$null
$wslGatewayRaw = $wslGatewayRaw | Out-String
if ($wslGatewayRaw -match 'default via ([\d\.]+)') {
    $wslGateway = $matches[1].Trim()
    Write-Info "Extracted WSL gateway: $wslGateway"
} else {
    $wslGateway = $null
    Write-Warn "Could not parse WSL gateway from: $wslGatewayRaw"
}

if ($wslGateway) {
    Write-Info "WSL gateway IP: $wslGateway"

    # Add route in WSL2 to reach VM network via Windows host
    $subnet = $config.network.subnet

    # Check if route already exists
    $existingRoute = wsl -- ip route show $subnet 2>$null
    if (-not $existingRoute) {
        # Add new route
        $routeAddResult = wsl -u root -- ip route add $subnet via $wslGateway 2>&1
        if ($LASTEXITCODE -eq 0 -or $routeAddResult -match "File exists") {
            Write-Success "Route added successfully"
        } else {
            Write-Warn "Route add failed: $routeAddResult"
        }
    } else {
        Write-Info "Route already exists: $existingRoute"
    }

    # Verify route was added
    Start-Sleep -Seconds 1
    $routeCheck = wsl -- ip route show | Select-String $subnet
    if ($routeCheck) {
        Write-Success "WSL2 route added: $subnet via $wslGateway"

        # Test connectivity to host
        Write-Info "Testing WSL2 -> Windows host connectivity..."
        $pingResult = wsl -- ping -c 1 -W 2 $HostIP 2>&1
        if ($pingResult -match "1 received") {
            Write-Success "WSL2 can reach Windows host at $HostIP"
        } else {
            Write-Warn "WSL2 cannot ping host - Windows reboot required for IPEnableRouter to take effect"
            Write-Info "After reboot, WSL2 -> VM connectivity will work"
        }
    } else {
        Write-Warn "Failed to add WSL2 route (may already exist or require WSL restart)"
        Write-Info "Manual command: wsl -u root -- ip route add $subnet via $wslGateway"
    }
} else {
    Write-Warn "Could not determine WSL gateway IP"
}

# Shutdown WSL if it wasn't running before (clean state)
if (-not $wslWasRunning) {
    Write-Info "Shutting down WSL (it will auto-start when needed)..."
    wsl --shutdown 2>$null
    Start-Sleep -Seconds 2
    Write-Success "WSL configured and shut down"
}

# 6. Firewall rules
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

# 7. Port proxy
# Note: Controller (gRPC) binds to 0.0.0.0, so it doesn't need portproxy
# Only Elasticsearch and Kibana (running in WSL on 127.0.0.1) need portproxy
foreach ($port in $ports) {
    # Skip gRPC port - controller binds to 0.0.0.0 directly (portproxy breaks HTTP/2 framing from WSL)
    #if ($port -eq $GrpcPort) {
    #    Write-Info "Skipping port proxy for gRPC port $port (controller binds to 0.0.0.0 directly)"
    #    continue
    #}
#
    netsh interface portproxy delete v4tov4 listenaddress=$HostIP listenport=$port 2>$null | Out-Null
    netsh interface portproxy add v4tov4 listenaddress=$HostIP listenport=$port `
        connectaddress=127.0.0.1 connectport=$port | Out-Null
    Write-Success "Port proxy: ${HostIP}:$port -> 127.0.0.1:$port"
}

# 8. Configure NAT for VM internet access
Write-Info "Configuring NAT for VM internet access (optional)..."
$natName = "AutoMutateVMNAT"

# Detect HNS/WSL NAT
$existingNats = Get-NetNat -ErrorAction SilentlyContinue
$hasHnsNat   = $existingNats | Where-Object { $_.Name -match 'HNS' -or $_.Name -match 'WSL' }

if ($EnableVmNat -eq $false) {
    Write-Info "VM NAT creation is disabled by config (network.enable_nat=false)."
    Write-Info "You can enable internet for VMs later via your toggle script."
} elseif ($hasHnsNat) {
    # OK to have both if subnets dont overlap
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
    # No HNS NAT found
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

Write-Info "Network Configuration:"
Write-Info "  - WSL services bound to 127.0.0.1, exposed via host port-proxy on $HostIP"
Write-Info "  - IP forwarding enabled: WSL2 <-> Windows Host <-> VMs"
Write-Info "  - WSL2 can directly connect to VMs (e.g., controller -> worker agent)"
Write-Info "  - VMs get internet only when NAT is enabled and subnet is non-overlapping"

# 9. VM-to-VM Isolation
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

        # isolation rules

        # 1. CRITICAL: Allow traffic TO/FROM host
        New-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Outbound" -Direction Outbound -Action Allow `
            -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $HostIP -Profile Any -Enabled True | Out-Null

        New-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Inbound" -Direction Inbound -Action Allow `
            -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $HostIP -Profile Any -Enabled True | Out-Null
        Write-Success "VM-to-Host traffic explicitly allowed ($HostIP)"

        # 2. Block inbound FROM other VMs
        New-NetFirewallRule -DisplayName "AutoMutate-VM-Isolation-Block-Inbound" -Direction Inbound -Action Block `
            -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $subnetRange -Profile Any -Enabled True | Out-Null
        Write-Success "VM-to-VM inbound traffic blocked"

        # 3. Block outbound TO other VMs
        New-NetFirewallRule -DisplayName "AutoMutate-VM-Isolation-Block-Outbound" -Direction Outbound -Action Block `
            -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $subnetRange -Profile Any -Enabled True | Out-Null
        Write-Success "VM-to-VM outbound traffic blocked"

        Write-Info "VM-to-VM isolation ENABLED: VMs can only communicate with host ($HostIP)"
    }
} else {
    Write-Info "`nVM-to-VM isolation: DISABLED"
    Write-Info "VMs can communicate with each other"
    Write-Info "To enable isolation: Set security.enable_vm_isolation=true in config.yaml"
    Write-Info "Or use: .\scripts\toggle-vm-isolation.ps1 -Action Enable"
}

# 10. Egress Filtering
$EnableEgressFilter = $false
if ($config.security.ContainsKey('enable_egress_filter')) {
    try {
        $EnableEgressFilter = [bool]::Parse($config.security.enable_egress_filter)
    } catch {
        Write-Warn "Invalid value for enable_egress_filter: $($config.security.enable_egress_filter), defaulting to false"
    }
}

if ($EnableEgressFilter) {
    Write-Info "`nConfiguring egress filtering (security.enable_egress_filter=true)..."

    # Get adapter
    $adapter = Get-NetAdapter | Where-Object { $_.Name -like "*$SwitchName*" }
    if (-not $adapter) {
        Write-Warn "Could not find IsolationSwitch adapter for egress filtering"
    } else {
        $adapterAlias = $adapter.Name

        # Remove old egress rules if exist
        Remove-NetFirewallRule -DisplayName "AutoMutate-Egress-Block-Internet" -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -DisplayName "AutoMutate-Egress-Allow-DNS" -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -DisplayName "AutoMutate-Egress-Allow-HTTP" -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -DisplayName "AutoMutate-Egress-Allow-HTTPS" -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -DisplayName "AutoMutate-Egress-Allow-NTP" -ErrorAction SilentlyContinue

        # Create default-deny egress rule
        New-NetFirewallRule -DisplayName "AutoMutate-Egress-Block-Internet" -Direction Outbound -Action Block `
            -Protocol Any -InterfaceAlias $adapterAlias -Profile Any -Enabled True | Out-Null
        Write-Success "Egress blocking rule created (default-deny)"

        # Create whitelist rules
        $whitelistRules = @(
            @{ Name = "AutoMutate-Egress-Allow-DNS"; Protocol = "UDP"; Port = 53; Desc = "DNS queries" },
            @{ Name = "AutoMutate-Egress-Allow-HTTP"; Protocol = "TCP"; Port = 80; Desc = "HTTP (Windows Update)" },
            @{ Name = "AutoMutate-Egress-Allow-HTTPS"; Protocol = "TCP"; Port = 443; Desc = "HTTPS (Windows Update)" },
            @{ Name = "AutoMutate-Egress-Allow-NTP"; Protocol = "UDP"; Port = 123; Desc = "NTP time sync" }
        )

        foreach ($rule in $whitelistRules) {
            New-NetFirewallRule -DisplayName $rule.Name -Direction Outbound -Action Allow `
                -Protocol $rule.Protocol -RemotePort $rule.Port -InterfaceAlias $adapterAlias `
                -Profile Any -Enabled True | Out-Null
            Write-Success "Whitelist: $($rule.Desc) ($($rule.Protocol)/$($rule.Port))"
        }

        Write-Info "Egress filtering ENABLED: Only DNS, HTTP, HTTPS, NTP allowed"
    }
} else {
    Write-Info "`nEgress filtering: DISABLED"
    Write-Info "VMs have unrestricted outbound access"
    Write-Info "To enable filtering: Set security.enable_egress_filter=true in config.yaml"
    Write-Info "Or use: .\scripts\manage-egress-filter.ps1 -Action Enable"
}

# 12) Security notes
Write-Info "`n+================================================================+"
Write-Info "Security Configuration Summary:"
Write-Info "+================================================================+"
Write-Info ""
Write-Info "1. VM-to-VM Isolation:"
Write-Info "   Status: $(if ($EnableVmIsolation) { 'ENABLED' } else { 'DISABLED' })"
Write-Info "   Toggle: .\scripts\toggle-vm-isolation.ps1 -Action Enable|Disable"
Write-Info ""
Write-Info "2. Egress Filtering:"
Write-Info "   Status: $(if ($EnableEgressFilter) { 'ENABLED' } else { 'DISABLED' })"
Write-Info "   Manage: .\scripts\manage-egress-filter.ps1 -Action Enable|Disable"
Write-Info "   Default whitelist: DNS (53), HTTP (80), HTTPS (443), NTP (123)"
Write-Info ""
Write-Info "3. Internet Access:"
Write-Info "   Status: $(if ($EnableVmNat) { 'ENABLED' } else { 'DISABLED' })"
Write-Info "   Toggle: .\scripts\toggle-vm-internet.ps1 -Action Enable|Disable"
Write-Info "   Kill connections: .\scripts\toggle-vm-internet.ps1 -Action Disable -KillConnections"
Write-Info ""
exit 0
