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

.PARAMETER HyperVHost
    Hyper-V host name (default: local computer)

.EXAMPLE
    # Local Hyper-V (script running on same machine as Hyper-V)
    .\sync-project-via-smb.ps1 -VMName "win10-worker-00"

.EXAMPLE
    # Specify IP address (if VM not accessible via Hyper-V)
    .\sync-project-via-smb.ps1 -VMName "win10-worker-00" -VMIPAddress "10.200.200.100"

.EXAMPLE
    # Remote Hyper-V host
    .\sync-project-via-smb.ps1 -VMName "win10-worker-00" -HyperVHost "HYPERV-SERVER"

.EXAMPLE
    # From different PC (no Hyper-V access, IP required)
    .\sync-project-via-smb.ps1 -VMName "win10-worker-00" -VMIPAddress "10.200.200.100"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [string]$VMIPAddress,

    [PSCredential]$Credential,

    [string]$SharePath = "C$\AutoMutate\dev",

    [string]$HyperVHost = $env:COMPUTERNAME
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

# Get VM (supports remote Hyper-V host)
$VM = $null
$VMDetectionFailed = $false

try {
    if ($HyperVHost -eq $env:COMPUTERNAME) {
        Write-Info "Checking VM on local Hyper-V..."
        $VM = Get-VM -Name $VMName -ErrorAction Stop
    } else {
        Write-Info "Checking VM on remote Hyper-V host: $HyperVHost..."
        $VM = Get-VM -Name $VMName -ComputerName $HyperVHost -ErrorAction Stop
    }

    if ($VM.State -ne "Running") {
        Write-Warn "VM is not running (state: $($VM.State))"
        Write-Info "VM must be running for sync to work"
    } else {
        Write-Success "VM found and running"
    }

} catch {
    Write-Warn "Could not access VM via Hyper-V: $($_.Exception.Message)"
    Write-Info "Will proceed with IP address only (if provided)"
    $VMDetectionFailed = $true
}

# Get IP address if not specified
if (-not $VMIPAddress) {
    if ($VMDetectionFailed) {
        Write-Err "VM IP address is required when Hyper-V access fails"
        Write-Info "Usage: .\sync-project-via-smb.ps1 -VMName '$VMName' -VMIPAddress '10.200.200.XXX'"
        Write-Info ""
        Write-Info "To find VM IP address:"
        Write-Info "  1. RDP to Hyper-V host or VM"
        Write-Info "  2. Run: ipconfig"
        Write-Info "  3. Or run on Hyper-V host: Get-VMNetworkAdapter -VMName '$VMName' | Select-Object IPAddresses"
        exit 1
    }

    Write-Info "Detecting VM IP address..."
    try {
        if ($HyperVHost -eq $env:COMPUTERNAME) {
            $VMAdapter = Get-VMNetworkAdapter -VMName $VMName -ErrorAction Stop | Where-Object { $_.IPAddresses.Count -gt 0 } | Select-Object -First 1
        } else {
            $VMAdapter = Get-VMNetworkAdapter -VMName $VMName -ComputerName $HyperVHost -ErrorAction Stop | Where-Object { $_.IPAddresses.Count -gt 0 } | Select-Object -First 1
        }

        if ($VMAdapter) {
            $VMIPAddress = $VMAdapter.IPAddresses[0]
            Write-Success "Detected IP: $VMIPAddress"
        } else {
            Write-Err "Could not detect VM IP address"
            Write-Info "Ensure VM has network adapter configured and IP assigned"
            Write-Info "Or specify IP manually: -VMIPAddress '10.200.200.XXX'"
            exit 1
        }
    } catch {
        Write-Err "Failed to get VM network adapter: $($_.Exception.Message)"
        Write-Info "Specify IP address manually: -VMIPAddress '10.200.200.XXX'"
        exit 1
    }
} else {
    Write-Info "Using provided IP address: $VMIPAddress"
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
