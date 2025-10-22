<#
.SYNOPSIS
    Comprehensive network and firewall dump for troubleshooting

.DESCRIPTION
    Prints ALL network configuration, routes, firewall rules, and DNS settings
    to identify connectivity issues between host and VMs
#>

$ErrorActionPreference = "Continue"

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          COMPREHENSIVE NETWORK DIAGNOSTICS                     |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

# ====================
# SECTION 1: Network Adapters
# ====================
Write-Host "=" * 80 -ForegroundColor Yellow
Write-Host "[1] ALL NETWORK ADAPTERS" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
Get-NetAdapter | Format-Table Name, Status, MacAddress, LinkSpeed -AutoSize

# ====================
# SECTION 2: IP Addresses
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[2] ALL IP ADDRESSES" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
Get-NetIPAddress -AddressFamily IPv4 |
    Format-Table InterfaceAlias, IPAddress, PrefixLength, PrefixOrigin, SuffixOrigin -AutoSize

# ====================
# SECTION 3: Routing Table
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[3] ROUTING TABLE" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
Get-NetRoute -AddressFamily IPv4 |
    Where-Object { $_.DestinationPrefix -notlike "224.*" -and $_.DestinationPrefix -notlike "255.*" } |
    Format-Table DestinationPrefix, NextHop, InterfaceAlias, RouteMetric -AutoSize

# ====================
# SECTION 4: IP Forwarding Status
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[4] IP FORWARDING STATUS (Per Interface)" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
Get-NetIPInterface -AddressFamily IPv4 |
    Format-Table InterfaceAlias, Forwarding, ConnectionState, DHCP -AutoSize

# ====================
# SECTION 5: Global IP Routing
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[5] GLOBAL IP ROUTING" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
$routing = (Get-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters" -Name "IPEnableRouter" -ErrorAction SilentlyContinue).IPEnableRouter
Write-Host "Registry: HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\IPEnableRouter"
Write-Host "Value: $routing (1 = Enabled, 0 = Disabled)"
if ($routing -eq 1) {
    Write-Host "Status: ENABLED" -ForegroundColor Green
} else {
    Write-Host "Status: DISABLED (VMs won't be able to route traffic!)" -ForegroundColor Red
}

# ====================
# SECTION 6: DNS Configuration
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[6] DNS CLIENT CONFIGURATION" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
Get-DnsClientServerAddress -AddressFamily IPv4 |
    Format-Table InterfaceAlias, ServerAddresses -AutoSize

# ====================
# SECTION 7: NAT Configuration
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[7] NAT CONFIGURATION" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
$nats = Get-NetNat -ErrorAction SilentlyContinue
if ($nats) {
    $nats | Format-Table Name, InternalIPInterfaceAddressPrefix, Active -AutoSize
    foreach ($nat in $nats) {
        if ($nat.InternalIPInterfaceAddressPrefix -like "*192.168.200*") {
            Write-Host "WARNING: NAT found for VM subnet - this conflicts with IP forwarding!" -ForegroundColor Red
        }
    }
} else {
    Write-Host "No NAT configurations (good - using IP forwarding)" -ForegroundColor Green
}

# ====================
# SECTION 8: AutoMutate Firewall Rules
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[8] AUTOMUTATE FIREWALL RULES" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
$autoMutateRules = Get-NetFirewallRule | Where-Object { $_.DisplayName -like "AutoMutate*" }
if ($autoMutateRules) {
    foreach ($rule in $autoMutateRules) {
        $filter = Get-NetFirewallAddressFilter -AssociatedNetFirewallRule $rule
        Write-Host "`nRule: $($rule.DisplayName)" -ForegroundColor Cyan
        Write-Host "  Direction: $($rule.Direction)"
        Write-Host "  Action: $($rule.Action)"
        Write-Host "  Enabled: $($rule.Enabled)"
        Write-Host "  Profile: $($rule.Profile)"
        Write-Host "  LocalAddress: $($filter.LocalAddress -join ', ')"
        Write-Host "  RemoteAddress: $($filter.RemoteAddress -join ', ')"

        # Get port filter
        $portFilter = Get-NetFirewallPortFilter -AssociatedNetFirewallRule $rule
        if ($portFilter.LocalPort -or $portFilter.RemotePort) {
            Write-Host "  LocalPort: $($portFilter.LocalPort -join ', ')"
            Write-Host "  RemotePort: $($portFilter.RemotePort -join ', ')"
            Write-Host "  Protocol: $($portFilter.Protocol)"
        }

        # Get interface filter
        $interfaceFilter = Get-NetFirewallInterfaceFilter -AssociatedNetFirewallRule $rule
        if ($interfaceFilter.InterfaceAlias) {
            Write-Host "  Interface: $($interfaceFilter.InterfaceAlias -join ', ')"
        }
    }
} else {
    Write-Host "ERROR: No AutoMutate firewall rules found!" -ForegroundColor Red
    Write-Host "Run: .\01-host-setup.ps1 -ConfigPath ..\config.yaml" -ForegroundColor Yellow
}

# ====================
# SECTION 9: Default Windows Firewall Rules (relevant ones)
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[9] WINDOWS FIREWALL - OUTBOUND RULES (Default)" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
$outboundDefault = Get-NetFirewallProfile | Select-Object Name, DefaultOutboundAction
$outboundDefault | Format-Table -AutoSize
Write-Host "NOTE: If DefaultOutboundAction = Block, then explicit ALLOW rules are needed for VM traffic" -ForegroundColor Yellow

# ====================
# SECTION 10: Internet Connectivity Test
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[10] HOST INTERNET CONNECTIVITY" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow

# DNS test
Write-Host "Testing DNS resolution..." -NoNewline
try {
    $dnsResult = Resolve-DnsName google.com -ErrorAction Stop
    Write-Host " SUCCESS" -ForegroundColor Green
    Write-Host "  Resolved to: $($dnsResult[0].IPAddress)"
} catch {
    Write-Host " FAILED" -ForegroundColor Red
    Write-Host "  Error: $($_.Exception.Message)"
}

# HTTP test
Write-Host "Testing HTTP connectivity to 8.8.8.8:53..." -NoNewline
try {
    $tcpTest = Test-NetConnection -ComputerName 8.8.8.8 -Port 53 -WarningAction SilentlyContinue -ErrorAction Stop
    if ($tcpTest.TcpTestSucceeded) {
        Write-Host " SUCCESS" -ForegroundColor Green
    } else {
        Write-Host " FAILED" -ForegroundColor Red
    }
} catch {
    Write-Host " FAILED" -ForegroundColor Red
    Write-Host "  Error: $($_.Exception.Message)"
}

# ====================
# SECTION 11: Hyper-V Switch Details
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[11] HYPER-V SWITCH CONFIGURATION" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
$switches = Get-VMSwitch -ErrorAction SilentlyContinue
if ($switches) {
    foreach ($switch in $switches) {
        Write-Host "`nSwitch: $($switch.Name)" -ForegroundColor Cyan
        Write-Host "  Type: $($switch.SwitchType)"
        Write-Host "  AllowManagementOS: $($switch.AllowManagementOS)"
        Write-Host "  NetAdapterInterfaceDescription: $($switch.NetAdapterInterfaceDescription)"
    }
} else {
    Write-Host "No Hyper-V switches found (Hyper-V may not be installed)" -ForegroundColor Yellow
}

# ====================
# SECTION 12: Port Proxy (for WSL)
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[12] PORT PROXY CONFIGURATION" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
$portProxies = netsh interface portproxy show all
Write-Host $portProxies

# ====================
# SECTION 13: Services Status
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Yellow
Write-Host "[13] RELEVANT SERVICES" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
$services = @("DNS", "SharedAccess", "RemoteAccess", "mpssvc")
foreach ($svc in $services) {
    $service = Get-Service -Name $svc -ErrorAction SilentlyContinue
    if ($service) {
        $color = if ($service.Status -eq "Running") { "Green" } else { "Yellow" }
        Write-Host "$($svc.PadRight(20)): $($service.Status)" -ForegroundColor $color
    } else {
        Write-Host "$($svc.PadRight(20)): Not Installed" -ForegroundColor DarkGray
    }
}

# ====================
# SUMMARY & RECOMMENDATIONS
# ====================
Write-Host "`n" + "=" * 80 -ForegroundColor Cyan
Write-Host "ANALYSIS & RECOMMENDATIONS" -ForegroundColor Cyan
Write-Host "=" * 80 -ForegroundColor Cyan

$issues = @()

# Check 1: IP Forwarding
$isolationAdapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" } | Select-Object -First 1
if ($isolationAdapter) {
    $fwd = Get-NetIPInterface -InterfaceAlias $isolationAdapter.Name -AddressFamily IPv4
    if ($fwd.Forwarding -ne 'Enabled') {
        $issues += "IP forwarding disabled on IsolationSwitch adapter"
    }
}

# Check 2: Global routing
if ($routing -ne 1) {
    $issues += "Global IP routing disabled (IPEnableRouter = $routing)"
}

# Check 3: Firewall rules
if (-not $autoMutateRules) {
    $issues += "AutoMutate firewall rules missing"
}

# Check 4: Outbound default action
$profiles = Get-NetFirewallProfile
foreach ($profile in $profiles) {
    if ($profile.DefaultOutboundAction -eq "Block") {
        $issues += "Firewall profile '$($profile.Name)' blocks outbound by default (needs explicit ALLOW rules)"
    }
}

if ($issues.Count -eq 0) {
    Write-Host "`n✓ No obvious issues detected" -ForegroundColor Green
    Write-Host "  If VMs still cannot connect, check:" -ForegroundColor White
    Write-Host "    1. VM network adapter configuration" -ForegroundColor White
    Write-Host "    2. VM DNS server settings" -ForegroundColor White
    Write-Host "    3. Physical network connectivity" -ForegroundColor White
} else {
    Write-Host "`n✗ Found $($issues.Count) potential issues:" -ForegroundColor Red
    foreach ($issue in $issues) {
        Write-Host "  - $issue" -ForegroundColor Yellow
    }
}

Write-Host ""
Write-Host "+================================================================+" -ForegroundColor Cyan
Write-Host "|          END OF DIAGNOSTICS                                    |" -ForegroundColor Cyan
Write-Host "+================================================================+" -ForegroundColor Cyan
Write-Host ""
