<#
.SYNOPSIS
    Enable SMB access to VM from remote PC

.DESCRIPTION
    Configures VM firewall to allow SMB (port 445) from a specific remote PC IP address.
    Useful when sync-project-via-smb.ps1 works from Hyper-V host but fails from other PCs.

.PARAMETER VMName
    Name of the VM to configure

.PARAMETER RemoteIPAddress
    IP address of the remote PC that needs SMB access (if not specified, allows all)

.PARAMETER HyperVHost
    Hyper-V host name (if running from different PC)

.EXAMPLE
    # From Hyper-V host - allow specific remote PC
    .\enable-smb-remote-access.ps1 -VMName "win10-worker-01" -RemoteIPAddress "192.168.1.100"

.EXAMPLE
    # From Hyper-V host - allow all IPs (less secure)
    .\enable-smb-remote-access.ps1 -VMName "win10-worker-01"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [string]$RemoteIPAddress,

    [string]$HyperVHost = $env:COMPUTERNAME
)

$ErrorActionPreference = "Stop"

# Colors
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERR] $M" -ForegroundColor Red }

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "Enable SMB Remote Access to VM" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

Write-Info "VM: $VMName"
if ($RemoteIPAddress) {
    Write-Info "Remote PC IP: $RemoteIPAddress"
} else {
    Write-Warn "No remote IP specified - will allow SMB from ANY IP (less secure)"
    $Response = Read-Host "Continue? (y/N)"
    if ($Response -ne 'y' -and $Response -ne 'Y') {
        Write-Info "Aborted by user"
        exit 0
    }
}

# Get VM
try {
    if ($HyperVHost -eq $env:COMPUTERNAME) {
        $VM = Get-VM -Name $VMName -ErrorAction Stop
    } else {
        $VM = Get-VM -Name $VMName -ComputerName $HyperVHost -ErrorAction Stop
    }
} catch {
    Write-Err "Cannot access VM: $($_.Exception.Message)"
    Write-Info "Ensure you have Hyper-V admin rights on $HyperVHost"
    exit 1
}

if ($VM.State -ne "Running") {
    Write-Err "VM is not running"
    exit 1
}

Write-Success "VM found and running"

# Test credentials
Write-Info "Testing PowerShell Direct authentication..."
$VMCredential = $null
try {
    $null = Invoke-Command -VMName $VMName -ScriptBlock { $env:COMPUTERNAME } -ErrorAction Stop
    Write-Success "Using implicit credentials"
} catch {
    Write-Info "Credentials required"
    $VMCredential = Get-Credential -UserName ".\Administrator" -Message "Enter VM credentials"
}

# Configure firewall on VM
Write-Info "Configuring firewall on VM..."

try {
    $params = @{
        VMName = $VMName
        ScriptBlock = {
            param($RemoteIP)

            Write-Host "[VM] Current SMB firewall rules:"
            Get-NetFirewallRule -DisplayGroup "File and Printer Sharing" |
                Where-Object { $_.Enabled -eq $true } |
                Select-Object DisplayName, Direction, Action |
                Format-Table

            # Enable File and Printer Sharing predefined rules
            Write-Host "[VM] Enabling File and Printer Sharing..."
            Enable-NetFirewallRule -DisplayGroup "File and Printer Sharing" -ErrorAction SilentlyContinue

            # If specific remote IP provided, create custom rule
            if ($RemoteIP) {
                Write-Host "[VM] Creating custom rule for remote IP: $RemoteIP"

                # Remove existing custom rules
                Get-NetFirewallRule -DisplayName "SMB Remote Access*" -ErrorAction SilentlyContinue |
                    Remove-NetFirewallRule -ErrorAction SilentlyContinue

                # Create new rule
                New-NetFirewallRule -DisplayName "SMB Remote Access - File Sharing (TCP-In)" `
                    -Direction Inbound `
                    -Protocol TCP `
                    -LocalPort 445 `
                    -RemoteAddress $RemoteIP `
                    -Action Allow `
                    -Profile Any `
                    -Enabled True `
                    -ErrorAction Stop | Out-Null

                Write-Host "[VM] Custom rule created for $RemoteIP"
            } else {
                Write-Host "[VM] No specific IP - using default File and Printer Sharing rules"
            }

            # Ensure SMB server is running
            Write-Host "[VM] Checking SMB server service..."
            $SmbServer = Get-Service -Name LanmanServer -ErrorAction SilentlyContinue
            if ($SmbServer.Status -ne "Running") {
                Write-Host "[VM] Starting SMB server service..."
                Start-Service -Name LanmanServer -ErrorAction Stop
            } else {
                Write-Host "[VM] SMB server is running"
            }

            # Verify C$ share exists
            Write-Host "[VM] Checking C$ admin share..."
            $CShare = Get-SmbShare -Name "C$" -ErrorAction SilentlyContinue
            if ($CShare) {
                Write-Host "[VM] C$ share exists"
            } else {
                Write-Host "[VM] WARNING: C$ share not found (may need to be enabled)"
            }

            Write-Host "[VM] Configuration complete"
        }
        ArgumentList = @($RemoteIPAddress)
        ErrorAction = 'Stop'
    }

    if ($VMCredential) {
        $params['Credential'] = $VMCredential
    }

    Invoke-Command @params

    Write-Success "Firewall configured successfully"

} catch {
    Write-Err "Failed to configure firewall: $($_.Exception.Message)"
    exit 1
}

# Test connectivity from remote PC
if ($RemoteIPAddress) {
    Write-Info ""
    Write-Info "Testing SMB access from remote PC..."
    Write-Info "On remote PC ($RemoteIPAddress), run:"
    Write-Host "  Test-NetConnection -ComputerName $($VM.NetworkAdapters[0].IPAddresses[0]) -Port 445" -ForegroundColor White
    Write-Host "  net use Z: \\$($VM.NetworkAdapters[0].IPAddresses[0])\C$ /user:.\Administrator" -ForegroundColor White
}

Write-Host "`n========================================" -ForegroundColor Green
Write-Host "SMB Access Configuration Complete!" -ForegroundColor Green
Write-Host "========================================`n" -ForegroundColor Green

Write-Info "You can now run sync-project-via-smb.ps1 from the remote PC"
