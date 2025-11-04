<#
.SYNOPSIS
    Harden VM integration services for malware lab security

.DESCRIPTION
    Disables risky Hyper-V integration services to reduce VM detection and escape vectors.

    Security levels:
    - Minimal: Only Heartbeat + Time Sync (recommended for production runs)
    - Development: Add Guest Service Interface (for file sync during development)
    - Paranoid: Disable everything (maximum isolation)
    - Default: Restore default integration services

.PARAMETER VMName
    Name of the VM(s) to harden. Use "*worker*" for all workers.

.PARAMETER Level
    Security level: Minimal (default), Development, Paranoid, Default

.PARAMETER ShowStatus
    Only show current status without making changes

.EXAMPLE
    # Harden all worker VMs (recommended)
    .\harden-vm-integration.ps1 -VMName "*worker*" -Level Minimal

.EXAMPLE
    # Enable for development (temporary)
    .\harden-vm-integration.ps1 -VMName "win10-worker-00" -Level Development

.EXAMPLE
    # Maximum security (disable everything)
    .\harden-vm-integration.ps1 -VMName "win10-worker-00" -Level Paranoid

.EXAMPLE
    # Show current status
    .\harden-vm-integration.ps1 -VMName "win10-worker-00" -ShowStatus

.EXAMPLE
    # Restore defaults
    .\harden-vm-integration.ps1 -VMName "win10-worker-00" -Level Default
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [ValidateSet("Minimal", "Development", "Paranoid", "Default")]
    [string]$Level = "Minimal",

    [switch]$ShowStatus
)

$ErrorActionPreference = "Stop"

# Colors
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERR] $M" -ForegroundColor Red }
function Write-Secure  { param($M) Write-Host "[SECURE] $M" -ForegroundColor Green }
function Write-Risk    { param($M) Write-Host "[RISK] $M" -ForegroundColor Red }

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "VM Integration Services Hardening" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

# Get VMs
$VMs = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if ($VMs.Count -eq 0) {
    Write-Err "No VMs found matching: $VMName"
    exit 1
}

Write-Info "Found $($VMs.Count) VM(s)"

# Integration service configurations
$Configurations = @{
    "Minimal" = @{
        Description = "Minimal attack surface (recommended for malware execution)"
        Enable = @("Heartbeat", "Time Synchronization")
        Disable = @("Guest Service Interface", "PowerShell Direct", "Key-Value Pair Exchange", "VSS", "Shutdown")
    }
    "Development" = @{
        Description = "Enable file sync for development (temporary use only)"
        Enable = @("Heartbeat", "Time Synchronization", "Guest Service Interface")
        Disable = @("PowerShell Direct", "Key-Value Pair Exchange", "VSS")
    }
    "Paranoid" = @{
        Description = "Maximum isolation (disable everything)"
        Enable = @()
        Disable = @("Heartbeat", "Time Synchronization", "Guest Service Interface", "PowerShell Direct", "Key-Value Pair Exchange", "VSS", "Shutdown")
    }
    "Default" = @{
        Description = "Default Hyper-V configuration (least secure)"
        Enable = @("Heartbeat", "Time Synchronization", "Guest Service Interface", "Shutdown", "VSS", "Key-Value Pair Exchange")
        Disable = @()
    }
}

# Security risk ratings
$RiskRatings = @{
    "Heartbeat" = @{Risk = "Low"; Reason = "Host monitors VM health only, no data to guest"}
    "Time Synchronization" = @{Risk = "Low"; Reason = "Prevents time-based detection"}
    "Guest Service Interface" = @{Risk = "Medium"; Reason = "File copy = VM detection + injection vector"}
    "PowerShell Direct" = @{Risk = "High"; Reason = "Remote code execution channel"}
    "Key-Value Pair Exchange" = @{Risk = "Medium"; Reason = "Data sharing channel"}
    "VSS" = @{Risk = "Low"; Reason = "Backup integration, rarely exploited"}
    "Shutdown" = @{Risk = "Low"; Reason = "Host can shutdown guest, minimal risk"}
}

# Function to display integration service status
function Show-IntegrationStatus {
    param([string]$VMName)

    Write-Host "`n----------------------------------------" -ForegroundColor Cyan
    Write-Info "VM: $VMName"
    Write-Host "----------------------------------------`n" -ForegroundColor Cyan

    $Services = Get-VMIntegrationService -VMName $VMName

    # Create status table
    $StatusTable = @()
    foreach ($Service in $Services) {
        $Risk = $RiskRatings[$Service.Name]
        $StatusTable += [PSCustomObject]@{
            Service = $Service.Name
            Enabled = $Service.Enabled
            Status = $Service.PrimaryStatusDescription
            Risk = if ($Risk) { $Risk.Risk } else { "Unknown" }
            Reason = if ($Risk) { $Risk.Reason } else { "" }
        }
    }

    $StatusTable | Format-Table -AutoSize

    # Security score
    $EnabledRiskyServices = $StatusTable | Where-Object {
        $_.Enabled -and ($_.Risk -eq "High" -or $_.Risk -eq "Medium")
    }

    Write-Host "`nSecurity Assessment:" -ForegroundColor Yellow
    if ($EnabledRiskyServices.Count -eq 0) {
        Write-Secure "No high-risk services enabled"
    } else {
        Write-Risk "$($EnabledRiskyServices.Count) risky service(s) enabled:"
        foreach ($Svc in $EnabledRiskyServices) {
            Write-Host "  - $($Svc.Service) [Risk: $($Svc.Risk)]" -ForegroundColor Red
        }
    }
}

# Show status only
if ($ShowStatus) {
    foreach ($VM in $VMs) {
        Show-IntegrationStatus -VMName $VM.Name
    }
    exit 0
}

# Apply configuration
$Config = $Configurations[$Level]

Write-Info "Security Level: $Level"
Write-Info "Description: $($Config.Description)"
Write-Host ""

# Warn about development mode
if ($Level -eq "Development") {
    Write-Warn "Development mode enables file sync but increases attack surface"
    Write-Warn "Use only during active development, then switch to 'Minimal'"
    Write-Host ""
    $Response = Read-Host "Continue? (y/N)"
    if ($Response -ne 'y' -and $Response -ne 'Y') {
        Write-Info "Aborted by user"
        exit 0
    }
}

# Confirm for paranoid mode
if ($Level -eq "Paranoid") {
    Write-Warn "Paranoid mode disables ALL integration services"
    Write-Warn "Host cannot monitor VM health or sync time"
    Write-Warn "VM may have time drift issues"
    Write-Host ""
    $Response = Read-Host "Continue? (y/N)"
    if ($Response -ne 'y' -and $Response -ne 'Y') {
        Write-Info "Aborted by user"
        exit 0
    }
}

# Warn about default mode
if ($Level -eq "Default") {
    Write-Warn "Default mode is LEAST SECURE"
    Write-Warn "Not recommended for malware analysis lab"
    Write-Host ""
    $Response = Read-Host "Continue? (y/N)"
    if ($Response -ne 'y' -and $Response -ne 'Y') {
        Write-Info "Aborted by user"
        exit 0
    }
}

# Apply to each VM
foreach ($VM in $VMs) {
    Write-Host "`n========================================" -ForegroundColor Cyan
    Write-Info "Hardening VM: $($VM.Name)"
    Write-Host "========================================" -ForegroundColor Cyan

    # Get all integration services
    $AllServices = Get-VMIntegrationService -VMName $VM.Name

    # Disable specified services
    if ($Config.Disable.Count -gt 0) {
        Write-Info "Disabling risky services..."
        foreach ($ServiceName in $Config.Disable) {
            $Service = $AllServices | Where-Object { $_.Name -eq $ServiceName }
            if ($Service) {
                if ($Service.Enabled) {
                    Disable-VMIntegrationService -VMName $VM.Name -Name $ServiceName
                    $Risk = $RiskRatings[$ServiceName]
                    Write-Secure "Disabled: $ServiceName [Risk: $($Risk.Risk)]"
                } else {
                    Write-Info "Already disabled: $ServiceName"
                }
            }
        }
    }

    # Enable specified services
    if ($Config.Enable.Count -gt 0) {
        Write-Info "`nEnabling essential services..."
        foreach ($ServiceName in $Config.Enable) {
            $Service = $AllServices | Where-Object { $_.Name -eq $ServiceName }
            if ($Service) {
                if (-not $Service.Enabled) {
                    Enable-VMIntegrationService -VMName $VM.Name -Name $ServiceName
                    $Risk = $RiskRatings[$ServiceName]
                    Write-Success "Enabled: $ServiceName [Risk: $($Risk.Risk)]"
                } else {
                    Write-Info "Already enabled: $ServiceName"
                }
            }
        }
    }

    # Show final status
    Write-Host ""
    Show-IntegrationStatus -VMName $VM.Name
}

Write-Host "`n========================================" -ForegroundColor Green
Write-Host "Hardening Complete!" -ForegroundColor Green
Write-Host "========================================`n" -ForegroundColor Green

# Recommendations based on level
switch ($Level) {
    "Minimal" {
        Write-Info "Recommendations:"
        Write-Info "  ✓ VM is now hardened for malware execution"
        Write-Info "  ✓ Use SMB-based sync for file transfers:"
        Write-Info "    .\sync-project-via-smb.ps1 -VMName '$($VMs[0].Name)' -VMIPAddress '10.200.200.100'"
        Write-Info "  ✓ Take a baseline snapshot:"
        Write-Info "    Checkpoint-VM -Name '$($VMs[0].Name)' -SnapshotName 'Baseline-Hardened'"
    }
    "Development" {
        Write-Warn "Development mode enabled (temporary use only)"
        Write-Info ""
        Write-Info "When done with development, revert to Minimal:"
        Write-Info "  .\harden-vm-integration.ps1 -VMName '$($VMs[0].Name)' -Level Minimal"
    }
    "Paranoid" {
        Write-Warn "Paranoid mode: All integration services disabled"
        Write-Info ""
        Write-Info "Host cannot:"
        Write-Info "  - Monitor VM health"
        Write-Info "  - Sync time (VM may drift)"
        Write-Info "  - Gracefully shutdown VM"
        Write-Info ""
        Write-Info "Use SMB for all file transfers"
    }
    "Default" {
        Write-Warn "Default mode: Multiple attack vectors enabled"
        Write-Info ""
        Write-Info "Consider using 'Minimal' for better security"
    }
}

Write-Host ""
