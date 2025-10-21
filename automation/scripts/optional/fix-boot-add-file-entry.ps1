<#
.SYNOPSIS
    Add UEFI File boot entry to match working VM configuration

.PARAMETER VMName
    Name of the VM to fix
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName
)

$ErrorActionPreference = "Stop"

Write-Host "`n=== Adding UEFI File Boot Entry to $VMName ===" -ForegroundColor Cyan

# Get current firmware
$firmware = Get-VMFirmware -VMName $VMName
Write-Host "`n[Current Boot Order]" -ForegroundColor Yellow
$firmware.BootOrder | ForEach-Object {
    Write-Host "  $($_.BootType) - $($_.Device)" -ForegroundColor Gray
}

# Check if File entry already exists
$hasFileEntry = $firmware.BootOrder | Where-Object { $_.BootType -eq 'File' }
if ($hasFileEntry) {
    Write-Host "`n[INFO] VM already has a File boot entry" -ForegroundColor Green
    exit 0
}

Write-Host "`n[WARNING] VM is missing UEFI File boot entry" -ForegroundColor Yellow
Write-Host "This is required for some ISOs to boot properly." -ForegroundColor Yellow

# Get all boot devices
$dvdDrive = Get-VMDvdDrive -VMName $VMName
$hddDrive = Get-VMHardDiskDrive -VMName $VMName
$netAdapter = Get-VMNetworkAdapter -VMName $VMName

# Unfortunately, PowerShell doesn't provide a direct way to ADD a File boot entry
# The only way is to:
# 1. Remove the VM and recreate it, OR
# 2. Use Hyper-V Manager GUI to add it manually

Write-Host "`n[SOLUTION 1] Manually add via Hyper-V Manager:" -ForegroundColor Cyan
Write-Host "  1. Open Hyper-V Manager" -ForegroundColor White
Write-Host "  2. Right-click $VMName → Settings" -ForegroundColor White
Write-Host "  3. Go to Firmware → Boot Order" -ForegroundColor White
Write-Host "  4. Click 'Add' → Select 'File' → Click OK" -ForegroundColor White
Write-Host "  5. Move File entry to the TOP of the list" -ForegroundColor White
Write-Host "  6. Apply and OK" -ForegroundColor White

Write-Host "`n[SOLUTION 2] Recreate VM with our updated script:" -ForegroundColor Cyan
Write-Host "  cd .." -ForegroundColor White
Write-Host "  .\scripts\cleanup-vm.ps1 -VMName $VMName" -ForegroundColor White
Write-Host "  .\setup-all.ps1" -ForegroundColor White

Write-Host "`n"
