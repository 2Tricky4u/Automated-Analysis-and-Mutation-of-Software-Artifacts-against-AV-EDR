<#
.SYNOPSIS
    Disable NAT to isolate lab VMs from internet

.DESCRIPTION
    Removes NAT configuration to ensure VMs cannot access external networks.
    Run this after completing VM initialization to enforce lab isolation.

.PARAMETER NatName
    Name of the NAT to remove (default: IsolationNAT)

.EXAMPLE
    .\disable-nat.ps1
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$NatName = "IsolationNAT"
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

Write-Host "`n=== Disabling NAT for Lab Isolation ===" -ForegroundColor Cyan

# Check if NAT exists
$nat = Get-NetNat -Name $NatName -ErrorAction SilentlyContinue

if (-not $nat) {
    Write-Info "NAT '$NatName' is not configured (already isolated)"
    exit 0
}

Write-Info "Current NAT configuration:"
Write-Host "  Name: $($nat.Name)" -ForegroundColor White
Write-Host "  Subnet: $($nat.InternalIPInterfaceAddressPrefix)" -ForegroundColor White

Write-Warn "Removing NAT will prevent VMs from accessing the internet"
Write-Host "VMs will only be able to communicate with:"
Write-Host "  - Host (192.168.200.1)" -ForegroundColor Yellow
Write-Host "  - Other VMs on IsolationSwitch (192.168.200.0/24)" -ForegroundColor Yellow
Write-Host "  - WSL2 controller (via host)" -ForegroundColor Yellow

$confirm = Read-Host "`nAre you sure you want to disable NAT? (yes/no)"

if ($confirm -ne "yes") {
    Write-Info "Operation cancelled"
    exit 0
}

# Remove NAT
Remove-NetNat -Name $NatName -Confirm:$false
Write-Success "NAT disabled: $NatName"

Write-Info "Lab network is now fully isolated"
Write-Info "To re-enable internet access for updates, run: .\01-setup-nat.ps1"

exit 0
