<#
.SYNOPSIS
    Manage egress filtering rules for VM subnet

.DESCRIPTION
    Standalone tool to manage whitelist-based egress filtering for worker VMs.
    Allows runtime control of allowed outbound traffic without affecting NAT.

.PARAMETER Action
    "Enable" to activate egress filtering, "Disable" to remove, "Status" to check, "AddRule" to whitelist, "RemoveRule" to unwhitelist

.PARAMETER Protocol
    Protocol for custom rule (TCP or UDP)

.PARAMETER Port
    Port number for custom rule

.PARAMETER Description
    Description for custom rule

.EXAMPLE
    .\manage-egress-filter.ps1 -Action Enable            # Enable default whitelist
    .\manage-egress-filter.ps1 -Action Disable           # Remove all egress filtering
    .\manage-egress-filter.ps1 -Action Status            # Show current rules
    .\manage-egress-filter.ps1 -Action AddRule -Protocol TCP -Port 8080 -Description "Custom service"
    .\manage-egress-filter.ps1 -Action RemoveRule -Protocol TCP -Port 8080

.NOTES
    Must be run as Administrator
    Can be used independently of toggle-vm-internet.ps1
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet("Enable", "Disable", "Status", "AddRule", "RemoveRule")]
    [string]$Action,

    [Parameter()]
    [ValidateSet("TCP", "UDP")]
    [string]$Protocol,

    [Parameter()]
    [int]$Port,

    [Parameter()]
    [string]$Description
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

$egressBlockRuleName = "AutoMutate-Egress-Block-Internet"

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Egress Filter Management                              |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

function Get-EgressStatus {
    $blockRule = Get-NetFirewallRule -DisplayName $egressBlockRuleName -ErrorAction SilentlyContinue
    $whitelistRules = Get-NetFirewallRule -DisplayName "AutoMutate-Egress-Allow-*" -ErrorAction SilentlyContinue

    return @{
        Enabled = ($blockRule -ne $null -and $blockRule.Enabled -eq 'True')
        BlockRule = $blockRule
        WhitelistRules = $whitelistRules
    }
}

function Enable-EgressFiltering {
    Write-Host "[INFO] Enabling egress filtering (default-deny + whitelist)..." -ForegroundColor Yellow

    # Remove old rules
    Remove-NetFirewallRule -DisplayName $egressBlockRuleName -ErrorAction SilentlyContinue

    # Get adapter
    $adapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" }
    $adapterAlias = if ($adapter) { $adapter.Name } else { $null }

    # Create default-deny rule
    $params = @{
        DisplayName = $egressBlockRuleName
        Direction = 'Outbound'
        Action = 'Block'
        Profile = 'Any'
        Enabled = 'True'
    }
    if ($adapterAlias) { $params['InterfaceAlias'] = $adapterAlias }
    New-NetFirewallRule @params | Out-Null
    Write-Host "[OK] Egress blocking rule created" -ForegroundColor Green

    # Create default whitelist
    $whitelistRules = @(
        @{ Name = "AutoMutate-Egress-Allow-DNS"; Protocol = "UDP"; Ports = @(53); Description = "DNS queries" },
        @{ Name = "AutoMutate-Egress-Allow-HTTP"; Protocol = "TCP"; Ports = @(80); Description = "HTTP (Windows Update, packages)" },
        @{ Name = "AutoMutate-Egress-Allow-HTTPS"; Protocol = "TCP"; Ports = @(443); Description = "HTTPS (Windows Update, packages)" },
        @{ Name = "AutoMutate-Egress-Allow-NTP"; Protocol = "UDP"; Ports = @(123); Description = "NTP time sync" }
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
        if ($adapterAlias) { $params['InterfaceAlias'] = $adapterAlias }
        New-NetFirewallRule @params | Out-Null
        Write-Host "[OK] Whitelist: $($rule.Description) (ports: $($rule.Ports -join ','))" -ForegroundColor Green
    }

    Write-Host "`n[OK] Egress filtering enabled" -ForegroundColor Green
}

function Disable-EgressFiltering {
    Write-Host "[INFO] Disabling egress filtering..." -ForegroundColor Yellow

    # Remove block rule
    $removed = Get-NetFirewallRule -DisplayName $egressBlockRuleName -ErrorAction SilentlyContinue
    if ($removed) {
        Remove-NetFirewallRule -DisplayName $egressBlockRuleName -Confirm:$false
        Write-Host "[OK] Egress blocking rule removed" -ForegroundColor Green
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

    # Remove custom rules
    $customRules = Get-NetFirewallRule -DisplayName "AutoMutate-Egress-Custom-*" -ErrorAction SilentlyContinue
    if ($customRules) {
        $customRules | Remove-NetFirewallRule -Confirm:$false
        Write-Host "[OK] Removed $($customRules.Count) custom whitelist rule(s)" -ForegroundColor Green
    }

    Write-Host "[OK] Egress filtering disabled" -ForegroundColor Green
}

function Add-CustomRule {
    param([string]$Proto, [int]$PortNum, [string]$Desc)

    $ruleName = "AutoMutate-Egress-Custom-$Proto-$PortNum"

    # Check if block rule exists
    $status = Get-EgressStatus
    if (-not $status.Enabled) {
        Write-Host "[WARN] Egress filtering not enabled. Enable it first with -Action Enable" -ForegroundColor Yellow
        return
    }

    # Remove old rule if exists
    Remove-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue

    # Get adapter
    $adapter = Get-NetAdapter | Where-Object { $_.Name -like "*IsolationSwitch*" }
    $adapterAlias = if ($adapter) { $adapter.Name } else { $null }

    # Create rule
    $params = @{
        DisplayName = $ruleName
        Direction = 'Outbound'
        Action = 'Allow'
        Protocol = $Proto
        RemotePort = $PortNum
        Profile = 'Any'
        Enabled = 'True'
    }
    if ($adapterAlias) { $params['InterfaceAlias'] = $adapterAlias }
    New-NetFirewallRule @params | Out-Null

    Write-Host "[OK] Added custom whitelist: $Proto/$PortNum - $Desc" -ForegroundColor Green
}

function Remove-CustomRule {
    param([string]$Proto, [int]$PortNum)

    $ruleName = "AutoMutate-Egress-Custom-$Proto-$PortNum"
    $removed = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue

    if ($removed) {
        Remove-NetFirewallRule -DisplayName $ruleName -Confirm:$false
        Write-Host "[OK] Removed custom whitelist: $Proto/$PortNum" -ForegroundColor Green
    } else {
        Write-Host "[INFO] Rule not found: $Proto/$PortNum" -ForegroundColor Yellow
    }
}

switch ($Action) {
    "Status" {
        $status = Get-EgressStatus

        Write-Host "Egress Filtering Status:" -ForegroundColor Yellow
        if ($status.Enabled) {
            Write-Host "  Status: ENABLED" -ForegroundColor Green
            Write-Host "  Mode: Default-deny with whitelist" -ForegroundColor White
            Write-Host "`nWhitelisted Traffic:" -ForegroundColor Cyan

            if ($status.WhitelistRules) {
                foreach ($rule in $status.WhitelistRules) {
                    $ports = $rule.RemotePort -join ','
                    $proto = $rule.Protocol
                    Write-Host "  ✓ $($rule.DisplayName): $proto/$ports" -ForegroundColor Gray
                }
            }

            Write-Host "`nBlocked Traffic:" -ForegroundColor Red
            Write-Host "  ✗ All other outbound connections" -ForegroundColor Gray
        } else {
            Write-Host "  Status: DISABLED" -ForegroundColor Red
            Write-Host "  Mode: Unrestricted (all traffic allowed)" -ForegroundColor Yellow
        }
    }

    "Enable" {
        Enable-EgressFiltering
    }

    "Disable" {
        Disable-EgressFiltering
    }

    "AddRule" {
        if (-not $Protocol -or -not $Port) {
            Write-Host "[ERROR] -Protocol and -Port are required for AddRule action" -ForegroundColor Red
            exit 1
        }
        if (-not $Description) {
            $Description = "Custom rule"
        }
        Add-CustomRule -Proto $Protocol -PortNum $Port -Desc $Description
    }

    "RemoveRule" {
        if (-not $Protocol -or -not $Port) {
            Write-Host "[ERROR] -Protocol and -Port are required for RemoveRule action" -ForegroundColor Red
            exit 1
        }
        Remove-CustomRule -Proto $Protocol -PortNum $Port
    }
}

Write-Host "`n+================================================================+`n" -ForegroundColor Cyan
