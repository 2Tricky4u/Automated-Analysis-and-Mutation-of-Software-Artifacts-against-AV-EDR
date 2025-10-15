<#
.SYNOPSIS
    Create baseline checkpoint for Worker VM

.PARAMETER WorkerName
    VM name to snapshot

.PARAMETER SnapshotName
    Checkpoint name (default: {WorkerName}-baseline)

.EXAMPLE
    .\05-create-baseline.ps1 -WorkerName "win11-worker-01"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$WorkerName,

    [Parameter()]
    [string]$SnapshotName = "$WorkerName-baseline"
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

$vm = Get-VM -Name $WorkerName

# Check if VM is running
if ($vm.State -eq "Running") {
    Write-Info "Shutting down $WorkerName gracefully..."
    Stop-VM -Name $WorkerName -Force

    # Wait for shutdown (max 60 seconds)
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

# Remove existing checkpoint with same name
$existing = Get-VMSnapshot -VMName $WorkerName -Name $SnapshotName -ErrorAction SilentlyContinue
if ($existing) {
    Write-Info "Removing existing checkpoint: $SnapshotName"
    Remove-VMSnapshot -VMName $WorkerName -Name $SnapshotName -Confirm:$false
    Write-Success "Old checkpoint removed"
}

# Create checkpoint
Write-Info "Creating baseline checkpoint: $SnapshotName"
Checkpoint-VM -VMName $WorkerName -SnapshotName $SnapshotName

# Validate checkpoint
$checkpoint = Get-VMSnapshot -VMName $WorkerName -Name $SnapshotName -ErrorAction SilentlyContinue
if (-not $checkpoint) {
    Write-Error "Checkpoint creation failed"
    exit 1
}

Write-Success "Baseline checkpoint created: $SnapshotName"
Write-Info "Checkpoint path: $($checkpoint.Path)"

# Start VM
Write-Info "Starting VM..."
Start-VM -Name $WorkerName

Write-Success "Baseline creation complete"
Write-Info "Fast revert: .\revert-worker.ps1 -WorkerName '$WorkerName'"

exit 0
