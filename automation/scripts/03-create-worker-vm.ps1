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
Write-Success "Attached ISO: $IsoPath"

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

Write-Success "VM $WorkerName ready for Windows installation"

# Display final configuration
$secureBootStatus = if ($Os -eq "windows11") { "Enabled (MicrosoftWindows)" } else { "Disabled" }
$tpmStatus = if ($tpmEnabled) { "Enabled" } else { "Failed (check logs)" }

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          VM Configuration Summary                              |" -ForegroundColor Cyan
Write-Host "+================================================================+" -ForegroundColor Cyan
Write-Host "| Name:        $WorkerName".PadRight(66) + "|" -ForegroundColor White
Write-Host "| OS Type:     $Os".PadRight(66) + "|" -ForegroundColor White
Write-Host "| IP Address:  $StaticIP".PadRight(66) + "|" -ForegroundColor White
Write-Host "| Generation:  2 (UEFI)".PadRight(66) + "|" -ForegroundColor White
Write-Host "| Secure Boot: $secureBootStatus".PadRight(66) + "|" -ForegroundColor White
Write-Host "| TPM 2.0:     $tpmStatus".PadRight(66) + "|" -ForegroundColor White
Write-Host "| Boot Order:  DVD -> HDD".PadRight(66) + "|" -ForegroundColor White
Write-Host "+================================================================+" -ForegroundColor Cyan

Write-Host "`n+================================================================+" -ForegroundColor Yellow
Write-Host "|          MANUAL INSTALLATION REQUIRED                          |" -ForegroundColor Yellow
Write-Host "+================================================================+" -ForegroundColor Yellow
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Step 1: Start VM and Install Windows                          |" -ForegroundColor White
Write-Host "|  ----------------------------------------------------------    |" -ForegroundColor DarkGray
Write-Host "|    * Open Hyper-V Manager                                      |" -ForegroundColor White
Write-Host "|    * Right-click '$WorkerName' -> Connect -> Start".PadRight(63) "|" -ForegroundColor White
Write-Host "|    * Wait for Windows Setup to boot from ISO                   |" -ForegroundColor White
Write-Host "|    * UEFI fails! Restart will appear-> press tab and then space|" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Step 2: Windows Setup Choices (MINIMAL INPUT REQUIRED)        |" -ForegroundColor White
Write-Host "|  ----------------------------------------------------------    |" -ForegroundColor DarkGray
Write-Host "|    * Language: English (United States) -> Next                 |" -ForegroundColor White
Write-Host "|    * Install now                                               |" -ForegroundColor White
Write-Host "|    * Product key: Skip / I don't have a product key            |" -ForegroundColor White
Write-Host "|    * Edition: Windows 10/11 Pro -> Next                        |" -ForegroundColor White
Write-Host "|    * Accept license -> Next                                    |" -ForegroundColor White
Write-Host "|    * Custom: Install Windows only (advanced)                   |" -ForegroundColor White
Write-Host "|    * Partition: Select 'Drive 0 Unallocated Space' -> Next     |" -ForegroundColor White
Write-Host "|      (Windows will auto-create partitions)                     |" -ForegroundColor DarkGray
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Step 3: OOBE (Out-of-Box Experience)                          |" -ForegroundColor White
Write-Host "|  ----------------------------------------------------------    |" -ForegroundColor DarkGray
Write-Host "|    * Region: United States -> Yes                              |" -ForegroundColor White
Write-Host "|    * Keyboard: US -> Yes                                       |" -ForegroundColor White
Write-Host "|    * Skip second keyboard layout                               |" -ForegroundColor White
Write-Host "|    * Network: 'I don't have internet' (bottom-left)            |" -ForegroundColor White
Write-Host "|      OR 'Skip for now' / 'Continue with limited setup'         |" -ForegroundColor DarkGray
Write-Host "|    * Account name:  worker-admin                               |" -ForegroundColor Cyan
Write-Host "|    * Password:      AutoMutate!Password                        |" -ForegroundColor Cyan
Write-Host "|    * Security questions: Answer anything (write them down)     |" -ForegroundColor White
Write-Host "|    * Privacy settings: Disable all (faster)                    |" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Step 4: After Desktop Appears (5-10 min)                      |" -ForegroundColor White
Write-Host "|  ----------------------------------------------------------    |" -ForegroundColor DarkGray
Write-Host "|    * From HOST PowerShell (Admin), run:                        |" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|      `$cred = Get-Credential -UserName 'worker-admin'           |" -ForegroundColor Green
Write-Host "|      (Enter password: AutoMutate!Password)                     |" -ForegroundColor DarkGray
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|      Invoke-Command -VMName '$WorkerName' ``".PadRight(66) + "|" -ForegroundColor Green
Write-Host "|        -FilePath '.\scripts\04-vm-init.ps1' ``".PadRight(66) + "|" -ForegroundColor Green
Write-Host "|        -ArgumentList '$StaticIP', '$WorkerName' ``".PadRight(66) + "|" -ForegroundColor Green
Write-Host "|        -Credential `$cred".PadRight(66) + "|" -ForegroundColor Green
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|    The script will configure network, install tools, etc.      |" -ForegroundColor DarkGray
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "+================================================================+" -ForegroundColor Yellow

Write-Host "`nCredentials Summary:" -ForegroundColor Magenta
Write-Host "  Username: worker-admin" -ForegroundColor Cyan
Write-Host "  Password: AutoMutate!Password" -ForegroundColor Cyan
Write-Host "  IP:       $StaticIP (will be configured by 04-vm-init.ps1)`n" -ForegroundColor Cyan

exit 0
