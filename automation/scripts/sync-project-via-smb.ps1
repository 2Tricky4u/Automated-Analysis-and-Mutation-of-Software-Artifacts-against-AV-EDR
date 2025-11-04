<#
.SYNOPSIS
    Sync project to VM using SMB share (alternative to Copy-VMFile)

.DESCRIPTION
    Alternative sync method that uses network shares instead of Hyper-V integration services.
    More reliable when Copy-VMFile has issues.

    Prerequisites:
    - VM must have network connectivity (IP address configured)
    - File sharing enabled on VM
    - Admin share C$ accessible (or custom share)

.PARAMETER VMName
    Name of the VM to sync to

.PARAMETER VMIPAddress
    IP address of the VM (if not specified, will try to detect)

.PARAMETER Credential
    Credentials for accessing VM (if not specified, will prompt)

.PARAMETER SharePath
    Share path on VM (default: C$\AutoMutate\dev)

.EXAMPLE
    .\sync-project-via-smb.ps1 -VMName "win10-worker-00" -VMIPAddress "10.200.200.100"

.EXAMPLE
    # Auto-detect IP
    .\sync-project-via-smb.ps1 -VMName "win10-worker-00"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [string]$VMIPAddress,

    [PSCredential]$Credential,

    [string]$SharePath = "C$\AutoMutate\dev"
)

$ErrorActionPreference = "Stop"

# Colors
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERR] $M" -ForegroundColor Red }

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "Project Sync via SMB: $VMName" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

# Determine project root
$ScriptDir = $PSScriptRoot
$AutomationDir = Split-Path $ScriptDir -Parent
$ProjectRoot = Split-Path $AutomationDir -Parent
$ProjectName = Split-Path $ProjectRoot -Leaf

Write-Info "Project root: $ProjectRoot"

# Get VM
$VM = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if (-not $VM) {
    Write-Err "VM not found: $VMName"
    exit 1
}

if ($VM.State -ne "Running") {
    Write-Err "VM is not running"
    exit 1
}

# Get IP address if not specified
if (-not $VMIPAddress) {
    Write-Info "Detecting VM IP address..."
    $VMAdapter = Get-VMNetworkAdapter -VMName $VMName | Where-Object { $_.IPAddresses.Count -gt 0 } | Select-Object -First 1
    if ($VMAdapter) {
        $VMIPAddress = $VMAdapter.IPAddresses[0]
        Write-Success "Detected IP: $VMIPAddress"
    } else {
        Write-Err "Could not detect VM IP address"
        Write-Info "Ensure VM has network adapter configured and IP assigned"
        exit 1
    }
}

# Get credentials if not specified
if (-not $Credential) {
    Write-Info "Enter credentials for VM ($VMIPAddress):"
    Write-Info "  Format: .\Administrator or Administrator or WORKGROUP\Administrator"
    $Credential = Get-Credential -UserName ".\Administrator" -Message "Enter VM credentials"
}

# Test connectivity
Write-Info "Testing connectivity to $VMIPAddress..."
if (-not (Test-Connection -ComputerName $VMIPAddress -Count 1 -Quiet)) {
    Write-Err "Cannot reach VM at $VMIPAddress"
    Write-Info "Check VM network configuration"
    exit 1
}
Write-Success "VM is reachable"

# Construct UNC path
$UNCPath = "\\$VMIPAddress\$SharePath"
Write-Info "UNC path: $UNCPath"

# Map network drive with credentials (required for robocopy)
Write-Info "Mapping network drive..."
try {
    # Remove existing Z: drive if present
    if (Test-Path "Z:\") {
        net use Z: /delete /y 2>$null | Out-Null
    }

    # Get username and password
    $Username = $Credential.UserName
    $Password = $Credential.GetNetworkCredential().Password

    # Normalize username format for SMB authentication
    # Try to ensure we have .\username format for local VM account
    if ($Username -notlike "*\*") {
        # No domain/computer prefix, add .\ for local account
        $Username = ".\$Username"
    }

    Write-Info "Attempting authentication as: $Username"

    # Map using net use (robocopy requires this for authentication)
    $NetUseResult = net use Z: "\\$VMIPAddress\C$" /user:$Username $Password 2>&1

    if ($LASTEXITCODE -ne 0) {
        # Try without .\ prefix (might be domain account)
        $AltUsername = $Credential.UserName
        if ($AltUsername -like ".\*") {
            $AltUsername = $AltUsername.Substring(2)
        }

        Write-Info "Retrying authentication as: $AltUsername"
        $NetUseResult = net use Z: "\\$VMIPAddress\C$" /user:$AltUsername $Password 2>&1

        if ($LASTEXITCODE -ne 0) {
            throw "net use failed (error $($LASTEXITCODE)): $NetUseResult"
        }
    }

    Write-Success "Mapped network drive: Z:"

    # Verify access
    if (-not (Test-Path "Z:\")) {
        throw "Drive Z: not accessible after mapping"
    }

} catch {
    Write-Err "Failed to map network drive: $($_.Exception.Message)"
    Write-Info ""
    Write-Info "Common causes for error 1326 (Logon failure):"
    Write-Info "  - Incorrect username or password"
    Write-Info "  - Account is not an administrator on the VM"
    Write-Info "  - Username format issue (try: .\Administrator)"
    Write-Info ""
    Write-Info "Ensure on VM:"
    Write-Info "  1. File and Printer Sharing is enabled"
    Write-Info "  2. Admin share C$ is accessible (run: net share)"
    Write-Info "  3. Firewall allows SMB (port 445)"
    Write-Info "  4. User account has admin privileges"
    Write-Info "  5. User Account Control (UAC) not blocking admin shares"
    Write-Info ""
    Write-Info "To test manually from host:"
    Write-Info "  net use Z: \\$VMIPAddress\C$ /user:.\Administrator"
    Write-Info ""
    Write-Info "To check on VM:"
    Write-Info "  net share"
    Write-Info "  Get-LocalUser Administrator"
    Write-Info "  Get-LocalGroupMember Administrators"
    exit 1
}

# Create destination directory structure
$DestPath = "Z:\AutoMutate\dev\$ProjectName"
Write-Info "Destination: $DestPath"

try {
    if (-not (Test-Path "Z:\AutoMutate")) {
        Write-Info "Creating Z:\AutoMutate..."
        New-Item -ItemType Directory -Path "Z:\AutoMutate" -Force -ErrorAction Stop | Out-Null
    }

    if (-not (Test-Path "Z:\AutoMutate\dev")) {
        Write-Info "Creating Z:\AutoMutate\dev..."
        New-Item -ItemType Directory -Path "Z:\AutoMutate\dev" -Force -ErrorAction Stop | Out-Null
    }

    if (-not (Test-Path $DestPath)) {
        Write-Info "Creating $DestPath..."
        New-Item -ItemType Directory -Path $DestPath -Force -ErrorAction Stop | Out-Null
    }

    Write-Success "Destination directories ready"
} catch {
    Write-Err "Failed to create destination directories: $($_.Exception.Message)"
    net use Z: /delete /y 2>$null | Out-Null
    exit 1
}

# Exclusions
$Exclusions = @(
    "target",
    ".git",
    "node_modules",
    ".vscode",
    ".idea",
    "*.log",
    "*.exe",
    "*.dll",
    "*.pdb"
)

# Use robocopy for fast sync
Write-Info "Syncing files..."
Write-Host ""

$RobocopyArgs = @(
    $ProjectRoot,
    $DestPath,
    "/MIR",           # Mirror (delete extra files)
    "/MT:8",          # Multi-threaded
    "/R:2",           # Retry count
    "/W:5",           # Wait between retries
    "/NP",            # No progress per file
    "/NDL",           # No directory list
    "/XD"             # Exclude directories
)

# Add exclusions
foreach ($excl in $Exclusions) {
    if ($excl -notlike "*.*") {
        $RobocopyArgs += $excl
    }
}

# Add file exclusions
$FileExclusions = $Exclusions | Where-Object { $_ -like "*.*" }
if ($FileExclusions) {
    $RobocopyArgs += "/XF"
    $RobocopyArgs += $FileExclusions
}

# Run robocopy
Write-Info "Starting robocopy (this may take a few minutes)..."
$RobocopyResult = & robocopy @RobocopyArgs 2>&1

# Robocopy exit codes: 0-7 are success, 8+ are errors
$ExitCode = $LASTEXITCODE

Write-Host ""

if ($ExitCode -ge 8) {
    Write-Err "Robocopy failed with exit code: $ExitCode"
    Write-Err "Robocopy output:"
    $RobocopyResult | ForEach-Object { Write-Host "  $_" -ForegroundColor Yellow }

    # Cleanup mapped drive
    net use Z: /delete /y 2>$null | Out-Null

    Write-Info ""
    Write-Info "Common causes:"
    Write-Info "  16 = Fatal error (no files copied)"
    Write-Info "       - Check source path exists: $ProjectRoot"
    Write-Info "       - Check destination path exists: $DestPath"
    Write-Info "       - Check permissions on VM"
    Write-Info "  8  = Some files failed to copy"
    Write-Info "       - May be locked files (running executables)"
    exit 1
}

Write-Success "Sync completed successfully (exit code: $ExitCode)"

# Cleanup mapped drive
Write-Info "Cleaning up mapped drive..."
net use Z: /delete /y 2>$null | Out-Null

Write-Host "`n========================================" -ForegroundColor Green
Write-Host "Sync Complete!" -ForegroundColor Green
Write-Host "========================================`n" -ForegroundColor Green

Write-Info "Project location on VM: C:\AutoMutate\dev\$ProjectName"
Write-Host ""
