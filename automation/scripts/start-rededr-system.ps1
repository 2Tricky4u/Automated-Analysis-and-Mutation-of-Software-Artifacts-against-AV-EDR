<#
.SYNOPSIS
    Start RedEDR with SYSTEM privileges (required for ETW tracing)

.DESCRIPTION
    Launches RedEDR.exe as NT AUTHORITY\SYSTEM using a scheduled task.
    This is required for ETW trace collection and kernel-level telemetry.

.PARAMETER Mode
    Tracing mode: "all", "etw", "kernel", "etwti", "dll" (default: "all")

.PARAMETER WebUI
    Enable web UI (default: true)

.PARAMETER WebUIPort
    Web UI port (default: 8080)

.PARAMETER TraceTarget
    Optional: target process to trace (e.g., "notepad.exe")

.PARAMETER StopOnly
    Stop any running RedEDR instance without starting a new one

.EXAMPLE
    .\start-rededr-system.ps1
    # Start RedEDR with all tracing enabled + web UI

.EXAMPLE
    .\start-rededr-system.ps1 -Mode etw -TraceTarget "malware.exe"
    # Trace specific executable with ETW only

.EXAMPLE
    .\start-rededr-system.ps1 -StopOnly
    # Stop running RedEDR instance

.NOTES
    Must be run as Administrator (will elevate to SYSTEM automatically)
    RedEDR must be installed at C:\RedEDR (default from 04-vm-init.ps1)
#>

[CmdletBinding()]
param(
    [Parameter()]
    [ValidateSet("all", "etw", "kernel", "etwti", "dll")]
    [string]$Mode = "all",

    [Parameter()]
    [bool]$WebUI = $true,

    [Parameter()]
    [int]$WebUIPort = 8080,

    [Parameter()]
    [string]$TraceTarget = "",

    [Parameter()]
    [switch]$StopOnly
)

$ErrorActionPreference = "Stop"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

# Check admin privileges
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Err "This script must be run as Administrator"
    Write-Info "Right-click PowerShell and select 'Run as Administrator'"
    exit 1
}

$RedEdrRoot = "C:\RedEDR"
$RedEdrExe = Join-Path $RedEdrRoot "RedEdr.exe"
$TaskName = "AutoMutate-RedEDR-SYSTEM"

Write-Host @"

+================================================================+
|          RedEDR SYSTEM Launcher                                |
+================================================================+

"@ -ForegroundColor Cyan

# Verify RedEDR installation
if (-not (Test-Path $RedEdrExe)) {
    Write-Err "RedEDR not found at: $RedEdrExe"
    Write-Info "Run 04-vm-init.ps1 to install RedEDR"
    exit 1
}

# Stop existing instance
Write-Info "Checking for existing RedEDR instances..."

# Stop scheduled task if running
$existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existingTask) {
    Write-Info "Stopping scheduled task: $TaskName"
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2

    Write-Info "Removing scheduled task: $TaskName"
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Success "Stopped existing RedEDR instance"
}

# Kill any lingering RedEdr.exe processes
$rededrProcs = Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue
if ($rededrProcs) {
    Write-Info "Terminating $($rededrProcs.Count) RedEdr.exe process(es)..."
    $rededrProcs | Stop-Process -Force
    Start-Sleep -Seconds 2
    Write-Success "Terminated RedEdr.exe processes"
}

if ($StopOnly) {
    Write-Success "RedEDR stopped (no new instance started)"
    exit 0
}

# Build command-line arguments
$args = @()

switch ($Mode) {
    "all" {
        $args += "--all"
        Write-Info "Mode: All tracing (ETW + Kernel + ETW-TI + DLL)"
    }
    "etw" {
        $args += "--etw"
        Write-Info "Mode: ETW only"
    }
    "kernel" {
        $args += "--kernel"
        Write-Info "Mode: Kernel callbacks only"
    }
    "etwti" {
        $args += "--etwti"
        Write-Info "Mode: ETW-TI (PPL) only"
    }
    "dll" {
        $args += "--dll"
        Write-Info "Mode: DLL injection only"
    }
}

if ($WebUI) {
    $args += "--web"
    $args += "--web-port"
    $args += "$WebUIPort"
    Write-Info "Web UI: Enabled (http://localhost:$WebUIPort)"
}

# IMPORTANT: Do NOT use --hide flag with scheduled tasks
# Scheduled tasks run without a console, and --hide causes immediate exit
# RedEDR will run in the background automatically when launched via scheduled task

if ($TraceTarget) {
    Write-Info "Target: $TraceTarget (monitoring mode - stays alive)"
    Write-Info "      RedEDR will continuously monitor for '$TraceTarget'"
    Write-Info "      Process will stay running even if target doesn't exist yet"

    # Create wrapper script that keeps RedEDR alive
    $wrapperScript = Join-Path $RedEdrRoot "RedEDR-Wrapper-Monitor.ps1"
    $wrapperContent = @"
`$ErrorActionPreference = 'Continue'
while (`$true) {
    Write-Host "[Monitor] Waiting for process: $TraceTarget" -ForegroundColor Cyan

    # Wait for target process to exist
    while (-not (Get-Process -Name '$($TraceTarget -replace '\.exe$','')' -ErrorAction SilentlyContinue)) {
        Start-Sleep -Seconds 5
    }

    Write-Host "[Monitor] Process '$TraceTarget' detected - starting trace" -ForegroundColor Green

    # Start RedEDR to trace the process
    `$rededrArgs = @($($args | ForEach-Object { '"' + $_ + '"' }), '--trace', '"$TraceTarget"')
    & "$RedEdrExe" `$rededrArgs

    Write-Host "[Monitor] RedEDR exited - restarting monitor loop" -ForegroundColor Yellow
    Start-Sleep -Seconds 2
}
"@

    $wrapperContent | Out-File -FilePath $wrapperScript -Encoding UTF8 -Force
    Write-Info "Created monitoring wrapper: $wrapperScript"

    # Task runs the wrapper script instead of RedEDR directly
    # Use full path to PowerShell and proper escaping for SYSTEM context
    $psPath = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    $psArgs = "-ExecutionPolicy Bypass -NoProfile -NonInteractive -WindowStyle Hidden -Command `"& '$wrapperScript'`""

    $action = New-ScheduledTaskAction -Execute $psPath `
        -Argument $psArgs `
        -WorkingDirectory $RedEdrRoot

    Write-Info "PowerShell: $psPath"
    Write-Info "Arguments: $psArgs"

    $usingWrapper = $true
} else {
    Write-Info "Target: All processes (system-wide)"

    $argsString = $args -join " "
    Write-Host ""
    Write-Info "Creating scheduled task to run as SYSTEM..."
    Write-Info "Command: $RedEdrExe $argsString"

    # Create scheduled task action (direct execution)
    $action = New-ScheduledTaskAction -Execute $RedEdrExe -Argument $argsString -WorkingDirectory $RedEdrRoot

    $usingWrapper = $false
}

# Run as SYSTEM with highest privileges
$principal = New-ScheduledTaskPrincipal -UserId "NT AUTHORITY\SYSTEM" -LogonType ServiceAccount -RunLevel Highest

# Task settings - configured to keep process alive indefinitely
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -DontStopOnIdleEnd `
    -ExecutionTimeLimit (New-TimeSpan -Days 0) `  # 0 = no limit (run forever)
    -RestartInterval (New-TimeSpan -Minutes 1) `  # Restart if crashes
    -RestartCount 999  # Unlimited restarts

# Register task
try {
    Register-ScheduledTask -TaskName $TaskName -Action $action -Principal $principal -Settings $settings -Force | Out-Null
    Write-Success "Scheduled task created: $TaskName"
} catch {
    Write-Err "Failed to create scheduled task: $($_.Exception.Message)"
    exit 1
}

# Start task
Write-Info "Starting RedEDR as SYSTEM..."
try {
    Start-ScheduledTask -TaskName $TaskName
    Write-Info "Task started, waiting for process to initialize..."

    # Wait up to 10 seconds for process to start
    $maxWait = 10
    $waited = 0
    while ($waited -lt $maxWait) {
        Start-Sleep -Seconds 1
        $waited++

        # Check if process exists early
        if (Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue) {
            Write-Info "Process detected after $waited second(s)"
            break
        }
    }

    # Check task info for exit code
    $taskInfo = Get-ScheduledTaskInfo -TaskName $TaskName
    $taskState = (Get-ScheduledTask -TaskName $TaskName).State
    Write-Info "Task state: $taskState"
    Write-Info "Last result: $($taskInfo.LastTaskResult) (0 = success, 1 = running)"

    # Verify process started (check for wrapper OR RedEDR process)
    if ($usingWrapper) {
        # When using wrapper, check for PowerShell monitor process
        # Note: CommandLine property requires Get-CimInstance, not Get-Process
        $wrapperProc = Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ErrorAction SilentlyContinue |
            Where-Object { $_.CommandLine -like "*RedEDR-Wrapper-Monitor.ps1*" } |
            Select-Object -First 1

        if ($wrapperProc) {
            Write-Success "Monitoring wrapper started successfully"
            Write-Info "Wrapper Process ID: $($wrapperProc.Id)"
            Write-Success "Verified: Running as SYSTEM (monitoring for '$TraceTarget')"
            Write-Info "Note: Wrapper will stay alive and launch RedEDR when target appears"

            # Check if target already exists
            $targetProc = Get-Process -Name ($TraceTarget -replace '\.exe$','') -ErrorAction SilentlyContinue
            if ($targetProc) {
                Write-Info "Target process '$TraceTarget' is already running"
                Write-Info "RedEDR should attach within 5-10 seconds"
            } else {
                Write-Info "Target process '$TraceTarget' not yet running"
                Write-Info "Wrapper is monitoring - will start tracing when it appears"
            }
        } else {
            Write-Warn "Monitoring wrapper process not detected"
            Write-Info "Task last result: $($taskInfo.LastTaskResult) (0 = success)"

            # Check if wrapper script exists
            if (Test-Path $wrapperScript) {
                Write-Info "Wrapper script exists at: $wrapperScript"

                # Try to read task history for better diagnostics
                Write-Host ""
                Write-Info "Checking Task Scheduler event log..."
                $taskEvents = Get-WinEvent -LogName "Microsoft-Windows-TaskScheduler/Operational" -MaxEvents 20 -ErrorAction SilentlyContinue |
                    Where-Object { $_.Message -like "*$TaskName*" } |
                    Select-Object -First 5

                if ($taskEvents) {
                    foreach ($event in $taskEvents) {
                        $msg = $event.Message.Split("`n")[0]
                        if ($event.LevelDisplayName -eq "Error") {
                            Write-Host "  [ERROR Event $($event.Id)] $msg" -ForegroundColor Red
                        } else {
                            Write-Host "  [Event $($event.Id)] $msg" -ForegroundColor Gray
                        }
                    }
                }

                Write-Host ""
                Write-Err "PowerShell wrapper failed to start"
                Write-Info "Possible causes:"
                Write-Info "  1. Execution policy blocking script (should be bypassed)"
                Write-Info "  2. Syntax error in wrapper script"
                Write-Info "  3. RedEDR.exe path incorrect"
                Write-Host ""
                Write-Info "Troubleshooting steps:"
                Write-Host "  1. Test wrapper manually (as Admin):" -ForegroundColor Yellow
                Write-Host "     powershell -ExecutionPolicy Bypass -File `"$wrapperScript`"" -ForegroundColor Gray
                Write-Host ""
                Write-Host "  2. Try simple mode instead:" -ForegroundColor Yellow
                Write-Host "     .\start-rededr-simple.ps1" -ForegroundColor Gray
                Write-Host ""
                Write-Host "  3. View wrapper script:" -ForegroundColor Yellow
                Write-Host "     Get-Content `"$wrapperScript`"" -ForegroundColor Gray
            } else {
                Write-Err "Wrapper script not found: $wrapperScript"
            }

            exit 1
        }
    } else {
        # Direct execution - check for RedEDR process
        $rededrProc = Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue
        if ($rededrProc) {
            $procUser = (Get-CimInstance -ClassName Win32_Process -Filter "ProcessId = $($rededrProc.Id)").GetOwner()
            $username = "$($procUser.Domain)\$($procUser.User)"

            Write-Success "RedEDR started successfully"
            Write-Info "Process ID: $($rededrProc.Id)"
            Write-Info "Running as: $username"

            if ($username -eq "NT AUTHORITY\SYSTEM") {
                Write-Success "Verified: Running as SYSTEM (ETW tracing enabled)"
            } else {
                Write-Warn "Warning: Not running as SYSTEM ($username)"
                Write-Info "ETW tracing may fail with permission errors"
            }
        } else {
            Write-Warn "RedEDR process not detected (process started but exited)"
            Write-Info "Task last result: $($taskInfo.LastTaskResult) (0 = success)"

            # Check event log for task execution details
            Write-Host ""
            Write-Info "Checking Task Scheduler event log for errors..."
            $taskEvents = Get-WinEvent -LogName "Microsoft-Windows-TaskScheduler/Operational" -MaxEvents 10 -ErrorAction SilentlyContinue |
                Where-Object { $_.Message -like "*$TaskName*" } |
                Select-Object -First 3

            if ($taskEvents) {
                foreach ($event in $taskEvents) {
                    Write-Host "  [Event $($event.Id)] $($event.Message.Split("`n")[0])" -ForegroundColor Gray
                }
            }

            Write-Host ""
            Write-Err "Possible causes:"
            Write-Info "  1. RedEDR.exe exited immediately"
            Write-Info "  2. Missing dependencies (check C:\RedEDR for all files)"
            Write-Info "  3. Command-line arguments invalid"
            Write-Info "  4. Drivers not loaded (check: sc query RedEdrPplService)"
            Write-Host ""
            Write-Info "Troubleshooting steps:"
            Write-Host "  1. Test manually: .\test-rededr-manual.ps1" -ForegroundColor Yellow
            Write-Host "  2. Try simple mode: .\start-rededr-simple.ps1" -ForegroundColor Yellow
            Write-Host "  3. Check task: Get-ScheduledTaskInfo -TaskName $TaskName" -ForegroundColor Yellow

            exit 1
        }
    }
} catch {
    Write-Err "Failed to start scheduled task: $($_.Exception.Message)"
    Write-Info "Check Task Scheduler manually: taskschd.msc"
    exit 1
}

Write-Host ""
Write-Host "+================================================================+" -ForegroundColor Green
Write-Host "|          RedEDR Started as SYSTEM                              |" -ForegroundColor Green
Write-Host "+================================================================+" -ForegroundColor Green

if ($WebUI) {
    Write-Host ""
    Write-Info "Access web UI: http://localhost:$WebUIPort"
}

Write-Host ""
Write-Info "To stop RedEDR:"
Write-Host "  .\start-rededr-system.ps1 -StopOnly" -ForegroundColor Yellow

Write-Host ""
Write-Info "To check status:"
Write-Host "  Get-Process -Name RedEdr" -ForegroundColor Yellow
Write-Host "  Get-ScheduledTask -TaskName $TaskName" -ForegroundColor Yellow

Write-Host ""
Write-Info "Logs (if enabled):"
Write-Host "  C:\RedEDR\logs\" -ForegroundColor Yellow

exit 0
