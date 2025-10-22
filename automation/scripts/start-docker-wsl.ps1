
<#
.SYNOPSIS
    Start Docker daemon in WSL2 and wait for it to be ready

.EXAMPLE
    .\start-docker-wsl.ps1
#>

$ErrorActionPreference = "Stop"

Write-Host "[INFO] Checking if Docker daemon is running in WSL..." -ForegroundColor Cyan

# Check if Docker is already running
$dockerRunning = wsl -d Ubuntu bash -c "docker info > /dev/null 2>&1 && echo 'yes' || echo 'no'" 2>$null

if ($dockerRunning -match "yes") {
    Write-Host "[OK] Docker daemon already running" -ForegroundColor Green
    exit 0
}

Write-Host "[INFO] Starting Docker daemon in WSL..." -ForegroundColor Cyan

# Start Docker daemon in background
wsl -d Ubuntu bash -c "sudo nohup dockerd > /var/log/docker.log 2>&1 &" 2>$null

# Wait for Docker to be ready (max 30 seconds)
$maxRetries = 30
$retries = 0

Write-Host "[INFO] Waiting for Docker daemon to be ready..." -ForegroundColor Cyan

while ($retries -lt $maxRetries) {
    $retries++

    $dockerReady = wsl -d Ubuntu bash -c "docker info > /dev/null 2>&1 && echo 'yes' || echo 'no'" 2>$null

    if ($dockerReady -match "yes") {
        Write-Host "[OK] Docker daemon is ready (took $retries seconds)" -ForegroundColor Green
        exit 0
    }

    Write-Host "." -NoNewline -ForegroundColor Gray
    Start-Sleep -Seconds 1
}

Write-Host ""
Write-Host "[ERROR] Docker daemon failed to start after 30 seconds" -ForegroundColor Red
Write-Host "[INFO] Check logs: wsl -d Ubuntu cat /var/log/docker.log" -ForegroundColor Yellow
exit 1
