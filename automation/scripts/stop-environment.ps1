<#
.SYNOPSIS
    Stop AutoMutate++ environment (Controller + Workers)

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.PARAMETER SkipController
    Skip stopping Controller services

.PARAMETER StopWSL
    Also stop WSL2 distribution

.PARAMETER Force
    Force immediate shutdown (no graceful stop)

.EXAMPLE
    .\stop-environment.ps1

.EXAMPLE
    .\stop-environment.ps1 -Force -StopWSL
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$ConfigPath = "..\config.yaml",

    [Parameter()]
    [switch]$SkipController,

    [Parameter()]
    [switch]$StopWSL,

    [Parameter()]
    [switch]$Force
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[✓] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[i] $M" -ForegroundColor Cyan }
function Write-Step { param($M) Write-Host "`n==> $M" -ForegroundColor Magenta }

Write-Host @"

╔═══════════════════════════════════════════════════════════════╗
║         Stopping AutoMutate++ Environment                     ║
╚═══════════════════════════════════════════════════════════════╝

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

# Extract worker names
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
        if ($Force) {
            Write-Info "Force stopping $workerName..."
            Stop-VM -Name $workerName -TurnOff
        } else {
            Write-Info "Gracefully stopping $workerName..."
            Stop-VM -Name $workerName -Force

            # Wait for shutdown (max 30 seconds)
            $timeout = 30
            $elapsed = 0
            while ((Get-VM -Name $workerName).State -ne "Off" -and $elapsed -lt $timeout) {
                Start-Sleep -Seconds 2
                $elapsed += 2
            }

            if ((Get-VM -Name $workerName).State -ne "Off") {
                Write-Warning "Forcing shutdown of $workerName..."
                Stop-VM -Name $workerName -TurnOff
            }
        }

        Write-Success "$workerName stopped"
    }
}

# Step 2: Stop Controller services
if (-not $SkipController) {
    Write-Step "Step 2/2: Stopping Controller Services"

    Write-Info "Stopping Elasticsearch + Kibana..."
    $ProjectRoot = Split-Path $PSScriptRoot -Parent | Split-Path -Parent
    $WslProjectRoot = $ProjectRoot -replace '\\', '/' -replace 'C:', '/mnt/c'

    wsl -d Ubuntu bash -c "cd '$WslProjectRoot/automation' && docker-compose down" 2>$null

    Write-Success "Controller services stopped"

    if ($StopWSL) {
        Write-Info "Stopping WSL2 Ubuntu..."
        wsl --terminate Ubuntu 2>$null
        Write-Success "WSL2 stopped"
    }
}

Write-Host ""
Write-Success "Environment stopped"
Write-Info "Start: .\start-environment.ps1"

exit 0
