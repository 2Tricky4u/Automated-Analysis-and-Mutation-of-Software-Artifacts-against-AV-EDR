<#
.SYNOPSIS
    Standalone RedEDR preparation script for Worker VMs.
.DESCRIPTION
    Configures a Windows VM for RedEDR telemetry collection only.
    Extracted from 04-vm-init.ps1 — contains ONLY RedEDR-relevant setup:
      - Defender exclusions (paths + processes)
      - Project directory creation
      - RedEDR extraction from zip
      - Driver certificate trust (ELAM)
      - Test-signing + HVCI disable
      - Audit policy (all categories + subcategories)
      - RedEDR driver + PPL service installation
      - SYSTEM launcher helper script + desktop shortcut

    Excludes: network config, hostname, tool installs (Rust, protoc, VS Build Tools),
    firewall/SMB rules, Chocolatey, privacy/telemetry suppression, UAC changes.
.PARAMETER RedEDRPort
    Port the RedEDR web UI listens on (default: 8081).
.PARAMETER RedEdrZipPath
    Path to RedEdr.zip (default: C:\AutoMutate\build\telemetry\RedEdr.zip).
.PARAMETER DisableEtwTi
    Skip ETW Threat-Intelligence provider registration.
.PARAMETER SkipReboot
    Skip the final reboot prompt.
.EXAMPLE
    .\prepare-rededr.ps1
.EXAMPLE
    .\prepare-rededr.ps1 -RedEdrZipPath "D:\RedEdr.zip" -SkipReboot
.NOTES
    Requires: Administrator privileges.
    Source: automation/scripts/04-vm-init.ps1 (sections 0, 6-10 only).
#>

[CmdletBinding()]
param(
    [Parameter()]
    [int]$RedEDRPort = 8081,

    [Parameter()]
    [string]$RedEdrZipPath = "C:\AutoMutate\build\telemetry\RedEdr.zip",

    [Parameter()]
    [switch]$DisableEtwTi,

    [Parameter()]
    [switch]$SkipReboot
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERR] $M" -ForegroundColor Red }

# --- Admin check ---
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Err "This script must be run as Administrator."
    exit 1
}

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          RedEDR Preparation                                    |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

# ============================================================================
# [1/8] Defender exclusions
# ============================================================================
Write-Info "[1/8] Configuring Windows Defender exclusions..."

try {
    $exclusionPaths = @(
        "C:\AutoMutate",
        "C:\RedEdr",
        "C:\Temp",
        $env:TEMP,
        "C:\Windows\Temp"
    )

    foreach ($path in $exclusionPaths) {
        Add-MpPreference -ExclusionPath $path -ErrorAction SilentlyContinue
    }

    # Process exclusions relevant to RedEDR operation
    $exclusionProcesses = @(
        "powershell.exe",
        "pwsh.exe"
    )

    foreach ($process in $exclusionProcesses) {
        Add-MpPreference -ExclusionProcess $process -ErrorAction SilentlyContinue
    }

    # Temporarily disable real-time monitoring during setup
    Set-MpPreference -DisableRealtimeMonitoring $true -ErrorAction SilentlyContinue

    Write-Success "Windows Defender exclusions configured"
    Write-Info "Real-time monitoring temporarily disabled for setup"

} catch {
    Write-Warn "Could not configure Defender exclusions: $($_.Exception.Message)"
    Write-Info "This may cause script execution to be blocked"
}

# ============================================================================
# [2/8] Project directories
# ============================================================================
Write-Info "[2/8] Creating AutoMutate project directories..."

$projectDirs = @(
    "C:\AutoMutate",
    "C:\AutoMutate\artifacts",
    "C:\AutoMutate\logs",
    "C:\AutoMutate\harness"
)

foreach ($dir in $projectDirs) {
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        Write-Success "Created: $dir"
    }
}

# ============================================================================
# [3/8] RedEDR extraction from zip
# ============================================================================
Write-Info "[3/8] RedEdr setup (extract from zip)..."

$RedEdrSourceZip = $RedEdrZipPath
$RedEdrZip = "$env:TEMP\RedEdr.zip"
$RedEdrRoot = "C:\RedEdr"   # only this path is supported

try {
    if (-not (Test-Path $RedEdrSourceZip)) {
        throw "RedEdr.zip not found at: $RedEdrSourceZip. Provide correct path via -RedEdrZipPath."
    }

    Write-Info "Found RedEdr.zip at: $RedEdrSourceZip"
    $sourceSize = (Get-Item $RedEdrSourceZip).Length
    Write-Info "Source file size: $([math]::Round($sourceSize/1MB, 2)) MB"

    # Copy to temp location for extraction
    if (Test-Path $RedEdrZip) {
        Write-Info "Removing existing temp copy..."
        Remove-Item $RedEdrZip -Force
    }

    Write-Info "Copying RedEdr.zip to temp location..."
    Copy-Item $RedEdrSourceZip $RedEdrZip -Force

    # Verify copied ZIP is valid
    $fileSize = (Get-Item $RedEdrZip).Length
    if ($fileSize -lt 100KB) {
        throw "RedEdr.zip is too small ($fileSize bytes), file may be corrupted."
    }

    # Verify ZIP signature (PK\x03\x04)
    $zipHeader = [System.IO.File]::ReadAllBytes($RedEdrZip)[0..3]
    if (-not ($zipHeader[0] -eq 0x50 -and $zipHeader[1] -eq 0x4B)) {
        throw "RedEdr.zip is not a valid ZIP archive (missing PK signature)."
    }

    Write-Success "RedEdr.zip validated ($([math]::Round($fileSize/1MB, 2)) MB)"

    # Prepare installation directory
    if (Test-Path $RedEdrRoot) {
        Write-Info "Clearing existing $RedEdrRoot"
        Remove-Item $RedEdrRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    New-Item -ItemType Directory -Path $RedEdrRoot -Force | Out-Null

    # Extract RedEdr to temporary location first
    $tempExtractPath = "$env:TEMP\RedEdr_extract"
    if (Test-Path $tempExtractPath) {
        Remove-Item $tempExtractPath -Recurse -Force
    }
    New-Item -ItemType Directory -Path $tempExtractPath -Force | Out-Null

    Write-Info "Extracting RedEdr.zip..."
    Expand-Archive -Path $RedEdrZip -DestinationPath $tempExtractPath -Force

    # Check if ZIP contains a nested folder or direct files
    $extractedItems = Get-ChildItem $tempExtractPath
    if ($extractedItems.Count -eq 1 -and $extractedItems[0].PSIsContainer) {
        # ZIP contains a single folder (e.g., RedEdr/), move its contents
        $nestedFolder = $extractedItems[0].FullName
        Write-Info "Moving contents from nested folder: $($extractedItems[0].Name)"
        Move-Item "$nestedFolder\*" $RedEdrRoot -Force
    } else {
        # ZIP contains direct files, move everything
        Write-Info "Moving extracted files to $RedEdrRoot..."
        Move-Item "$tempExtractPath\*" $RedEdrRoot -Force
    }

    # Cleanup temp extraction folder
    Remove-Item $tempExtractPath -Recurse -Force -ErrorAction SilentlyContinue

    Write-Success "RedEdr extracted to $RedEdrRoot"

    # Verify main executable exists
    $rededrExe = Join-Path $RedEdrRoot "RedEdr.exe"
    if (Test-Path $rededrExe) {
        $exeVersion = (Get-Item $rededrExe).VersionInfo.FileVersion
        Write-Success "RedEdr.exe found at $RedEdrRoot (version: $exeVersion)"
    } else {
        Write-Warn "RedEdr.exe not found at $RedEdrRoot"
        Write-Info "Listing contents of ${RedEdrRoot}:"
        Get-ChildItem $RedEdrRoot -Recurse | Select-Object -First 10 | ForEach-Object {
            Write-Info "  $($_.FullName)"
        }
    }

} catch {
    Write-Err "Failed to install RedEdr: $($_.Exception.Message)"
    Write-Info ""
    Write-Info "Troubleshooting:"
    Write-Info "  - Ensure RedEdr.zip exists at the specified path"
    Write-Info "  - Use -RedEdrZipPath to specify an alternate location"
    Write-Info "  - Check source location: $RedEdrSourceZip"
    Write-Info ""
    Write-Info "Manual installation:"
    Write-Info "  1. Place RedEdr.zip at: $RedEdrSourceZip"
    Write-Info "  2. Re-run this script"
}

# Defender exclusion for RedEdr.exe
$rededrExe = Join-Path $RedEdrRoot "RedEdr.exe"
try {
    if (Test-Path $rededrExe) {
        Add-MpPreference -ExclusionPath $rededrExe -ErrorAction SilentlyContinue
        Write-Success "Defender exclusion added: $rededrExe"
    } else { Write-Warn "RedEdr.exe not found yet at $rededrExe" }
} catch { Write-Warn "Could not add Defender exclusion: $($_.Exception.Message)" }

# ============================================================================
# [4/8] Driver certificate trust
# ============================================================================
Write-Info "[4/8] Extracting and trusting RedEdr driver certificate..."
$elamDriver = Join-Path $RedEdrRoot "elam_driver.sys"
$certTempDir = "C:\Temp"

try {
    # Ensure C:\Temp exists
    if (-not (Test-Path $certTempDir)) {
        New-Item -ItemType Directory -Path $certTempDir -Force | Out-Null
    }

    if (Test-Path $elamDriver) {
        Write-Info "Step 1: Extracting signer certificate from elam_driver.sys"

        # Get the Authenticode signature
        $sig = Get-AuthenticodeSignature $elamDriver
        $cert = $sig.SignerCertificate

        if ($cert) {
            Write-Info "Certificate Details:"
            Write-Host "  Subject   : $($cert.Subject)" -ForegroundColor Gray
            Write-Host "  Issuer    : $($cert.Issuer)" -ForegroundColor Gray
            Write-Host "  Thumbprint: $($cert.Thumbprint)" -ForegroundColor Gray
            Write-Host "  NotAfter  : $($cert.NotAfter)" -ForegroundColor Gray

            # Export certificate
            $certPath = Join-Path $certTempDir "elam_signer.cer"
            Export-Certificate -Cert $cert -FilePath $certPath -Force | Out-Null
            Write-Success "Certificate exported to: $certPath"

            Write-Info "Step 2: Installing certificate as trusted"

            # Install to Trusted Root CA
            Import-Certificate -FilePath $certPath -CertStoreLocation Cert:\LocalMachine\Root -ErrorAction Stop | Out-Null
            Write-Success "Certificate installed to Trusted Root Certification Authorities"

            # Install to Trusted Publishers
            Import-Certificate -FilePath $certPath -CertStoreLocation Cert:\LocalMachine\TrustedPublisher -ErrorAction Stop | Out-Null
            Write-Success "Certificate installed to Trusted Publishers"

            Write-Info "Step 3: Verifying signature status"

            # Re-check signature
            $verifySign = Get-AuthenticodeSignature $elamDriver
            if ($verifySign.Status -eq "Valid") {
                Write-Success "Driver signature verified: Valid"
            } else {
                Write-Warn "Driver signature status: $($verifySign.Status)"
                Write-Info "If status is not Valid, the certificate chain may be incomplete"
                Write-Info "This may require intermediate certificates or reboot to take effect"
            }

        } else {
            Write-Warn "Could not extract certificate from driver (SignerCertificate is null)"
            Write-Info "The driver may not be signed, or signature extraction failed"
        }

    } else {
        Write-Warn "ELAM driver not found at: $elamDriver"
        Write-Info "Certificate trust setup skipped (driver will be untrusted until installed)"
    }

} catch {
    Write-Warn "Failed to trust driver certificate: $($_.Exception.Message)"
    Write-Info "Driver may not load until certificate is manually trusted"
    Write-Info "Manual steps:"
    Write-Info "  1. cd C:\RedEdr"
    Write-Info "  2. `$sig = Get-AuthenticodeSignature .\elam_driver.sys"
    Write-Info "  3. Export-Certificate -Cert `$sig.SignerCertificate -FilePath C:\Temp\elam_signer.cer"
    Write-Info "  4. Import-Certificate -FilePath C:\Temp\elam_signer.cer -CertStoreLocation Cert:\LocalMachine\Root"
    Write-Info "  5. Import-Certificate -FilePath C:\Temp\elam_signer.cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher"
}

# ============================================================================
# [5/8] Test-signing and HVCI disable
# ============================================================================
Write-Info "[5/8] Kernel driver allowances (testsigning, debug)..."
# Required for RedEdr kernel callbacks / KAPC injection / ETW-TI PPL via ELAM
try {
    bcdedit /enum | Out-Null
    & bcdedit /set testsigning on   | Out-Null
    & bcdedit -debug on             | Out-Null
    Write-Success "Enabled test-signed drivers and kernel debug"
    Write-Info "If running on Hyper-V, disable Secure Boot on the VM (host-side setting)."
} catch { Write-Warn "BCDEdit changes failed: $($_.Exception.Message)" }

# Disable HVCI/Memory Integrity: often blocks test-signed drivers
try {
    $dg = "HKLM:\SYSTEM\CurrentControlSet\Control\DeviceGuard"
    if (-not (Test-Path $dg)) { New-Item $dg -Force | Out-Null }
    New-ItemProperty -Path $dg -Name "EnableVirtualizationBasedSecurity" -PropertyType DWord -Value 0 -Force | Out-Null
    $ci = "HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy"
    if (-not (Test-Path $ci)) { New-Item $ci -Force | Out-Null }
    New-ItemProperty -Path $ci -Name "VerifiedAndReputablePolicyState" -PropertyType DWord -Value 0 -Force | Out-Null
    Write-Info "Disabled VBS/HVCI policy (effective after reboot) if it was on."
} catch { Write-Warn "Could not adjust DeviceGuard/HVCI: $($_.Exception.Message)" }

# ============================================================================
# [6/8] Audit policy (ALL categories + subcategories)
# ============================================================================
Write-Info "[6/8] Enabling audit policies for Security-Auditing ETW (MAXIMUM TELEMETRY)..."
# Some Microsoft-Windows-Security-Auditing events require audit categories enabled and SYSTEM token

# Force Advanced Audit Policy mode (legacy audit policy silently overrides)
$auditPolicyKey = "HKLM:\SYSTEM\CurrentControlSet\Control\Lsa"
if (-not (Test-Path $auditPolicyKey)) {
    New-Item -Path $auditPolicyKey -Force | Out-Null
}
# SCENoApplyLegacyAuditPolicy = 1 means "use Advanced Audit Policy, ignore legacy"
Set-ItemProperty -Path $auditPolicyKey -Name "SCENoApplyLegacyAuditPolicy" -Value 1 -Type DWord -Force
Write-Success "Enabled Advanced Audit Policy mode (disabled legacy audit policy)"

# Enable ALL audit categories for maximum telemetry using auditpol
$cats = @(
    "Logon","Policy Change","Account Logon","Account Management","Privilege Use",
    "System","DS Access","Object Access","Detailed Tracking"
)
foreach($c in $cats){
    try { & auditpol /set /category:$c /success:enable /failure:enable | Out-Null } catch {}
}

# Enable critical subcategories for EDR/malware analysis
$subcategories = @(
    # Detailed Tracking - Process telemetry
    "Process Creation",
    "Process Termination",
    "DPAPI Activity",
    "RPC Events",
    "Plug and Play Events",

    # Object Access - Handle/injection detection
    "Handle Manipulation",
    "Kernel Object",
    "File System",
    "Registry",
    "SAM",
    "Other Object Access Events",
    "Removable Storage",
    "Central Policy Staging",
    "Detailed File Share",

    # System - Driver/service loads
    "Security State Change",
    "Security System Extension",
    "System Integrity",
    "IPsec Driver",
    "Other System Events",

    # Privilege Use - Token manipulation
    "Sensitive Privilege Use",
    "Non Sensitive Privilege Use",
    "Other Privilege Use Events",

    # Account Logon/Management - Credential access
    "Credential Validation",
    "Kerberos Authentication Service",
    "Kerberos Service Ticket Operations",
    "User Account Management",
    "Computer Account Management",
    "Security Group Management",
    "Distribution Group Management",
    "Application Group Management",

    # Policy Change - Security policy modifications
    "Audit Policy Change",
    "Authentication Policy Change",
    "Authorization Policy Change",
    "MPSSVC Rule-Level Policy Change",
    "Filtering Platform Policy Change",

    # Logon/Logoff - Session tracking
    "Logon",
    "Logoff",
    "Account Lockout",
    "IPsec Main Mode",
    "IPsec Quick Mode",
    "IPsec Extended Mode",
    "Special Logon",
    "Other Logon/Logoff Events",
    "Network Policy Server",
    "User / Device Claims",
    "Group Membership"
)

foreach($subcat in $subcategories){
    try {
        & auditpol /set /subcategory:"$subcat" /success:enable /failure:enable 2>$null | Out-Null
    } catch {
        # Silently continue if subcategory not available on this Windows version
    }
}

# Persist auditpol settings into the Local Group Policy database so they survive gpupdate
Write-Info "Backing up audit policy configuration to Local Group Policy..."
$auditBackupPath = "$env:TEMP\audit-policy-backup.csv"
try {
    & auditpol /backup /file:$auditBackupPath | Out-Null
    if (Test-Path $auditBackupPath) {
        # Restore from backup to force it into the policy database
        & auditpol /restore /file:$auditBackupPath | Out-Null
        Remove-Item $auditBackupPath -Force -ErrorAction SilentlyContinue
        Write-Success "Audit policy synchronized with Local Group Policy database"
    }
} catch {
    Write-Warn "Could not backup/restore audit policy: $($_.Exception.Message)"
}

# Force Group Policy refresh so audit settings take effect immediately
Write-Info "Forcing Group Policy update to apply audit settings..."
try {
    & gpupdate /force | Out-Null
    Write-Success "Group Policy updated"
} catch {
    Write-Warn "gpupdate failed: $($_.Exception.Message)"
}

# Enable command-line logging for Process Creation events (Event ID 4688)
$auditProcessKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\Audit"
if (-not (Test-Path $auditProcessKey)) {
    New-Item -Path $auditProcessKey -Force | Out-Null
}
Set-ItemProperty -Path $auditProcessKey -Name "ProcessCreationIncludeCmdLine_Enabled" -Value 1 -Type DWord -Force

# Enable PowerShell script block logging
$psLoggingKey = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\PowerShell\ScriptBlockLogging"
if (-not (Test-Path $psLoggingKey)) {
    New-Item -Path $psLoggingKey -Force | Out-Null
}
Set-ItemProperty -Path $psLoggingKey -Name "EnableScriptBlockLogging" -Value 1 -Type DWord -Force

# Enable PowerShell module logging
$psModuleLoggingKey = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\PowerShell\ModuleLogging"
if (-not (Test-Path $psModuleLoggingKey)) {
    New-Item -Path $psModuleLoggingKey -Force | Out-Null
}
Set-ItemProperty -Path $psModuleLoggingKey -Name "EnableModuleLogging" -Value 1 -Type DWord -Force

# Enable PowerShell transcription
$psTranscriptKey = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\PowerShell\Transcription"
if (-not (Test-Path $psTranscriptKey)) {
    New-Item -Path $psTranscriptKey -Force | Out-Null
}
Set-ItemProperty -Path $psTranscriptKey -Name "EnableTranscripting" -Value 1 -Type DWord -Force
Set-ItemProperty -Path $psTranscriptKey -Name "EnableInvocationHeader" -Value 1 -Type DWord -Force
Set-ItemProperty -Path $psTranscriptKey -Name "OutputDirectory" -Value "C:\AutoMutate\logs\ps-transcripts" -Type String -Force

# Create PS transcript directory
if (-not (Test-Path "C:\AutoMutate\logs\ps-transcripts")) {
    New-Item -ItemType Directory -Path "C:\AutoMutate\logs\ps-transcripts" -Force | Out-Null
}

Write-Success "Audit policy updated (success+failure) for ALL categories and critical subcategories"
Write-Success "Enabled: Command-line logging, PowerShell script block logging, module logging, transcription"
Write-Info "For Security-Auditing ETW, start RedEdr as SYSTEM when needed."

# ============================================================================
# [7/8] RedEDR drivers + PPL service
# ============================================================================
Write-Info "[7/8] Installing RedEdr drivers & services (ETW, Kernel, ETW-TI/PPL)..."

# Attempt to install any driver *.inf shipped inside the release
try {
    $infFiles = Get-ChildItem -Path $RedEdrRoot -Recurse -Include *.inf -ErrorAction SilentlyContinue
    foreach($inf in $infFiles){
        Write-Info "Installing driver from: $($inf.FullName)"
        pnputil /add-driver "$($inf.FullName)" /install | Out-Null
    }
    if ($infFiles.Count -gt 0) { Write-Success "Driver(s) installed via pnputil" }
    else { Write-Info "No INF drivers found in release package" }
} catch { Write-Warn "Driver install via pnputil failed: $($_.Exception.Message)" }

# Register ETW-TI PPL service if present and not disabled
if (-not $DisableEtwTi) {
    # Heuristics: find a service binary likely named RedEdrPplService.exe
    $pplCandidate = Get-ChildItem -Path $RedEdrRoot -Recurse -Include RedEdrPplService.exe, *PplService*.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($pplCandidate) {
        try {
            $svcName="RedEdrPplService"
            if (-not (Get-Service -Name $svcName -ErrorAction SilentlyContinue)) {
                sc.exe create $svcName binPath= "`"$($pplCandidate.FullName)`"" start= demand | Out-Null
                sc.exe description $svcName "RedEdr ETW-TI PPL Service" | Out-Null
                Write-Success "Created service $svcName"
            } else { Write-Info "Service $svcName already exists" }

            # ELAM registration typically occurs via driver/INF
            Write-Info "If ETW-TI fails, ensure the ELAM driver from the release got installed (via INF) and reboot."
        } catch { Write-Warn "Failed to create ETW-TI service: $($_.Exception.Message)" }
    } else {
        Write-Info "No PPL service binary found in release; ETW-TI may be unavailable until compiled."
    }
} else {
    Write-Info "ETW-TI setup skipped by request (-DisableEtwTi)."
}

# ============================================================================
# [8/8] SYSTEM launcher helper script + desktop shortcut
# ============================================================================
Write-Info "[8/8] Creating SYSTEM launcher helper script..."

$helperScriptContent = @'
<#
.SYNOPSIS
    Start RedEDR as SYSTEM with maximum telemetry collection (-e -g -k --web).

.DESCRIPTION
    Creates and starts a Scheduled Task that runs RedEdr.exe as NT AUTHORITY\SYSTEM
    with the following flags:
      -e, --etw      : Consume ETW Events
      -g, --etwti    : Consume ETW-TI Events (requires ELAM driver)
      -k, --hook     : Kernel and ntdll hooks
      -w, --web      : Enable web server on port 8081

    Optional parameters:
      -t, --trace    : Process name to observe (default: malware)
      -p, --port     : Web server port (default: 8081)

    Notes:
      - ETW-TI requires ELAM driver and RedEdrPplService (snapshot VM first)
      - Kernel hooks require test-signed driver support (bcdedit /set testsigning on)
      - For Security-Auditing ETW, must run as SYSTEM (this script does that)

.PARAMETER TraceTarget
    Process name to observe (default: malware). Use "*" to trace all processes.

.PARAMETER Port
    Web server port (default: 8081).

.PARAMETER StopOnly
    Stop running instance and remove the task without starting a new one.
#>
param(
    [string]$TraceTarget = "malware",
    [int]$Port = 8081,
    [switch]$StopOnly
)

$ErrorActionPreference = "Stop"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

# --- Admin check ---
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Err "Must run this script as Administrator."
    exit 1
}

$RedEdrExe = "C:\RedEDR\RedEdr.exe"
$TaskName  = "AutoMutate-RedEDR-SYSTEM"

Write-Info "Verifying RedEDR binary at: $RedEdrExe"
if (-not (Test-Path $RedEdrExe)) {
    Write-Err "RedEdr.exe not found. Install to C:\RedEDR first."
    exit 1
}

# --- Stop/clean existing ---
$existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existingTask) {
    Write-Info "Stopping existing RedEDR scheduled task..."
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue | Out-Null
    Start-Sleep -Seconds 2
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
}
Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

if ($StopOnly) {
    Write-Success "RedEDR stopped and task removed (-StopOnly)."
    exit 0
}

# --- Build argument string (MAXIMUM TELEMETRY MODE) ---
Write-Info "Configuring RedEDR with MAXIMUM telemetry collection:"
Write-Host "  -e, --etw      : ETW Events" -ForegroundColor Gray
Write-Host "  -g, --etwti    : ETW-TI Events (kernel callbacks)" -ForegroundColor Gray
Write-Host "  -k, --hook     : Kernel and ntdll hooks" -ForegroundColor Gray
Write-Host "  -w, --web      : Web UI on port $Port" -ForegroundColor Gray
Write-Host "  -t, --trace    : Target process '$TraceTarget'" -ForegroundColor Gray

$argList = @("-e", "-g", "-k", "-w", "-p", $Port.ToString(), "-t", $TraceTarget)

$rededrArgs = ($argList | ForEach-Object {
    if ($_ -match '\s') { '"' + $_ + '"' } else { $_ }
}) -join ' '

Write-Host ""
Write-Info "Creating SYSTEM scheduled task with command:"
Write-Host "  $RedEdrExe $rededrArgs" -ForegroundColor Gray
Write-Warn "ETW-TI requires ELAM driver (snapshot VM before first run)"
Write-Warn "Kernel hooks require test-signed drivers (bcdedit /set testsigning on)"

# --- Create task to run as SYSTEM ---
$action    = New-ScheduledTaskAction -Execute $RedEdrExe -Argument $rededrArgs -WorkingDirectory "C:\RedEDR"
$principal = New-ScheduledTaskPrincipal -UserId "NT AUTHORITY\SYSTEM" -LogonType ServiceAccount -RunLevel Highest
$settings  = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -DontStopOnIdleEnd `
    -ExecutionTimeLimit (New-TimeSpan -Days 0) `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -RestartCount 999

Register-ScheduledTask -TaskName $TaskName -Action $action -Principal $principal -Settings $settings -Force | Out-Null

# --- Start it ---
Write-Info "Starting RedEDR as SYSTEM..."
Start-ScheduledTask -TaskName $TaskName
Start-Sleep -Seconds 3

$proc = Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue
if ($proc) {
    Write-Success "RedEDR started as SYSTEM (PID: $($proc.Id))"
    Write-Info    "Mode        : MAXIMUM TELEMETRY (ETW + ETW-TI + Hooks)"
    Write-Info    "Target      : $TraceTarget"
    Write-Info    "Web UI      : http://localhost:$Port"
    Write-Info    "Stop        : .\Start-RedEDR-SYSTEM.ps1 -StopOnly"
    Write-Host ""
    Write-Info "Telemetry channels active:"
    Write-Host "  [checkmark] ETW kernel providers - process, thread, network, registry, file" -ForegroundColor Green
    Write-Host "  [checkmark] ETW-TI - stack traces, image loads, thread context" -ForegroundColor Green
    Write-Host "  [checkmark] Kernel hooks - syscall interception" -ForegroundColor Green
    Write-Host "  [checkmark] Security-Auditing events - audit policy enabled" -ForegroundColor Green
} else {
    Write-Warn "RedEDR process not detected."
    Write-Info "Check Task Scheduler (taskschd.msc) - Task '$TaskName' - History for details."
    Write-Info "Common issues:"
    Write-Info "  - ETW-TI requires ELAM driver (check driver installation)"
    Write-Info "  - Kernel hooks require testsigning on (bcdedit /set testsigning on)"
    Write-Info "  - Reboot may be required after driver installation"
}
'@

try {
    $helperScript = Join-Path $RedEdrRoot "Start-RedEDR-SYSTEM.ps1"
    $helperScriptContent | Out-File -FilePath $helperScript -Encoding UTF8 -Force
    Write-Success "Created SYSTEM launcher: $helperScript"
} catch {
    Write-Warn "Could not create SYSTEM launcher: $($_.Exception.Message)"
}

# Desktop shortcut pointing to SYSTEM launcher (requires Admin)
try {
    $WScriptShell = New-Object -ComObject WScript.Shell
    $Shortcut = $WScriptShell.CreateShortcut("$([Environment]::GetFolderPath('Desktop'))\RedEDR (SYSTEM).lnk")
    $Shortcut.TargetPath = "powershell.exe"
    $Shortcut.Arguments = "-ExecutionPolicy Bypass -NoProfile -File `"$helperScript`""
    $Shortcut.WorkingDirectory = $RedEdrRoot
    $Shortcut.IconLocation = "$rededrExe,0"
    $Shortcut.Save()
    Write-Success "Desktop shortcut created: RedEDR (SYSTEM).lnk"
    Write-Info "Right-click shortcut -> Run as Administrator to start RedEDR"
} catch { Write-Warn "Could not create desktop shortcut: $($_.Exception.Message)" }

# ============================================================================
# Re-enable Defender real-time monitoring
# ============================================================================
Write-Info "Re-enabling Windows Defender real-time monitoring..."
try {
    Set-MpPreference -DisableRealtimeMonitoring $false -ErrorAction SilentlyContinue
    Write-Success "Defender real-time monitoring re-enabled"
    Write-Info "Note: Path/process exclusions remain in place for C:\AutoMutate and C:\RedEdr"
} catch {
    Write-Warn "Could not re-enable Defender: $($_.Exception.Message)"
}

# ============================================================================
# Verification (RedEDR-relevant subset)
# ============================================================================
Write-Host "`n+================================================================+" -ForegroundColor Green
Write-Host "|          RedEDR Preparation Complete - Verification            |" -ForegroundColor Green
Write-Host "+================================================================+" -ForegroundColor Green

$verificationResults = @()

# RedEdr presence
$rededrPresent = (Test-Path $rededrExe)
$verificationResults += [PSCustomObject]@{
    Component = "RedEdr"
    Status    = if ($rededrPresent) { "OK" } else { "FAIL" }
    Details   = if ($rededrPresent) { "Installed at C:\RedEdr" } else { "Missing RedEdr.exe" }
}

# Testsigning state
try {
    $bcd = bcdedit /enum | Out-String
    $tsOn = $bcd -match "testsigning\s+Yes"
    $dbgOn = $bcd -match "debug\s+Yes"
    $verificationResults += [PSCustomObject]@{
        Component = "BootConfig"
        Status    = if ($tsOn -and $dbgOn) { "OK" } else { "WARN" }
        Details   = "testsigning: " + ($(if($tsOn){"on"}else{"off"})) + ", debug: " + ($(if($dbgOn){"on"}else{"off"}))
    }
} catch {
    $verificationResults += [PSCustomObject]@{ Component="BootConfig"; Status="WARN"; Details="bcdedit unavailable" }
}

# Audit policy check
try {
    $auditCheck = & auditpol /get /category:"Detailed Tracking" 2>$null | Out-String
    $auditOk = $auditCheck -match "Success and Failure"
    $verificationResults += [PSCustomObject]@{
        Component = "AuditPolicy"
        Status    = if ($auditOk) { "OK" } else { "WARN" }
        Details   = if ($auditOk) { "Detailed Tracking: Success+Failure" } else { "Check auditpol /get /category:*" }
    }
} catch {
    $verificationResults += [PSCustomObject]@{ Component="AuditPolicy"; Status="WARN"; Details="auditpol unavailable" }
}

# Driver certificate trust
try {
    $certInRoot = Get-ChildItem Cert:\LocalMachine\Root | Where-Object { $_.Subject -match "RedEdr|ELAM" }
    $certInPublisher = Get-ChildItem Cert:\LocalMachine\TrustedPublisher | Where-Object { $_.Subject -match "RedEdr|ELAM" }
    $certStatus = if ($certInRoot -and $certInPublisher) { "OK" } elseif ($certInRoot -or $certInPublisher) { "WARN" } else { "FAIL" }
    $certDetails = "Root: $(if($certInRoot){'installed'}else{'missing'}), TrustedPublisher: $(if($certInPublisher){'installed'}else{'missing'})"
    $verificationResults += [PSCustomObject]@{
        Component = "DriverCert"
        Status    = $certStatus
        Details   = $certDetails
    }
} catch {
    $verificationResults += [PSCustomObject]@{ Component="DriverCert"; Status="WARN"; Details="Could not check cert stores" }
}

# HVCI / VBS state
try {
    $dgKey = Get-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\DeviceGuard" -Name "EnableVirtualizationBasedSecurity" -ErrorAction SilentlyContinue
    $hvciOff = ($dgKey.EnableVirtualizationBasedSecurity -eq 0)
    $verificationResults += [PSCustomObject]@{
        Component = "HVCI/VBS"
        Status    = if ($hvciOff) { "OK" } else { "WARN" }
        Details   = if ($hvciOff) { "Disabled (test-signed drivers allowed)" } else { "May block test-signed drivers" }
    }
} catch {
    $verificationResults += [PSCustomObject]@{ Component="HVCI/VBS"; Status="WARN"; Details="Could not check DeviceGuard" }
}

foreach ($result in $verificationResults) {
    $statusColor = switch ($result.Status) { "OK" { "Green" } "WARN" { "Yellow" } "FAIL" { "Red" } }
    $line = "| [$($result.Status.PadRight(4))] $($result.Component.PadRight(15)) $($result.Details)"
    Write-Host $line.PadRight(66) + "|" -ForegroundColor $statusColor
}
Write-Host "+================================================================+" -ForegroundColor Green

# ============================================================================
# Reboot prompt
# ============================================================================
Write-Host "`n+================================================================+" -ForegroundColor Yellow
Write-Host "|          REBOOT REQUIRED                                       |" -ForegroundColor Yellow
Write-Host "+================================================================+" -ForegroundColor Yellow
Write-Host "|  Changes requiring reboot:                                     |"
Write-Host "|    * Test-signed driver support & kernel debug (bcdedit)       |"
Write-Host "|    * VBS/HVCI policy changes                                   |"
Write-Host "|    * Driver certificate trust propagation                      |"
Write-Host "|                                                                |"
Write-Host "|  IMPORTANT for Hyper-V: Disable Secure Boot on the host VM.   |"
Write-Host "|                                                                |"
Write-Host "|  After reboot, you can:                                        |"
Write-Host "|                                                                |"
Write-Host "|  1. Start RedEdr telemetry collector (as SYSTEM):              |"
Write-Host "|     Right-click desktop shortcut 'RedEDR (SYSTEM).lnk'        |"
Write-Host "|     -> Run as Administrator                                    |"
Write-Host "|                                                                |"
Write-Host "|     OR manually:                                               |"
Write-Host "|     cd C:\RedEdr                                               |"
Write-Host "|     .\Start-RedEDR-SYSTEM.ps1                                  |"
Write-Host "|                                                                |"
Write-Host "|     Then open http://localhost:$RedEDRPort".PadRight(64) "|"
Write-Host "+================================================================+" -ForegroundColor Yellow

if (-not $SkipReboot) {
    Write-Host "`nRebooting in 10 seconds (Ctrl+C to cancel)..." -ForegroundColor Yellow
    Start-Sleep -Seconds 10
    Restart-Computer -Force
} else {
    Write-Info "Reboot skipped (-SkipReboot). Manual reboot strongly recommended."
}
