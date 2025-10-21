<#
.SYNOPSIS
    Create virtual floppy disk with autounattend.xml for automated Windows installation

.PARAMETER OutputPath
    Path where the .vfd floppy disk will be created

.PARAMETER AutounattendPath
    Path to the autounattend.xml template file
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$OutputPath = "..\templates\autounattend.vfd",

    [Parameter()]
    [string]$AutounattendPath = "..\templates\autounattend.xml"
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }

Write-Info "Creating virtual floppy disk with autounattend.xml..."

# Resolve full paths
$OutputPath = Resolve-Path $OutputPath -ErrorAction SilentlyContinue
if (-not $OutputPath) {
    $OutputPath = Join-Path (Split-Path $PSScriptRoot -Parent) "templates\autounattend.vfd"
}

$AutounattendPath = Resolve-Path $AutounattendPath -ErrorAction SilentlyContinue
if (-not $AutounattendPath) {
    $AutounattendPath = Join-Path (Split-Path $PSScriptRoot -Parent) "templates\autounattend.xml"
}

# Verify autounattend.xml exists
if (-not (Test-Path $AutounattendPath)) {
    Write-Error "autounattend.xml not found at: $AutounattendPath"
    exit 1
}

# Create output directory if needed
$OutputDir = Split-Path $OutputPath -Parent
if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
}

# Create a 1.44MB floppy disk image (standard floppy size)
$FloppySize = 1474560  # 1.44 MB in bytes

# Create empty VFD file
$bytes = New-Object byte[] $FloppySize
[System.IO.File]::WriteAllBytes($OutputPath, $bytes)
Write-Info "Created blank floppy image: $OutputPath"

# Mount the VFD as a drive (Windows only, requires admin)
try {
    # Step 1: Attach VFD
    $attachScript = @"
select vdisk file="$OutputPath"
attach vdisk
"@
    $tempScript = [System.IO.Path]::GetTempFileName()
    $attachScript | Out-File $tempScript -Encoding ASCII
    $attachOutput = diskpart /s $tempScript 2>&1 | Out-String

    if ($attachOutput -notmatch "DiskPart successfully") {
        throw "Failed to attach VFD"
    }

    Start-Sleep -Seconds 2

    # Step 2: Find the mounted disk
    $listDiskScript = @"
select vdisk file="$OutputPath"
detail vdisk
"@
    $listDiskScript | Out-File $tempScript -Encoding ASCII
    $detailOutput = diskpart /s $tempScript 2>&1 | Out-String

    # Step 3: Create partition and format
    $formatScript = @"
select vdisk file="$OutputPath"
create partition primary
format fs=fat quick label="UNATTEND"
assign
"@
    $formatScript | Out-File $tempScript -Encoding ASCII
    Write-Info "Formatting floppy disk..."
    $formatOutput = diskpart /s $tempScript 2>&1 | Out-String

    Start-Sleep -Seconds 2

    # Step 4: Find assigned drive letter
    $listVolScript = @"
select vdisk file="$OutputPath"
detail vdisk
"@
    $listVolScript | Out-File $tempScript -Encoding ASCII
    $volOutput = diskpart /s $tempScript 2>&1 | Out-String

    # Try multiple methods to find drive letter
    $driveLetter = $null

    # Method 1: Look for "Volume X     Y" pattern in detail output
    if ($volOutput -match "Volume\s+\d+\s+([A-Z])") {
        $driveLetter = $matches[1]
    }

    # Method 2: Check all volumes for UNATTEND label
    if (-not $driveLetter) {
        $volumes = Get-Volume | Where-Object { $_.FileSystemLabel -eq "UNATTEND" }
        if ($volumes) {
            $driveLetter = $volumes[0].DriveLetter
        }
    }

    # Method 3: Get newest drive letter
    if (-not $driveLetter) {
        Start-Sleep -Seconds 1
        $drives = Get-PSDrive -PSProvider FileSystem | Where-Object { $_.Name -match '^[A-Z]$' } | Sort-Object -Property Root -Descending
        $driveLetter = $drives[0].Name
    }

    if ($driveLetter) {
        Write-Success "Mounted floppy as drive ${driveLetter}:"

        # Copy autounattend.xml to the floppy
        $destPath = "${driveLetter}:\autounattend.xml"
        Copy-Item $AutounattendPath $destPath -Force
        Write-Success "Copied autounattend.xml to floppy disk"

        # Detach the VFD
        $detachScript = @"
select vdisk file="$OutputPath"
detach vdisk
"@
        $detachScript | Out-File $tempScript -Encoding ASCII
        diskpart /s $tempScript | Out-Null
        Write-Success "Detached floppy disk"

    } else {
        throw "Could not determine drive letter after mounting"
    }

    Remove-Item $tempScript -Force -ErrorAction SilentlyContinue

} catch {
    Write-Warning "Diskpart method failed: $_"
    Write-Info "Trying alternative method..."

    # Cleanup: try to detach if still attached
    try {
        $cleanupScript = @"
select vdisk file="$OutputPath"
detach vdisk
"@
        $tempCleanup = [System.IO.Path]::GetTempFileName()
        $cleanupScript | Out-File $tempCleanup -Encoding ASCII
        diskpart /s $tempCleanup 2>&1 | Out-Null
        Remove-Item $tempCleanup -Force -ErrorAction SilentlyContinue
    } catch {}

    Write-Warning "Automatic floppy creation failed."
    Write-Info "Manual workaround:"
    Write-Info "  1. In Windows Explorer, double-click: $OutputPath"
    Write-Info "  2. Right-click the mounted drive -> Format -> FAT -> Start"
    Write-Info "  3. Copy $AutounattendPath to the drive"
    Write-Info "  4. Eject the drive"
    Write-Info ""
    Write-Info "OR use the pre-formatted floppy from the repository (if available)"
}

Write-Success "Virtual floppy disk ready: $OutputPath"
Write-Info "Attach this to your VM during Windows installation"

exit 0
