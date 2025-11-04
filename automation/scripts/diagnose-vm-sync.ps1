<#
.SYNOPSIS
    Diagnose VM sync issues (integration services, permissions, connectivity)

.DESCRIPTION
    Checks common issues that prevent syncing projects to VMs:
    - Hyper-V integration services status
    - VM heartbeat and connectivity
    - File system permissions
    - PowerShell Direct access

.PARAMETER VMName
    Name of the VM to diagnose

.EXAMPLE
    .\diagnose-vm-sync.ps1 -VMName "win10-worker-00"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName
)

$ErrorActionPreference = "Stop"

# Colors
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERR] $M" -ForegroundColor Red }

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "VM Sync Diagnostics: $VMName" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

# Check 1: VM exists
Write-Info "[1/7] Checking if VM exists..."
$VM = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if (-not $VM) {
    Write-Err "VM not found: $VMName"
    Write-Info "Available VMs:"
    Get-VM | Select-Object Name, State | Format-Table
    exit 1
}
Write-Success "VM exists: $VMName"

# Check 2: VM state
Write-Info "[2/7] Checking VM state..."
if ($VM.State -ne "Running") {
    Write-Err "VM is not running (state: $($VM.State))"
    Write-Info "Start the VM first: Start-VM -Name '$VMName'"
    exit 1
}
Write-Success "VM is running"

# Check 3: Integration services
Write-Info "[3/7] Checking integration services..."
$IntegrationServices = Get-VMIntegrationService -VMName $VMName

Write-Host "`nIntegration Services Status:" -ForegroundColor Cyan
$IntegrationServices | Select-Object Name, Enabled, PrimaryStatusDescription | Format-Table

$GuestService = $IntegrationServices | Where-Object { $_.Name -eq "Guest Service Interface" }
if (-not $GuestService) {
    Write-Err "Guest Service Interface not found"
    exit 1
}

if (-not $GuestService.Enabled) {
    Write-Warn "Guest Service Interface is DISABLED"
    Write-Info "Attempting to enable..."
    Enable-VMIntegrationService -VMName $VMName -Name "Guest Service Interface"
    Start-Sleep -Seconds 3
    $GuestService = Get-VMIntegrationService -VMName $VMName -Name "Guest Service Interface"
    if ($GuestService.Enabled) {
        Write-Success "Guest Service Interface enabled"
    } else {
        Write-Err "Failed to enable Guest Service Interface"
        exit 1
    }
} else {
    Write-Success "Guest Service Interface is enabled"
}

# Check 4: Heartbeat
Write-Info "[4/7] Checking VM heartbeat..."
$Heartbeat = $IntegrationServices | Where-Object { $_.Name -eq "Heartbeat" }
if ($Heartbeat.PrimaryStatusDescription -eq "OK") {
    Write-Success "Heartbeat: OK"
} else {
    Write-Warn "Heartbeat: $($Heartbeat.PrimaryStatusDescription)"
    Write-Info "VM may not be fully booted or integration services need updating"
}

# Check 5: PowerShell Direct
Write-Info "[5/7] Testing PowerShell Direct..."
try {
    $Result = Invoke-Command -VMName $VMName -ScriptBlock {
        $env:COMPUTERNAME
    } -ErrorAction Stop

    Write-Success "PowerShell Direct works: $Result"
} catch {
    Write-Err "PowerShell Direct failed: $($_.Exception.Message)"
    Write-Info "Try updating integration services in the VM"
    exit 1
}

# Check 6: Destination directory
Write-Info "[6/7] Checking destination directory..."
try {
    $DestCheck = Invoke-Command -VMName $VMName -ScriptBlock {
        param($Path)
        $Exists = Test-Path $Path
        if (-not $Exists) {
            try {
                New-Item -ItemType Directory -Path $Path -Force | Out-Null
                return "created"
            } catch {
                return "failed: $($_.Exception.Message)"
            }
        }
        return "exists"
    } -ArgumentList "C:\AutoMutate\dev" -ErrorAction Stop

    Write-Success "Destination directory: $DestCheck"
} catch {
    Write-Err "Failed to check destination: $($_.Exception.Message)"
}

# Check 7: Test file copy
Write-Info "[7/7] Testing file copy..."
$TestFile = [System.IO.Path]::GetTempFileName()
Set-Content -Path $TestFile -Value "AutoMutate++ sync test"

try {
    $TestDest = "C:\AutoMutate\dev\sync-test.txt"
    Copy-VMFile -VMName $VMName -SourcePath $TestFile -DestinationPath $TestDest -FileSource Host -Force -ErrorAction Stop

    # Verify
    $Verify = Invoke-Command -VMName $VMName -ScriptBlock {
        param($Path)
        if (Test-Path $Path) {
            $Content = Get-Content $Path
            Remove-Item $Path -Force
            return $Content
        }
        return $null
    } -ArgumentList $TestDest

    if ($Verify -eq "AutoMutate++ sync test") {
        Write-Success "File copy test: PASSED"
    } else {
        Write-Warn "File copy test: Content mismatch"
    }
} catch {
    Write-Err "File copy test: FAILED"
    Write-Err "Error: $($_.Exception.Message)"
    Write-Info ""
    Write-Info "Possible solutions:"
    Write-Info "  1. Update integration services in the VM:"
    Write-Info "     - Insert vmguest.iso in VM"
    Write-Info "     - Run D:\setup.exe in VM"
    Write-Info "     - Reboot VM"
    Write-Info ""
    Write-Info "  2. Check disk space in VM"
    Write-Info ""
    Write-Info "  3. Restart VM and try again"
} finally {
    Remove-Item $TestFile -Force -ErrorAction SilentlyContinue
}

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "Diagnostics Complete" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

Write-Info "If all checks passed, try syncing again:"
Write-Info "  .\sync-project-to-vm.ps1 -VMName '$VMName'"
Write-Host ""
