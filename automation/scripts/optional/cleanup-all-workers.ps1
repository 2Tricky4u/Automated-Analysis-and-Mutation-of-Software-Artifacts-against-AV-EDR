<#
.SYNOPSIS
    Clean up all worker VMs

.DESCRIPTION
    Removes all worker VMs and their VHDs to start fresh
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

Write-Host "`n=== Cleaning Up All Worker VMs ===" -ForegroundColor Cyan

$workers = @("win10-worker-01", "win11-worker-01", "win11-worker-02")

foreach ($vmName in $workers) {
    Write-Host "`n[INFO] Processing: $vmName" -ForegroundColor Cyan

    # Stop and remove VM
    $vm = Get-VM -Name $vmName -ErrorAction SilentlyContinue
    if ($vm) {
        if ($vm.State -ne 'Off') {
            Write-Host "  Stopping VM..." -ForegroundColor Yellow
            Stop-VM -Name $vmName -Force -TurnOff
            Start-Sleep -Seconds 2
        }
        Write-Host "  Removing VM..." -ForegroundColor Yellow
        Remove-VM -Name $vmName -Force
        Start-Sleep -Seconds 2
        Write-Host "  [OK] VM removed" -ForegroundColor Green
    } else {
        Write-Host "  [SKIP] VM not found" -ForegroundColor Gray
    }

    # Remove VHD
    $vhdPath = "C:\HyperV\VHDs\$vmName.vhdx"
    if (Test-Path $vhdPath) {
        Write-Host "  Removing VHD..." -ForegroundColor Yellow
        try {
            Remove-Item $vhdPath -Force -ErrorAction Stop
            Write-Host "  [OK] VHD removed" -ForegroundColor Green
        } catch {
            Write-Host "  [WARN] VHD locked, will retry after service restart" -ForegroundColor Yellow
            $script:lockedVhds += @($vhdPath)
        }
    } else {
        Write-Host "  [SKIP] VHD not found" -ForegroundColor Gray
    }
}

# Restart Hyper-V service if any VHDs were locked
if ($script:lockedVhds) {
    Write-Host "`n[INFO] Restarting Hyper-V service to release locked VHDs..." -ForegroundColor Cyan
    Stop-Service vmms -Force
    Start-Sleep -Seconds 3
    Start-Service vmms
    Start-Sleep -Seconds 2

    foreach ($vhdPath in $script:lockedVhds) {
        Write-Host "  Retrying: $vhdPath" -ForegroundColor Yellow
        try {
            Remove-Item $vhdPath -Force -ErrorAction Stop
            Write-Host "  [OK] VHD removed" -ForegroundColor Green
        } catch {
            Write-Host "  [ERROR] Still locked: $_" -ForegroundColor Red
        }
    }
}

Write-Host "`n=== Cleanup Complete ===" -ForegroundColor Green
Write-Host "You can now re-run: .\setup-all.ps1`n" -ForegroundColor Cyan
