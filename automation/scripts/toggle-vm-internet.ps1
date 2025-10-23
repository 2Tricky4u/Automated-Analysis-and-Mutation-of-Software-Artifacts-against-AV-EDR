<#
.SYNOPSIS
    Toggle internet access for worker VMs (NAT kill switch)

.DESCRIPTION
    Enables or disables internet access for VMs by controlling the NAT configuration ONLY.
    Does NOT manage egress filtering (use manage-egress-filter.ps1 for that).
    Perfect for testing artifacts in an air-gapped environment.

.PARAMETER Action
    "Disable" to cut internet, "Enable" to restore internet, "Status" to check

.PARAMETER KillConnections
    When disabling, forcefully terminate existing TCP connections to external IPs

.PARAMETER Force
    Skip confirmation prompts

.EXAMPLE
    .\toggle-vm-internet.ps1 -Action Disable   # Cut internet for all VMs
    .\toggle-vm-internet.ps1 -Action Disable -KillConnections   # Also kill existing connections
    .\toggle-vm-internet.ps1 -Action Enable    # Restore internet
    .\toggle-vm-internet.ps1 -Action Status    # Check current state

.NOTES
    Must be run as Administrator
    This script ONLY controls NAT (internet on/off)
    Use manage-egress-filter.ps1 to control traffic whitelisting
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet("Enable", "Disable", "Status")]
    [string]$Action,

    [Parameter()]
    [switch]$KillConnections,

    [Parameter()]
    [switch]$Force
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

    # Check egress filtering status
    $egressBlock = Get-NetFirewallRule -DisplayName "AutoMutate-Egress-Block-Internet" -ErrorAction SilentlyContinue

    if ($nat) {
        return @{
            Enabled = $true
            Subnet = $nat.InternalIPInterfaceAddressPrefix
            Active = $nat.Active
            EgressFilteringActive = ($egressBlock -ne $null -and $egressBlock.Enabled -eq 'True')
        }
    } else {
        return @{
            Enabled = $false
            Subnet = $null
            Active = $false
            EgressFilteringActive = ($egressBlock -ne $null -and $egressBlock.Enabled -eq 'True')
        }
    }
}

function Kill-VMExternalConnections {
    param([string]$SubnetPrefix)

    Write-Host "`n[INFO] Terminating existing external connections from VMs..." -ForegroundColor Yellow

    # Extract subnet prefix (e.g., "10.200.200" from "10.200.200.0/24")
    $subnetBase = $SubnetPrefix.Split('/')[0]
    $subnetPattern = ($subnetBase.Split('.')[0..2] -join '.') + '.*'

    # Get all TCP connections originating from VM subnet
    $connections = Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object {
        $_.LocalAddress -like $subnetPattern -and
        $_.RemoteAddress -notlike $subnetPattern -and
        $_.RemoteAddress -notlike "127.0.0.*" -and
        $_.State -eq 'Established'
    }

    if ($connections) {
        $killCount = 0
        foreach ($conn in $connections) {
            try {
                # Create temporary firewall rule to block this specific connection
                $ruleName = "AutoMutate-Kill-$($conn.OwningProcess)-$($conn.LocalPort)"
                New-NetFirewallRule -DisplayName $ruleName -Direction Outbound -Action Block `
                    -Protocol TCP -LocalAddress $conn.LocalAddress -LocalPort $conn.LocalPort `
                    -RemoteAddress $conn.RemoteAddress -RemotePort $conn.RemotePort `
                    -ErrorAction SilentlyContinue | Out-Null

                # Remove the rule after 2 seconds
                Start-Sleep -Milliseconds 100
                Remove-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue

                $killCount++
                Write-Host "  [x] Killed: $($conn.LocalAddress):$($conn.LocalPort) -> $($conn.RemoteAddress):$($conn.RemotePort)" -ForegroundColor Gray
            } catch {
                # Ignore errors (connection might have closed naturally)
            }
        }
        Write-Host "[OK] Terminated $killCount external connection(s)" -ForegroundColor Green
    } else {
        Write-Host "[OK] No active external connections found" -ForegroundColor Green
    }
}

switch ($Action) {
    "Status" {
        $status = Get-InternetStatus

        Write-Host "Current Status:" -ForegroundColor Yellow
        if ($status.Enabled) {
            Write-Host "  Internet Access (NAT): ENABLED" -ForegroundColor Green
            Write-Host "  NAT Subnet: $($status.Subnet)" -ForegroundColor White
            Write-Host "  NAT Active: $($status.Active)" -ForegroundColor White
            Write-Host "`nVMs CAN reach the internet" -ForegroundColor Green
        } else {
            Write-Host "  Internet Access (NAT): DISABLED" -ForegroundColor Red
            Write-Host "`nVMs are AIR-GAPPED (no internet access)" -ForegroundColor Yellow
        }

        # Show egress filtering status
        Write-Host "`nEgress Filtering (managed separately):" -ForegroundColor Cyan
        if ($status.EgressFilteringActive) {
            Write-Host "  Status: ENABLED (whitelist active)" -ForegroundColor Yellow
            Write-Host "  Manage: .\manage-egress-filter.ps1 -Action Status|Enable|Disable" -ForegroundColor Gray
        } else {
            Write-Host "  Status: DISABLED (unrestricted traffic when NAT enabled)" -ForegroundColor Gray
            Write-Host "  Manage: .\manage-egress-filter.ps1 -Action Enable" -ForegroundColor Gray
        }

        # Show active external connections
        $subnetBase = $subnet.Split('/')[0]
        $subnetPattern = ($subnetBase.Split('.')[0..2] -join '.') + '.*'
        $activeConns = Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object {
            $_.LocalAddress -like $subnetPattern -and
            $_.RemoteAddress -notlike $subnetPattern -and
            $_.RemoteAddress -notlike "127.0.0.*" -and
            $_.State -eq 'Established'
        }
        if ($activeConns) {
            Write-Host "`n  Active External Connections: $($activeConns.Count)" -ForegroundColor Yellow
            Write-Host "  (Use -KillConnections with Disable to terminate)" -ForegroundColor Gray
        }
    }

    "Disable" {
        Write-Host "Disabling internet access for VMs (removing NAT)..." -ForegroundColor Yellow

        # Kill existing connections if requested
        if ($KillConnections) {
            Kill-VMExternalConnections -SubnetPrefix $subnet
        }

        # Remove NAT
        $nat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
        if ($nat) {
            Remove-NetNat -Name $natName -Confirm:$false
            Write-Host "`n[OK] NAT removed - VMs are now air-gapped" -ForegroundColor Green
        } else {
            Write-Host "`n[INFO] NAT already disabled" -ForegroundColor Yellow
        }

        Write-Host "`n+================================================================+" -ForegroundColor Red
        Write-Host "|          INTERNET ACCESS DISABLED                              |" -ForegroundColor Red
        Write-Host "+================================================================+" -ForegroundColor Red
        Write-Host "`nWhat changed:" -ForegroundColor White
        Write-Host "  [x] NAT Removed - No IP translation for VM subnet" -ForegroundColor Red
        if ($KillConnections) {
            Write-Host "  [x] Connections Terminated - Existing sessions killed" -ForegroundColor Red
        } else {
            Write-Host "  [!] Existing Connections - May still be active (use -KillConnections)" -ForegroundColor Yellow
        }

        Write-Host "`nWorker VMs cannot reach external networks" -ForegroundColor White
        Write-Host "VMs can still communicate with:" -ForegroundColor White
        Write-Host "  - Host (10.200.200.1)" -ForegroundColor Gray
        Write-Host "  - Other VMs (10.200.200.x)" -ForegroundColor Gray
        Write-Host "  - WSL/Elasticsearch (via port forwarding)" -ForegroundColor Gray

        Write-Host "`nNote: Egress filtering is NOT managed by this script" -ForegroundColor Cyan
        Write-Host "      Use .\manage-egress-filter.ps1 to control traffic whitelisting" -ForegroundColor Cyan

        Write-Host "`nTo restore internet: .\toggle-vm-internet.ps1 -Action Enable`n" -ForegroundColor Cyan
    }

    "Enable" {
        Write-Host "Enabling internet access for VMs (creating NAT)..." -ForegroundColor Yellow

        # Create NAT
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
        Write-Host "`nWhat changed:" -ForegroundColor White
        Write-Host "  [+] NAT Created - IP translation enabled for VM subnet" -ForegroundColor Green

        Write-Host "`nWorker VMs can now reach:" -ForegroundColor White
        Write-Host "  - External internet (via NAT)" -ForegroundColor Green
        Write-Host "  - Download packages (Chocolatey, Rust, etc.)" -ForegroundColor Green
        Write-Host "  - Windows Update" -ForegroundColor Green

        # Check egress filtering status
        $egressBlock = Get-NetFirewallRule -DisplayName "AutoMutate-Egress-Block-Internet" -ErrorAction SilentlyContinue
        if ($egressBlock -and $egressBlock.Enabled -eq 'True') {
            Write-Host "`nEgress Filtering: ENABLED (traffic restricted to whitelist)" -ForegroundColor Yellow
            Write-Host "  Allowed: DNS (53), HTTP (80), HTTPS (443), NTP (123)" -ForegroundColor Cyan
            Write-Host "  Blocked: All other outbound traffic" -ForegroundColor Yellow
            Write-Host "  Manage: .\manage-egress-filter.ps1 -Action Disable" -ForegroundColor Gray
        } else {
            Write-Host "`nEgress Filtering: DISABLED (unrestricted traffic)" -ForegroundColor Gray
            Write-Host "  VMs have full internet access (no traffic filtering)" -ForegroundColor White
            Write-Host "  To restrict: .\manage-egress-filter.ps1 -Action Enable" -ForegroundColor Cyan
        }

        Write-Host "`nTo air-gap VMs: .\toggle-vm-internet.ps1 -Action Disable`n" -ForegroundColor Cyan
    }
}

Write-Host "+================================================================+`n" -ForegroundColor Cyan
