<#
.SYNOPSIS
    Clean up partially created VMs

.PARAMETER VMName
    Name of VM to remove (e.g., "win10-worker-01")

.EXAMPLE
    .\cleanup-vm.ps1 -VMName "win10-worker-01"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName
)

$ErrorActionPreference = "Stop"

Write-Host "[INFO] Cleaning up VM: $VMName" -ForegroundColor Cyan

# Stop VM if running
$vm = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if ($vm) {
    if ($vm.State -ne 'Off') {
        Write-Host "[INFO] Stopping VM (state: $($vm.State))..." -ForegroundColor Cyan
        Stop-VM -Name $VMName -Force -TurnOff
        Start-Sleep -Seconds 2  # Wait for clean shutdown
    }

    # Remove VM
    Write-Host "[INFO] Removing VM..." -ForegroundColor Cyan
    Remove-VM -Name $VMName -Force
    Write-Host "[OK] VM removed" -ForegroundColor Green

    # Wait for Hyper-V to release VHD lock
    Start-Sleep -Seconds 3
} else {
    Write-Host "[WARN] VM not found: $VMName" -ForegroundColor Yellow
}

# Remove VHD (enabled by default)
$VhdPath = "C:\HyperV\VHDs\$VMName.vhdx"
if (Test-Path $VhdPath) {
    Write-Host "[INFO] Removing VHD: $VhdPath" -ForegroundColor Cyan
    try {
        Remove-Item $VhdPath -Force -ErrorAction Stop
        Write-Host "[OK] VHD removed" -ForegroundColor Green
    } catch {
        Write-Host "[WARN] Could not remove VHD (may be locked): $_" -ForegroundColor Yellow
        Write-Host "[INFO] Try running: Stop-Service vmms; Start-Service vmms" -ForegroundColor Cyan
    }
}

Write-Host "[OK] Cleanup complete" -ForegroundColor Green
