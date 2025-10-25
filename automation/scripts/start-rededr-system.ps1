<#
.SYNOPSIS
    Start RedEDR as SYSTEM for either "hooking" (kernel+APC inject) or "etw" (ETW+ETW-TI),
    and trace a chosen target process. Keeps RedEDR alive via a SYSTEM scheduled task.

.DESCRIPTION
    - "hooking": uses ntdll.dll hooking via KAPC DLL injection
        Equivalent to: .\RedEdr.exe --kernel --inject --trace <proc>
        Note: Requires self-signed kernel modules to load.

    - "etw": enables ETW + ETW-TI
        Equivalent to: .\RedEdr.exe --etw --etwti --trace <proc>
        Notes:
          * ETW-TI requires an ELAM driver to start RedEdrPplService (self-signed kernel driver).
            Make a VM snapshot first; the PPL service cannot be removed cleanly afterwards.
          * For Microsoft-Windows-Security-Auditing, start as SYSTEM and configure auditing in gpedit.msc.

    The script prompts for mode and target process if not provided, writes a monitoring wrapper
    that restarts RedEDR if it exits, and registers a SYSTEM scheduled task that runs at startup
    and can be launched immediately.

.PARAMETER Mode
    "hooking" or "etw". If omitted, you will be prompted.

.PARAMETER TraceTarget
    Process to observe, e.g. "notepad.exe". If omitted, you will be prompted.

.PARAMETER WebUI
    Enable web UI (default: $true)

.PARAMETER WebUIPort
    Web UI port (default: 8080)

.PARAMETER StopOnly
    Stop any running RedEDR instance and remove the scheduled task without starting a new one.

.EXAMPLE
    .\start-rededr-system.ps1
    # Prompted flow; choose mode and target, runs as SYSTEM and keeps it alive.

.EXAMPLE
    .\start-rededr-system.ps1 -Mode hooking -TraceTarget notepad.exe
    # KAPC DLL injection (kernel+inject) against notepad.exe, runs as SYSTEM.

.EXAMPLE
    .\start-rededr-system.ps1 -Mode etw -TraceTarget notepad.exe -WebUI:$false
    # ETW + ETW-TI tracing of notepad.exe, no web UI.

.NOTES
    * Must be run as Administrator.
    * RedEDR must exist at C:\RedEDR\RedEdr.exe
    * For ETW/ETW-TI/kernel features you may need self-signed kernel modules and ELAM.
#>

[CmdletBinding()]
param(
    [Parameter()]
    [ValidateSet("hooking","etw")]
    [string]$Mode,

    [Parameter()]
    [string]$TraceTarget,

    [Parameter()]
    [bool]$WebUI = $true,

    [Parameter()]
    [int]$WebUIPort = 8080,

    [Parameter()]
    [switch]$StopOnly
)

$ErrorActionPreference = "Stop"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

# --- Admin check ---
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Err "Run this script in an elevated PowerShell (Run as Administrator)."
    exit 1
}

$RedEdrRoot = "C:\RedEDR"
$RedEdrExe  = Join-Path $RedEdrRoot "RedEdr.exe"
$TaskName   = "AutoMutate-RedEDR-SYSTEM"
$Wrapper    = Join-Path $RedEdrRoot "RedEDR-Wrapper-Monitor.ps1"
$WrapperLog = Join-Path $RedEdrRoot "RedEDR-Wrapper.log"

Write-Host @"
+================================================================+
|                    RedEDR SYSTEM Launcher                      |
+================================================================+
"@ -ForegroundColor Cyan

# --- Verify files ---
if (-not (Test-Path $RedEdrExe)) {
    Write-Err "RedEDR not found at: $RedEdrExe"
    Write-Info "Ensure RedEDR is installed to C:\RedEDR."
    exit 1
}

# --- Stop/clean previous task if requested OR before recreating ---
Write-Info "Checking for existing scheduled task: $TaskName"
$existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existingTask) {
    Write-Info "Stopping and removing existing task..."
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue | Out-Null
    Start-Sleep -Seconds 1
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Success "Previous task removed."
}

# Kill lingering processes
$rededrProcs = Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue
if ($rededrProcs) {
    Write-Info "Terminating lingering RedEdr.exe process(es)..."
    $rededrProcs | Stop-Process -Force
    Start-Sleep -Seconds 1
    Write-Success "Terminated lingering RedEdr.exe."
}

if ($StopOnly) {
    Write-Success "Stopped and cleaned up. No new instance started (-StopOnly)."
    exit 0
}

# --- Interactive prompts if not provided ---
if (-not $Mode) {
    Write-Host ""
    Write-Host "Choose tracing mode:" -ForegroundColor Cyan
    Write-Host "  [1] Hooking (kernel + APC inject)  ->  --kernel --inject --trace <proc>"
    Write-Host "      Requires self-signed kernel modules." -ForegroundColor DarkYellow
    Write-Host "  [2] ETW (ETW + ETW-TI)             ->  --etw --etwti --trace <proc>"
    Write-Host "      ETW-TI requires ELAM driver and PPL service (irreversible install)." -ForegroundColor DarkYellow
    $sel = Read-Host "Enter 1 or 2"
    switch ($sel) {
        "1" { $Mode = "hooking" }
        "2" { $Mode = "etw" }
        default { Write-Err "Invalid selection."; exit 1 }
    }
}

if (-not $TraceTarget -or [string]::IsNullOrWhiteSpace($TraceTarget)) {
    $TraceTarget = Read-Host "Enter process name to trace (e.g., notepad.exe)"
    if (-not $TraceTarget) { Write-Err "A trace target is required."; exit 1 }
}

# Normalize target (keep .exe if provided; strip spaces)
$TraceTarget = $TraceTarget.Trim()

# --- Build RedEDR args based on mode ---
$args = @()

switch ($Mode) {
    "hooking" {
        Write-Info "Mode: Hooking (kernel + APC DLL injection)"
        Write-Warn "Requires self-signed kernel modules (driver load)."
        $args += "--kernel"
        $args += "--inject"
        $args += "--trace"
        $args += $TraceTarget
    }
    "etw" {
        Write-Info "Mode: ETW + ETW-TI"
        Write-Warn "ETW-TI requires ELAM and starting RedEdrPplService (PPL install is not reversible). Snapshot your VM first."
        $args += "--etw"
        $args += "--etwti"
        $args += "--trace"
        $args += $TraceTarget
    }
    default {
        Write-Err "Unsupported mode: $Mode"
        exit 1
    }
}

if ($WebUI) {
    $args += "--web"
    $args += "--web-port"
    $args += "$WebUIPort"
    Write-Info "Web UI: http://localhost:$WebUIPort"
}

# --- Write a resilient wrapper that keeps RedEDR alive & logs output ---
$wrapperContent = @"
`$ErrorActionPreference = 'Continue'
`$exe = '$RedEdrExe'
`$log = '$WrapperLog'
`$args = @($(($args | ForEach-Object { "'$_'" }) -join ", "))

while (`$true) {
  try {
    # Compose a readable line in the logfile
    "`$(Get-Date -Format o) Launching: `$exe `$($args -join ' ')" | Add-Content `$log
    & `$exe @args *> `$log 2>&1
    "`$(Get-Date -Format o) RedEDR exited with code `$LASTEXITCODE. Restarting in 2s..." | Add-Content `$log
  } catch {
    "`$(Get-Date -Format o) ERROR: `$($_.Exception.Message)" | Add-Content `$log
  }
  Start-Sleep -Seconds 2
}
"@

$null = New-Item -ItemType Directory -Path $RedEdrRoot -Force
$wrapperContent | Out-File -FilePath $Wrapper -Encoding UTF8 -Force
Write-Success "Created wrapper: $Wrapper"
Write-Info    "Wrapper log: $WrapperLog"

# --- Register SYSTEM scheduled task to run wrapper at startup and now ---
$psPath = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
$psArgs = "-ExecutionPolicy Bypass -NoProfile -Command `"& '$Wrapper'`""

$action    = New-ScheduledTaskAction -Execute $psPath -Argument $psArgs -WorkingDirectory $RedEdrRoot
$trigger   = New-ScheduledTaskTrigger -AtStartup
$principal = New-ScheduledTaskPrincipal -UserId "NT AUTHORITY\SYSTEM" -LogonType ServiceAccount -RunLevel Highest
$settings  = New-ScheduledTaskSettingsSet `
    -StartWhenAvailable `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit (New-TimeSpan -Days 0) `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -RestartCount  999

try {
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
    Write-Success "Scheduled task created: $TaskName"
} catch {
    Write-Err "Failed to create scheduled task: $($_.Exception.Message)"
    exit 1
}

# --- Start now ---
Write-Info "Starting RedEDR as SYSTEM via scheduled task..."
try {
    Start-ScheduledTask -TaskName $TaskName
    Start-Sleep -Seconds 2
    Write-Info "Task started. Tailing last 20 lines of wrapper log (Ctrl+C to stop):"
    if (Test-Path $WrapperLog) {
        Get-Content $WrapperLog -Tail 20
    } else {
        Write-Warn "Log file not yet created. It will appear at: $WrapperLog"
    }
} catch {
    Write-Err "Failed to start scheduled task: $($_.Exception.Message)"
    exit 1
}

Write-Host ""
Write-Host "+================================================================+" -ForegroundColor Green
Write-Host "|         RedEDR scheduled as SYSTEM and set to keep alive       |" -ForegroundColor Green
Write-Host "+================================================================+" -ForegroundColor Green
Write-Host ""
Write-Info "Task: $TaskName    (runs at startup, restarts on failure)"
Write-Info "Wrapper log: $WrapperLog"
Write-Info "Trace target: $TraceTarget"
Write-Info "Mode: $Mode"
if ($WebUI) { Write-Info "Web UI: http://localhost:$WebUIPort" }

Write-Host ""
Write-Info "To stop and remove:"
Write-Host "  Unregister-ScheduledTask -TaskName $TaskName -Confirm:`$false" -ForegroundColor Yellow
Write-Host "  Get-Process RedEdr -ErrorAction SilentlyContinue | Stop-Process -Force" -ForegroundColor Yellow
