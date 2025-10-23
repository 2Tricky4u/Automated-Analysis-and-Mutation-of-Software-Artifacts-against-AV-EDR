<#
.SYNOPSIS
    Complete network cleanup - Remove ALL network configurations

.DESCRIPTION
    This script removes ALL network-related configurations created by the setup scripts:
    - NAT configuration (AutoMutateVMNAT)
    - Hyper-V Internal Switch (IsolationSwitch) and its vEthernet adapter
    - All firewall rules (VM isolation, egress filtering, port forwarding, WSL)
    - Port proxy rules
    - Network adapter IP configuration

    After running this script, the system will be in the same state as before
    running 01-host-setup.ps1 (network-wise, not data-wise).

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.PARAMETER Force
    Skip confirmation prompts

.EXAMPLE
    .\cleanup-network.ps1
    .\cleanup-network.ps1 -Force
    .\cleanup-network.ps1 -ConfigPath "..\config.yaml"

.NOTES
    Must be run as Administrator
    This does NOT remove VMs, VHDs, or data - only network configurations
    WSL and Hyper-V features remain enabled
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$ConfigPath = (Join-Path $PSScriptRoot "..\config.yaml"),

    [Parameter()]
    [switch]$Force
)

$ErrorActionPreference = "Stop"

# Color output functions
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

# Check admin
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Err "This script must be run as Administrator"
    exit 1
}

Write-Host "`n+================================================================+" -ForegroundColor Red
Write-Host "|          NETWORK CLEANUP - COMPLETE REMOVAL                    |" -ForegroundColor Red
Write-Host "+================================================================+`n" -ForegroundColor Red

# Load config
if (-not (Test-Path $ConfigPath)) {
    Write-Err "Config file not found: $ConfigPath"
    exit 1
}

$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?([^#]+?)"?\s*(?:#.*)?$' -and $section) {
        $config[$section][$matches[1]] = $matches[2].Trim().Trim('"')
    }
}

$SwitchName = $config.network.switch_name
$HostIP     = $config.network.host_ip
$Subnet     = $config.network.subnet
$GrpcPort   = $config.controller.grpc_port
$EsPort     = $config.controller.elasticsearch_port
$KibanaPort = $config.controller.kibana_port

$natName = "AutoMutateVMNAT"

Write-Info "Configuration loaded:"
Write-Info "  Switch Name: $SwitchName"
Write-Info "  Host IP: $HostIP"
Write-Info "  Subnet: $Subnet"
Write-Info "  NAT Name: $natName"

# Confirmation prompt
if (-not $Force) {
    Write-Warn "`nThis will remove ALL network configurations created by the setup scripts:"
    Write-Host "  [1] NAT configuration ($natName)" -ForegroundColor Gray
    Write-Host "  [2] Hyper-V Internal Switch ($SwitchName)" -ForegroundColor Gray
    Write-Host "  [3] All firewall rules (VM isolation, egress filtering, port forwarding)" -ForegroundColor Gray
    Write-Host "  [4] Port proxy rules (WSL service forwarding)" -ForegroundColor Gray
    Write-Host "  [5] Network adapter IP configuration" -ForegroundColor Gray
    Write-Host "`n  NOTE: VMs, VHDs, and data are NOT removed - only network configs" -ForegroundColor Yellow
    Write-Host "`n  Press Ctrl+C to cancel, or" -NoNewline
    $response = Read-Host " type YES to continue"

    if ($response -ne "YES") {
        Write-Info "Cleanup cancelled by user"
        exit 0
    }
}

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Starting Network Cleanup                              |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

$removedCount = 0

# ============================================================================
# STEP 1: Remove NAT Configuration
# ============================================================================
Write-Info "[1/6] Removing NAT configuration..."

$nat = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
if ($nat) {
    Remove-NetNat -Name $natName -Confirm:$false
    Write-Success "Removed NAT: $natName"
    $removedCount++
} else {
    Write-Info "NAT not found: $natName (already removed)"
}

# ============================================================================
# STEP 2: Remove All Firewall Rules
# ============================================================================
Write-Info "`n[2/6] Removing firewall rules..."

$firewallRules = @(
    # AutoMutate service rules (port forwarding)
    "AutoMutate-Allow-$GrpcPort",
    "AutoMutate-Allow-$EsPort",
    "AutoMutate-Allow-$KibanaPort",

    # WSL network rules
    "WSL-Internet-Outbound",
    "WSL-Network-Services",

    # VM isolation rules
    "AutoMutate-VM-Isolation-Block-Inbound",
    "AutoMutate-VM-Isolation-Block-Outbound",
    "AutoMutate-VM-Isolation-Block",
    "AutoMutate-VM-Allow-Host",
    "AutoMutate-VM-Allow-Host-Outbound",
    "AutoMutate-VM-Allow-Host-Inbound",

    # Egress filtering rules
    "AutoMutate-Egress-Block-Internet",
    "AutoMutate-Egress-Allow-DNS",
    "AutoMutate-Egress-Allow-HTTP",
    "AutoMutate-Egress-Allow-HTTPS",
    "AutoMutate-Egress-Allow-NTP"
)

foreach ($ruleName in $firewallRules) {
    $rule = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
    if ($rule) {
        Remove-NetFirewallRule -DisplayName $ruleName -Confirm:$false
        Write-Success "Removed firewall rule: $ruleName"
        $removedCount++
    }
}

# Remove custom egress rules
$customEgressRules = Get-NetFirewallRule -DisplayName "AutoMutate-Egress-Custom-*" -ErrorAction SilentlyContinue
if ($customEgressRules) {
    $customEgressRules | Remove-NetFirewallRule -Confirm:$false
    Write-Success "Removed $($customEgressRules.Count) custom egress rule(s)"
    $removedCount += $customEgressRules.Count
}

# Remove temporary kill-connection rules (if any)
$killRules = Get-NetFirewallRule -DisplayName "AutoMutate-Kill-*" -ErrorAction SilentlyContinue
if ($killRules) {
    $killRules | Remove-NetFirewallRule -Confirm:$false
    Write-Success "Removed $($killRules.Count) temporary kill-connection rule(s)"
    $removedCount += $killRules.Count
}

Write-Success "Firewall rules cleanup complete"

# ============================================================================
# STEP 3: Remove Port Proxy Rules
# ============================================================================
Write-Info "`n[3/6] Removing port proxy rules..."

$ports = @($GrpcPort, $EsPort, $KibanaPort)
$portProxyRemoved = 0

foreach ($port in $ports) {
    # Check if port proxy exists
    $existingProxy = netsh interface portproxy show v4tov4 | Select-String -Pattern ".*$HostIP.*$port.*"

    if ($existingProxy) {
        netsh interface portproxy delete v4tov4 listenaddress=$HostIP listenport=$port 2>$null | Out-Null
        Write-Success "Removed port proxy: ${HostIP}:$port -> 127.0.0.1:$port"
        $portProxyRemoved++
        $removedCount++
    }
}

if ($portProxyRemoved -eq 0) {
    Write-Info "No port proxy rules found (already removed)"
}

# ============================================================================
# STEP 4: Remove Network Adapter IP Configuration
# ============================================================================
Write-Info "`n[4/6] Removing network adapter IP configuration..."

$adapter = Get-NetAdapter | Where-Object { $_.Name -like "*$SwitchName*" } | Select-Object -First 1

if ($adapter) {
    Write-Info "Found vEthernet adapter: $($adapter.Name)"

    # Remove IP address
    $existingIp = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $HostIP -ErrorAction SilentlyContinue
    if ($existingIp) {
        Remove-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $HostIP -Confirm:$false -ErrorAction SilentlyContinue
        Write-Success "Removed IP address: $HostIP from adapter $($adapter.Name)"
        $removedCount++
    } else {
        Write-Info "IP address not configured on adapter (already removed)"
    }
} else {
    Write-Info "vEthernet adapter not found: $SwitchName (switch may not exist)"
}

# ============================================================================
# STEP 5: Remove Hyper-V Switch
# ============================================================================
Write-Info "`n[5/6] Removing Hyper-V switch..."

$switch = Get-VMSwitch -Name $SwitchName -ErrorAction SilentlyContinue
if ($switch) {
    # Check if any VMs are using this switch
    $vmsUsingSwitch = Get-VM | Where-Object {
        (Get-VMNetworkAdapter -VMName $_.Name -ErrorAction SilentlyContinue |
         Where-Object { $_.SwitchName -eq $SwitchName }).Count -gt 0
    }

    if ($vmsUsingSwitch) {
        $vmCount = ($vmsUsingSwitch | Measure-Object).Count
        Write-Warn "WARNING: $vmCount VM(s) are connected to this switch:"
        foreach ($vm in $vmsUsingSwitch) {
            Write-Host "  - $($vm.Name) (State: $($vm.State))" -ForegroundColor Yellow
        }

        if (-not $Force) {
            Write-Warn "`nRemoving the switch will disconnect these VMs from the network."
            Write-Host "  Type YES to continue or Ctrl+C to cancel: " -NoNewline
            $response = Read-Host
            if ($response -ne "YES") {
                Write-Info "Switch removal cancelled by user"
                Write-Info "You can manually disconnect VMs and run this script again"
                exit 0
            }
        }
    }

    Remove-VMSwitch -Name $SwitchName -Force
    Write-Success "Removed Hyper-V switch: $SwitchName"
    $removedCount++
} else {
    Write-Info "Hyper-V switch not found: $SwitchName (already removed)"
}

# ============================================================================
# STEP 6: Verify Cleanup
# ============================================================================
Write-Info "`n[6/6] Verifying cleanup..."

$verificationPassed = $true

# Check NAT
$natCheck = Get-NetNat -Name $natName -ErrorAction SilentlyContinue
if ($natCheck) {
    Write-Warn "NAT still exists: $natName"
    $verificationPassed = $false
} else {
    Write-Success "NAT removed: $natName"
}

# Check Switch
$switchCheck = Get-VMSwitch -Name $SwitchName -ErrorAction SilentlyContinue
if ($switchCheck) {
    Write-Warn "Switch still exists: $SwitchName"
    $verificationPassed = $false
} else {
    Write-Success "Switch removed: $SwitchName"
}

# Check firewall rules
$remainingRules = Get-NetFirewallRule -DisplayName "AutoMutate-*" -ErrorAction SilentlyContinue
if ($remainingRules) {
    Write-Warn "Found $($remainingRules.Count) remaining AutoMutate firewall rule(s):"
    foreach ($rule in $remainingRules) {
        Write-Host "  - $($rule.DisplayName)" -ForegroundColor Yellow
    }
    $verificationPassed = $false
} else {
    Write-Success "All AutoMutate firewall rules removed"
}

# Check port proxy
$remainingProxies = netsh interface portproxy show v4tov4 | Select-String -Pattern ".*$HostIP.*"
if ($remainingProxies) {
    Write-Warn "Found remaining port proxy rule(s) for $HostIP"
    $verificationPassed = $false
} else {
    Write-Success "All port proxy rules removed"
}

# ============================================================================
# SUMMARY
# ============================================================================
Write-Host "`n+================================================================+" -ForegroundColor $(if ($verificationPassed) { "Green" } else { "Yellow" })
Write-Host "|          NETWORK CLEANUP SUMMARY                               |" -ForegroundColor $(if ($verificationPassed) { "Green" } else { "Yellow" })
Write-Host "+================================================================+" -ForegroundColor $(if ($verificationPassed) { "Green" } else { "Yellow" })

Write-Host "`nRemoved Components: $removedCount" -ForegroundColor White
Write-Host "`nCleaned up:" -ForegroundColor White
Write-Host "  [x] NAT configuration" -ForegroundColor Green
Write-Host "  [x] Hyper-V Internal Switch" -ForegroundColor Green
Write-Host "  [x] Firewall rules (VM isolation, egress filtering, port forwarding)" -ForegroundColor Green
Write-Host "  [x] Port proxy rules (WSL service forwarding)" -ForegroundColor Green
Write-Host "  [x] Network adapter IP configuration" -ForegroundColor Green

Write-Host "`nNOT removed (by design):" -ForegroundColor Cyan
Write-Host "  [o] VMs and VHDs" -ForegroundColor Gray
Write-Host "  [o] WSL2 installation" -ForegroundColor Gray
Write-Host "  [o] Hyper-V and WSL2 Windows features" -ForegroundColor Gray
Write-Host "  [o] .wslconfig file" -ForegroundColor Gray
Write-Host "  [o] Data, logs, and configurations" -ForegroundColor Gray

if ($verificationPassed) {
    Write-Host "`nStatus: SUCCESS" -ForegroundColor Green
    Write-Host "The system is now in the same network state as before setup." -ForegroundColor Green
    Write-Host "You can safely run 01-host-setup.ps1 again to recreate the network." -ForegroundColor Cyan
} else {
    Write-Host "`nStatus: PARTIAL CLEANUP" -ForegroundColor Yellow
    Write-Host "Some components could not be removed (see warnings above)." -ForegroundColor Yellow
    Write-Host "You may need to manually remove remaining components." -ForegroundColor Yellow
}

Write-Host "`nNext steps:" -ForegroundColor White
Write-Host "  1. To recreate network: .\scripts\01-host-setup.ps1 -ConfigPath ..\config.yaml" -ForegroundColor Cyan
Write-Host "  2. To remove VMs: Get-VM | Where-Object { `$_.Name -like '*worker*' } | Remove-VM" -ForegroundColor Cyan
Write-Host "  3. To check WSL status: wsl --status" -ForegroundColor Cyan

Write-Host "`n+================================================================+`n" -ForegroundColor $(if ($verificationPassed) { "Green" } else { "Yellow" })

exit $(if ($verificationPassed) { 0 } else { 1 })
