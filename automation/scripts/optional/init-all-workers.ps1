<#
.SYNOPSIS
    Initialize all Worker VMs after Windows installation

.DESCRIPTION
    Runs 04-vm-init.ps1 on all VMs defined in config.yaml

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.EXAMPLE
    .\init-all-workers.ps1
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$ConfigPath = "..\config.yaml"
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

Write-Host "`n=== Worker VM Initialization ===" -ForegroundColor Cyan

# Parse config
$ConfigContent = Get-Content $ConfigPath -Raw
$WorkersSection = ($ConfigContent -split 'workers:')[1] -split 'storage:' | Select-Object -First 1
$WorkerMatches = [regex]::Matches($WorkersSection, '- name:\s*"([^"]+)"[\s\S]*?os:\s*"([^"]+)"[\s\S]*?ip:\s*"([^"]+)"')

$workers = @()
foreach ($match in $WorkerMatches) {
    $workers += @{
        Name = $match.Groups[1].Value
        Os = $match.Groups[2].Value
        IP = $match.Groups[3].Value
    }
}

Write-Info "Found $($workers.Count) worker(s) in configuration"

# Get credentials once (reuse for all VMs)
Write-Info "Enter worker VM credentials (default: worker-admin / AutoMutate!Password)"
$cred = Get-Credential -UserName "worker-admin" -Message "Enter password for worker VMs"

$InitScript = Join-Path $PSScriptRoot "04-vm-init.ps1"
if (-not (Test-Path $InitScript)) {
    Write-Error "Init script not found: $InitScript"
    exit 1
}

# Initialize each VM
$successCount = 0
$failedVMs = @()

foreach ($worker in $workers) {
    Write-Host "`n--- Initializing $($worker.Name) ---" -ForegroundColor Yellow

    # Check if VM is running
    $vm = Get-VM -Name $worker.Name -ErrorAction SilentlyContinue
    if (-not $vm) {
        Write-Warn "VM not found: $($worker.Name), skipping..."
        $failedVMs += $worker.Name
        continue
    }

    if ($vm.State -ne "Running") {
        Write-Warn "VM $($worker.Name) is not running (state: $($vm.State))"
        Write-Info "Starting VM..."
        Start-VM -Name $worker.Name
        Start-Sleep -Seconds 10
    }

    try {
        Invoke-Command -VMName $worker.Name -FilePath $InitScript `
            -ArgumentList $worker.IP, $worker.Name -Credential $cred -ErrorAction Stop

        Write-Success "✓ $($worker.Name) initialized successfully"
        $successCount++
    } catch {
        Write-Warn "✗ Failed to initialize $($worker.Name): $_"
        $failedVMs += $worker.Name
    }
}

# Summary
Write-Host "`n=== Initialization Summary ===" -ForegroundColor Cyan
Write-Info "Total VMs: $($workers.Count)"
Write-Success "Successful: $successCount"

if ($failedVMs.Count -gt 0) {
    Write-Warn "Failed: $($failedVMs.Count)"
    Write-Host "Failed VMs:" -ForegroundColor Yellow
    foreach ($vmName in $failedVMs) {
        Write-Host "  - $vmName" -ForegroundColor Red
    }

    Write-Host "`nTo retry failed VMs:" -ForegroundColor Cyan
    foreach ($vmName in $failedVMs) {
        $workerInfo = $workers | Where-Object { $_.Name -eq $vmName }
        Write-Host "`$cred = Get-Credential -UserName 'worker-admin'" -ForegroundColor Gray
        Write-Host "Invoke-Command -VMName '$vmName' -FilePath '.\04-vm-init.ps1' -ArgumentList '$($workerInfo.IP)', '$vmName' -Credential `$cred" -ForegroundColor Gray
        Write-Host ""
    }
} else {
    Write-Success "All VMs initialized successfully!"
}

if ($successCount -gt 0) {
    Write-Host "`nNext steps:" -ForegroundColor Green
    Write-Host "  1. Create baseline checkpoints:" -ForegroundColor Cyan
    Write-Host "     .\create-all-baselines.ps1" -ForegroundColor Gray
    Write-Host ""
    Write-Host "  2. Disable NAT for lab isolation (RECOMMENDED):" -ForegroundColor Cyan
    Write-Host "     .\disable-nat.ps1" -ForegroundColor Gray
    Write-Host ""
    Write-Info "⚠️  NAT is currently ENABLED - VMs can access the internet"
    Write-Info "   For a confined lab, disable NAT after creating baselines"
}

exit 0
