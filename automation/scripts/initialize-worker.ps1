<#
.SYNOPSIS
    Initialize Worker VM after manual Windows installation

.DESCRIPTION
    Disables Secure Boot and runs 04-vm-init.ps1 inside the VM.
    Run this after Windows installation is complete and VM has been shut down.

.PARAMETER VMName
    Name of the worker VM (e.g., "win11-worker-01")

.PARAMETER IPAddress
    Static IP for the worker (e.g., "10.200.200.110")

.PARAMETER Username
    VM admin username (default: "worker-admin")

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.EXAMPLE
    .\initialize-worker.ps1 -VMName "win11-worker-01" -IPAddress "10.200.200.110"

.NOTES
    Prerequisites:
    - Windows installation completed
    - VM must be STOPPED (Secure Boot can only be disabled while VM is off)
    - PowerShell Direct enabled (happens automatically in 04-vm-init.ps1)
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [Parameter(Mandatory)]
    [string]$IPAddress,

    [Parameter()]
    [string]$Username = "worker-admin",

    [Parameter()]
    [string]$ConfigPath = "..\config.yaml",

    [Parameter()]
    [int]$RedEDRPort = 8080
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Err { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

Write-Host @"

+================================================================+
|          Worker VM Initialization Wrapper                      |
+================================================================+

"@ -ForegroundColor Cyan

# Check if VM exists
$vm = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if (-not $vm) {
    Write-Err "VM not found: $VMName"
    Write-Info "Available VMs:"
    Get-VM | Select-Object Name, State | Format-Table
    exit 1
}

Write-Info "VM: $VMName"
Write-Info "IP: $IPAddress"
Write-Info ""

# Step 1: Ensure VM is stopped
if ($vm.State -ne "Off") {
    Write-Info "VM is currently $($vm.State). Stopping VM..."
    Stop-VM -Name $VMName -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 5
    Write-Success "VM stopped"
} else {
    Write-Success "VM already stopped"
}

# Step 2: Disable Secure Boot
Write-Info "Disabling Secure Boot..."
try {
    Set-VMFirmware -VMName $VMName -EnableSecureBoot Off
    Write-Success "Secure Boot disabled"
} catch {
    Write-Err "Failed to disable Secure Boot: $($_.Exception.Message)"
    exit 1
}

# Verify Secure Boot is disabled
$firmware = Get-VMFirmware -VMName $VMName
if ($firmware.SecureBoot -eq "Off") {
    Write-Success "Verified: Secure Boot is OFF"
} else {
    Write-Err "Secure Boot is still enabled: $($firmware.SecureBoot)"
    exit 1
}

# Step 3: Start VM
Write-Info "Starting VM..."
Start-VM -Name $VMName
Write-Success "VM started"

# Wait for VM to boot and be ready for PowerShell Direct
Write-Info "Waiting for VM to boot (30 seconds)..."
Start-Sleep -Seconds 30

# Wait for heartbeat
Write-Info "Waiting for VM heartbeat..."
$maxWait = 60
$waited = 0
while ($waited -lt $maxWait) {
    $vm = Get-VM -Name $VMName
    if ($vm.Heartbeat -eq "OkApplicationsHealthy" -or $vm.Heartbeat -eq "OkApplicationsUnknown") {
        Write-Success "VM heartbeat detected"
        break
    }
    Start-Sleep -Seconds 2
    $waited += 2
}

if ($waited -ge $maxWait) {
    Write-Err "VM heartbeat not detected after $maxWait seconds"
    Write-Info "VM may still be booting. You can try running 04-vm-init.ps1 manually."
    exit 1
}

# Step 4: Get credentials
Write-Host ""
Write-Info "Enter VM credentials for PowerShell Direct..."
$cred = Get-Credential -UserName $Username -Message "Enter password for $Username"

# Step 5: Run 04-vm-init.ps1 via PowerShell Direct
Write-Host ""
Write-Info "Running 04-vm-init.ps1 inside VM via PowerShell Direct..."
Write-Info "This will take several minutes (dependencies, RedEDR, configuration)..."
Write-Host ""

try {
    # Resolve config path (works from automation/ or scripts/ folder)
    $ResolvedConfigPath = $ConfigPath
    if (-not [System.IO.Path]::IsPathRooted($ConfigPath)) {
        # Relative path - try to find config.yaml
        if (Test-Path $ConfigPath) {
            $ResolvedConfigPath = $ConfigPath
        } elseif (Test-Path (Join-Path $PSScriptRoot "..\config.yaml")) {
            # Running from scripts folder
            $ResolvedConfigPath = Join-Path $PSScriptRoot "..\config.yaml"
        } elseif (Test-Path ".\config.yaml") {
            # Running from automation folder
            $ResolvedConfigPath = ".\config.yaml"
        } else {
            Write-Err "Cannot find config.yaml. Tried: $ConfigPath"
            exit 1
        }
    }

    # Load config to get network settings
    $config = @{}
    $section = $null
    Get-Content $ResolvedConfigPath | ForEach-Object {
        if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
        elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
    }

    $Gateway = $config.network.gateway
    $Prefix = ($config.network.subnet -split '/')[1]

    # Resolve 04-vm-init.ps1 path (always in scripts folder)
    $InitScriptPath = Join-Path $PSScriptRoot "04-vm-init.ps1"
    if (-not (Test-Path $InitScriptPath)) {
        Write-Err "Script not found: $InitScriptPath"
        exit 1
    }

    Invoke-Command -VMName $VMName -FilePath $InitScriptPath `
        -ArgumentList $IPAddress, $VMName, $Gateway, $Prefix, $RedEDRPort `
        -Credential $cred

    Write-Host ""
    Write-Success "Initialization complete!"
} catch {
    Write-Err "Failed to run 04-vm-init.ps1: $($_.Exception.Message)"
    Write-Info "Troubleshooting:"
    Write-Info "  1. Verify PowerShell Direct is enabled (check VM Integration Services)"
    Write-Info "  2. Verify credentials are correct"
    Write-Info "  3. Check VM is fully booted (may need more time)"
    Write-Info "  4. Try running manually from within the VM"
    exit 1
}

Write-Host ""
Write-Host "+================================================================+" -ForegroundColor Green
Write-Host "|          Worker VM Ready                                       |" -ForegroundColor Green
Write-Host "+================================================================+" -ForegroundColor Green
Write-Host ""
Write-Info "VM: $VMName"
Write-Info "IP: $IPAddress"
Write-Info "Secure Boot: DISABLED"
Write-Host ""

exit 0
