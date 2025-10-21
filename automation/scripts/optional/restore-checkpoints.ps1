<#
.SYNOPSIS
    Restore Worker VMs from backup

.PARAMETER BackupPath
    Path to backup directory (e.g., D:\Backups\AutoMutate\automutate-backup-20250114-153000)

.PARAMETER WorkerName
    Specific worker to restore (default: all workers in backup)

.PARAMETER Force
    Overwrite existing VMs without confirmation

.EXAMPLE
    .\restore-checkpoints.ps1 -BackupPath "D:\Backups\AutoMutate\automutate-backup-20250114-153000"

.EXAMPLE
    .\restore-checkpoints.ps1 -BackupPath "\\NAS\Backups\automutate-backup-20250114-153000" -WorkerName "win11-worker-01" -Force
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$BackupPath,

    [Parameter()]
    [string]$WorkerName,

    [Parameter()]
    [switch]$Force
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

Write-Host @"

╔═══════════════════════════════════════════════════════════════╗
║         Restore Worker Checkpoints                            ║
╚═══════════════════════════════════════════════════════════════╝

"@ -ForegroundColor Cyan

# Validate backup path
if (-not (Test-Path $BackupPath)) {
    Write-Err "Backup path not found: $BackupPath"
    exit 1
}

# Read manifest
$manifestPath = Join-Path $BackupPath "manifest.json"
if (-not (Test-Path $manifestPath)) {
    Write-Err "Manifest not found: $manifestPath"
    Write-Info "This may not be a valid AutoMutate backup"
    exit 1
}

$manifest = Get-Content $manifestPath | ConvertFrom-Json
Write-Info "Backup timestamp: $($manifest.timestamp)"
Write-Info "Workers in backup: $($manifest.workers.Count)"

# Get worker directories
$workerDirs = Get-ChildItem -Directory $BackupPath | Where-Object { $_.Name -ne "manifest.json" }

if ($WorkerName) {
    $workerDirs = $workerDirs | Where-Object { $_.Name -eq $WorkerName }
    if ($workerDirs.Count -eq 0) {
        Write-Err "Worker not found in backup: $WorkerName"
        exit 1
    }
}

Write-Info "Restoring $($workerDirs.Count) worker(s)"

foreach ($workerDir in $workerDirs) {
    $vmName = $workerDir.Name
    Write-Host ""
    Write-Info "Worker: $vmName"

    # Check if VM already exists
    $existingVM = Get-VM -Name $vmName -ErrorAction SilentlyContinue
    if ($existingVM) {
        if (-not $Force) {
            Write-Warn "VM already exists: $vmName"
            $response = Read-Host "Overwrite existing VM? (y/n)"
            if ($response -ne "y") {
                Write-Info "Skipping $vmName"
                continue
            }
        }

        Write-Info "Removing existing VM: $vmName"
        if ($existingVM.State -eq "Running") {
            Stop-VM -Name $vmName -TurnOff
        }
        Remove-VM -Name $vmName -Force
        Write-Success "Existing VM removed"
    }

    # Find VM config in backup
    $vmcxPath = Get-ChildItem -Recurse -Filter "*.vmcx" -Path $workerDir.FullName | Select-Object -First 1
    if (-not $vmcxPath) {
        Write-Err "VM config not found in backup: $($workerDir.FullName)"
        continue
    }

    # Import VM
    Write-Info "Importing VM from: $($vmcxPath.FullName)"
    $importedVM = Import-VM -Path $vmcxPath.FullName -Copy -GenerateNewId

    Write-Success "VM imported: $vmName"

    # Verify checkpoints
    $checkpoints = Get-VMSnapshot -VMName $vmName
    Write-Info "Checkpoints restored: $($checkpoints.Count)"
    foreach ($cp in $checkpoints) {
        Write-Info "  - $($cp.Name) ($($cp.CreationTime))"
    }

    # Verify baseline exists
    $baseline = Get-VMSnapshot -VMName $vmName -Name "$vmName-baseline" -ErrorAction SilentlyContinue
    if ($baseline) {
        Write-Success "Baseline checkpoint verified"
    } else {
        Write-Warn "Baseline checkpoint not found"
    }
}

Write-Host ""
Write-Host "="*70 -ForegroundColor Cyan
Write-Success "Restore complete"
Write-Info "Restored $($workerDirs.Count) worker(s) from backup"
Write-Host "="*70 -ForegroundColor Cyan

exit 0
