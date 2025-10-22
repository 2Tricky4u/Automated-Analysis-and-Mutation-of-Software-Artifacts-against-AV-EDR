<#
.SYNOPSIS
    Diagnose host network configuration for VM connectivity

.DESCRIPTION
    Checks IP forwarding, firewall rules, and connectivity to help troubleshoot VM network issues
#>

$ErrorActionPreference = "Continue"

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Host Network Diagnostics                              |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

# 1. Check if IsolationSwitch adapter exists and has IP
Write-Host "[1] IsolationSwitch Adapter:" -ForegroundColor Yellow
$adapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" }
if ($adapter) {
    Write-Host "  Name: $($adapter.Name)" -ForegroundColor White
    Write-Host "  Status: $($adapter.Status)" -ForegroundColor $(if ($adapter.Status -eq 'Up') { 'Green' } else { 'Red' })

    $ip = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue
    if ($ip) {
        Write-Host "  IP Address: $($ip.IPAddress)/$($ip.PrefixLength)" -ForegroundColor Green
    } else {
        Write-Host "  ERROR: No IP address assigned!" -ForegroundColor Red
        Write-Host "  FIX: Run .\01-host-setup.ps1 to configure IP" -ForegroundColor Yellow
    }
} else {
    Write-Host "  ERROR: IsolationSwitch adapter not found!" -ForegroundColor Red
    Write-Host "  FIX: Run .\01-host-setup.ps1 to create the switch" -ForegroundColor Yellow
}

# 2. Check IP forwarding on the adapter
Write-Host "`n[2] IP Forwarding Status:" -ForegroundColor Yellow
if ($adapter) {
    $forwarding = Get-NetIPInterface -InterfaceAlias $adapter.Name -AddressFamily IPv4 -ErrorAction SilentlyContinue
    if ($forwarding) {
        Write-Host "  Interface Forwarding: $($forwarding.Forwarding)" -ForegroundColor $(if ($forwarding.Forwarding -eq 'Enabled') { 'Green' } else { 'Red' })
        if ($forwarding.Forwarding -ne 'Enabled') {
            Write-Host "  FIX: Set-NetIPInterface -InterfaceAlias '$($adapter.Name)' -Forwarding Enabled" -ForegroundColor Yellow
        }
    } else {
        Write-Host "  ERROR: Cannot query forwarding status" -ForegroundColor Red
    }
} else {
    Write-Host "  Cannot check - adapter not found" -ForegroundColor Red
}

# 3. Check global IP routing
Write-Host "`n[3] Global IP Routing:" -ForegroundColor Yellow
$routingKey = "HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters"
$routing = (Get-ItemProperty -Path $routingKey -Name "IPEnableRouter" -ErrorAction SilentlyContinue).IPEnableRouter
if ($routing -eq 1) {
    Write-Host "  IPEnableRouter: Enabled (1)" -ForegroundColor Green
} else {
    Write-Host "  IPEnableRouter: Disabled ($routing)" -ForegroundColor Red
    Write-Host "  WARNING: Requires REBOOT to take effect!" -ForegroundColor Yellow
    Write-Host "  FIX: Set-ItemProperty -Path '$routingKey' -Name IPEnableRouter -Value 1; Restart-Computer" -ForegroundColor Yellow
}

# 4. Check firewall rules
Write-Host "`n[4] Firewall Rules:" -ForegroundColor Yellow
$rules = @{
    "AutoMutate-VM-Inbound" = Get-NetFirewallRule -DisplayName "AutoMutate-VM-Inbound" -ErrorAction SilentlyContinue
    "AutoMutate-VM-Outbound" = Get-NetFirewallRule -DisplayName "AutoMutate-VM-Outbound" -ErrorAction SilentlyContinue
    "AutoMutate-DNS-Forwarding" = Get-NetFirewallRule -DisplayName "AutoMutate-DNS-Forwarding" -ErrorAction SilentlyContinue
}

foreach ($ruleName in $rules.Keys) {
    $rule = $rules[$ruleName]
    if ($rule) {
        $status = if ($rule.Enabled -eq 'True') { "Enabled" } else { "Disabled" }
        $color = if ($rule.Enabled -eq 'True') { 'Green' } else { 'Yellow' }
        Write-Host "  $ruleName : $status" -ForegroundColor $color
    } else {
        Write-Host "  $ruleName : NOT FOUND" -ForegroundColor Red
    }
}

if (-not $rules["AutoMutate-VM-Inbound"] -or -not $rules["AutoMutate-VM-Outbound"]) {
    Write-Host "  FIX: Run .\01-host-setup.ps1 to create firewall rules" -ForegroundColor Yellow
}

# 5. Check if host can reach internet
Write-Host "`n[5] Host Internet Connectivity:" -ForegroundColor Yellow
$pingResult = Test-Connection -ComputerName 8.8.8.8 -Count 2 -Quiet -ErrorAction SilentlyContinue
if ($pingResult) {
    Write-Host "  Ping 8.8.8.8: Success" -ForegroundColor Green
} else {
    Write-Host "  Ping 8.8.8.8: Failed" -ForegroundColor Red
    Write-Host "  WARNING: Host cannot reach internet - VMs won't either!" -ForegroundColor Yellow
}

# 6. Check DNS configuration
Write-Host "`n[6] Host DNS Configuration:" -ForegroundColor Yellow
$dnsServers = Get-DnsClientServerAddress -AddressFamily IPv4 |
    Where-Object { $_.ServerAddresses.Count -gt 0 -and $_.InterfaceAlias -notlike "*IsolationSwitch*" } |
    Select-Object -First 1

if ($dnsServers) {
    Write-Host "  Interface: $($dnsServers.InterfaceAlias)" -ForegroundColor White
    Write-Host "  DNS Servers: $($dnsServers.ServerAddresses -join ', ')" -ForegroundColor Green
} else {
    Write-Host "  WARNING: No DNS servers configured!" -ForegroundColor Yellow
}

# 7. Check NAT conflicts (should be none if using IP forwarding)
Write-Host "`n[7] NAT Configuration (should be WSL only):" -ForegroundColor Yellow
$allNats = Get-NetNat -ErrorAction SilentlyContinue
if ($allNats) {
    foreach ($nat in $allNats) {
        $color = if ($nat.InternalIPInterfaceAddressPrefix -like "*192.168.200*") { 'Red' } else { 'Green' }
        Write-Host "  $($nat.Name): $($nat.InternalIPInterfaceAddressPrefix)" -ForegroundColor $color
        if ($nat.Name -eq "IsolationNAT") {
            Write-Host "    ERROR: IsolationNAT found - conflicts with WSL!" -ForegroundColor Red
            Write-Host "    FIX: Remove-NetNat -Name 'IsolationNAT' -Confirm:`$false" -ForegroundColor Yellow
        }
    }
} else {
    Write-Host "  No NAT configurations found (good - using IP forwarding)" -ForegroundColor Green
}

# Summary
Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Summary                                               |" -ForegroundColor Cyan
Write-Host "+================================================================+" -ForegroundColor Cyan

$issues = @()
if (-not $adapter) { $issues += "IsolationSwitch adapter missing" }
if ($adapter -and -not $ip) { $issues += "IsolationSwitch has no IP address" }
if ($adapter -and $forwarding -and $forwarding.Forwarding -ne 'Enabled') { $issues += "IP forwarding disabled on adapter" }
if ($routing -ne 1) { $issues += "Global IP routing disabled (needs reboot)" }
if (-not $rules["AutoMutate-VM-Inbound"]) { $issues += "Firewall rules missing" }
if (-not $pingResult) { $issues += "Host cannot reach internet" }
if ($allNats | Where-Object { $_.Name -eq "IsolationNAT" }) { $issues += "IsolationNAT conflict with WSL" }

if ($issues.Count -eq 0) {
    Write-Host "`n  All checks passed!" -ForegroundColor Green
    Write-Host "  If VM still cannot ping gateway, try:" -ForegroundColor White
    Write-Host "    1. Restart the VM (network stack may need reset)" -ForegroundColor White
    Write-Host "    2. Check Windows Firewall on the host is not blocking ICMP" -ForegroundColor White
} else {
    Write-Host "`n  Found $($issues.Count) issue(s):" -ForegroundColor Yellow
    foreach ($issue in $issues) {
        Write-Host "    - $issue" -ForegroundColor Red
    }
    Write-Host "`n  Recommended action:" -ForegroundColor Yellow
    Write-Host "    1. Run: .\01-host-setup.ps1 -ConfigPath ..\config.yaml" -ForegroundColor Cyan
    Write-Host "    2. Reboot host if IPEnableRouter was changed" -ForegroundColor Cyan
    Write-Host "    3. Re-run this diagnostic script to verify" -ForegroundColor Cyan
}

Write-Host ""
