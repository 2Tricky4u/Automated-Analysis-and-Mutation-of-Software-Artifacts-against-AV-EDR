# Check current RedEDR configuration and status

Write-Host "=== RedEDR Status Check ===" -ForegroundColor Cyan
Write-Host ""

# 1. Check if RedEDR is running
Write-Host "1. RedEDR Process Status:" -ForegroundColor Yellow
$process = Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue
if ($process) {
    Write-Host "  ✓ RedEDR is running (PID: $($process.Id))" -ForegroundColor Green

    # Get command line arguments (requires WMI)
    try {
        $wmi = Get-WmiObject Win32_Process -Filter "ProcessId = $($process.Id)"
        Write-Host "  Command line: $($wmi.CommandLine)" -ForegroundColor Gray
    } catch {
        Write-Host "  Could not retrieve command line arguments" -ForegroundColor Yellow
    }
} else {
    Write-Host "  ✗ RedEDR is NOT running" -ForegroundColor Red
}
Write-Host ""

# 2. Check RedEDR API stats
Write-Host "2. RedEDR API Stats:" -ForegroundColor Yellow
try {
    $stats = Invoke-RestMethod -Uri "http://localhost:8081/api/stats" -Method Get -ErrorAction Stop
    Write-Host "  Total events: $($stats.events_count)" -ForegroundColor Gray
    Write-Host "  Kernel events: $($stats.num_kernel)" -ForegroundColor Gray
    Write-Host "  ETW events: $($stats.num_etw)" -ForegroundColor Gray
    Write-Host "  ETW-TI events: $($stats.num_etwti)" -ForegroundColor Gray
    Write-Host "  DLL events: $($stats.num_dll)" -ForegroundColor $(if ($stats.num_dll -gt 0) { "Green" } else { "Red" })
} catch {
    Write-Host "  ✗ Failed to connect to RedEDR API: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

# 3. Get sample events and check configuration
Write-Host "3. RedEDR Configuration (from events):" -ForegroundColor Yellow
try {
    $events = Invoke-RestMethod -Uri "http://localhost:8081/api/logs/rededr" -Method Get -ErrorAction Stop

    # Find the 'meta' event which contains config
    $metaEvent = $events | Where-Object { $_.type -eq "meta" } | Select-Object -First 1

    if ($metaEvent) {
        Write-Host "  RedEDR version: $($metaEvent.version)" -ForegroundColor Gray
        Write-Host "  ETW enabled: $($metaEvent.do_etw)" -ForegroundColor $(if ($metaEvent.do_etw) { "Green" } else { "Red" })
        Write-Host "  ETW-TI enabled: $($metaEvent.do_etwti)" -ForegroundColor $(if ($metaEvent.do_etwti) { "Green" } else { "Red" })
        Write-Host "  Hooks enabled: $($metaEvent.do_hook)" -ForegroundColor $(if ($metaEvent.do_hook) { "Red" } else { "Red" })
        Write-Host "  Hook callstack: $($metaEvent.do_hook_callstack)" -ForegroundColor Gray
        Write-Host "  Target: $($metaEvent.target)" -ForegroundColor Gray

        if (-not $metaEvent.do_hook) {
            Write-Host ""
            Write-Host "  ⚠️  KERNEL HOOKS ARE DISABLED!" -ForegroundColor Red
            Write-Host "  This is why you don't see DLL events." -ForegroundColor Yellow
        }
    } else {
        Write-Host "  No meta event found (RedEDR may not have been initialized)" -ForegroundColor Yellow
    }

    # Count event types
    Write-Host ""
    Write-Host "  Event type distribution:" -ForegroundColor Gray
    $events | Group-Object -Property type | Sort-Object Count -Descending | ForEach-Object {
        Write-Host "    $($_.Name): $($_.Count)" -ForegroundColor Gray
    }
} catch {
    Write-Host "  Failed to retrieve events: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

# 4. Check scheduled task configuration
Write-Host "4. Scheduled Task Configuration:" -ForegroundColor Yellow
try {
    $task = Get-ScheduledTask -TaskName "AutoMutate-RedEDR-SYSTEM" -ErrorAction Stop
    if ($task) {
        Write-Host "  Task status: $($task.State)" -ForegroundColor $(if ($task.State -eq "Running") { "Green" } else { "Yellow" })

        # Get action arguments
        $action = $task.Actions | Select-Object -First 1
        Write-Host "  Executable: $($action.Execute)" -ForegroundColor Gray
        Write-Host "  Arguments: $($action.Arguments)" -ForegroundColor Gray

        # Check if -k flag is present
        if ($action.Arguments -match "-k") {
            Write-Host "  ✓ -k flag IS configured in task" -ForegroundColor Green
        } else {
            Write-Host "  ✗ -k flag NOT found in task arguments!" -ForegroundColor Red
        }
    }
} catch {
    Write-Host "  Scheduled task not found or error: $($_.Exception.Message)" -ForegroundColor Yellow
}
Write-Host ""

# 5. Recommendations
Write-Host "=== Recommendations ===" -ForegroundColor Cyan
Write-Host ""

$needsRestart = $false

if (-not $process) {
    Write-Host "❌ RedEDR is not running. Start it with:" -ForegroundColor Red
    Write-Host "   cd C:\RedEdr" -ForegroundColor Gray
    Write-Host "   .\Start-RedEDR-SYSTEM.ps1" -ForegroundColor Gray
    $needsRestart = $true
}

if ($metaEvent -and -not $metaEvent.do_hook) {
    Write-Host "❌ RedEDR is running WITHOUT kernel hooks (-k flag)." -ForegroundColor Red
    Write-Host "   This is why DLL events are missing." -ForegroundColor Yellow
    Write-Host ""
    Write-Host "   To fix:" -ForegroundColor Yellow
    Write-Host "   1. Stop RedEDR: .\Start-RedEDR-SYSTEM.ps1 -StopOnly" -ForegroundColor Gray
    Write-Host "   2. Start with hooks: .\Start-RedEDR-SYSTEM.ps1" -ForegroundColor Gray
    Write-Host ""
    Write-Host "   Note: Kernel hooks require:" -ForegroundColor Yellow
    Write-Host "     - bcdedit /set testsigning on" -ForegroundColor Gray
    Write-Host "     - Secure Boot disabled (Hyper-V VM setting)" -ForegroundColor Gray
    Write-Host "     - Reboot after bcdedit" -ForegroundColor Gray
    $needsRestart = $true
}

if ($metaEvent -and $metaEvent.do_hook) {
    Write-Host "✓ RedEDR is correctly configured with kernel hooks!" -ForegroundColor Green
    Write-Host "  If you still don't see DLL events, the artifact may not be loading DLLs." -ForegroundColor Yellow
}

if (-not $needsRestart -and $metaEvent) {
    Write-Host "✓ RedEDR configuration looks correct!" -ForegroundColor Green
}

Write-Host ""
Write-Host "=== Check complete ===" -ForegroundColor Cyan
