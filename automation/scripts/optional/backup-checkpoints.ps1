<#
.SYNOPSIS
    Backup Worker VM checkpoints to external location

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.PARAMETER BackupPath
    Backup destination directory

.PARAMETER WorkerName
    Specific worker to backup (default: all workers)

.EXAMPLE
    .\backup-checkpoints.ps1 -BackupPath "D:\Backups\AutoMutate"

.EXAMPLE
    .\backup-checkpoints.ps1 -BackupPath "\\NAS\Backups" -WorkerName "win11-worker-01"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$BackupPath,

    [Parameter()]
    [string]$ConfigPath = "..\config.yaml",

    [Parameter()]
    [string]$WorkerName
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

Write-Host @"

+===============================================================+
|         Backup Worker Checkpoints                             |
+===============================================================+

"@ -ForegroundColor Cyan

# Validate backup path
if (-not (Test-Path $BackupPath)) {
    Write-Info "Creating backup directory: $BackupPath"
    New-Item -ItemType Directory -Path $BackupPath -Force | Out-Null
}

$timestamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$BackupRoot = Join-Path $BackupPath "automutate-backup-$timestamp"
New-Item -ItemType Directory -Path $BackupRoot -Force | Out-Null

Write-Info "Backup destination: $BackupRoot"

# Parse config to get workers
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)\"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$ConfigContent = Get-Content $ConfigPath -Raw
$WorkersSection = ($ConfigContent -split 'workers:')[1] -split 'storage:' | Select-Object -First 1
$WorkerMatches = [regex]::Matches($WorkersSection, '- name:\s*"([^"]+)"')

$WorkerNames = @()
foreach ($match in $WorkerMatches) {
    $WorkerNames += $match.Groups[1].Value
}

# Filter if specific worker requested
if ($WorkerName) {
    if ($WorkerNames -contains $WorkerName) {
        $WorkerNames = @($WorkerName)
    } else {
        Write-Error "Worker not found in config: $WorkerName"
        exit 1
    }
}

Write-Info "Backing up $($WorkerNames.Count) worker(s)"

foreach ($worker in $WorkerNames) {
    Write-Host ""
    Write-Info "Worker: $worker"

    $vm = Get-VM -Name $worker -ErrorAction SilentlyContinue
    if (-not $vm) {
        Write-Warn "VM not found: $worker (skipping)"
        continue
    }

    # Export VM (includes all checkpoints)
    $exportPath = Join-Path $BackupRoot $worker
    Write-Info "Exporting VM to: $exportPath"

    Export-VM -Name $worker -Path $exportPath

    Write-Success "VM exported successfully"

    # Get checkpoint info
    $checkpoints = Get-VMSnapshot -VMName $worker
    Write-Info "Checkpoints included: $($checkpoints.Count)"
    foreach ($cp in $checkpoints) {
        Write-Info "  - $($cp.Name) ($($cp.CreationTime))"
    }
}

# Create manifest
$manifestPath = Join-Path $BackupRoot "manifest.json"
$manifest = @{
    timestamp = $timestamp
    workers = $WorkerNames
    config_path = $ConfigPath
    backup_path = $BackupRoot
}

$manifest | ConvertTo-Json -Depth 10 | Out-File $manifestPath -Encoding UTF8
Write-Success "Manifest created: $manifestPath"

# Calculate size
$totalSizeGB = (Get-ChildItem -Recurse $BackupRoot | Measure-Object -Property Length -Sum).Sum / 1GB
$totalSizeGB = [math]::Round($totalSizeGB, 2)

Write-Host ""
Write-Host "="*70 -ForegroundColor Cyan
Write-Success "Backup complete"
Write-Info "Location: $BackupRoot"
Write-Info "Size: $totalSizeGB GB"
Write-Info "Workers: $($WorkerNames.Count)"
Write-Host "="*70 -ForegroundColor Cyan

exit 0
