<#
.SYNOPSIS
    Fix boot order for VMs that won't boot from ISO

.PARAMETER VMName
    Name of the VM to fix
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName
)

$ErrorActionPreference = "Stop"

Write-Host "`n=== Fixing Boot Order for $VMName ===" -ForegroundColor Cyan

# Check VM exists
$vm = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if (-not $vm) {
    Write-Error "VM not found: $VMName"
    exit 1
}

# Get current state
Write-Host "`n[Current Configuration]" -ForegroundColor Yellow
$firmware = Get-VMFirmware -VMName $VMName
Write-Host "  Secure Boot: $($firmware.SecureBoot)" -ForegroundColor White
Write-Host "  Current Boot Order:" -ForegroundColor White
$firmware.BootOrder | ForEach-Object {
    Write-Host "    - $($_.BootType): $($_.Device)" -ForegroundColor Gray
}

# Get DVD and HDD
$dvd = Get-VMDvdDrive -VMName $VMName
$hdd = Get-VMHardDiskDrive -VMName $VMName

if (-not $dvd) {
    Write-Error "No DVD drive found on VM"
    exit 1
}

Write-Host "`n[Applying Fix]" -ForegroundColor Cyan

# Method 1: Try setting DVD as FirstBootDevice (simpler, what Hyper-V Manager does)
try {
    Write-Host "  Trying FirstBootDevice method..." -ForegroundColor White
    Set-VMFirmware -VMName $VMName -FirstBootDevice $dvd
    Write-Success "  Set DVD as first boot device"
} catch {
    Write-Warning "  FirstBootDevice method failed: $_"

    # Method 2: Fallback to explicit BootOrder
    Write-Host "  Trying BootOrder method..." -ForegroundColor White
    if ($hdd) {
        Set-VMFirmware -VMName $VMName -BootOrder $dvd, $hdd
    } else {
        Set-VMFirmware -VMName $VMName -BootOrder $dvd
    }
    Write-Host "  Set explicit boot order (DVD, HDD)" -ForegroundColor Green
}

# Verify
Write-Host "`n[New Configuration]" -ForegroundColor Green
$firmware = Get-VMFirmware -VMName $VMName
Write-Host "  New Boot Order:" -ForegroundColor White
$firmware.BootOrder | ForEach-Object {
    Write-Host "    - $($_.BootType): $($_.Device)" -ForegroundColor Gray
}

$firstDevice = $firmware.BootOrder[0]
if ($firstDevice.Device -like "*DVD*" -or $firstDevice.BootType -eq "Drive") {
    Write-Host "`n[SUCCESS] DVD is now first boot device!" -ForegroundColor Green
} else {
    Write-Warning "DVD may not be first boot device. Check output above."
}

Write-Host "`nNext: Start the VM and verify it boots from ISO`n" -ForegroundColor Cyan

function Write-Success { param($M) Write-Host $M -ForegroundColor Green }
