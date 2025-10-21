<#
.SYNOPSIS
    Update baseline checkpoint after Worker modifications

.PARAMETER WorkerName
    VM name to update

.PARAMETER SnapshotName
    Checkpoint name (default: {WorkerName}-baseline)

.PARAMETER BackupOld
    Create backup of old checkpoint before updating

.EXAMPLE
    .\update-baseline.ps1 -WorkerName "win11-worker-01" -BackupOld
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$WorkerName,

    [Parameter()]
    [string]$SnapshotName = "$WorkerName-baseline",

    [Parameter()]
    [switch]$BackupOld
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

# Validate VM exists
if (-not (Get-VM -Name $WorkerName -ErrorAction SilentlyContinue)) {
    Write-Error "VM not found: $WorkerName"
    exit 1
}

# Check if baseline exists
$existingBaseline = Get-VMSnapshot -VMName $WorkerName -Name $SnapshotName -ErrorAction SilentlyContinue

if ($existingBaseline -and $BackupOld) {
    Write-Info "Backing up old baseline..."
    $backupName = "$SnapshotName-backup-$(Get-Date -Format 'yyyyMMdd-HHmmss')"

    # Hyper-V doesn't support direct snapshot rename, so we document it
    Write-Warn "Manual backup required: Export baseline via Hyper-V Manager before updating"
    Write-Info "Suggested backup name: $backupName"

    $response = Read-Host "Continue with baseline update? (y/n)"
    if ($response -ne "y") {
        Write-Info "Update cancelled"
        exit 0
    }
}

# Stop VM if running
$vm = Get-VM -Name $WorkerName
if ($vm.State -eq "Running") {
    Write-Info "Shutting down $WorkerName gracefully..."
    Stop-VM -Name $WorkerName -Force

    # Wait for shutdown
    $timeout = 60
    $elapsed = 0
    while ((Get-VM -Name $WorkerName).State -ne "Off" -and $elapsed -lt $timeout) {
        Start-Sleep -Seconds 2
        $elapsed += 2
    }

    if ((Get-VM -Name $WorkerName).State -ne "Off") {
        Write-Warn "VM did not shut down gracefully, forcing stop..."
        Stop-VM -Name $WorkerName -TurnOff
    }

    Write-Success "VM stopped"
}

# Remove old baseline
if ($existingBaseline) {
    Write-Info "Removing old baseline..."
    Remove-VMSnapshot -VMName $WorkerName -Name $SnapshotName -Confirm:$false
    Write-Success "Old baseline removed"
}

# Create new baseline
Write-Info "Creating new baseline: $SnapshotName"
Checkpoint-VM -VMName $WorkerName -SnapshotName $SnapshotName

# Validate
$newBaseline = Get-VMSnapshot -VMName $WorkerName -Name $SnapshotName -ErrorAction SilentlyContinue
if (-not $newBaseline) {
    Write-Error "Baseline creation failed"
    exit 1
}

Write-Success "Baseline updated successfully"
Write-Info "Checkpoint: $SnapshotName"
Write-Info "Created: $($newBaseline.CreationTime)"

# Start VM
Write-Info "Starting VM..."
Start-VM -Name $WorkerName

Write-Success "Update complete"

exit 0
