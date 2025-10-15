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

# CRITICAL: Use Gen1 (BIOS) for Windows 10, Gen2 (UEFI) for Windows 11
# Gen2 VMs often fail to boot from ISO with Windows 10 (UEFI firmware issues)
# Gen2 is required for Windows 11 (needs TPM 2.0 + Secure Boot)
$Generation = if ($Os -eq "windows11") { 2 } else { 1 }

Write-Info "Creating VM: $WorkerName ($Os, Gen$Generation) - $StaticIP"

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

# Create VM (Gen1 for Win10, Gen2 for Win11)
if (-not (Get-VM -Name $WorkerName -ErrorAction SilentlyContinue)) {
    New-VM -Name $WorkerName -MemoryStartupBytes ($MemoryGB * 1GB) `
        -Generation $Generation -VHDPath $VhdPath -SwitchName $SwitchName | Out-Null
    Write-Success "Created VM: $WorkerName (Generation $Generation)"
} else {
    Write-Info "VM $WorkerName already exists (will reconfigure)"
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
Write-Success "Attached ISO"

# Configure boot order and firmware (Gen2 only - Gen1 uses BIOS boot menu)
if ($Generation -eq 2) {
    # Gen2: Configure UEFI firmware settings (boot order + Secure Boot)
    # CRITICAL: Use FirstBootDevice method (simpler and more reliable than explicit BootOrder)
    $dvdDrive = Get-VMDvdDrive -VMName $WorkerName

    if (-not $dvdDrive) {
        Write-Error "DVD drive not found after attachment"
        exit 1
    }

    if ($Os -eq "windows11") {
        # Windows 11: Requires Secure Boot + TPM
        # CRITICAL: Enable Secure Boot FIRST, then TPM (reverse order from before)
        Set-VMFirmware -VMName $WorkerName -FirstBootDevice $dvdDrive `
            -EnableSecureBoot On -SecureBootTemplate "MicrosoftWindows"
        Write-Success "Configured firmware: DVD first boot + Secure Boot enabled"
    } else {
        # Windows 10 Gen2: Keep Secure Boot OFF (better compatibility with older ISOs)
        Set-VMFirmware -VMName $WorkerName -FirstBootDevice $dvdDrive `
            -EnableSecureBoot Off
        Write-Success "Configured firmware: DVD first boot, Secure Boot disabled"
    }
} else {
    # Gen1: BIOS-based boot (DVD automatically detected, no firmware config needed)
    Write-Info "Gen1 VM uses BIOS boot (DVD will auto-boot if bootable)"
}

# Enable TPM AFTER Secure Boot is configured (Gen2 only - Gen1 doesn't support TPM)
# CRITICAL: For Windows 11, TPM must be enabled AFTER Secure Boot is ON
$tpmEnabled = $false
if ($Generation -eq 2 -and $Os -eq "windows11") {
    try {
        Enable-VMTPM -VMName $WorkerName -ErrorAction Stop
        Write-Success "Enabled TPM 2.0"
        $tpmEnabled = $true
    } catch {
        $errorMsg = $_.Exception.Message
        Write-Warning "TPM enable failed: $errorMsg"
        Write-Warning "Windows 11 installation may fail without TPM."
        Write-Info "You can enable TPM manually: VM Settings → Security → Enable Trusted Platform Module"
        Write-Info "Continuing anyway..."
    }
} elseif ($Generation -eq 1) {
    Write-Info "Gen1 VM - TPM not available (not needed for Windows 10)"
}

# Disable Guest Services (security)
Get-VMIntegrationService -VMName $WorkerName | Where-Object { $_.Name -eq "Guest Service Interface" } | Disable-VMIntegrationService

Write-Success "VM $WorkerName ready for Windows install"
Write-Info "Next: Start VM in Hyper-V Manager and install Windows"
Write-Info "After install, run: .\04-vm-init.ps1 -StaticIP $StaticIP -WorkerName $WorkerName"

# Display final configuration for troubleshooting
$firmwareType = if ($Generation -eq 2) { "UEFI" } else { "BIOS" }
$secureBootStatus = if ($Generation -eq 2) {
    if ($Os -eq "windows11") { "Enabled (MicrosoftWindows)" } else { "Disabled" }
} else {
    "N/A (Gen1)"
}
$tpmStatus = if ($Generation -eq 2) {
    if ($tpmEnabled) { "Enabled" } else { "Failed/Not Available" }
} else {
    "N/A (Gen1)"
}

Write-Host "`n--- VM Configuration Summary ---" -ForegroundColor Cyan
Write-Host "Name:         $WorkerName" -ForegroundColor White
Write-Host "OS Type:      $Os" -ForegroundColor White
Write-Host "Generation:   $Generation ($firmwareType)" -ForegroundColor White
Write-Host "Secure Boot:  $secureBootStatus" -ForegroundColor White
Write-Host "TPM 2.0:      $tpmStatus" -ForegroundColor White
Write-Host "First Boot:   DVD (ISO)" -ForegroundColor White
Write-Host "ISO Path:     $IsoPath" -ForegroundColor White
Write-Host "-------------------------------`n" -ForegroundColor Cyan

exit 0
