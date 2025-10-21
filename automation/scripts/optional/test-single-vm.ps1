<#
.SYNOPSIS
    Test creating a single VM to verify the fix

.PARAMETER VMName
    Name of the VM to test (default: win10-worker-01)
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$VMName = "win10-worker-01"
)

$ErrorActionPreference = "Stop"

Write-Host "`n=== Testing VM Creation: $VMName ===" -ForegroundColor Cyan

# Load config
$ConfigPath = "..\config.yaml"
$ConfigContent = Get-Content $ConfigPath -Raw
$WorkersSection = ($ConfigContent -split 'workers:')[1] -split 'storage:' | Select-Object -First 1
$WorkerMatches = [regex]::Matches($WorkersSection, '- name:\s*"([^"]+)"[\s\S]*?os:\s*"([^"]+)"[\s\S]*?iso_path:\s*"([^"]+)"[\s\S]*?ip:\s*"([^"]+)"')

$worker = $null
foreach ($match in $WorkerMatches) {
    if ($match.Groups[1].Value -eq $VMName) {
        $worker = @{
            Name = $match.Groups[1].Value
            Os = $match.Groups[2].Value
            IsoPath = $match.Groups[3].Value
            IP = $match.Groups[4].Value
        }
        break
    }
}

if (-not $worker) {
    Write-Error "Worker '$VMName' not found in config.yaml"
    exit 1
}

Write-Host "Found worker config:" -ForegroundColor Green
Write-Host "  Name:    $($worker.Name)" -ForegroundColor White
Write-Host "  OS:      $($worker.Os)" -ForegroundColor White
Write-Host "  ISO:     $($worker.IsoPath)" -ForegroundColor White
Write-Host "  IP:      $($worker.IP)" -ForegroundColor White

# Cleanup existing VM
Write-Host "`n[1/2] Cleaning up existing VM..." -ForegroundColor Cyan
.\cleanup-vm.ps1 -VMName $worker.Name

# Create VM
Write-Host "`n[2/2] Creating VM..." -ForegroundColor Cyan
.\03-create-worker-vm.ps1 `
    -WorkerName $worker.Name `
    -Os $worker.Os `
    -IsoPath $worker.IsoPath `
    -StaticIP $worker.IP `
    -ConfigPath $ConfigPath

Write-Host "`n=== Test Complete ===" -ForegroundColor Green
Write-Host "If you see 'Configured firmware' above, the fix worked!" -ForegroundColor Green
Write-Host "Next: Start the VM in Hyper-V Manager and verify it boots from ISO`n" -ForegroundColor Cyan
