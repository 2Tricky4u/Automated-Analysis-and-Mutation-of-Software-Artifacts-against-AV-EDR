<#
.SYNOPSIS
    Toggle internet access for worker VMs (kill switch)

.DESCRIPTION
    Enables or disables internet access for VMs by controlling the NAT configuration.
    Perfect for testing artifacts in an air-gapped environment.

.PARAMETER Action
    "Disable" to cut internet, "Enable" to restore internet, "Status" to check

.EXAMPLE
    .\toggle-vm-internet.ps1 -Action Disable   # Cut internet for all VMs
    .\toggle-vm-internet.ps1 -Action Enable    # Restore internet
    .\toggle-vm-internet.ps1 -Action Status    # Check current state

.NOTES
    Must be run as Administrator
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet("Enable", "Disable", "Status")]
    [string]$Action
)

$ErrorActionPreference = "Stop"

# Check admin
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "[ERROR] This script must be run as Administrator" -ForegroundColor Red
    exit 1
}

# Load config to get subnet
$ConfigPath = Join-Path $PSScriptRoot "..\config.yaml"
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$natName = "AutoMutateVMNAT"
$subnet = $config.network.subnet

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          VM Internet Access Control                            |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

function Get-InternetStatus {
    $nat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
    if ($nat) {
        return @{
            Enabled = $true
            Subnet = $nat.InternalIPInterfaceAddressPrefix
            Active = $nat.Active
        }
    } else {
        return @{
            Enabled = $false
            Subnet = $null
            Active = $false
        }
    }
}

switch ($Action) {
    "Status" {
        $status = Get-InternetStatus

        Write-Host "Current Status:" -ForegroundColor Yellow
        if ($status.Enabled) {
            Write-Host "  Internet Access: ENABLED" -ForegroundColor Green
            Write-Host "  NAT Subnet: $($status.Subnet)" -ForegroundColor White
            Write-Host "  NAT Active: $($status.Active)" -ForegroundColor White
            Write-Host "`nVMs CAN reach the internet" -ForegroundColor Green
        } else {
            Write-Host "  Internet Access: DISABLED" -ForegroundColor Red
            Write-Host "`nVMs are AIR-GAPPED (no internet access)" -ForegroundColor Yellow
        }
    }

    "Disable" {
        Write-Host "Disabling internet access for VMs..." -ForegroundColor Yellow

        $nat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
        if ($nat) {
            Remove-NetNat -Name $natName -Confirm:$false
            Write-Host "[OK] NAT removed - VMs are now air-gapped" -ForegroundColor Green
        } else {
            Write-Host "[INFO] NAT already disabled" -ForegroundColor Yellow
        }

        Write-Host "`n+================================================================+" -ForegroundColor Red
        Write-Host "|          INTERNET ACCESS DISABLED                              |" -ForegroundColor Red
        Write-Host "+================================================================+" -ForegroundColor Red
        Write-Host "`nWorker VMs cannot reach external networks" -ForegroundColor White
        Write-Host "VMs can still communicate with:" -ForegroundColor White
        Write-Host "  - Host (10.200.200.1)" -ForegroundColor Gray
        Write-Host "  - Other VMs (10.200.200.x)" -ForegroundColor Gray
        Write-Host "  - WSL/Elasticsearch (via port forwarding)" -ForegroundColor Gray
        Write-Host "`nTo restore internet: .\toggle-vm-internet.ps1 -Action Enable`n" -ForegroundColor Cyan
    }

    "Enable" {
        Write-Host "Enabling internet access for VMs..." -ForegroundColor Yellow

        $nat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
        if ($nat) {
            Write-Host "[INFO] NAT already enabled" -ForegroundColor Green
        } else {
            New-NetNat -Name $natName -InternalIPInterfaceAddressPrefix $subnet | Out-Null
            Write-Host "[OK] NAT created - VMs can now reach internet" -ForegroundColor Green
        }

        Write-Host "`n+================================================================+" -ForegroundColor Green
        Write-Host "|          INTERNET ACCESS ENABLED                               |" -ForegroundColor Green
        Write-Host "+================================================================+" -ForegroundColor Green
        Write-Host "`nWorker VMs can now reach:" -ForegroundColor White
        Write-Host "  - External internet (via NAT)" -ForegroundColor Green
        Write-Host "  - Download packages (Chocolatey, Rust, etc.)" -ForegroundColor Green
        Write-Host "  - Windows Update" -ForegroundColor Green
        Write-Host "`nTo air-gap VMs: .\toggle-vm-internet.ps1 -Action Disable`n" -ForegroundColor Cyan
    }
}

Write-Host "+================================================================+`n" -ForegroundColor Cyan
