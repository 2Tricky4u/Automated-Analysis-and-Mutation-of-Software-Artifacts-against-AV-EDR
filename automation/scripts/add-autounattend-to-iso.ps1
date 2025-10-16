<#
.SYNOPSIS
    Add autounattend.xml to Windows ISO (create custom unattended installation ISO)

.PARAMETER IsoPath
    Path to original Windows ISO

.PARAMETER OutputIsoPath
    Path for new ISO with autounattend.xml (optional, defaults to original_autounattend.iso)

.PARAMETER AutounattendPath
    Path to autounattend.xml template
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$IsoPath,

    [Parameter()]
    [string]$OutputIsoPath,

    [Parameter()]
    [string]$AutounattendPath = "..\templates\autounattend.xml"
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

# Validate inputs
if (-not (Test-Path $IsoPath)) {
    Write-Error "ISO not found: $IsoPath"
    exit 1
}

if (-not (Test-Path $AutounattendPath)) {
    Write-Error "autounattend.xml not found: $AutounattendPath"
    exit 1
}

# Default output path
if (-not $OutputIsoPath) {
    $isoName = [System.IO.Path]::GetFileNameWithoutExtension($IsoPath)
    $isoDir = Split-Path $IsoPath -Parent
    $OutputIsoPath = Join-Path $isoDir "${isoName}_autounattend.iso"
}

Write-Info "Source ISO: $IsoPath"
Write-Info "Output ISO: $OutputIsoPath"
Write-Info "Autounattend: $AutounattendPath"
Write-Info ""

# Create temp directory
$tempDir = Join-Path $env:TEMP "iso_extract_$(Get-Random)"
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
Write-Info "Created temp directory: $tempDir"

try {
    # Mount the original ISO
    Write-Info "Mounting original ISO..."
    $mountResult = Mount-DiskImage -ImagePath $IsoPath -PassThru
    $driveLetter = ($mountResult | Get-Volume).DriveLetter

    if (-not $driveLetter) {
        throw "Failed to mount ISO or get drive letter"
    }

    Write-Success "Mounted ISO as ${driveLetter}:"

    # Copy all ISO contents to temp directory
    Write-Info "Copying ISO contents (this may take a few minutes)..."
    $sourceRoot = "${driveLetter}:\"
    Copy-Item -Path "$sourceRoot*" -Destination $tempDir -Recurse -Force
    Write-Success "Copied ISO contents to temp directory"

    # Dismount original ISO
    Dismount-DiskImage -ImagePath $IsoPath | Out-Null
    Write-Info "Dismounted original ISO"

    # Copy autounattend.xml to root of temp directory
    $destAutounattend = Join-Path $tempDir "autounattend.xml"
    Copy-Item $AutounattendPath $destAutounattend -Force
    Write-Success "Added autounattend.xml to ISO root"

    # Check if oscdimg.exe is available (from Windows ADK)
    $oscdimg = $null
    $adkPaths = @(
        "${env:ProgramFiles(x86)}\Windows Kits\10\Assessment and Deployment Kit\Deployment Tools\amd64\Oscdimg\oscdimg.exe",
        "${env:ProgramFiles}\Windows Kits\10\Assessment and Deployment Kit\Deployment Tools\amd64\Oscdimg\oscdimg.exe",
        "${env:ProgramFiles(x86)}\Windows Kits\10\Assessment and Deployment Kit\Deployment Tools\x86\Oscdimg\oscdimg.exe"
    )

    foreach ($path in $adkPaths) {
        if (Test-Path $path) {
            $oscdimg = $path
            break
        }
    }

    if (-not $oscdimg) {
        Write-Warn "oscdimg.exe not found (Windows ADK not installed)"
        Write-Info ""
        Write-Info "ALTERNATIVE: Manual ISO Creation"
        Write-Info "The ISO contents (with autounattend.xml) are ready at:"
        Write-Info "  $tempDir"
        Write-Info ""
        Write-Info "Option A: Use a tool like ImgBurn, AnyBurn, or PowerISO to create the ISO"
        Write-Info "Option B: Install Windows ADK and re-run this script"
        Write-Info "  Download: https://go.microsoft.com/fwlink/?linkid=2243390"
        Write-Info "  Install only: Deployment Tools"
        Write-Info ""
        Write-Warn "Temp directory will be kept: $tempDir"
        Write-Info "Delete it manually when done"
        exit 0
    }

    Write-Success "Found oscdimg.exe: $oscdimg"

    # Get boot sector from original ISO for UEFI boot
    Write-Info "Creating bootable ISO..."

    # Boot options for UEFI + BIOS (Gen2 VM)
    $bootData = Join-Path $tempDir "efi\microsoft\boot\efisys.bin"
    if (-not (Test-Path $bootData)) {
        $bootData = Join-Path $tempDir "boot\etfsboot.com"  # Fallback to BIOS boot
    }

    $oscdimgArgs = @(
        "-m",                    # Ignore max size limit
        "-o",                    # Optimize (duplicate file merge)
        "-u2",                   # UDF file system
        "-udfver102",           # UDF version 1.02
        "-bootdata:2",          # Two boot entries (BIOS + UEFI)
        "#p0,e,b`"$bootData`"", # BIOS boot
        "#pEF,e,b`"$bootData`"" # UEFI boot
        "-t",                    # Timestamp
        (Get-Date).ToString("MM/dd/yyyy,HH:mm:ss"),
        "-l`"WIN_UNATTEND`"",   # Volume label
        "`"$tempDir`"",         # Source directory
        "`"$OutputIsoPath`""    # Output ISO
    )

    # Simplified command for compatibility
    $cmd = "& `"$oscdimg`" -m -o -u2 -udfver102 -l`"WIN_UNATTEND`" `"$tempDir`" `"$OutputIsoPath`""

    Write-Info "Running: oscdimg.exe"
    Invoke-Expression $cmd | Out-Null

    if (Test-Path $OutputIsoPath) {
        Write-Success "Created custom ISO: $OutputIsoPath"
        Write-Info ""
        $sizeGB = [math]::Round((Get-Item $OutputIsoPath).Length / 1GB, 2)
        Write-Info "ISO Size: $sizeGB GB"
        Write-Info ""
        Write-Success "You can now use this ISO for automated Windows installation!"
        Write-Info "Update your config or scripts to use: $OutputIsoPath"
    } else {
        throw "ISO creation failed - output file not found"
    }

} catch {
    Write-Error "Failed: $_"
    Write-Warn "Temp directory preserved for debugging: $tempDir"
    exit 1
} finally {
    # Cleanup temp directory on success
    if (Test-Path $OutputIsoPath) {
        Remove-Item $tempDir -Recurse -Force -ErrorAction SilentlyContinue
        Write-Info "Cleaned up temp directory"
    }
}

exit 0
