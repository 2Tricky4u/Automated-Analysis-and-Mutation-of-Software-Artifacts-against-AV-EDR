<#
.SYNOPSIS
    Start AutoMutate++ environment (Controller + Workers)

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.PARAMETER SkipController
    Skip Controller services (Elasticsearch, Kibana, binaries)

.PARAMETER WorkersOnly
    Only start Worker VMs (implies -SkipController)

.EXAMPLE
    .\start-environment.ps1

.EXAMPLE
    .\start-environment.ps1 -WorkersOnly
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
|         Starting AutoMutate++ Environment                     |
+===============================================================+

"@ -ForegroundColor Cyan

# Parse config
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)\"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

# Step 1: Start WSL2 Controller
if (-not $SkipController) {
    Write-Step "Step 1/3: Starting WSL2 Controller"

    # Check if WSL is running
    $wslState = wsl --list --running 2>$null | Select-String "Ubuntu"
    if (-not $wslState) {
        Write-Info "Starting WSL2 Ubuntu..."
        wsl -d Ubuntu bash -c "echo 'WSL2 started'" 2>$null
        Start-Sleep -Seconds 3
        Write-Success "WSL2 started"
    } else {
        Write-Success "WSL2 already running"
    }

    # Start Docker daemon via systemctl
    Write-Info "Ensuring Docker daemon is running..."
    $dockerStatus = wsl -d Ubuntu bash -c "sudo systemctl is-active docker" 2>$null
    if ($dockerStatus -ne "active") {
        Write-Info "Starting Docker service..."
        wsl -d Ubuntu bash -c "sudo systemctl start docker" 2>$null
        Start-Sleep -Seconds 3
    } else {
        Write-Success "Docker daemon already running"
    }

    # Check and start Elasticsearch + Kibana
    Write-Info "Checking Elasticsearch + Kibana status..."

    $ProjectRoot = Split-Path $PSScriptRoot -Parent | Split-Path -Parent
    $WslProjectRoot = $ProjectRoot -replace '\\', '/' -replace 'C:', '/mnt/c'

    # Check if Elasticsearch is already accessible
    $esRunning = $false
    try {
        $response = Invoke-WebRequest -Uri "http://localhost:9200" -TimeoutSec 2 -ErrorAction SilentlyContinue
        if ($response.StatusCode -eq 200) {
            $esRunning = $true
            Write-Success "Elasticsearch already running"
        }
    } catch {
        # Not running
    }

    # Check if Kibana is already accessible
    $kibanaRunning = $false
    try {
        $response = Invoke-WebRequest -Uri "http://localhost:5601" -TimeoutSec 2 -ErrorAction SilentlyContinue
        if ($response.StatusCode -in @(200, 302)) {
            $kibanaRunning = $true
            Write-Success "Kibana already running"
        }
    } catch {
        # Not running
    }

    # Start services if needed
    if (-not $esRunning -or -not $kibanaRunning) {
        Write-Info "Starting required containers..."
        Write-Info "Note: Containers run in foreground mode via background processes"
        Write-Info "      (Required due to systemd compatibility with docker compose)"

        if (-not $esRunning) {
            Write-Info "Starting Elasticsearch container..."
            $null = Start-Process -FilePath "wsl" -ArgumentList "-d Ubuntu bash -c `"cd '$WslProjectRoot/automation' && nohup docker compose up elasticsearch > /tmp/elasticsearch.log 2>&1 &`"" -WindowStyle Hidden
            Start-Sleep -Seconds 3
        }

        if (-not $kibanaRunning) {
            Write-Info "Starting Kibana container..."
            $null = Start-Process -FilePath "wsl" -ArgumentList "-d Ubuntu bash -c `"cd '$WslProjectRoot/automation' && nohup docker compose up kibana > /tmp/kibana.log 2>&1 &`"" -WindowStyle Hidden
            Start-Sleep -Seconds 3
        }

        # Wait for Elasticsearch to be ready (if it was just started)
        if (-not $esRunning) {
            Write-Info "Waiting for Elasticsearch (http://localhost:9200)..."
            $maxRetries = 30
            $retries = 0
            while ($retries -lt $maxRetries) {
                try {
                    $response = Invoke-WebRequest -Uri "http://localhost:9200" -TimeoutSec 2 -ErrorAction SilentlyContinue
                    if ($response.StatusCode -eq 200) {
                        Write-Success "Elasticsearch ready"
                        break
                    }
                } catch {
                    # Continue
                }
                Start-Sleep -Seconds 2
                $retries++
            }

            if ($retries -eq $maxRetries) {
                Write-Warning "Elasticsearch not responding after 60 seconds."
                Write-Info "Check logs in WSL: wsl -d Ubuntu tail -f /tmp/elasticsearch.log"
            }
        }
    }

    Write-Success "Controller services ready"
    Write-Info "Elasticsearch: http://localhost:9200"
    Write-Info "Kibana: http://localhost:5601"
    Write-Info ""
    Write-Info "Logs available in WSL:"
    Write-Info "  wsl -d Ubuntu tail -f /tmp/elasticsearch.log"
    Write-Info "  wsl -d Ubuntu tail -f /tmp/kibana.log"
}

# Step 2: Start Worker VMs
Write-Step "Step 2/3: Starting Worker VMs"

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

    if ($vm.State -eq "Running") {
        Write-Success "$workerName already running"
    } else {
        Write-Info "Starting $workerName..."
        Start-VM -Name $workerName
        Write-Success "$workerName started"
    }
}

# Step 3: Summary
Write-Step "Step 3/3: Environment Status"

Write-Host ""
Write-Host "Controller:" -ForegroundColor Yellow
if (-not $SkipController) {
    Write-Host "  Elasticsearch: http://localhost:9200" -ForegroundColor Cyan
    Write-Host "  Kibana: http://localhost:5601" -ForegroundColor Cyan
    Write-Host "  gRPC: $($config.network.host_ip):$($config.controller.grpc_port)" -ForegroundColor Cyan
} else {
    Write-Host "  (skipped)" -ForegroundColor Gray
}

Write-Host ""
Write-Host "Workers:" -ForegroundColor Yellow
foreach ($workerName in $WorkerNames) {
    $vm = Get-VM -Name $workerName -ErrorAction SilentlyContinue
    if ($vm) {
        $state = $vm.State
        $color = if ($state -eq "Running") { "Green" } else { "Red" }
        Write-Host "  $workerName : $state" -ForegroundColor $color
    }
}

Write-Host ""
Write-Success "Environment started"
Write-Info "Validate: .\validate-environment.ps1"
Write-Info "Stop: .\stop-environment.ps1"

exit 0
