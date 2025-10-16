<#
.SYNOPSIS
    Add NAT to IsolationSwitch for VM internet access

.PARAMETER SwitchName
    Hyper-V switch name (default: IsolationSwitch)

.PARAMETER GatewayIP
    Host IP on the switch (default: 192.168.200.1)

.PARAMETER Subnet
    Network subnet (default: 192.168.200.0/24)
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$SwitchName = "IsolationSwitch",

    [Parameter()]
    [string]$GatewayIP = "192.168.200.1",

    [Parameter()]
    [string]$Subnet = "192.168.200.0/24"
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }

Write-Info "Configuring NAT for $SwitchName..."

# Check if NAT already exists
$existingNat = Get-NetNat -Name "IsolationNAT" -ErrorAction SilentlyContinue
if ($existingNat) {
    Write-Info "NAT already exists, removing old configuration..."
    Remove-NetNat -Name "IsolationNAT" -Confirm:$false
}

# Create NAT
New-NetNat -Name "IsolationNAT" -InternalIPInterfaceAddressPrefix $Subnet | Out-Null
Write-Success "Created NAT: IsolationNAT for $Subnet"

Write-Success "VMs on $SwitchName can now access the internet via NAT"
Write-Info "Gateway: $GatewayIP"
Write-Info "VMs should use $GatewayIP as their DNS server and default gateway"

# Test internet connectivity from host
Write-Info "Testing host internet connectivity..."
try {
    $test = Test-NetConnection -ComputerName "8.8.8.8" -InformationLevel Quiet
    if ($test) {
        Write-Success "Host has internet access - VMs should work"
    } else {
        Write-Warning "Host may not have internet access - check your network adapter"
    }
} catch {
    Write-Warning "Could not test internet connectivity: $_"
}

exit 0
