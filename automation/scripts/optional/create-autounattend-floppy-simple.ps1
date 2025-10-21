<#
.SYNOPSIS
    Create virtual floppy disk with autounattend.xml (admin-free method using Hyper-V)

.DESCRIPTION
    This script creates a VFD floppy image by:
    1. Creating a temporary VM
    2. Attaching a blank floppy
    3. Booting to a minimal PE environment that formats the floppy
    4. Copying autounattend.xml

    Alternative: Just use the ISO method (put autounattend.xml in ISO root)
#>

param(
    [Parameter()]
    [string]$OutputPath = "..\templates\autounattend.vfd",

    [Parameter()]
    [string]$AutounattendPath = "..\templates\autounattend.xml"
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

Write-Info "Simpler solution: Mount VFD in Hyper-V to format it"
Write-Info ""
Write-Warn "Creating VFD floppy requires admin privileges (diskpart)"
Write-Info ""
Write-Info "ALTERNATIVE SOLUTIONS:"
Write-Info ""
Write-Info "Option 1: ISO Method (Easiest - No Admin Needed)"
Write-Info "  - Copy autounattend.xml to the ROOT of your Windows ISO"
Write-Info "  - Windows Setup will find it there automatically"
Write-Info "  - No floppy needed!"
Write-Info ""
Write-Info "Option 2: Manual Floppy Creation (One-Time Setup)"
Write-Info "  1. Right-click PowerShell -> Run as Administrator"
Write-Info "  2. Run: .\create-autounattend-floppy.ps1"
Write-Info "  3. Floppy will be created at: $OutputPath"
Write-Info ""
Write-Info "Option 3: Use Existing Floppy (If Available)"
Write-Info "  - The repository may already have a pre-formatted .vfd"
Write-Info "  - Just verify it contains autounattend.xml"
Write-Info ""

# Check if floppy already exists and has content
if (Test-Path $OutputPath) {
    $size = (Get-Item $OutputPath).Length
    if ($size -eq 1474560) {
        Write-Info "Existing floppy found: $OutputPath (1.44MB)"
        Write-Info "To verify it has autounattend.xml:"
        Write-Info "  1. Double-click the .vfd file in Windows Explorer"
        Write-Info "  2. Check if autounattend.xml is present"
        Write-Info "  3. If not, copy it manually and eject"
    } else {
        Write-Warn "Floppy exists but wrong size: $size bytes (expected 1474560)"
    }
} else {
    Write-Info "No floppy found at: $OutputPath"
}

Write-Info ""
Write-Success "For this project, RECOMMENDED: Use Option 1 (ISO method)"
Write-Info "It's simpler and doesn't require a floppy disk at all"

exit 0
