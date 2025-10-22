<#
.SYNOPSIS
    Stop AutoMutate++ environment (Controller + Workers)

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.PARAMETER SkipController
    Skip stopping Controller services (Elasticsearch, Kibana)

.PARAMETER WorkersOnly
    Only stop Worker VMs (implies -SkipController)

.EXAMPLE
    .\stop-environment.ps1

.EXAMPLE
    .\stop-environment.ps1 -WorkersOnly
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$ConfigPath = "..\config.yaml",

    [Parameter()]
    [switch]$SkipController,

    [Parameter()]
    [switch]$WorkersOnly
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Step { param($M) Write-Host "`n==> $M" -ForegroundColor Magenta }

if ($WorkersOnly) { $SkipController = $true }

Write-Host @"

+===============================================================+
|         Stopping AutoMutate++ Environment                     |
+===============================================================+

"@ -ForegroundColor Cyan

# Parse config
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)\"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

# Step 1: Stop Worker VMs
Write-Step "Step 1/2: Stopping Worker VMs"

# Extract worker names from config
$ConfigContent = Get-Content $ConfigPath -Raw
$WorkersSection = ($ConfigContent -split 'workers:')[1] -split 'storage:' | Select-Object -First 1
$WorkerMatches = [regex]::Matches($WorkersSection, '- name:\s*"([^"]+)"')

$WorkerNames = @()
foreach ($match in $WorkerMatches) {
    $WorkerNames += $match.Groups[1].Value
}

Write-Info "Found $($WorkerNames.Count) worker(s)"

foreach ($workerName in $WorkerNames) {
    $vm = Get-VM -Name $workerName -ErrorAction SilentlyContinue
    if (-not $vm) {
        Write-Warning "VM not found: $workerName (skipping)"
        continue
    }

    if ($vm.State -eq "Off") {
        Write-Success "$workerName already stopped"
    } else {
        Write-Info "Stopping $workerName..."
        Stop-VM -Name $workerName -Force
        Write-Success "$workerName stopped"
    }
}

# Step 2: Stop Controller Services
if (-not $SkipController) {
    Write-Step "Step 2/2: Stopping Controller Services"

    Write-Info "Stopping Elasticsearch + Kibana containers..."

    $ProjectRoot = Split-Path $PSScriptRoot -Parent | Split-Path -Parent
    $WslProjectRoot = $ProjectRoot -replace '\\', '/' -replace 'C:', '/mnt/c'

    # Stop containers using docker compose down
    wsl -d Ubuntu bash -c "cd '$WslProjectRoot/automation' && docker compose down 2>&1" 2>&1 | Out-Null

    Write-Success "Containers stopped"
    Write-Info "Note: Background processes may still be running"
    Write-Info "      They will exit automatically once containers are stopped"
} else {
    Write-Info "Skipping Controller services (--SkipController)"
}

# Step 3: Summary
Write-Step "Environment Stopped"

Write-Host ""
Write-Host "Workers:" -ForegroundColor Yellow
foreach ($workerName in $WorkerNames) {
    $vm = Get-VM -Name $workerName -ErrorAction SilentlyContinue
    if ($vm) {
        $state = $vm.State
        $color = if ($state -eq "Off") { "Green" } else { "Yellow" }
        Write-Host "  $workerName : $state" -ForegroundColor $color
    }
}

if (-not $SkipController) {
    Write-Host ""
    Write-Host "Controller:" -ForegroundColor Yellow
    Write-Host "  Elasticsearch/Kibana: Stopped" -ForegroundColor Green
}

Write-Host ""
Write-Success "Environment stopped"
Write-Info "Restart: .\start-environment.ps1"

exit 0
