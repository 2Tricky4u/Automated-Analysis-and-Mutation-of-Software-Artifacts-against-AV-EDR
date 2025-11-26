<#  (truncated header unchanged)  #>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$StaticIP,

    [Parameter(Mandatory)]
    [string]$WorkerName,

    [Parameter()]
    [string]$Gateway = "10.200.200.1",

    [Parameter()]
    [int]$Prefix = 24,

    [Parameter()]
    [int]$RedEDRPort = 8081,

    [Parameter()]
    [switch]$SkipReboot,

    [Parameter()]
    [switch]$DisableEtwTi
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERR] $M" -ForegroundColor Red }

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Worker VM Initialization                              |" -ForegroundColor Cyan
Write-Host "|          $WorkerName -> $StaticIP".PadRight(64) "|" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

# ===================== PRE-SECTION: Disable Defender for Automation ==========
Write-Info "[0/10] Disabling Windows Defender for automation directories..."

try {
    # Add path exclusions for automation-related directories
    $exclusionPaths = @(
        "C:\AutoMutate",
        "C:\RedEdr",
        "C:\Repos",
        "C:\Temp",
        $env:TEMP,
        "C:\Windows\Temp"
    )

    foreach ($path in $exclusionPaths) {
        Add-MpPreference -ExclusionPath $path -ErrorAction SilentlyContinue
    }

    # Add process exclusions for PowerShell and build tools
    $exclusionProcesses = @(
        "powershell.exe",
        "pwsh.exe",
        "msbuild.exe",
        "cargo.exe",
        "rustc.exe",
        "git.exe"
    )

    foreach ($process in $exclusionProcesses) {
        Add-MpPreference -ExclusionProcess $process -ErrorAction SilentlyContinue
    }

    # Temporarily disable real-time monitoring during initialization
    Set-MpPreference -DisableRealtimeMonitoring $true -ErrorAction SilentlyContinue

    Write-Success "Windows Defender exclusions configured"
    Write-Info "Real-time monitoring temporarily disabled for initialization"

} catch {
    Write-Warn "Could not configure Defender exclusions: $($_.Exception.Message)"
    Write-Info "This may cause script execution to be blocked"
}

# ===================== SECTION 1: System Configuration =====================
Write-Info "[1/10] System-level configuration..."

# 1a. Computer name
$currentName = $env:COMPUTERNAME
if ($currentName -ne $WorkerName) {
    Write-Info "Renaming computer: $currentName -> $WorkerName"
    Rename-Computer -NewName $WorkerName -Force -ErrorAction SilentlyContinue
    Write-Success "Computer renamed (requires reboot)"
} else { Write-Success "Computer name already set: $WorkerName" }

# 1b. Timezone to UTC
$currentTZ = (Get-TimeZone).Id
if ($currentTZ -ne "UTC") { Set-TimeZone -Id "UTC" -ErrorAction SilentlyContinue; Write-Success "Timezone set to UTC" }
else { Write-Success "Timezone already UTC" }

# 1c. Execution policy
$execPolicy = Get-ExecutionPolicy
if ($execPolicy -eq "Restricted" -or $execPolicy -eq "Undefined") {
    Set-ExecutionPolicy RemoteSigned -Scope LocalMachine -Force
    Write-Success "Execution policy set to RemoteSigned"
} else { Write-Success "Execution policy already configured: $execPolicy" }

# 1d. Disable UAC for lab
$uacKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"
$currentUAC = (Get-ItemProperty -Path $uacKey -Name "EnableLUA" -ErrorAction SilentlyContinue).EnableLUA
if ($currentUAC -ne 0) { Set-ItemProperty -Path $uacKey -Name "EnableLUA" -Value 0 -Force; Write-Success "UAC disabled" }
else { Write-Success "UAC already disabled" }

# 1e. Windows Update auto-restart
$auKey = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU"
if (-not (Test-Path $auKey)) { New-Item -Path $auKey -Force | Out-Null }
Set-ItemProperty -Path $auKey -Name "NoAutoRebootWithLoggedOnUsers" -Value 1 -Force
Write-Success "Windows Update auto-restart disabled"

# ===================== SECTION 2: Network Configuration ====================
Write-Info "[2/10] Network configuration..."
$adapter = Get-NetAdapter | Where-Object { $_.Status -eq "Up" -and $_.Name -notlike "*Loopback*" } | Select-Object -First 1
if ($adapter) {
    # Remove existing IP if DHCP
    Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue | Remove-NetIPAddress -Confirm:$false -ErrorAction SilentlyContinue
    Get-NetRoute -InterfaceIndex $adapter.ifIndex -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue | Remove-NetRoute -Confirm:$false -ErrorAction SilentlyContinue

    # Configure static IP
    New-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $StaticIP -PrefixLength $Prefix -DefaultGateway $Gateway | Out-Null

    # Configure DNS: Use public DNS directly (8.8.8.8 primary)
    Set-DnsClientServerAddress -InterfaceIndex $adapter.ifIndex -ServerAddresses @("8.8.8.8", "8.8.4.4", $Gateway)
    Write-Success "Static IP configured: $StaticIP/$Prefix (gateway: $Gateway)"
    Write-Info "DNS servers: 8.8.8.8 (primary), 8.8.4.4, $Gateway (fallback)"

    # Flush DNS cache to ensure fresh resolution
    Clear-DnsClientCache -ErrorAction SilentlyContinue
    ipconfig /flushdns | Out-Null

    # Wait for network stack to stabilize
    Start-Sleep -Seconds 3
    Write-Info "Testing network connectivity..."

    # Check if gateway route exists
    $route = Get-NetRoute -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue | Where-Object { $_.NextHop -eq $Gateway }
    if ($route) {
        Write-Success "Default route to gateway configured"
    } else {
        Write-Warn "Default route not found (may cause connectivity issues)"
    }

    # Try DNS resolution with retry
    Write-Info "Waiting for DNS resolution to stabilize (up to 30 seconds)..."
    $dnsWorking = $false
    $maxRetries = 10
    $retryCount = 0

    while (-not $dnsWorking -and $retryCount -lt $maxRetries) {
        $retryCount++
        try {
            $dnsTest = Resolve-DnsName -Name "google.com" -Type A -DnsOnly -ErrorAction Stop 2>$null
            if ($dnsTest) {
                Write-Success "Internet connectivity verified (DNS resolution works after $retryCount attempt(s))"
                $dnsWorking = $true
                break
            }
        } catch {
            if ($retryCount -lt $maxRetries) {
                Write-Info "DNS attempt $retryCount/$maxRetries failed, retrying in 3 seconds..."
                Start-Sleep -Seconds 3
            }
        }
    }

    if (-not $dnsWorking) {
        Write-Warn "DNS resolution still failing after $maxRetries attempts"
        Write-Info "This may cause issues with Chocolatey/Rust installation"
        Write-Info "If installation fails, check NAT status on host: Get-NetNat"
    }

    # Optional ICMP ping
    $pingTest = Test-Connection -ComputerName $Gateway -Count 1 -Quiet -ErrorAction SilentlyContinue
    if ($pingTest) {
        Write-Success "Gateway responds to ping: $Gateway"
    } else {
        Write-Info "Gateway ping failed (normal)"
        Write-Info "TCP/UDP connectivity works even if ping fails"
    }
} else {
    Write-Warn "No active network adapter found"
}

# ===================== SECTION 3: Privacy & Telemetry ===========
Write-Info "[3/10] Privacy & telemetry configuration..."
Set-ItemProperty -Path "HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection" -Name "AllowTelemetry" -Value 0 -Force -ErrorAction SilentlyContinue
Write-Success "Windows telemetry disabled"
$cortanaKey = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Windows Search"
if (-not (Test-Path $cortanaKey)) { New-Item -Path $cortanaKey -Force | Out-Null }
Set-ItemProperty -Path $cortanaKey -Name "AllowCortana" -Value 0 -Force
Write-Success "Cortana disabled"

# ===================== SECTION 4: Defender Baseline ========================
Write-Info "[4/10] Windows Defender configuration (keep enabled for baseline)..."
Write-Success "Windows Defender remains ENABLED"
Write-Info "To disable for specific experiments: Set-MpPreference -DisableRealtimeMonitoring `$true"

# ===================== SECTION 5: Dev Tools & Runtimes =====================
Write-Info "[5/10] Installing development tools & runtimes..."

# 5a. Chocolatey
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Write-Info "Installing Chocolatey..."
    Set-ExecutionPolicy Bypass -Scope Process -Force
    [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12
    $ok=$false; for($i=1;$i -le 5 -and -not $ok;$i++){
        try { iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1')); $ok=$true }
        catch { Write-Warn "Chocolatey attempt $i failed: $($_.Exception.Message)"; Start-Sleep 5 }
    }
    if ($ok) { Write-Success "Chocolatey installed" } else { Write-Warn "Chocolatey install failed; continuing" }
} else { Write-Success "Chocolatey already installed" }

# 5b. Install Rust
if (-not (Test-Path "$env:USERPROFILE\.cargo\bin\rustc.exe")) {
    Write-Info "Installing Rust toolchain..."
    $rustExe = "$env:TEMP\rustup-init.exe"

    # Retry download
    $rustDownloaded = $false
    $maxRustRetries = 5
    $rustRetryCount = 0

    while (-not $rustDownloaded -and $rustRetryCount -lt $maxRustRetries) {
        $rustRetryCount++
        try {
            Write-Info "Downloading Rust installer (attempt $rustRetryCount/$maxRustRetries)..."
            Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustExe -UseBasicParsing -ErrorAction Stop
            $rustDownloaded = $true
        } catch {
            Write-Warn "Rust download failed: $($_.Exception.Message)"
            if ($rustRetryCount -lt $maxRustRetries) {
                Write-Info "Retrying in 5 seconds..."
                Start-Sleep -Seconds 5
            } else {
                Write-Warn "Rust installer download failed after $maxRustRetries attempts"
                Write-Info "You may need to install manually after reboot"
            }
        }
    }

    if ($rustDownloaded) {
        Start-Process -FilePath $rustExe -ArgumentList "-y --default-toolchain stable" -NoNewWindow -Wait
        $env:Path += ";$env:USERPROFILE\.cargo\bin"
        Write-Success "Rust installed"
    }
} else {
    Write-Success "Rust already installed"
}

# 5c. Install protoc
if (-not (Test-Path "C:\protoc\bin\protoc.exe")) {
    Write-Info "Installing protoc 25.1..."
    $zip = "$env:TEMP\protoc.zip"

    # Retry download for protoc
    $protocDownloaded = $false
    $maxProtocRetries = 5
    $protocRetryCount = 0

    while (-not $protocDownloaded -and $protocRetryCount -lt $maxProtocRetries) {
        $protocRetryCount++
        try {
            Write-Info "Downloading protoc (attempt $protocRetryCount/$maxProtocRetries)..."
            Invoke-WebRequest -Uri "https://github.com/protocolbuffers/protobuf/releases/download/v25.1/protoc-25.1-win64.zip" -OutFile $zip -UseBasicParsing -ErrorAction Stop
            $protocDownloaded = $true
        } catch {
            Write-Warn "protoc download failed: $($_.Exception.Message)"
            if ($protocRetryCount -lt $maxProtocRetries) {
                Write-Info "Retrying in 5 seconds..."
                Start-Sleep -Seconds 5
            } else {
                Write-Warn "protoc download failed after $maxProtocRetries attempts"
                Write-Info "You may need to install manually after reboot"
            }
        }
    }

    if ($protocDownloaded) {
        Expand-Archive -Path $zip -DestinationPath "C:\protoc" -Force
        $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
        if ($machinePath -notlike "*C:\protoc\bin*") {
            [Environment]::SetEnvironmentVariable("Path", "$machinePath;C:\protoc\bin", "Machine")
        }
        Remove-Item $zip -Force -ErrorAction SilentlyContinue
        Write-Success "protoc installed"
    }
} else {
    Write-Success "protoc already installed"
}

# 5d. VC++ Runtime
try {
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) { Write-Warn "Skipping vcredist install (choco missing)" }
    else {
        choco install -y vcredist140 --no-progress -r -y -failonstderr -force | Out-Null
        choco install -y vcredist2015-2022 --no-progress -r -y -failonstderr -force | Out-Null
        Write-Success "VC++ runtime installed (vcredist)"
    }
} catch { Write-Warn "VC++ runtime install failed: $($_.Exception.Message)" }

# 5e. Visual Studio 2022
$vsInstallPath = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools"
$vsInstallerPath = "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"

if (-not (Test-Path $vsInstallerPath) -and -not (Test-Path $vsInstallPath)) {
    Write-Info "Installing Visual Studio 2022 Build Tools (this may take 15-30 minutes)..."
    Write-Info "Components: C++ build tools, Windows SDK, MSBuild, CMake"

    # Download VS Build Tools bootstrapper
    $vsBootstrapper = "$env:TEMP\vs_buildtools.exe"
    $vsUrl = "https://aka.ms/vs/17/release/vs_buildtools.exe"

    # Retry download
    $vsDownloaded = $false
    $maxVsRetries = 5
    $vsRetryCount = 0

    while (-not $vsDownloaded -and $vsRetryCount -lt $maxVsRetries) {
        $vsRetryCount++
        try {
            Write-Info "Downloading VS Build Tools bootstrapper (attempt $vsRetryCount/$maxVsRetries)..."
            Invoke-WebRequest -Uri $vsUrl -OutFile $vsBootstrapper -UseBasicParsing -ErrorAction Stop
            $vsDownloaded = $true
        } catch {
            Write-Warn "VS Build Tools download failed: $($_.Exception.Message)"
            if ($vsRetryCount -lt $maxVsRetries) {
                Write-Info "Retrying in 10 seconds..."
                Start-Sleep -Seconds 10
            } else {
                Write-Warn "VS Build Tools download failed after $maxVsRetries attempts"
                Write-Info "You may need to install manually after reboot"
            }
        }
    }

    if ($vsDownloaded) {
        # memo
        # --quiet: No UI, only show progress
        # --wait: Wait for installer to complete
        # --norestart: Don't restart automatically
        # --nocache: Don't cache packages (saves disk space)
        # --installPath: Installation directory
        # --add: Components to install

        $vsArgs = @(
            "--quiet",
            "--wait",
            "--norestart",
            "--nocache",
            "--installPath", "`"$vsInstallPath`"",
            "--add", "Microsoft.VisualStudio.Workload.VCTools",
            "--add", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "--add", "Microsoft.VisualStudio.Component.Windows11SDK.22621",
            "--add", "Microsoft.VisualStudio.Component.VC.CMake.Project",
            "--includeRecommended"
        )

        Write-Info "Starting VS Build Tools installation..."
        Write-Info "Progress will be displayed below (this is a large download ~2-4 GB)..."
        Write-Host ""

        # Start installation with progress monitoring
        $vsProcess = Start-Process -FilePath $vsBootstrapper -ArgumentList $vsArgs -NoNewWindow -PassThru

        # Monitor progress by checking installer logs
        $logPath = "$env:TEMP\dd_bootstrapper_*.log"
        $lastProgress = ""
        $progressChars = @('|', '/', '-', '\')
        $progressIndex = 0

        while (-not $vsProcess.HasExited) {
            Start-Sleep -Seconds 5

            # Simple spinner to show activity
            $progressIndex = ($progressIndex + 1) % 4
            $spinner = $progressChars[$progressIndex]
            Write-Host "`r  [$spinner] Installing Visual Studio 2022 Build Tools... " -NoNewline -ForegroundColor Cyan
        }

        Write-Host ""

        # Check exit code
        $exitCode = $vsProcess.ExitCode
        Write-Info "VS installer exited with code: $exitCode"

        if ($exitCode -eq 0 -or $exitCode -eq 3010) {
            # 0 = success, 3010 = success but reboot required
            Write-Success "Visual Studio 2022 Build Tools installed successfully"
            if ($exitCode -eq 3010) {
                Write-Info "Reboot required to complete VS installation"
            }

            # Add VS tools to PATH
            $vsMSBuildPath = "$vsInstallPath\MSBuild\Current\Bin"
            $vsVCPath = "$vsInstallPath\VC\Tools\MSVC"

            if (Test-Path $vsMSBuildPath) {
                $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
                if ($machinePath -notlike "*$vsMSBuildPath*") {
                    [Environment]::SetEnvironmentVariable("Path", "$machinePath;$vsMSBuildPath", "Machine")
                    Write-Success "MSBuild added to PATH"
                }
            }

            # Cleanup bootstrapper
            Remove-Item $vsBootstrapper -Force -ErrorAction SilentlyContinue

        } elseif ($exitCode -eq 5007) {
            Write-Warn "VS Build Tools installation blocked by pending reboot"
            Write-Info "Please reboot and run this script again"
        } elseif ($exitCode -eq 1602) {
            Write-Warn "VS Build Tools installation cancelled by user (exit code: $exitCode)"
        } elseif ($exitCode -eq 1618) {
            Write-Warn "Another installation is already in progress (exit code: $exitCode)"
            Write-Info "Wait for other installations to complete, then retry"
        } elseif ($exitCode -eq -2147205120 -or $exitCode -eq 2147762176) {
            # 0x80070422 = Windows Update/BITS service disabled
            Write-Warn "VS Build Tools requires Windows Update service (exit code: $exitCode)"
            Write-Info "Enable Windows Update service and retry"
        } else {
            Write-Warn "VS Build Tools installation failed with exit code: $exitCode"
            Write-Info "Check logs at: $env:TEMP\dd_*.log"
            Write-Info "Common exit codes:"
            Write-Info "  -2147205120 (0x80070422): Windows Update service disabled"
            Write-Info "  1602: User cancelled"
            Write-Info "  1618: Another installation in progress"
            Write-Info "  5007: Pending reboot required"
            Write-Info ""
            Write-Info "Manual installation: https://aka.ms/vs/17/release/vs_buildtools.exe"
        }
    }
} else {
    Write-Success "Visual Studio 2022 Build Tools already installed"
}


# ===================== SECTION 6: AutoMutate Dirs =============
Write-Info "[6/10] Creating AutoMutate project directories..."

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


# ===================== SECTION 7: RedEdr ==============
Write-Info "[7/10] RedEdr setup (extract from local zip)..."

# RedEdr zip should be pre-staged in the project's telemetry folder
# This script expects it to be available at C:\AutoMutate\build\telemetry\RedEdr.zip
$RedEdrSourceZip = "C:\AutoMutate\build\telemetry\RedEdr.zip"
$RedEdrZip = "$env:TEMP\RedEdr.zip"
$RedEdrRoot = "C:\RedEdr"   # only this path is supported

try {
    # Check if source zip exists (should be copied by initialize-worker.ps1)
    if (-not (Test-Path $RedEdrSourceZip)) {
        throw "RedEdr.zip not found at: $RedEdrSourceZip. Ensure initialize-worker.ps1 copied the build package."
    }

    Write-Info "Found RedEdr.zip in build package"
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
    Write-Info "  - Ensure RedEdr.zip exists in project root: <project>/telemetry/RedEdr.zip"
    Write-Info "  - Verify initialize-worker.ps1 copied the full build package"
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

# Trust RedEdr driver certificate
Write-Info "Extracting and trusting RedEdr driver certificate..."
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

# Open firewall for web UI
try {
    $firewallRuleName = "RedEdr Web $RedEDRPort"
    if (-not (Get-NetFirewallRule -DisplayName $firewallRuleName -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName $firewallRuleName -Direction Inbound -Action Allow -Protocol TCP -LocalPort $RedEDRPort | Out-Null
    }
    Write-Success "Firewall rule OK for TCP $RedEDRPort"
} catch { Write-Warn "Firewall rule failed: $($_.Exception.Message)" }

# Enable SMB/File Sharing from Controller
Write-Info "Enabling SMB access from controller ($StaticIP)..."
try {
    # Enable File and Printer Sharing predefined rules
    Enable-NetFirewallRule -DisplayGroup "File and Printer Sharing" -ErrorAction SilentlyContinue

    # Create specific rule for controller IP (SMB port 445)
    $smbRuleName = "SMB from Controller"
    if (-not (Get-NetFirewallRule -DisplayName $smbRuleName -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName $smbRuleName `
            -Direction Inbound `
            -Protocol TCP `
            -LocalPort 445 `
            -RemoteAddress $StaticIP `
            -Action Allow `
            -Profile Any `
            -ErrorAction Stop | Out-Null
    }

    # Ensure SMB server is running
    $smbService = Get-Service -Name LanmanServer -ErrorAction SilentlyContinue
    if ($smbService.Status -ne "Running") {
        Start-Service -Name LanmanServer -ErrorAction Stop
    }

    Write-Success "SMB enabled from controller ($StaticIP) on port 445"
} catch { Write-Warn "SMB firewall rule failed: $($_.Exception.Message)" }

# ===================== SECTION 8: Boot Config for Test-Signed Drivers ======
Write-Info "[8/10] Kernel driver allowances (testsigning, debug)..."
# Required for RedEdr kernel callbacks / KAPC injection / ETW-TI PPL via ELAM
try {
    bcdedit /enum | Out-Null
    & bcdedit /set testsigning on   | Out-Null
    & bcdedit -debug on             | Out-Null
    Write-Success "Enabled test-signed drivers and kernel debug"
    Write-Info "If running on Hyper-V, disable Secure Boot on the VM (host-side setting)."
} catch { Write-Warn "BCDEdit changes failed: $($_.Exception.Message)" }

# Option:disable HVCI/Memory Integrity:often blocks test-signed drivers
try {
    $dg = "HKLM:\SYSTEM\CurrentControlSet\Control\DeviceGuard"
    if (-not (Test-Path $dg)) { New-Item $dg -Force | Out-Null }
    New-ItemProperty -Path $dg -Name "EnableVirtualizationBasedSecurity" -PropertyType DWord -Value 0 -Force | Out-Null
    $ci = "HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy"
    if (-not (Test-Path $ci)) { New-Item $ci -Force | Out-Null }
    New-ItemProperty -Path $ci -Name "VerifiedAndReputablePolicyState" -PropertyType DWord -Value 0 -Force | Out-Null
    Write-Info "Disabled VBS/HVCI policy (effective after reboot) if it was on."
} catch { Write-Warn "Could not adjust DeviceGuard/HVCI: $($_.Exception.Message)" }

# ===================== SECTION 9: Telemetry: Audit Policy for ETW ==========
Write-Info "[9/10] Enabling audit policies for Security-Auditing ETW (MAXIMUM TELEMETRY)..."
# Some Microsoft-Windows-Security-Auditing events require audit categories enabled and SYSTEM token
# ( PsExec -i -s cmd.exe)

# IMPORTANT: We need to configure BOTH local audit policy AND Group Policy settings
# Group Policy overrides local settings, so we must write to the registry locations
# that Group Policy uses for Advanced Audit Policy Configuration

# Step 1: Force use of Advanced Audit Policy (disable legacy audit policy)
$auditPolicyKey = "HKLM:\SYSTEM\CurrentControlSet\Control\Lsa"
if (-not (Test-Path $auditPolicyKey)) {
    New-Item -Path $auditPolicyKey -Force | Out-Null
}
# SCENoApplyLegacyAuditPolicy = 1 means "use Advanced Audit Policy, ignore legacy"
Set-ItemProperty -Path $auditPolicyKey -Name "SCENoApplyLegacyAuditPolicy" -Value 1 -Type DWord -Force
Write-Success "Enabled Advanced Audit Policy mode (disabled legacy audit policy)"

# Step 2: Configure Advanced Audit Policy via registry
# The Group Policy settings for Advanced Audit Policy are stored in:
# HKLM:\SECURITY\Policy\PolAdtEv (binary format, requires special handling)
#
# Since direct registry modification of SECURITY hive is complex, we'll use
# a combination approach: auditpol + Local Group Policy refresh

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

# Step 3: Backup current audit policy to a file and force it into Local Group Policy
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

# Step 4: Force Group Policy refresh to apply changes
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

# Enable PowerShell script block logging (malware often uses PS)
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

# Enable PowerShell transcription (optional, creates large logs)
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

# ===================== SECTION 10: Drivers/Services from Release ===========
Write-Info "[10/10] Installing RedEdr drivers & services (ETW, Kernel, ETW-TI/PPL)..."

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

# Copy SYSTEM launcher helper script to RedEDR directory
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

# ===================== VERIFICATION & SUMMARY ==========
Write-Info "[Verification] Summaries..."

$verificationResults = @()
# Check network
$routeCheck = Get-NetRoute -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue | Where-Object { $_.NextHop -eq $Gateway }
$networkStatus = if ($routeCheck) { "OK" } else { "FAIL" }
$networkDetails = "IP: $StaticIP, GW: $Gateway"

# Try DNS as additional verification
$dnsWorks = $false
try {
    $dnsResult = Resolve-DnsName -Name "google.com" -Type A -DnsOnly -ErrorAction Stop 2>$null
    if ($dnsResult) {
        $dnsWorks = $true
        $networkDetails += " (Internet: OK)"
    }
} catch {
    $networkDetails += " (Internet: Pending)"
}

$verificationResults += [PSCustomObject]@{
    Component = "Network"
    Status = $networkStatus
    Details = $networkDetails
}

# Check Rust
$rustVersion = if (Test-Path "$env:USERPROFILE\.cargo\bin\rustc.exe") {
    (& "$env:USERPROFILE\.cargo\bin\rustc.exe" --version) -replace 'rustc ', ''
} else { "NOT FOUND" }
$verificationResults += [PSCustomObject]@{
    Component = "Rust"
    Status = if ($rustVersion -ne "NOT FOUND") { "OK" } else { "FAIL" }
    Details = $rustVersion
}

# Check protoc
$protocVersion = if (Test-Path "C:\protoc\bin\protoc.exe") {
    (& "C:\protoc\bin\protoc.exe" --version) -replace 'libprotoc ', ''
} else { "NOT FOUND" }
$verificationResults += [PSCustomObject]@{
    Component = "protoc"
    Status = if ($protocVersion -ne "NOT FOUND") { "OK" } else { "FAIL" }
    Details = $protocVersion
}

# Check UAC
$uacStatus = (Get-ItemProperty -Path "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System" -Name "EnableLUA").EnableLUA
$verificationResults += [PSCustomObject]@{
    Component = "UAC"
    Status = if ($uacStatus -eq 0) { "OK" } else { "WARN" }
    Details = if ($uacStatus -eq 0) { "Disabled (lab)" } else { "Enabled" }
}

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

# Firewall port
try {
    $firewallRuleName = "RedEdr Web $RedEDRPort"
    $fw = Get-NetFirewallRule -DisplayName $firewallRuleName -ErrorAction SilentlyContinue
    $verificationResults += [PSCustomObject]@{
        Component = "Firewall"
        Status    = if ($fw) { "OK" } else { "WARN" }
        Details   = "TCP $RedEDRPort inbound for web UI"
    }
} catch {
    $verificationResults += [PSCustomObject]@{ Component="Firewall"; Status="WARN"; Details="Check rule for TCP $RedEDRPort" }
}

# SMB access verification
try {
    $smbRuleName = "SMB from Controller"
    $smbRule = Get-NetFirewallRule -DisplayName $smbRuleName -ErrorAction SilentlyContinue
    $smbService = Get-Service -Name LanmanServer -ErrorAction SilentlyContinue
    $smbRunning = $smbService.Status -eq "Running"

    $verificationResults += [PSCustomObject]@{
        Component = "SMB"
        Status    = if ($smbRule -and $smbRunning) { "OK" } else { "WARN" }
        Details   = "Port 445 from controller, service $(if ($smbRunning) { 'running' } else { 'stopped' })"
    }
} catch {
    $verificationResults += [PSCustomObject]@{ Component="SMB"; Status="WARN"; Details="Check SMB service and firewall" }
}

# Re-enable Defender real-time monitoring
Write-Info "Re-enabling Windows Defender real-time monitoring..."
try {
    Set-MpPreference -DisableRealtimeMonitoring $false -ErrorAction SilentlyContinue
    Write-Success "Defender real-time monitoring re-enabled"
    Write-Info "Note: Path/process exclusions remain in place for C:\AutoMutate and C:\RedEdr"
} catch {
    Write-Warn "Could not re-enable Defender: $($_.Exception.Message)"
}

# Print verification table
Write-Host "`n+================================================================+" -ForegroundColor Green
Write-Host "|          Initialization Complete - Verification                |" -ForegroundColor Green
Write-Host "+================================================================+" -ForegroundColor Green
foreach ($result in $verificationResults) {
    $statusColor = switch ($result.Status) { "OK" { "Green" } "WARN" { "Yellow" } "FAIL" { "Red" } }
    $line = "| [$($result.Status.PadRight(4))] $($result.Component.PadRight(15)) $($result.Details)"
    Write-Host $line.PadRight(66) + "|" -ForegroundColor $statusColor
}
Write-Host "+================================================================+" -ForegroundColor Green

# Reboot prompt (extended notes)
Write-Host "`n+================================================================+" -ForegroundColor Yellow
Write-Host "|          REBOOT REQUIRED                                       |" -ForegroundColor Yellow
Write-Host "+================================================================+" -ForegroundColor Yellow
Write-Host "|  Changes requiring reboot:                                     |"
Write-Host "|    * Computer name change                                      |"
Write-Host "|    * UAC disabled                                              |"
Write-Host "|    * Environment variables (PATH)                              |"
Write-Host "|    * Test-signed driver support & kernel debug (bcdedit)       |"
Write-Host "|                                                                |"
Write-Host "|  IMPORTANT for Hyper-V: Disable Secure Boot on the host VM.    |"
Write-Host "|                                                                |"
Write-Host "|  After reboot, you can:                                        |"
Write-Host "|                                                                |"
Write-Host "|  1. Start RedEdr telemetry collector (as SYSTEM):             |"
Write-Host "|     Right-click desktop shortcut 'RedEDR (SYSTEM).lnk'        |"
Write-Host "|     -> Run as Administrator                                    |"
Write-Host "|                                                                |"
Write-Host "|     OR manually:                                               |"
Write-Host "|     cd C:\RedEdr                                               |"
Write-Host "|     .\Start-RedEDR-SYSTEM.ps1                                  |"
Write-Host "|                                                                |"
Write-Host "|     Then open http://localhost:$RedEDRPort".PadRight(64) "|"
Write-Host "|                                                                |"
Write-Host "|  2. Worker Agent will be deployed separately by controller     |"
Write-Host "+================================================================+" -ForegroundColor Yellow

if (-not $SkipReboot) {
    Write-Host "`nRebooting in 10 seconds (Ctrl+C to cancel)..." -ForegroundColor Yellow
    Start-Sleep -Seconds 10
    Restart-Computer -Force
} else {
    Write-Info "Reboot skipped (-SkipReboot). Manual reboot strongly recommended."
}