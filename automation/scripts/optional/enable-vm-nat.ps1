<#
.SYNOPSIS
    Enable NAT for VMs without conflicting with WSL

.DESCRIPTION
    Creates a NetNat configuration specifically for the VM subnet (192.168.200.0/24)
    This allows VMs to access internet by translating their private IPs to the host's IP.

.NOTES
    Must be run as Administrator
    Safe approach: Use specific subnet, not wildcard NAT
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

# Check admin
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "[ERROR] This script must be run as Administrator" -ForegroundColor Red
    exit 1
}

Write-Host "`n=== Enabling NAT for VM Subnet ===" -ForegroundColor Cyan

$vmSubnet = "192.168.200.0/24"
$natName = "AutoMutateVMNAT"

# Check for existing NAT
$existingNat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue

if ($existingNat) {
    Write-Host "[INFO] NAT '$natName' already exists" -ForegroundColor Yellow
    Write-Host "  Subnet: $($existingNat.InternalIPInterfaceAddressPrefix)" -ForegroundColor White

    if ($existingNat.InternalIPInterfaceAddressPrefix -ne $vmSubnet) {
        Write-Host "[WARN] NAT subnet mismatch - removing and recreating..." -ForegroundColor Yellow
        Remove-NetNat -Name $natName -Confirm:$false
        $existingNat = $null
    } else {
        Write-Host "[OK] NAT configuration is correct" -ForegroundColor Green
    }
}

if (-not $existingNat) {
    Write-Host "[INFO] Creating new NAT: $natName for subnet $vmSubnet" -ForegroundColor Cyan

    try {
        New-NetNat -Name $natName -InternalIPInterfaceAddressPrefix $vmSubnet | Out-Null
        Write-Host "[OK] NAT created successfully!" -ForegroundColor Green
    } catch {
        Write-Host "[ERROR] Failed to create NAT: $($_.Exception.Message)" -ForegroundColor Red
        Write-Host "" -ForegroundColor White
        Write-Host "Possible causes:" -ForegroundColor Yellow
        Write-Host "  1. Another NAT already exists (only ONE NAT allowed per subnet)" -ForegroundColor White
        Write-Host "  2. Subnet overlaps with existing NAT" -ForegroundColor White
        Write-Host "" -ForegroundColor White
        Write-Host "Check existing NATs:" -ForegroundColor Yellow
        Write-Host "  Get-NetNat" -ForegroundColor Cyan
        Write-Host "" -ForegroundColor White
        exit 1
    }
}

# Verify
Write-Host "`n=== Verification ===" -ForegroundColor Cyan
$allNats = Get-NetNat
Write-Host "All NAT configurations:" -ForegroundColor Yellow
$allNats | Format-Table Name, InternalIPInterfaceAddressPrefix, Active -AutoSize

# Check for WSL NAT
$wslNat = $allNats | Where-Object { $_.InternalIPInterfaceAddressPrefix -like "*172.*" }
if ($wslNat) {
    Write-Host "[OK] WSL NAT detected: $($wslNat.InternalIPInterfaceAddressPrefix)" -ForegroundColor Green
    Write-Host "     WSL and VM NATs coexist without conflict" -ForegroundColor White
}

Write-Host "`n=== Next Steps ===" -ForegroundColor Cyan
Write-Host "1. Test connectivity from VM:" -ForegroundColor White
Write-Host "   Resolve-DnsName google.com" -ForegroundColor Yellow
Write-Host "   Test-NetConnection google.com -Port 443" -ForegroundColor Yellow
Write-Host ""
Write-Host "2. If still failing, check:" -ForegroundColor White
Write-Host "   - VM has correct gateway (192.168.200.1)" -ForegroundColor White
Write-Host "   - VM has DNS servers configured" -ForegroundColor White
Write-Host "   - Firewall rules allow forwarded traffic" -ForegroundColor White
Write-Host ""
