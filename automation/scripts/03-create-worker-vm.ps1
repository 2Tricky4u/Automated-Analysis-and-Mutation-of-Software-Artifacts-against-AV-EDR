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
    Write-Info "VM $WorkerName already exists (will reconfigure)"
}

# CRITICAL: ALWAYS disable Secure Boot first (whether VM is new or existing)
# This prevents key protector errors when enabling TPM later
Set-VMFirmware -VMName $WorkerName -EnableSecureBoot Off

# Verify Secure Boot is actually disabled
$firmware = Get-VMFirmware -VMName $WorkerName
if ($firmware.SecureBoot -eq 'Off') {
    Write-Info "Disabled Secure Boot (verified: $($firmware.SecureBoot))"
} else {
    Write-Warning "Secure Boot is STILL ON ($($firmware.SecureBoot)) - TPM may fail!"
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

# Create autounattend ISO (Gen2 VMs don't support floppy drives)
$templatesDir = Join-Path (Split-Path $PSScriptRoot -Parent) "templates"
$autounattendXml = Join-Path $templatesDir "autounattend.xml"
$autounattendIso = Join-Path $templatesDir "autounattend.iso"

if (-not (Test-Path $autounattendXml)) {
    Write-Warning "autounattend.xml not found: $autounattendXml"
    Write-Info "Installation will require manual input"
} else {
    Write-Info "Creating autounattend ISO for secondary DVD drive..."

    # Create temp directory for ISO contents
    $tempIsoDir = Join-Path $env:TEMP "autounattend_iso_$(Get-Random)"
    New-Item -ItemType Directory -Path $tempIsoDir -Force | Out-Null

    # Copy autounattend.xml to temp directory
    Copy-Item $autounattendXml (Join-Path $tempIsoDir "autounattend.xml") -Force

    # Simple method: Create ISO using mkisofs/genisoimage if available, or skip for now
    $mkisofs = $null
    $searchPaths = @(
        "C:\Program Files\cdrtools\mkisofs.exe",
        "C:\cdrtools\mkisofs.exe",
        "${env:ProgramFiles(x86)}\cdrtools\mkisofs.exe"
    )

    foreach ($path in $searchPaths) {
        if (Test-Path $path) { $mkisofs = $path; break }
    }

    if ($mkisofs) {
        & $mkisofs -o $autounattendIso -V "AUTOUNATTEND" -J -R $tempIsoDir 2>&1 | Out-Null
        if (Test-Path $autounattendIso) {
            Write-Success "Created autounattend ISO: $autounattendIso"
        }
    } else {
        # For now, just skip the ISO and provide instructions
        Write-Info "Skipping autounattend ISO creation (no ISO tools found)"
        Write-Info ""
        Write-Info "WORKAROUND: Add autounattend.xml to Windows ISO manually:"
        Write-Info "  1. Mount your Windows ISO (double-click)"
        Write-Info "  2. Copy contents to a folder (e.g., C:\WinISO)"
        Write-Info "  3. Copy autounattend.xml to that folder root"
        Write-Info "  4. Use Rufus or similar to create new ISO"
        Write-Info ""
        Write-Info "OR: Continue without autounattend - install Windows manually"
        Write-Info "    Username: worker-admin"
        Write-Info "    Password: AutoMutate!Password"
        Write-Info ""
    }

    Remove-Item $tempIsoDir -Recurse -Force -ErrorAction SilentlyContinue

    # Attach autounattend ISO as second DVD drive (Gen2 VMs support multiple DVDs)
    if (Test-Path $autounattendIso) {
        # Check if second DVD drive exists
        $dvdDrives = Get-VMDvdDrive -VMName $WorkerName
        if ($dvdDrives.Count -lt 2) {
            Add-VMDvdDrive -VMName $WorkerName -Path $autounattendIso
            Write-Success "Attached autounattend ISO as secondary DVD"
        } else {
            Set-VMDvdDrive -VMName $WorkerName -ControllerNumber 0 -ControllerLocation 1 -Path $autounattendIso
            Write-Success "Attached autounattend ISO to existing secondary DVD"
        }
    }
}

# Enable TPM with proper key protector
$tpmEnabled = $false
try {
    # Create a new local key protector (required for TPM)
    Set-VMKeyProtector -VMName $WorkerName -NewLocalKeyProtector
    Enable-VMTPM -VMName $WorkerName
    Write-Success "Enabled TPM 2.0"
    $tpmEnabled = $true
} catch {
    $errorMsg = $_.Exception.Message
    Write-Warning "TPM enable failed: $errorMsg"
    if ($Os -eq "windows11") {
        Write-Warning "Windows 11 installation may fail without TPM."
        Write-Info "Continuing anyway..."
    }
}

# Configure firmware settings (boot order + Secure Boot) AFTER TPM is enabled
# Use FirstBootDevice method (simpler and more reliable than BootOrder array)
$dvdDrive = Get-VMDvdDrive -VMName $WorkerName | Select-Object -First 1

if (-not $dvdDrive) {
    Write-Error "DVD drive not found after attachment"
    exit 1
}

if ($Os -eq "windows11") {
    # Windows 11: Set DVD as first boot + Enable Secure Boot
    # Don't specify template - let it use the default
    Set-VMFirmware -VMName $WorkerName -FirstBootDevice $dvdDrive -EnableSecureBoot On
    Write-Success "Configured firmware: DVD first boot + Secure Boot enabled"
} else {
    # Windows 10: Set DVD as first boot, keep Secure Boot OFF
    Set-VMFirmware -VMName $WorkerName -FirstBootDevice $dvdDrive -EnableSecureBoot Off
    Write-Success "Configured firmware: DVD first boot, Secure Boot disabled"
}

# Disable Guest Services (security)
Get-VMIntegrationService -VMName $WorkerName | Where-Object { $_.Name -eq "Guest Service Interface" } | Disable-VMIntegrationService

Write-Success "VM $WorkerName ready for Windows install"
Write-Info "Next: Start VM in Hyper-V Manager and install Windows"
Write-Info "After install, run: .\04-vm-init.ps1 -StaticIP $StaticIP -WorkerName $WorkerName"

# Display final configuration for troubleshooting
$secureBootStatus = if ($Os -eq "windows11") { "Enabled (MicrosoftWindows)" } else { "Disabled" }

Write-Host "`n--- VM Configuration Summary ---" -ForegroundColor Cyan
Write-Host "Name:        $WorkerName" -ForegroundColor White
Write-Host "OS Type:     $Os" -ForegroundColor White
Write-Host "Generation:  2 (UEFI)" -ForegroundColor White
Write-Host "Secure Boot: $secureBootStatus" -ForegroundColor White
Write-Host "TPM 2.0:     Attempted (check logs above for status)" -ForegroundColor White
Write-Host "Boot Order:  DVD → HDD" -ForegroundColor White
Write-Host "ISO Path:    $IsoPath" -ForegroundColor White
Write-Host "-------------------------------`n" -ForegroundColor Cyan

exit 0
