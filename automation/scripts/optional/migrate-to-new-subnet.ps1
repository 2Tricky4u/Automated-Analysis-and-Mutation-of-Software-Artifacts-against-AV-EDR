<#
.SYNOPSIS
    Migrate from conflicting subnet 192.168.200.0/24 to isolated subnet 10.200.200.0/24

.DESCRIPTION
    This script reconfigures the host networking to use a non-conflicting subnet.
    Run this as Administrator to fix the subnet conflict issue.

.NOTES
    Must be run as Administrator
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

# Check admin
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "[ERROR] This script must be run as Administrator" -ForegroundColor Red
    exit 1
}

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Subnet Migration: 192.168.200.x -> 10.200.200.x      |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

$oldSubnet = "192.168.200.0/24"
$newSubnet = "10.200.200.0/24"
$oldHostIP = "192.168.200.1"
$newHostIP = "10.200.200.1"

# Step 1: Remove old NAT
Write-Host "[1/5] Removing old NAT configuration..." -ForegroundColor Yellow
$oldNat = Get-NetNat -Name "AutoMutateVMNAT" -ErrorAction SilentlyContinue
if ($oldNat) {
    Remove-NetNat -Name "AutoMutateVMNAT" -Confirm:$false
    Write-Host "  Removed old NAT" -ForegroundColor Green
} else {
    Write-Host "  No old NAT found (OK)" -ForegroundColor Green
}

# Step 2: Reconfigure IsolationSwitch IP
Write-Host "`n[2/5] Reconfiguring IsolationSwitch adapter..." -ForegroundColor Yellow
$adapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" } | Select-Object -First 1

if ($adapter) {
    # Remove old IP
    $oldIP = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $oldHostIP -ErrorAction SilentlyContinue
    if ($oldIP) {
        Remove-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $oldHostIP -Confirm:$false -ErrorAction SilentlyContinue
        Write-Host "  Removed old IP: $oldHostIP" -ForegroundColor Green
    }

    # Add new IP
    $newIP = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $newHostIP -ErrorAction SilentlyContinue
    if (-not $newIP) {
        New-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $newHostIP -PrefixLength 24 | Out-Null
        Write-Host "  Configured new IP: $newHostIP/24" -ForegroundColor Green
    } else {
        Write-Host "  New IP already configured: $newHostIP/24" -ForegroundColor Green
    }
} else {
    Write-Host "  ERROR: IsolationSwitch adapter not found!" -ForegroundColor Red
    exit 1
}

# Step 3: Create new NAT
Write-Host "`n[3/5] Creating new NAT configuration..." -ForegroundColor Yellow
$newNat = Get-NetNat -Name "AutoMutateVMNAT" -ErrorAction SilentlyContinue
if (-not $newNat) {
    New-NetNat -Name "AutoMutateVMNAT" -InternalIPInterfaceAddressPrefix $newSubnet | Out-Null
    Write-Host "  Created NAT for $newSubnet" -ForegroundColor Green
} else {
    if ($newNat.InternalIPInterfaceAddressPrefix -eq $newSubnet) {
        Write-Host "  NAT already configured for $newSubnet" -ForegroundColor Green
    } else {
        Write-Host "  WARNING: NAT exists but for wrong subnet: $($newNat.InternalIPInterfaceAddressPrefix)" -ForegroundColor Yellow
        Write-Host "  Please remove it manually: Remove-NetNat -Name AutoMutateVMNAT" -ForegroundColor Yellow
    }
}

# Step 4: Update firewall rules
Write-Host "`n[4/5] Updating firewall rules..." -ForegroundColor Yellow

# Remove old rules
$oldRules = Get-NetFirewallRule -DisplayName "AutoMutate*" -ErrorAction SilentlyContinue
foreach ($rule in $oldRules) {
    $filter = Get-NetFirewallAddressFilter -AssociatedNetFirewallRule $rule -ErrorAction SilentlyContinue
    if ($filter) {
        if ($filter.LocalAddress -like "*192.168.200*" -or $filter.RemoteAddress -like "*192.168.200*") {
            Write-Host "  Removing old rule: $($rule.DisplayName)" -ForegroundColor Gray
            Remove-NetFirewallRule -DisplayName $rule.DisplayName -ErrorAction SilentlyContinue
        }
    }
}

# Create new rules
$rules = @{
    "AutoMutate-VM-Inbound" = @{
        Direction = "Inbound"
        RemoteAddress = $newSubnet
    }
    "AutoMutate-VM-Forward-Internet" = @{
        Direction = "Outbound"
        LocalAddress = $newSubnet
    }
    "AutoMutate-VM-Forward-Responses" = @{
        Direction = "Inbound"
        RemoteAddress = $newSubnet
    }
    "AutoMutate-DNS-Forwarding" = @{
        Direction = "Inbound"
        Protocol = "UDP"
        LocalPort = 53
        RemoteAddress = $newSubnet
    }
}

foreach ($ruleName in $rules.Keys) {
    $existing = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
    if (-not $existing) {
        $ruleParams = $rules[$ruleName]
        $params = @{
            DisplayName = $ruleName
            Direction = $ruleParams.Direction
            Action = "Allow"
            Profile = "Any"
        }
        if ($ruleParams.RemoteAddress) { $params.RemoteAddress = $ruleParams.RemoteAddress }
        if ($ruleParams.LocalAddress) { $params.LocalAddress = $ruleParams.LocalAddress }
        if ($ruleParams.Protocol) { $params.Protocol = $ruleParams.Protocol }
        if ($ruleParams.LocalPort) { $params.LocalPort = $ruleParams.LocalPort }

        New-NetFirewallRule @params | Out-Null
        Write-Host "  Created: $ruleName" -ForegroundColor Green
    }
}

# Step 5: Verification
Write-Host "`n[5/5] Verification..." -ForegroundColor Yellow

$verifyAdapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" } | Select-Object -First 1
$verifyIP = Get-NetIPAddress -InterfaceIndex $verifyAdapter.ifIndex -IPAddress $newHostIP -ErrorAction SilentlyContinue
$verifyNat = Get-NetNat -Name "AutoMutateVMNAT" -ErrorAction SilentlyContinue

Write-Host "`n+================================================================+" -ForegroundColor Green
Write-Host "|          Migration Complete                                    |" -ForegroundColor Green
Write-Host "+================================================================+" -ForegroundColor Green

if ($verifyIP -and $verifyNat) {
    Write-Host "`nHost Configuration:" -ForegroundColor White
    Write-Host "  Interface: $($verifyAdapter.Name)" -ForegroundColor Green
    Write-Host "  IP Address: $newHostIP/24" -ForegroundColor Green
    Write-Host "  NAT Subnet: $newSubnet" -ForegroundColor Green
    Write-Host "  NAT Active: $($verifyNat.Active)" -ForegroundColor Green

    Write-Host "`nNext Steps:" -ForegroundColor Cyan
    Write-Host "  1. Reconfigure existing VMs with new IPs:" -ForegroundColor White
    Write-Host "     win10-worker-01: 10.200.200.100" -ForegroundColor Yellow
    Write-Host "     win11-worker-01: 10.200.200.110" -ForegroundColor Yellow
    Write-Host "     win11-worker-02: 10.200.200.111" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  2. From inside each VM, run:" -ForegroundColor White
    Write-Host "     Remove-NetIPAddress -IPAddress 192.168.200.* -Confirm:`$false" -ForegroundColor Cyan
    Write-Host "     New-NetIPAddress -InterfaceAlias Ethernet -IPAddress 10.200.200.100 -PrefixLength 24 -DefaultGateway 10.200.200.1" -ForegroundColor Cyan
    Write-Host "     Set-DnsClientServerAddress -InterfaceAlias Ethernet -ServerAddresses 10.200.200.1,8.8.8.8,8.8.4.4" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  3. Test connectivity:" -ForegroundColor White
    Write-Host "     Resolve-DnsName google.com" -ForegroundColor Cyan
    Write-Host ""
} else {
    Write-Host "`nERROR: Migration incomplete!" -ForegroundColor Red
    Write-Host "  Verify IP: $($null -ne $verifyIP)" -ForegroundColor Yellow
    Write-Host "  Verify NAT: $($null -ne $verifyNat)" -ForegroundColor Yellow
}

Write-Host "+================================================================+`n" -ForegroundColor Green
