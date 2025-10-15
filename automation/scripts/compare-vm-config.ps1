<#
.SYNOPSIS
    Compare VM configurations to diagnose boot issues

.PARAMETER WorkingVM
    Name of a working VM you created manually

.PARAMETER BrokenVM
    Name of a VM created by our script that won't boot
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$WorkingVM,

    [Parameter(Mandatory)]
    [string]$BrokenVM
)

$ErrorActionPreference = "Stop"

Write-Host "`n=== VM Configuration Comparison ===" -ForegroundColor Cyan

function Get-VMConfig {
    param([string]$VMName)

    $vm = Get-VM -Name $VMName -ErrorAction Stop
    $firmware = Get-VMFirmware -VMName $VMName
    $dvd = Get-VMDvdDrive -VMName $VMName
    $hdd = Get-VMHardDiskDrive -VMName $VMName

    return @{
        Name = $VMName
        State = $vm.State
        Generation = $vm.Generation
        SecureBoot = $firmware.SecureBoot
        SecureBootTemplate = $firmware.SecureBootTemplate
        BootOrder = ($firmware.BootOrder | ForEach-Object { "$($_.BootType):$($_.Device)" } ) -join ", "
        FirstBootDevice = if ($firmware.BootOrder.Count -gt 0) { $firmware.BootOrder[0].Device } else { "None" }
        DVDPath = $dvd.Path
        DVDAttached = ($dvd -ne $null)
        HDDPath = $hdd.Path
    }
}

Write-Host "`n[Working VM: $WorkingVM]" -ForegroundColor Green
$working = Get-VMConfig -VMName $WorkingVM
$working.GetEnumerator() | Sort-Object Name | ForEach-Object {
    Write-Host "  $($_.Key): $($_.Value)" -ForegroundColor White
}

Write-Host "`n[Broken VM: $BrokenVM]" -ForegroundColor Red
$broken = Get-VMConfig -VMName $BrokenVM
$broken.GetEnumerator() | Sort-Object Name | ForEach-Object {
    Write-Host "  $($_.Key): $($_.Value)" -ForegroundColor White
}

Write-Host "`n=== Differences ===" -ForegroundColor Magenta
$differences = @()
foreach ($key in $working.Keys) {
    if ($working[$key] -ne $broken[$key]) {
        $differences += @{
            Setting = $key
            Working = $working[$key]
            Broken = $broken[$key]
        }
        Write-Host "  $key" -ForegroundColor Yellow
        Write-Host "    Working: $($working[$key])" -ForegroundColor Green
        Write-Host "    Broken:  $($broken[$key])" -ForegroundColor Red
    }
}

if ($differences.Count -eq 0) {
    Write-Host "  No differences found! Issue may be elsewhere." -ForegroundColor Yellow
}

Write-Host "`n=== Boot Order Details ===" -ForegroundColor Cyan
Write-Host "`nWorking VM Boot Order:" -ForegroundColor Green
$workingFirmware = Get-VMFirmware -VMName $WorkingVM
$workingFirmware.BootOrder | ForEach-Object {
    Write-Host "  $($_.BootType) - $($_.Device)" -ForegroundColor White
}

Write-Host "`nBroken VM Boot Order:" -ForegroundColor Red
$brokenFirmware = Get-VMFirmware -VMName $BrokenVM
$brokenFirmware.BootOrder | ForEach-Object {
    Write-Host "  $($_.BootType) - $($_.Device)" -ForegroundColor White
}

Write-Host "`n"
