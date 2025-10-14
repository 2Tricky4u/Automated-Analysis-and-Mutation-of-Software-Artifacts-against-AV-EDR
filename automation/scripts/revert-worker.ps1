<#
.SYNOPSIS
    Fast snapshot restore for Worker VM

.PARAMETER WorkerName
    VM name to revert

.PARAMETER SnapshotName
    Checkpoint name (default: {WorkerName}-baseline)

.PARAMETER NoStart
    Do not start VM after revert

.EXAMPLE
    .\revert-worker.ps1 -WorkerName "win11-worker-01"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$WorkerName,

    [Parameter()]
    [string]$SnapshotName = "$WorkerName-baseline",

    [Parameter()]
    [switch]$NoStart
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[✓] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[i] $M" -ForegroundColor Cyan }

$startTime = Get-Date

# Validate VM exists
if (-not (Get-VM -Name $WorkerName -ErrorAction SilentlyContinue)) {
    Write-Error "VM not found: $WorkerName"
    exit 1
}

# Validate checkpoint exists
$checkpoint = Get-VMSnapshot -VMName $WorkerName -Name $SnapshotName -ErrorAction SilentlyContinue
if (-not $checkpoint) {
    Write-Error "Checkpoint not found: $SnapshotName"
    Write-Info "Available checkpoints:"
    Get-VMSnapshot -VMName $WorkerName | Format-Table Name, CreationTime -AutoSize
    exit 1
}

# Stop VM if running
$vm = Get-VM -Name $WorkerName
if ($vm.State -eq "Running") {
    Write-Info "Stopping VM..."
    Stop-VM -Name $WorkerName -TurnOff
}

# Restore checkpoint
Write-Info "Restoring checkpoint: $SnapshotName"
Restore-VMSnapshot -VMName $WorkerName -Name $SnapshotName -Confirm:$false

Write-Success "Checkpoint restored"

# Start VM
if (-not $NoStart) {
    Write-Info "Starting VM..."
    Start-VM -Name $WorkerName

    # Wait for VM to be running
    while ((Get-VM -Name $WorkerName).State -ne "Running") {
        Start-Sleep -Milliseconds 500
    }

    Write-Success "VM started"
}

$elapsed = (Get-Date) - $startTime
Write-Success "Revert complete in $([int]$elapsed.TotalSeconds) seconds"

exit 0
