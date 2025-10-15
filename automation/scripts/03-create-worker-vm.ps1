<#
.SYNOPSIS
    Create Worker VM with TPM + Secure Boot

.PARAMETER WorkerName
    VM name (e.g., "win11-worker-01")

.PARAMETER Os
    "windows10" or "windows11"

.PARAMETER IsoPath
    Path to Windows ISO

.PARAMETER StaticIP
    Worker IP in 192.168.200.0/24

.PARAMETER ConfigPath
    Path to config.yaml
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$WorkerName,

    [Parameter(Mandatory)]
    [ValidateSet("windows10", "windows11")]
    [string]$Os,

    [Parameter(Mandatory)]
    [string]$IsoPath,

    [Parameter(Mandatory)]
    [string]$StaticIP,

    [Parameter()]
    [string]$ConfigPath = "..\config.yaml"
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }

# Load config
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$SwitchName = $config.network.switch_name
$VhdRoot = $config.storage.vhd_root

# Worker config (find in config.yaml workers list - simplified)
$CpuCount = 2
$MemoryGB = if ($Os -eq "windows11") { 6 } else { 4 }
$DiskGB = if ($Os -eq "windows11") { 80 } else { 64 }

Write-Info "Creating VM: $WorkerName ($Os) - $StaticIP"

# Validate ISO
if (-not (Test-Path $IsoPath)) {
    Write-Error "ISO not found: $IsoPath"
    exit 1
}

# Create VHD folder
$VhdPath = Join-Path $VhdRoot "$WorkerName.vhdx"
$VhdFolder = Split-Path $VhdPath -Parent
if (-not (Test-Path $VhdFolder)) {
    New-Item -ItemType Directory -Path $VhdFolder -Force | Out-Null
}

# Create VHDX
if (-not (Test-Path $VhdPath)) {
    New-VHD -Path $VhdPath -SizeBytes ($DiskGB * 1GB) -Dynamic | Out-Null
    Write-Success "Created VHDX: $VhdPath"
}

# Create Gen2 VM
if (-not (Get-VM -Name $WorkerName -ErrorAction SilentlyContinue)) {
    New-VM -Name $WorkerName -MemoryStartupBytes ($MemoryGB * 1GB) `
        -Generation 2 -VHDPath $VhdPath -SwitchName $SwitchName | Out-Null
    Write-Success "Created VM: $WorkerName"
} else {
    Write-Info "VM $WorkerName already exists"
}

# Configure VM
Set-VMProcessor -VMName $WorkerName -Count $CpuCount
Set-VM -VMName $WorkerName -AutomaticCheckpointsEnabled $false

# Attach ISO
$dvd = Get-VMDvdDrive -VMName $WorkerName -ErrorAction SilentlyContinue
if (-not $dvd) {
    Add-VMDvdDrive -VMName $WorkerName -Path $IsoPath
} else {
    Set-VMDvdDrive -VMName $WorkerName -Path $IsoPath
}
Set-VMFirmware -VMName $WorkerName -FirstBootDevice (Get-VMDvdDrive -VMName $WorkerName)
Write-Success "Attached ISO"

# Secure Boot & TPM Configuration
if ($Os -eq "windows11") {
    # Windows 11 requires both Secure Boot and TPM 2.0

    # Use MicrosoftWindows template (most compatible with official ISOs)
    Set-VMFirmware -VMName $WorkerName -EnableSecureBoot On `
        -SecureBootTemplate "MicrosoftWindows"
    Write-Success "Enabled Secure Boot (MicrosoftWindows template)"

    # Enable TPM 2.0 (required for Windows 11)
    try {
        Enable-VMTPM -VMName $WorkerName -ErrorAction Stop
        Write-Success "Enabled TPM 2.0"
    } catch {
        Write-Error "TPM enable failed: $_. Windows 11 requires TPM 2.0. Check host TPM support."
        exit 1
    }
} else {
    # Windows 10 doesn't require Secure Boot or TPM
    Set-VMFirmware -VMName $WorkerName -EnableSecureBoot Off
    Write-Success "Secure Boot disabled (not required for Windows 10)"

    # Enable TPM anyway (good practice, optional for Win10)
    try {
        Enable-VMTPM -VMName $WorkerName -ErrorAction Stop
        Write-Success "Enabled TPM 2.0 (optional for Windows 10)"
    } catch {
        Write-Info "TPM enable failed (not critical for Windows 10): $_"
    }
}

# Disable Guest Services (security)
Get-VMIntegrationService -VMName $WorkerName | Where-Object { $_.Name -eq "Guest Service Interface" } | Disable-VMIntegrationService

Write-Success "VM $WorkerName ready for Windows install"
Write-Info "Next: Start VM in Hyper-V Manager and install Windows"
Write-Info "After install, run: .\04-vm-init.ps1 -StaticIP $StaticIP -WorkerName $WorkerName"

exit 0
