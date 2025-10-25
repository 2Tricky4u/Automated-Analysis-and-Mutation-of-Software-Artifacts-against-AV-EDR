<#
.SYNOPSIS
    Continuous WSL keepalive daemon (runs in background)

.DESCRIPTION
    Runs a continuous loop that pings WSL every 3 seconds to prevent shutdown.
    This script is designed to run as a background job managed by start/stop-environment.ps1

.PARAMETER Action
    Start: Launch keepalive daemon in background
    Stop: Stop keepalive daemon
    Status: Check if daemon is running

.EXAMPLE
    .\wsl-keepalive-daemon.ps1 -Action Start
    .\wsl-keepalive-daemon.ps1 -Action Status
    .\wsl-keepalive-daemon.ps1 -Action Stop
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet("Start", "Stop", "Status")]
    [string]$Action
)

$ErrorActionPreference = "Stop"

$JobName = "WSLKeepalive-AutoMutate"
$LogFile = "$env:TEMP\wsl-keepalive.log"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Err { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

switch ($Action) {
    "Start" {
        Write-Info "Starting WSL keepalive daemon..."

        # Check if already running
        $existingJob = Get-Job -Name $JobName -ErrorAction SilentlyContinue
        if ($existingJob -and $existingJob.State -eq "Running") {
            Write-Info "Daemon already running (Job ID: $($existingJob.Id))"
            return
        }

        # Remove old completed/failed jobs
        Get-Job -Name $JobName -ErrorAction SilentlyContinue | Remove-Job -Force -ErrorAction SilentlyContinue

        # Start background job with continuous ping loop
        $job = Start-Job -Name $JobName -ScriptBlock {
            param($LogPath)

            $intervalSeconds = 3
            $lastLog = Get-Date

            # Continuous loop
            while ($true) {
                try {
                    # Ping WSL (lightweight systemctl check)
                    $result = wsl -d Ubuntu -u root systemctl is-active elastic-stack 2>&1

                    # Log every 60 seconds (reduce log spam)
                    if (((Get-Date) - $lastLog).TotalSeconds -ge 60) {
                        $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
                        Add-Content -Path $LogPath -Value "[$timestamp] WSL pinged successfully (status: $result)"
                        $lastLog = Get-Date
                    }
                } catch {
                    $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
                    Add-Content -Path $LogPath -Value "[$timestamp] ERROR: $($_.Exception.Message)"
                }

                Start-Sleep -Seconds $intervalSeconds
            }
        } -ArgumentList $LogFile

        # Wait a moment to ensure job started
        Start-Sleep -Seconds 1

        if ($job.State -eq "Running") {
            Write-Success "Daemon started (Job ID: $($job.Id))"
            Write-Info "WSL will be pinged every 3 seconds"
            Write-Info "Log file: $LogFile"
        } else {
            Write-Err "Failed to start daemon"
            Write-Info "Check job state: Get-Job -Name '$JobName'"
        }
    }

    "Stop" {
        Write-Info "Stopping WSL keepalive daemon..."

        $existingJob = Get-Job -Name $JobName -ErrorAction SilentlyContinue
        if (-not $existingJob) {
            Write-Info "Daemon not running (nothing to stop)"
            return
        }

        # Stop the job
        Stop-Job -Name $JobName -ErrorAction SilentlyContinue
        Remove-Job -Name $JobName -Force -ErrorAction SilentlyContinue

        Write-Success "Daemon stopped"
        Write-Info "WSL may auto-shutdown after idle timeout (~8 seconds)"
    }

    "Status" {
        Write-Info "Checking WSL keepalive daemon status..."

        $existingJob = Get-Job -Name $JobName -ErrorAction SilentlyContinue
        if ($existingJob) {
            Write-Success "Daemon is running"
            Write-Info "Job ID: $($existingJob.Id)"
            Write-Info "State: $($existingJob.State)"
            Write-Info "Started: $($existingJob.PSBeginTime)"

            # Show last few log lines
            if (Test-Path $LogFile) {
                Write-Info ""
                Write-Info "Last 5 log entries:"
                Get-Content $LogFile -Tail 5 | ForEach-Object { Write-Host "  $_" -ForegroundColor Gray }
            }

            # Check current WSL status
            Write-Info ""
            Write-Info "Current WSL status:"
            try {
                $wslUptime = wsl -d Ubuntu uptime 2>&1
                Write-Success "WSL is running: $wslUptime"

                # Check containers
                $containers = wsl -d Ubuntu docker ps --format "table {{.Names}}\t{{.Status}}" 2>&1 | Select-String -Pattern "(elasticsearch|kibana)"
                if ($containers) {
                    Write-Info "Containers:"
                    $containers | ForEach-Object { Write-Host "  $_" -ForegroundColor Gray }
                }
            } catch {
                Write-Err "WSL is not running or not responding"
            }
        } else {
            Write-Err "Daemon is NOT running"
            Write-Info "Start it with: .\wsl-keepalive-daemon.ps1 -Action Start"
        }
    }
}
