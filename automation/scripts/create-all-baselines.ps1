<#
.SYNOPSIS
    Create baseline checkpoints for all Worker VMs

.DESCRIPTION
    Reads config.yaml and creates baseline checkpoints for all defined workers

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.EXAMPLE
    .\create-all-baselines.ps1
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

Write-Host "`n=== Baseline Checkpoint Creation ===" -ForegroundColor Cyan

# Parse config
$ConfigContent = Get-Content $ConfigPath -Raw
$WorkersSection = ($ConfigContent -split 'workers:')[1] -split 'storage:' | Select-Object -First 1
$WorkerMatches = [regex]::Matches($WorkersSection, '- name:\s*"([^"]+)"')

$workers = @()
foreach ($match in $WorkerMatches) {
    $workers += @{
        Name = $match.Groups[1].Value
        SnapshotName = "$($match.Groups[1].Value)-baseline"
    }
}

Write-Info "Found $($workers.Count) worker(s) in configuration"

$BaselineScript = Join-Path $PSScriptRoot "05-create-baseline.ps1"
if (-not (Test-Path $BaselineScript)) {
    Write-Error "Baseline script not found: $BaselineScript"
    exit 1
}

# Create baseline for each VM
$successCount = 0
$failedVMs = @()

foreach ($worker in $workers) {
    Write-Host "`n--- Creating baseline: $($worker.Name) ---" -ForegroundColor Yellow

    # Check if VM exists
    $vm = Get-VM -Name $worker.Name -ErrorAction SilentlyContinue
    if (-not $vm) {
        Write-Warn "VM not found: $($worker.Name), skipping..."
        $failedVMs += $worker.Name
        continue
    }

    try {
        & $BaselineScript -WorkerName $worker.Name -SnapshotName $worker.SnapshotName

        if ($LASTEXITCODE -eq 0) {
            Write-Success "✓ $($worker.Name) baseline created"
            $successCount++
        } else {
            Write-Warn "✗ Baseline creation returned error code: $LASTEXITCODE"
            $failedVMs += $worker.Name
        }
    } catch {
        Write-Warn "✗ Failed to create baseline for $($worker.Name): $_"
        $failedVMs += $worker.Name
    }
}

# Summary
Write-Host "`n=== Baseline Creation Summary ===" -ForegroundColor Cyan
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
        Write-Host ".\scripts\05-create-baseline.ps1 -WorkerName '$vmName'" -ForegroundColor Gray
    }
} else {
    Write-Success "All baselines created successfully!"
}

if ($successCount -gt 0) {
    Write-Host "`nNext steps:" -ForegroundColor Green
    Write-Host "  1. Disable internet access for lab isolation (RECOMMENDED):" -ForegroundColor Cyan
    Write-Host "     .\scripts\toggle-vm-internet.ps1 -Action Disable" -ForegroundColor Gray
    Write-Host ""
    Write-Host "  2. Validate environment:" -ForegroundColor Cyan
    Write-Host "     .\scripts\validate-environment.ps1" -ForegroundColor Gray
    Write-Host ""
    Write-Host "  3. Start mutation loop:" -ForegroundColor Cyan
    Write-Host "     wsl" -ForegroundColor Gray
    Write-Host "     cd /mnt/c/.../controller" -ForegroundColor Gray
    Write-Host "     cargo run --bin mutator -- --config ../automation/config.yaml" -ForegroundColor Gray
    Write-Host ""
    Write-Info "⚠️  Internet access is currently ENABLED - VMs can reach the internet"
    Write-Info "   For a confined lab, disable internet access now with toggle-vm-internet.ps1"
}

exit 0
