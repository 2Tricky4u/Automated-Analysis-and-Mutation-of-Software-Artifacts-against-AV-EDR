<#
.SYNOPSIS
    List all registered workers from controller

.DESCRIPTION
    Queries the controller via gRPC to list all workers with their capabilities,
    status, and metadata. Displays in a human-readable format.

.PARAMETER ControllerAddress
    Controller gRPC address (default: 10.200.200.1:50051)

.PARAMETER OutputFormat
    Output format: 'table' (default), 'json', or 'csv'

.PARAMETER FilterStatus
    Filter by worker status: 'available', 'busy', 'offline', or 'all' (default)

.PARAMETER FilterCapability
    Filter workers that have a specific capability (e.g., 'rededr', 'gpu')

.EXAMPLE
    .\list-workers.ps1

.EXAMPLE
    .\list-workers.ps1 -FilterStatus available

.EXAMPLE
    .\list-workers.ps1 -FilterCapability gpu -OutputFormat json

.EXAMPLE
    .\list-workers.ps1 -ControllerAddress "localhost:50051"
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$ControllerAddress = "10.200.200.1:50051",

    [Parameter()]
    [ValidateSet('table', 'json', 'csv')]
    [string]$OutputFormat = 'table',

    [Parameter()]
    [ValidateSet('all', 'available', 'busy', 'offline')]
    [string]$FilterStatus = 'all',

    [Parameter()]
    [string]$FilterCapability = $null
)

$ErrorActionPreference = "Stop"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

# Check if grpcurl is available
$grpcurlPath = Get-Command grpcurl -ErrorAction SilentlyContinue
if (-not $grpcurlPath) {
    Write-Err "grpcurl not found in PATH"
    Write-Info "Install grpcurl:"
    Write-Info "  Windows: choco install grpcurl"
    Write-Info "  Or download from: https://github.com/fullstorydev/grpcurl/releases"
    exit 1
}

Write-Info "Querying workers from controller at $ControllerAddress..."

# Call ListWorkers RPC
try {
    $result = grpcurl -plaintext -d '{}' $ControllerAddress edr.controller.Controller/ListWorkers 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Failed to connect to controller"
        Write-Info "Error: $result"
        Write-Info ""
        Write-Info "Troubleshooting:"
        Write-Info "  1. Check controller is running:"
        Write-Info "     cd controller/scheduler && cargo run --release"
        Write-Info "  2. Verify controller address: $ControllerAddress"
        Write-Info "  3. Check network connectivity:"
        Write-Info "     Test-NetConnection $($ControllerAddress.Split(':')[0]) -Port $($ControllerAddress.Split(':')[1])"
        exit 1
    }

    $workers = $result | ConvertFrom-Json
} catch {
    Write-Err "Failed to parse response: $_"
    exit 1
}

if (-not $workers.workers) {
    Write-Warn "No workers registered"
    Write-Info ""
    Write-Info "To add workers:"
    Write-Info "  1. Local worker: cd worker/agent && cargo run --release"
    Write-Info "  2. Remote worker: .\scripts\workers\deploy-remote-worker.ps1 -RemoteHost <IP> -Username <user> -WorkerConfigPath <path>"
    exit 0
}

# Filter workers
$filteredWorkers = $workers.workers
if ($FilterStatus -ne 'all') {
    $filteredWorkers = $filteredWorkers | Where-Object { $_.status -eq $FilterStatus }
}
if ($FilterCapability) {
    $filteredWorkers = $filteredWorkers | Where-Object { $_.capabilities -contains $FilterCapability }
}

if ($filteredWorkers.Count -eq 0) {
    Write-Warn "No workers match the filter criteria"
    Write-Info "  Status filter: $FilterStatus"
    if ($FilterCapability) {
        Write-Info "  Capability filter: $FilterCapability"
    }
    exit 0
}

# Output in requested format
switch ($OutputFormat) {
    'json' {
        $filteredWorkers | ConvertTo-Json -Depth 10
    }

    'csv' {
        $filteredWorkers | ForEach-Object {
            [PSCustomObject]@{
                WorkerId = $_.worker_id
                Address = $_.address
                Status = $_.status
                OS = $_.os_version
                Capabilities = ($_.capabilities -join ';')
                Registration = $_.registration_type
                LastPing = "$($_.last_ping_seconds_ago)s ago"
                CurrentJob = $_.current_job_id
            }
        } | ConvertTo-Csv -NoTypeInformation
    }

    'table' {
        Write-Host "`n" + ("="*80) -ForegroundColor Cyan
        Write-Host "  Registered Workers" -ForegroundColor Cyan
        Write-Host ("="*80) -ForegroundColor Cyan

        foreach ($worker in $filteredWorkers) {
            Write-Host ""

            # Worker ID with status indicator
            $statusColor = switch ($worker.status) {
                'available' { 'Green' }
                'busy'      { 'Yellow' }
                'offline'   { 'Red' }
                default     { 'Gray' }
            }
            Write-Host "Worker ID: " -NoNewline
            Write-Host "$($worker.worker_id)" -ForegroundColor $statusColor -NoNewline
            Write-Host " [$($worker.status.ToUpper())]" -ForegroundColor $statusColor

            # Basic info
            Write-Host "  Address:        $($worker.address)"
            Write-Host "  OS Version:     $($worker.os_version)"
            Write-Host "  Registration:   $($worker.registration_type)"
            Write-Host "  Last Ping:      $($worker.last_ping_seconds_ago)s ago"

            # Current job (if busy)
            if ($worker.current_job_id) {
                Write-Host "  Current Job:    " -NoNewline
                Write-Host "$($worker.current_job_id)" -ForegroundColor Yellow
            }

            # Capabilities
            if ($worker.capabilities -and $worker.capabilities.Count -gt 0) {
                Write-Host "  Capabilities:   " -NoNewline
                $capList = $worker.capabilities -join ', '
                Write-Host $capList -ForegroundColor Cyan
            } else {
                Write-Host "  Capabilities:   (none detected)"
            }

            # Tools
            if ($worker.tools) {
                $toolsList = @()
                if ($worker.tools.rededr_version) {
                    $toolsList += "RedEDR v$($worker.tools.rededr_version)"
                }
                if ($worker.tools.defender_version) {
                    $toolsList += "Defender v$($worker.tools.defender_version)"
                }
                if ($worker.tools.etw_version) {
                    $toolsList += "ETW ($($worker.tools.etw_version))"
                }
                if ($toolsList.Count -gt 0) {
                    Write-Host "  Tools:          $($toolsList -join ', ')"
                }
            }

            # Metadata
            if ($worker.metadata -and $worker.metadata.PSObject.Properties.Count -gt 0) {
                Write-Host "  Metadata:"
                foreach ($key in ($worker.metadata.PSObject.Properties | Sort-Object Name)) {
                    Write-Host "    - $($key.Name): $($key.Value)"
                }
            }
        }

        Write-Host ""
        Write-Host ("="*80) -ForegroundColor Cyan

        # Summary
        $totalWorkers = $workers.workers.Count
        $availableCount = ($workers.workers | Where-Object { $_.status -eq 'available' }).Count
        $busyCount = ($workers.workers | Where-Object { $_.status -eq 'busy' }).Count
        $offlineCount = ($workers.workers | Where-Object { $_.status -eq 'offline' }).Count

        Write-Host "Total: $totalWorkers worker(s)" -ForegroundColor Green -NoNewline
        Write-Host " | Available: " -NoNewline
        Write-Host $availableCount -ForegroundColor Green -NoNewline
        Write-Host " | Busy: " -NoNewline
        Write-Host $busyCount -ForegroundColor Yellow -NoNewline
        Write-Host " | Offline: " -NoNewline
        Write-Host $offlineCount -ForegroundColor Red

        if ($FilterStatus -ne 'all' -or $FilterCapability) {
            Write-Host "Showing: $($filteredWorkers.Count) worker(s) (filtered)" -ForegroundColor Cyan
        }

        Write-Host ""
    }
}
