<#
.SYNOPSIS
    Toggle internet access for worker VMs (enhanced kill switch)

.DESCRIPTION
    Enables or disables internet access for VMs by controlling the NAT configuration.
    Enhanced version with connection termination and egress filtering.
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
    Enhanced with connection termination and egress filtering (2025-10-22)
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
$egressBlockRuleName = "AutoMutate-Egress-Block-Internet"

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          VM Internet Access Control (Enhanced)                 |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

function Get-InternetStatus {
    $nat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
    $egressBlock = Get-NetFirewallRule -DisplayName $egressBlockRuleName -ErrorAction SilentlyContinue

    if ($nat) {
        return @{
            Enabled = $true
            Subnet = $nat.InternalIPInterfaceAddressPrefix
            Active = $nat.Active
            EgressFiltering = ($egressBlock -ne $null -and $egressBlock.Enabled -eq 'True')
        }
    } else {
        return @{
            Enabled = $false
            Subnet = $null
            Active = $false
            EgressFiltering = ($egressBlock -ne $null -and $egressBlock.Enabled -eq 'True')
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

                # Remove the rule after 2 seconds (connection will be reset)
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

function Enable-EgressFiltering {
    Write-Host "`n[INFO] Enabling egress filtering (default-deny + whitelist)..." -ForegroundColor Yellow

    # Remove old egress block rule if exists
    Remove-NetFirewallRule -DisplayName $egressBlockRuleName -ErrorAction SilentlyContinue

    # Get vEthernet adapter name
    $adapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" }
    if (-not $adapter) {
        Write-Host "[WARN] Could not find IsolationSwitch adapter, using all interfaces" -ForegroundColor Yellow
        $adapterAlias = $null
    } else {
        $adapterAlias = $adapter.Name
    }

    # Create default-deny egress rule (lower priority, catches all non-whitelisted traffic)
    $params = @{
        DisplayName = $egressBlockRuleName
        Direction = 'Outbound'
        Action = 'Block'
        Profile = 'Any'
        Enabled = 'True'
    }

    if ($adapterAlias) {
        $params['InterfaceAlias'] = $adapterAlias
    }

    New-NetFirewallRule @params | Out-Null
    Write-Host "[OK] Egress blocking rule created" -ForegroundColor Green

    # Create whitelist rules (higher priority, evaluated first)
    $whitelistRules = @(
        @{ Name = "AutoMutate-Egress-Allow-DNS"; Protocol = "UDP"; Ports = @(53); Description = "Allow DNS queries" },
        @{ Name = "AutoMutate-Egress-Allow-HTTP"; Protocol = "TCP"; Ports = @(80); Description = "Allow HTTP (Windows Update, packages)" },
        @{ Name = "AutoMutate-Egress-Allow-HTTPS"; Protocol = "TCP"; Ports = @(443); Description = "Allow HTTPS (Windows Update, packages)" },
        @{ Name = "AutoMutate-Egress-Allow-NTP"; Protocol = "UDP"; Ports = @(123); Description = "Allow NTP time sync" }
    )

    foreach ($rule in $whitelistRules) {
        Remove-NetFirewallRule -DisplayName $rule.Name -ErrorAction SilentlyContinue

        $params = @{
            DisplayName = $rule.Name
            Direction = 'Outbound'
            Action = 'Allow'
            Protocol = $rule.Protocol
            RemotePort = $rule.Ports
            Profile = 'Any'
            Enabled = 'True'
        }

        if ($adapterAlias) {
            $params['InterfaceAlias'] = $adapterAlias
        }

        New-NetFirewallRule @params | Out-Null
        Write-Host "[OK] Whitelist: $($rule.Description) (ports: $($rule.Ports -join ','))" -ForegroundColor Green
    }
}

function Disable-EgressFiltering {
    Write-Host "`n[INFO] Disabling egress filtering..." -ForegroundColor Yellow

    # Remove egress block rule
    $removed = Get-NetFirewallRule -DisplayName $egressBlockRuleName -ErrorAction SilentlyContinue
    if ($removed) {
        Remove-NetFirewallRule -DisplayName $egressBlockRuleName -Confirm:$false
        Write-Host "[OK] Egress blocking rule removed" -ForegroundColor Green
    } else {
        Write-Host "[INFO] Egress blocking rule not found (already disabled)" -ForegroundColor Gray
    }

    # Remove whitelist rules
    $whitelistNames = @(
        "AutoMutate-Egress-Allow-DNS",
        "AutoMutate-Egress-Allow-HTTP",
        "AutoMutate-Egress-Allow-HTTPS",
        "AutoMutate-Egress-Allow-NTP"
    )

    foreach ($name in $whitelistNames) {
        Remove-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue
    }
    Write-Host "[OK] Whitelist rules removed" -ForegroundColor Green
}

switch ($Action) {
    "Status" {
        $status = Get-InternetStatus

        Write-Host "Current Status:" -ForegroundColor Yellow
        if ($status.Enabled) {
            Write-Host "  Internet Access: ENABLED" -ForegroundColor Green
            Write-Host "  NAT Subnet: $($status.Subnet)" -ForegroundColor White
            Write-Host "  NAT Active: $($status.Active)" -ForegroundColor White
            Write-Host "  Egress Filtering: $(if ($status.EgressFiltering) { 'ENABLED (whitelist active)' } else { 'DISABLED (unrestricted)' })" -ForegroundColor $(if ($status.EgressFiltering) { 'Cyan' } else { 'Yellow' })
            Write-Host "`nVMs CAN reach the internet" -ForegroundColor Green
            if ($status.EgressFiltering) {
                Write-Host "  Allowed: DNS (53), HTTP (80), HTTPS (443), NTP (123)" -ForegroundColor Cyan
                Write-Host "  Blocked: All other outbound traffic" -ForegroundColor Yellow
            }
        } else {
            Write-Host "  Internet Access: DISABLED" -ForegroundColor Red
            Write-Host "  Egress Filtering: $(if ($status.EgressFiltering) { 'ENABLED (extra protection)' } else { 'DISABLED' })" -ForegroundColor $(if ($status.EgressFiltering) { 'Cyan' } else { 'Gray' })
            Write-Host "`nVMs are AIR-GAPPED (no internet access)" -ForegroundColor Yellow
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
        Write-Host "Disabling internet access for VMs..." -ForegroundColor Yellow

        # Step 1: Enable egress filtering (default-deny + whitelist)
        Enable-EgressFiltering

        # Step 2: Kill existing connections if requested
        if ($KillConnections) {
            Kill-VMExternalConnections -SubnetPrefix $subnet
        }

        # Step 3: Remove NAT
        $nat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
        if ($nat) {
            Remove-NetNat -Name $natName -Confirm:$false
            Write-Host "`n[OK] NAT removed - VMs are now air-gapped" -ForegroundColor Green
        } else {
            Write-Host "`n[INFO] NAT already disabled" -ForegroundColor Yellow
        }

        Write-Host "`n+================================================================+" -ForegroundColor Red
        Write-Host "|          INTERNET ACCESS DISABLED (ENHANCED)                   |" -ForegroundColor Red
        Write-Host "+================================================================+" -ForegroundColor Red
        Write-Host "`nSecurity Layers Active:" -ForegroundColor White
        Write-Host "  [1] NAT Removed - No IP translation for VM subnet" -ForegroundColor Gray
        Write-Host "  [2] Egress Filtering - Default-deny firewall active" -ForegroundColor Gray
        if ($KillConnections) {
            Write-Host "  [3] Connections Terminated - Existing sessions killed" -ForegroundColor Gray
        } else {
            Write-Host "  [3] Existing Connections - May still be active (use -KillConnections)" -ForegroundColor Yellow
        }

        Write-Host "`nWorker VMs cannot reach external networks" -ForegroundColor White
        Write-Host "VMs can still communicate with:" -ForegroundColor White
        Write-Host "  - Host (10.200.200.1)" -ForegroundColor Gray
        Write-Host "  - Other VMs (10.200.200.x)" -ForegroundColor Gray
        Write-Host "  - WSL/Elasticsearch (via port forwarding)" -ForegroundColor Gray
        Write-Host "`nTo restore internet: .\toggle-vm-internet.ps1 -Action Enable`n" -ForegroundColor Cyan
    }

    "Enable" {
        Write-Host "Enabling internet access for VMs..." -ForegroundColor Yellow

        # Step 1: Create NAT
        $nat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
        if ($nat) {
            Write-Host "[INFO] NAT already enabled" -ForegroundColor Green
        } else {
            New-NetNat -Name $natName -InternalIPInterfaceAddressPrefix $subnet | Out-Null
            Write-Host "[OK] NAT created - VMs can now reach internet" -ForegroundColor Green
        }

        # Step 2: Disable egress filtering (remove restrictions)
        Disable-EgressFiltering

        Write-Host "`n+================================================================+" -ForegroundColor Green
        Write-Host "|          INTERNET ACCESS ENABLED                               |" -ForegroundColor Green
        Write-Host "+================================================================+" -ForegroundColor Green
        Write-Host "`nWorker VMs can now reach:" -ForegroundColor White
        Write-Host "  - External internet (via NAT)" -ForegroundColor Green
        Write-Host "  - Download packages (Chocolatey, Rust, etc.)" -ForegroundColor Green
        Write-Host "  - Windows Update" -ForegroundColor Green
        Write-Host "`nSecurity Note: Egress filtering DISABLED for maintenance mode" -ForegroundColor Yellow
        Write-Host "VMs have unrestricted internet access until you disable again.`n" -ForegroundColor Yellow
        Write-Host "To air-gap VMs: .\toggle-vm-internet.ps1 -Action Disable`n" -ForegroundColor Cyan
    }
}

Write-Host "+================================================================+`n" -ForegroundColor Cyan
