<#
.SYNOPSIS
    Toggle VM-to-VM isolation (prevent lateral movement)

.DESCRIPTION
    Controls whether VMs can communicate with each other on the subnet.
    When enabled, VMs can ONLY communicate with the host (10.200.200.1).
    When disabled, VMs can communicate with each other (default behavior).

.PARAMETER Action
    "Enable" to isolate VMs, "Disable" to allow VM-to-VM communication, "Status" to check

.EXAMPLE
    .\toggle-vm-isolation.ps1 -Action Enable    # Isolate VMs (no VM-to-VM traffic)
    .\toggle-vm-isolation.ps1 -Action Disable   # Allow VM-to-VM communication
    .\toggle-vm-isolation.ps1 -Action Status    # Check current state

.NOTES
    Must be run as Administrator
    This is useful for preventing malware lateral movement during experiments
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

# Load config
$ConfigPath = Join-Path $PSScriptRoot "..\config.yaml"
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$isolationRuleName = "AutoMutate-VM-Isolation-Block"
$subnet = $config.network.subnet
$hostIP = $config.network.host_ip

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          VM-to-VM Isolation Control                            |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

function Get-IsolationStatus {
    $blockRule = Get-NetFirewallRule -DisplayName $isolationRuleName -ErrorAction SilentlyContinue

    if ($blockRule -and $blockRule.Enabled -eq 'True') {
        return @{
            Enabled = $true
            RuleExists = $true
        }
    } else {
        return @{
            Enabled = $false
            RuleExists = ($blockRule -ne $null)
        }
    }
}

function Enable-VMIsolation {
    Write-Host "[INFO] Enabling VM-to-VM isolation..." -ForegroundColor Yellow

    # Remove old rule if exists
    Remove-NetFirewallRule -DisplayName $isolationRuleName -ErrorAction SilentlyContinue

    # Get vEthernet adapter
    $adapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" }
    if (-not $adapter) {
        Write-Host "[ERROR] Could not find IsolationSwitch adapter" -ForegroundColor Red
        exit 1
    }
    $adapterAlias = $adapter.Name

    # Extract subnet base (e.g., "10.200.200.0/24" -> "10.200.200.0-255")
    $subnetBase = $subnet.Split('/')[0]
    $prefix = [int]$subnet.Split('/')[1]

    # Calculate subnet range for firewall rule
    # For /24: 10.200.200.0-10.200.200.255
    $octets = $subnetBase.Split('.')
    $subnetRange = "$($octets[0]).$($octets[1]).$($octets[2]).0-$($octets[0]).$($octets[1]).$($octets[2]).255"

    # Create inbound rule: Block traffic FROM other VMs TO this VM
    $inboundParams = @{
        DisplayName = "$isolationRuleName-Inbound"
        Direction = 'Inbound'
        Action = 'Block'
        Protocol = 'Any'
        InterfaceAlias = $adapterAlias
        RemoteAddress = $subnetRange
        Profile = 'Any'
        Enabled = 'True'
    }
    New-NetFirewallRule @inboundParams | Out-Null
    Write-Host "[OK] Inbound isolation rule created (block FROM other VMs)" -ForegroundColor Green

    # CRITICAL: Create Allow rules FIRST (both directions) to ensure host access
    # Remove old rules
    Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host" -ErrorAction SilentlyContinue
    Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Outbound" -ErrorAction SilentlyContinue
    Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Inbound" -ErrorAction SilentlyContinue

    # Allow outbound to host
    New-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Outbound" -Direction Outbound -Action Allow `
        -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $hostIP -Profile Any -Enabled True | Out-Null

    # Allow inbound from host
    New-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Inbound" -Direction Inbound -Action Allow `
        -Protocol Any -InterfaceAlias $adapterAlias -RemoteAddress $hostIP -Profile Any -Enabled True | Out-Null

    Write-Host "[OK] Allow rules for host ($hostIP) created (inbound + outbound)" -ForegroundColor Green

    # Block all other subnet traffic (outbound)
    $outboundParams = @{
        DisplayName = "$isolationRuleName-Outbound"
        Direction = 'Outbound'
        Action = 'Block'
        Protocol = 'Any'
        InterfaceAlias = $adapterAlias
        RemoteAddress = $subnetRange
        Profile = 'Any'
        Enabled = 'True'
    }
    New-NetFirewallRule @outboundParams | Out-Null
    Write-Host "[OK] Outbound isolation rule created (block TO other VMs)" -ForegroundColor Green

    Write-Host "`n+================================================================+" -ForegroundColor Yellow
    Write-Host "|          VM-TO-VM ISOLATION ENABLED                            |" -ForegroundColor Yellow
    Write-Host "+================================================================+" -ForegroundColor Yellow
    Write-Host "`nVMs are now isolated from each other:" -ForegroundColor White
    Write-Host "  [+] VMs CAN communicate with host ($hostIP)" -ForegroundColor Green
    Write-Host "  [+] VMs CAN access WSL services (Elasticsearch, Kibana, gRPC)" -ForegroundColor Green
    Write-Host "  [x] VMs CANNOT communicate with other VMs" -ForegroundColor Red
    Write-Host "`nThis prevents lateral movement during malware experiments." -ForegroundColor Gray
    Write-Host "`nTo restore VM-to-VM communication: .\toggle-vm-isolation.ps1 -Action Disable`n" -ForegroundColor Cyan
}

function Disable-VMIsolation {
    Write-Host "[INFO] Disabling VM-to-VM isolation..." -ForegroundColor Yellow

    # Remove inbound rule
    $inboundRemoved = Get-NetFirewallRule -DisplayName "$isolationRuleName-Inbound" -ErrorAction SilentlyContinue
    if ($inboundRemoved) {
        Remove-NetFirewallRule -DisplayName "$isolationRuleName-Inbound" -Confirm:$false
        Write-Host "[OK] Inbound isolation rule removed" -ForegroundColor Green
    }

    # Remove outbound rule
    $outboundRemoved = Get-NetFirewallRule -DisplayName "$isolationRuleName-Outbound" -ErrorAction SilentlyContinue
    if ($outboundRemoved) {
        Remove-NetFirewallRule -DisplayName "$isolationRuleName-Outbound" -Confirm:$false
        Write-Host "[OK] Outbound isolation rule removed" -ForegroundColor Green
    }

    # Remove allow-host rules (both directions)
    $allowHostOutbound = Get-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Outbound" -ErrorAction SilentlyContinue
    if ($allowHostOutbound) {
        Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Outbound" -Confirm:$false
        Write-Host "[OK] Allow-host outbound rule removed" -ForegroundColor Green
    }

    $allowHostInbound = Get-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Inbound" -ErrorAction SilentlyContinue
    if ($allowHostInbound) {
        Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host-Inbound" -Confirm:$false
        Write-Host "[OK] Allow-host inbound rule removed" -ForegroundColor Green
    }

    # Remove legacy rule if exists
    Remove-NetFirewallRule -DisplayName "AutoMutate-VM-Allow-Host" -ErrorAction SilentlyContinue

    if (-not $inboundRemoved -and -not $outboundRemoved) {
        Write-Host "[INFO] VM isolation already disabled" -ForegroundColor Gray
    }

    Write-Host "`n+================================================================+" -ForegroundColor Green
    Write-Host "|          VM-TO-VM ISOLATION DISABLED                           |" -ForegroundColor Green
    Write-Host "+================================================================+" -ForegroundColor Green
    Write-Host "`nVMs can now communicate with each other:" -ForegroundColor White
    Write-Host "  [+] VMs CAN communicate with host ($hostIP)" -ForegroundColor Green
    Write-Host "  [+] VMs CAN communicate with other VMs ($subnet)" -ForegroundColor Green
    Write-Host "  [+] VM-to-VM traffic allowed (RDP, SMB, etc.)" -ForegroundColor Green
    Write-Host "`nUseful for testing lateral movement techniques.`n" -ForegroundColor Gray
    Write-Host "To isolate VMs again: .\toggle-vm-isolation.ps1 -Action Enable`n" -ForegroundColor Cyan
}

switch ($Action) {
    "Status" {
        $status = Get-IsolationStatus

        Write-Host "VM-to-VM Isolation Status:" -ForegroundColor Yellow
        if ($status.Enabled) {
            Write-Host "  Status: ENABLED" -ForegroundColor Yellow
            Write-Host "  Mode: VMs isolated from each other" -ForegroundColor White
            Write-Host "`nCommunication Matrix:" -ForegroundColor Cyan
            Write-Host "  VM -> Host ($hostIP): ALLOWED" -ForegroundColor Green
            Write-Host "  VM -> Other VMs ($subnet): BLOCKED" -ForegroundColor Red
            Write-Host "  VM -> Internet: Depends on NAT status" -ForegroundColor Gray
        } else {
            Write-Host "  Status: DISABLED" -ForegroundColor Green
            Write-Host "  Mode: VMs can communicate with each other" -ForegroundColor White
            Write-Host "`nCommunication Matrix:" -ForegroundColor Cyan
            Write-Host "  VM -> Host ($hostIP): ALLOWED" -ForegroundColor Green
            Write-Host "  VM -> Other VMs ($subnet): ALLOWED" -ForegroundColor Green
            Write-Host "  VM -> Internet: Depends on NAT status" -ForegroundColor Gray
        }

        # Show active VM-to-VM connections
        $subnetBase = $subnet.Split('/')[0]
        $subnetPattern = ($subnetBase.Split('.')[0..2] -join '.') + '.*'
        $vmToVmConns = Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object {
            $_.LocalAddress -like $subnetPattern -and
            $_.RemoteAddress -like $subnetPattern -and
            $_.RemoteAddress -ne $hostIP -and
            $_.State -eq 'Established'
        }
        if ($vmToVmConns) {
            Write-Host "`n  Active VM-to-VM Connections: $($vmToVmConns.Count)" -ForegroundColor Yellow
            foreach ($conn in $vmToVmConns | Select-Object -First 5) {
                Write-Host "    $($conn.LocalAddress):$($conn.LocalPort) <-> $($conn.RemoteAddress):$($conn.RemotePort)" -ForegroundColor Gray
            }
            if ($vmToVmConns.Count -gt 5) {
                Write-Host "    ... and $($vmToVmConns.Count - 5) more" -ForegroundColor Gray
            }
        }
    }

    "Enable" {
        Enable-VMIsolation
    }

    "Disable" {
        Disable-VMIsolation
    }
}

Write-Host "+================================================================+`n" -ForegroundColor Cyan
