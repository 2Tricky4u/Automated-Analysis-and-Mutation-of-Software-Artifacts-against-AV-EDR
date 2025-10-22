<#
.SYNOPSIS
    Enable ICMP (ping) responses on the IsolationSwitch adapter

.DESCRIPTION
    Creates firewall rules to allow VMs to ping the host gateway.
    This is optional - VMs can function without ping, but it's useful for diagnostics.
#>

$ErrorActionPreference = "Stop"

Write-Host "`n=== Enabling ICMP (Ping) for VMs ===" -ForegroundColor Cyan

# Allow ICMPv4 Echo Request (ping) on IsolationSwitch
$icmpRule = "AutoMutate-Allow-ICMP-Echo"

$existingRule = Get-NetFirewallRule -DisplayName $icmpRule -ErrorAction SilentlyContinue

if ($existingRule) {
    Write-Host "[OK] ICMP rule already exists: $icmpRule" -ForegroundColor Green
} else {
    # Get the IsolationSwitch adapter
    $adapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" } | Select-Object -First 1

    if ($adapter) {
        # Create inbound rule for ICMPv4 Echo Request
        New-NetFirewallRule -DisplayName $icmpRule `
            -Direction Inbound `
            -Action Allow `
            -Protocol ICMPv4 `
            -IcmpType 8 `
            -InterfaceAlias $adapter.Name `
            -RemoteAddress 192.168.200.0/24 `
            -Profile Any | Out-Null

        Write-Host "[OK] Created ICMP firewall rule: $icmpRule" -ForegroundColor Green
        Write-Host "     VMs can now ping the gateway (192.168.200.1)" -ForegroundColor White
    } else {
        Write-Host "[ERROR] IsolationSwitch adapter not found!" -ForegroundColor Red
        exit 1
    }
}

Write-Host "`nTest from VM with: Test-Connection -ComputerName 192.168.200.1 -Count 2`n" -ForegroundColor Yellow
