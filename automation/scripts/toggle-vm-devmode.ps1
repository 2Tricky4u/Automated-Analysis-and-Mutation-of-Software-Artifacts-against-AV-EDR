<#
.SYNOPSIS
    Quick toggle between secure mode and development mode

.DESCRIPTION
    Convenience wrapper for common workflows:
    - DevMode: Enable file sync temporarily for development
    - SecureMode: Disable risky services for malware execution

.PARAMETER VMName
    Name of the VM to toggle

.PARAMETER Mode
    Mode to switch to: Dev or Secure

.EXAMPLE
    # Enable development mode (for syncing files)
    .\toggle-vm-devmode.ps1 -VMName "win10-worker-00" -Mode Dev

.EXAMPLE
    # Return to secure mode (before running artifacts)
    .\toggle-vm-devmode.ps1 -VMName "win10-worker-00" -Mode Secure
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [Parameter(Mandatory)]
    [ValidateSet("Dev", "Secure")]
    [string]$Mode
)

$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot

switch ($Mode) {
    "Dev" {
        Write-Host "`n[DEV MODE] Enabling file sync capabilities..." -ForegroundColor Yellow
        & "$ScriptDir\harden-vm-integration.ps1" -VMName $VMName -Level Development
        Write-Host "`nYou can now sync files:" -ForegroundColor Cyan
        Write-Host "  .\sync-project-to-vm.ps1 -VMName '$VMName'" -ForegroundColor White
        Write-Host "  .\sync-project-incremental.ps1 -VMName '$VMName'" -ForegroundColor White
        Write-Host "`nRemember to switch back to Secure mode before executing artifacts!" -ForegroundColor Yellow
    }
    "Secure" {
        Write-Host "`n[SECURE MODE] Hardening VM for malware execution..." -ForegroundColor Green
        & "$ScriptDir\harden-vm-integration.ps1" -VMName $VMName -Level Minimal
        Write-Host "`nVM is now hardened. Use SMB for file transfers:" -ForegroundColor Cyan
        Write-Host "  .\sync-project-via-smb.ps1 -VMName '$VMName' -VMIPAddress '10.200.200.100'" -ForegroundColor White
    }
}

Write-Host ""
