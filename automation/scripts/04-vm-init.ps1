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


# ===================== SECTION 7: RedEdr with PPLRunner ==============
Write-Info "[7/10] RedEdr with PPLRunner setup (extract from local zip)..."

# RedEdrDeploy.zip should be pre-staged in the project's telemetry folder
# This script expects it to be available at C:\AutoMutate\build\telemetry\RedEdrDeploy.zip
$RedEdrSourceZip = "C:\AutoMutate\build\telemetry\RedEdrDeploy.zip"
$RedEdrZip = "$env:TEMP\RedEdrDeploy.zip"
$RedEdrRoot = "C:\RedEdr"   # only this path is supported

try {
    # Check if source zip exists (should be copied by initialize-worker.ps1)
    if (-not (Test-Path $RedEdrSourceZip)) {
        throw "RedEdrDeploy.zip not found at: $RedEdrSourceZip. Ensure initialize-worker.ps1 copied the build package."
    }

    Write-Info "Found RedEdrDeploy.zip in build package"
    $sourceSize = (Get-Item $RedEdrSourceZip).Length
    Write-Info "Source file size: $([math]::Round($sourceSize/1MB, 2)) MB"

    # Copy to temp location for extraction
    if (Test-Path $RedEdrZip) {
        Write-Info "Removing existing temp copy..."
        Remove-Item $RedEdrZip -Force
    }

    Write-Info "Copying RedEdrDeploy.zip to temp location..."
    Copy-Item $RedEdrSourceZip $RedEdrZip -Force

    # Verify copied ZIP is valid
    $fileSize = (Get-Item $RedEdrZip).Length
    if ($fileSize -lt 100KB) {
        throw "RedEdrDeploy.zip is too small ($fileSize bytes), file may be corrupted."
    }

    # Verify ZIP signature (PK\x03\x04)
    $zipHeader = [System.IO.File]::ReadAllBytes($RedEdrZip)[0..3]
    if (-not ($zipHeader[0] -eq 0x50 -and $zipHeader[1] -eq 0x4B)) {
        throw "RedEdrDeploy.zip is not a valid ZIP archive (missing PK signature)."
    }

    Write-Success "RedEdrDeploy.zip validated ($([math]::Round($fileSize/1MB, 2)) MB)"

    # Prepare installation directory
    if (Test-Path $RedEdrRoot) {
        Write-Info "Clearing existing $RedEdrRoot"
        Remove-Item $RedEdrRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    New-Item -ItemType Directory -Path $RedEdrRoot -Force | Out-Null

    # Extract RedEdrDeploy to temporary location first
    $tempExtractPath = "$env:TEMP\RedEdr_extract"
    if (Test-Path $tempExtractPath) {
        Remove-Item $tempExtractPath -Recurse -Force
    }
    New-Item -ItemType Directory -Path $tempExtractPath -Force | Out-Null

    Write-Info "Extracting RedEdrDeploy.zip..."
    Expand-Archive -Path $RedEdrZip -DestinationPath $tempExtractPath -Force

    # Check if ZIP contains a nested folder or direct files
    $extractedItems = Get-ChildItem $tempExtractPath
    if ($extractedItems.Count -eq 1 -and $extractedItems[0].PSIsContainer) {
        # ZIP contains a single folder (e.g., RedEdrDeploy/), move its contents
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

    # Verify critical files exist
    $criticalFiles = @{
        "RedEdr.exe" = "Main RedEDR executable"
        "ppl_runner.exe" = "PPLRunner service installer"
        "elam_driver.sys" = "ELAM driver for PPL protection"
        "RedEdrDriver.sys" = "Kernel driver"
        "RedEdrDll.dll" = "Injection DLL"
        "RedEdrPplService.exe" = "PPL service for ETW-TI"
    }

    $missingFiles = @()
    foreach ($file in $criticalFiles.Keys) {
        $filePath = Join-Path $RedEdrRoot $file
        if (Test-Path $filePath) {
            Write-Success "$file found: $($criticalFiles[$file])"
        } else {
            $missingFiles += $file
            Write-Warn "$file MISSING: $($criticalFiles[$file])"
        }
    }

    if ($missingFiles.Count -gt 0) {
        Write-Warn "Some critical files are missing. Listing extracted contents:"
        Get-ChildItem $RedEdrRoot | ForEach-Object { Write-Info "  $($_.Name)" }
    }

    # Install PPLRunner service
    $pplRunnerExe = Join-Path $RedEdrRoot "ppl_runner.exe"
    if (Test-Path $pplRunnerExe) {
        Write-Info "Installing PPLRunner service..."
        Push-Location $RedEdrRoot
        try {
            & .\ppl_runner.exe install 2>&1 | Out-Host
            if ($LASTEXITCODE -eq 0) {
                Write-Success "PPLRunner service installed successfully"

                # Configure registry for maximum telemetry
                Write-Info "Configuring PPLRunner registry for maximum telemetry..."
                $commandLine = "C:\RedEdr\RedEdr.exe - e -g -k --web"
                REG.exe ADD "HKLM\SOFTWARE\PPL_RUNNER" /ve /t REG_SZ /d $commandLine /f | Out-Null
                Write-Success "Registry configured: $commandLine"

                # Verify service exists
                $svcCheck = sc.exe query ppl_runner 2>&1
                if ($LASTEXITCODE -eq 0) {
                    Write-Success "PPLRunner service verified (currently stopped)"
                } else {
                    Write-Warn "PPLRunner service not found after installation"
                }
            } else {
                Write-Warn "PPLRunner installation may have failed (exit code: $LASTEXITCODE)"
                Write-Info "Check if test signing is enabled (will be done in Section 8)"
            }
        } catch {
            Write-Warn "PPLRunner installation error: $($_.Exception.Message)"
            Write-Info "This is normal if test signing is not yet enabled"
        } finally {
            Pop-Location
        }
    } else {
        Write-Warn "ppl_runner.exe not found, cannot install service"
    }

} catch {
    Write-Err "Failed to install RedEdr: $($_.Exception.Message)"
    Write-Info ""
    Write-Info "Troubleshooting:"
    Write-Info "  - Ensure RedEdrDeploy.zip exists in project root: <project>/telemetry/RedEdrDeploy.zip"
    Write-Info "  - Verify initialize-worker.ps1 copied the full build package"
    Write-Info "  - Check source location: $RedEdrSourceZip"
    Write-Info ""
    Write-Info "Manual installation:"
    Write-Info "  1. Place RedEdrDeploy.zip at: $RedEdrSourceZip"
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
Write-Info "[9/10] Enabling audit policies for Security-Auditing ETW..."
# Some Microsoft-Windows-Security-Auditing events require audit categories enabled and SYSTEM token
# ( PsExec -i -s cmd.exe)
$cats = @(
    "Logon","Policy Change","Account Logon","Account Management","Privilege Use",
    "System","DS Access","Object Access","Detailed Tracking"
)
foreach($c in $cats){
    try { & auditpol /set /category:$c /success:enable /failure:enable | Out-Null } catch {}
}
Write-Success "Audit policy updated (success+failure) for common categories"
Write-Info "For Security-Auditing ETW, start RedEdr as SYSTEM when needed."

# ===================== SECTION 10: Build Worker Agent ======================
Write-Info "[10/11] Building Worker Agent..."

$buildDir = "C:\AutoMutate\build"
$workerAgentDir = Join-Path $buildDir "worker\agent"

if (Test-Path $workerAgentDir) {
    Write-Info "Build sources found at: $buildDir"

    try {
        # Update PATH to include Rust and protoc
        $env:Path = "$env:USERPROFILE\.cargo\bin;C:\protoc\bin;$env:Path"

        # Verify build tools are available
        $rustcPath = "$env:USERPROFILE\.cargo\bin\rustc.exe"
        $protocPath = "C:\protoc\bin\protoc.exe"

        if (-not (Test-Path $rustcPath)) {
            Write-Warn "Rust not found at $rustcPath - build will be skipped"
            Write-Info "Reboot may be required for PATH changes to take effect"
        } elseif (-not (Test-Path $protocPath)) {
            Write-Warn "protoc not found at $protocPath - build will be skipped"
        } else {
            Write-Info "Building worker agent (this may take 5-10 minutes on first build)..."

            # Change to build directory
            Push-Location $buildDir

            # Build worker agent (release mode)
            Write-Host "  Running: cargo build --release -p worker-agent" -ForegroundColor Gray
            $buildOutput = cargo build --release -p worker-agent 2>&1
            $buildExitCode = $LASTEXITCODE

            if ($buildExitCode -eq 0) {
                # Find the built binary
                $agentBinary = Join-Path $buildDir "target\release\worker-agent.exe"

                if (Test-Path $agentBinary) {
                    # Copy to main AutoMutate directory
                    $agentDest = "C:\AutoMutate\worker-agent.exe"
                    Copy-Item $agentBinary $agentDest -Force

                    Write-Success "Worker agent built successfully: $agentDest"
                    Write-Info "Binary size: $((Get-Item $agentDest).Length / 1MB | ForEach-Object { '{0:N2}' -f $_ }) MB"
                } else {
                    Write-Warn "Build succeeded but binary not found at: $agentBinary"
                    Write-Info "Check: $buildDir\target\release\"
                }
            } else {
                Write-Warn "Worker agent build failed (exit code: $buildExitCode)"
                Write-Info "Build output (last 20 lines):"
                $buildOutput | Select-Object -Last 20 | ForEach-Object { Write-Host "  $_" -ForegroundColor Gray }
                Write-Info "You can retry the build manually after reboot:"
                Write-Info "  cd $buildDir"
                Write-Info "  cargo build --release -p worker-agent"
            }

            Pop-Location
        }
    } catch {
        Write-Warn "Worker agent build error: $($_.Exception.Message)"
        Write-Info "You can build manually after reboot:"
        Write-Info "  cd $buildDir"
        Write-Info "  cargo build --release -p worker-agent"
    }
} else {
    Write-Info "Build sources not found at: $buildDir"
    Write-Info "This is normal if running 04-vm-init.ps1 directly without initialize-worker.ps1"
    Write-Info "To build worker agent later, copy source files to VM and run:"
    Write-Info "  cd C:\AutoMutate\build"
    Write-Info "  cargo build --release -p worker-agent"
}

# ===================== SECTION 11: PPLRunner Service Configuration =========
Write-Info "[11/11] PPLRunner service configuration summary..."

# Note: PPLRunner service was already installed in Section 7
# The service is configured to run: C:\RedEdr\RedEdr.exe - e -g -k --web

try {
    $svcCheck = sc.exe query ppl_runner 2>&1
    if ($LASTEXITCODE -eq 0) {
        Write-Success "PPLRunner service configured and ready"
        Write-Info "  Service name: ppl_runner"
        Write-Info "  Command: C:\RedEdr\RedEdr.exe - e -g -k --web"
        Write-Info "  Registry: HKLM\SOFTWARE\PPL_RUNNER"
        Write-Info ""
        Write-Info "Service will run RedEDR as PPL-protected SYSTEM process with:"
        Write-Info "  - ETW events (Microsoft-Windows-Security-Auditing, etc.)"
        Write-Info "  - ETW-TI events (Microsoft-Windows-Threat-Intelligence)"
        Write-Info "  - Kernel callbacks (process, thread, image load)"
        Write-Info "  - DLL injection (ntdll hooking via KAPC)"
        Write-Info "  - Web UI on port $RedEDRPort"
    } else {
        Write-Warn "PPLRunner service not found"
        Write-Info "This is expected if test signing is not yet enabled"
        Write-Info "After reboot with test signing enabled, run:"
        Write-Info "  cd C:\RedEdr"
        Write-Info "  .\ppl_runner.exe install"
    }
} catch {
    Write-Warn "Could not verify PPLRunner service: $($_.Exception.Message)"
}

if (-not $DisableEtwTi) {
    Write-Info "ETW-TI will be available after reboot via PPLRunner/RedEdrPplService.exe"
} else {
    Write-Info "ETW-TI setup skipped by request (-DisableEtwTi)."
}

# Copy PPLRunner launcher helper script to RedEDR directory
$helperScriptContent = @'
<#
.SYNOPSIS
    Start/Stop RedEDR as PPL-protected SYSTEM process using PPLRunner.

.DESCRIPTION
    This script manages RedEDR execution through PPLRunner, which provides:
      - SYSTEM privileges (access to all ETW providers)
      - PPL (Protected Process Light) anti-tampering protection
      - Maximum telemetry collection capability

    RedEDR is pre-configured to run with: - e -g -k --web
      - e -g -k  : Enable all telemetry (ETW, ETW-TI, Kernel callbacks, DLL injection)
      --web  : Start web UI on http://localhost:8081

.PARAMETER Stop
    Stop RedEDR (requires reboot or PPLRunner removal command).

.PARAMETER ConfigureOnly
    Only update the registry configuration without starting the service.

.PARAMETER Command
    Custom command line to execute (default: "C:\RedEdr\RedEdr.exe - e -g -k --web").

.EXAMPLE
    .\Start-RedEDR-PPL.ps1
    Starts RedEDR with default configuration (all telemetry + web UI).

.EXAMPLE
    .\Start-RedEDR-PPL.ps1 -Command "C:\RedEdr\RedEdr.exe --etw --etwti --web"
    Starts RedEDR with only ETW and ETW-TI (no kernel hooks or DLL injection).

.EXAMPLE
    .\Start-RedEDR-PPL.ps1 -Stop
    Shows instructions to stop PPL-protected RedEDR.

.NOTES
    PPL Protection: RedEDR runs as a Protected Process Light and cannot be killed
    by normal tools (Task Manager, Stop-Process, etc.). To stop it, you must either:
      1. Reboot the system
      2. Use PPLRunner removal command (see -Stop for instructions)
#>
[CmdletBinding()]
param(
    [switch]$Stop,
    [switch]$ConfigureOnly,
    [string]$Command = "C:\RedEdr\RedEdr.exe - e -g -k --web"
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

$RedEdrRoot = "C:\RedEdr"
$RedEdrExe = "$RedEdrRoot\RedEdr.exe"
$PPLRunnerExe = "$RedEdrRoot\ppl_runner.exe"

# Verify installation
Write-Info "Verifying RedEDR installation at: $RedEdrRoot"
if (-not (Test-Path $RedEdrExe)) {
    Write-Err "RedEdr.exe not found at $RedEdrExe"
    Write-Info "Run 04-vm-init.ps1 to install RedEDR first"
    exit 1
}

if (-not (Test-Path $PPLRunnerExe)) {
    Write-Err "ppl_runner.exe not found at $PPLRunnerExe"
    Write-Info "Ensure RedEdrDeploy.zip was properly extracted"
    exit 1
}

# --- Stop Mode ---
if ($Stop) {
    Write-Host ""
    Write-Host "=== Stopping PPL-Protected RedEDR ===" -ForegroundColor Yellow
    Write-Host ""

    $rededrProc = Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue
    if (-not $rededrProc) {
        Write-Info "RedEDR is not currently running"
        exit 0
    }

    Write-Info "RedEDR is running as PPL-protected process (PID: $($rededrProc.Id))"
    Write-Warn "Normal termination methods (Task Manager, Stop-Process) will fail"
    Write-Host ""
    Write-Host "Option 1: Reboot (recommended)" -ForegroundColor Cyan
    Write-Host "  shutdown /r /t 0" -ForegroundColor Gray
    Write-Host ""
    Write-Host "Option 2: Use PPLRunner removal command" -ForegroundColor Cyan
    Write-Host "  REG.exe ADD `"HKLM\SOFTWARE\PPL_RUNNER`" /ve /t REG_SZ /d `"$PPLRunnerExe remove`" /f" -ForegroundColor Gray
    Write-Host "  net start ppl_runner" -ForegroundColor Gray
    Write-Host ""
    Write-Info "This will use PPLRunner to remove itself and terminate RedEDR"
    exit 0
}

# --- Verify PPLRunner service exists ---
Write-Info "Checking PPLRunner service status..."
$svcCheck = sc.exe query ppl_runner 2>&1
if ($LASTEXITCODE -ne 0 -and $svcCheck -match "does not exist") {
    Write-Warn "PPLRunner service not installed"
    Write-Info "Installing PPLRunner service..."

    Push-Location $RedEdrRoot
    try {
        & .\ppl_runner.exe install 2>&1 | Out-Host
        if ($LASTEXITCODE -eq 0) {
            Write-Success "PPLRunner service installed"
        } else {
            Write-Err "PPLRunner installation failed (exit code: $LASTEXITCODE)"
            Write-Info "Ensure test signing is enabled: bcdedit /set testsigning on"
            Write-Info "Then reboot and run this script again"
            exit 1
        }
    } finally {
        Pop-Location
    }
} else {
    Write-Success "PPLRunner service found"
}

# --- Configure Registry ---
Write-Info "Configuring PPLRunner registry..."
Write-Host "  Command: $Command" -ForegroundColor Gray

REG.exe ADD "HKLM\SOFTWARE\PPL_RUNNER" /ve /t REG_SZ /d $Command /f | Out-Null
if ($LASTEXITCODE -eq 0) {
    Write-Success "Registry configured"
} else {
    Write-Err "Failed to configure registry"
    exit 1
}

# Verify registry
$regValue = (Get-ItemProperty -Path "HKLM:\SOFTWARE\PPL_RUNNER" -Name "(Default)" -ErrorAction SilentlyContinue).'(Default)'
if ($regValue -eq $Command) {
    Write-Success "Registry verification passed"
} else {
    Write-Warn "Registry value mismatch: Expected '$Command', Got '$regValue'"
}

if ($ConfigureOnly) {
    Write-Info "Configuration complete (-ConfigureOnly specified)"
    Write-Info "Run without -ConfigureOnly to start the service"
    exit 0
}

# --- Start PPLRunner Service ---
Write-Host ""
Write-Host "=== Starting RedEDR as PPL ===(" -ForegroundColor Cyan
Write-Info "Starting PPLRunner service..."
Write-Info "PPLRunner will spawn RedEdr.exe as PPL-protected SYSTEM process"

net start ppl_runner 2>&1 | Out-Host

# Give it time to spawn RedEdr.exe
Start-Sleep -Seconds 3

# --- Verify RedEDR is Running ---
$rededrProc = Get-Process -Name "RedEdr" -ErrorAction SilentlyContinue
$pplServiceProc = Get-Process -Name "RedEdrPplService" -ErrorAction SilentlyContinue

Write-Host ""
if ($rededrProc) {
    Write-Success "RedEDR started successfully"
    Write-Info "  Process: RedEdr.exe"
    Write-Info "  PID: $($rededrProc.Id)"
    Write-Info "  User: NT AUTHORITY\SYSTEM"
    Write-Info "  Protection: PPL (Protected Process Light)"
    Write-Info "  Command: $Command"

    if ($pplServiceProc) {
        Write-Info "  PPL Service: RedEdrPplService.exe (PID: $($pplServiceProc.Id))"
    }

    Write-Host ""
    Write-Success "Web UI should be available at: http://localhost:8081" -ForegroundColor Green
    Write-Host ""
    Write-Info "Telemetry sources enabled: ETW, ETW-TI, Kernel Callbacks, DLL Injection"
    Write-Host ""
    Write-Warn "RedEDR is PPL-protected and cannot be terminated normally"
    Write-Info "To stop: Run this script with -Stop flag"

} else {
    Write-Warn "RedEDR process not detected after service start"
    Write-Host ""
    Write-Info "Troubleshooting steps:"
    Write-Info "  1. Check PPLRunner service: sc query ppl_runner"
    Write-Info "  2. Check Event Viewer: eventvwr.msc → Windows Logs → System"
    Write-Info "  3. Use DebugView to see PPLRunner output: DbgView.exe (Sysinternals)"
    Write-Info "  4. Verify test signing is enabled: bcdedit /enum | findstr testsigning"
    Write-Host ""
    Write-Info "Registry configuration:"
    Write-Info "  Key: HKLM\SOFTWARE\PPL_RUNNER"
    Write-Info "  Value: $Command"
}

Write-Host ""
'@

try {
    $helperScript = Join-Path $RedEdrRoot "Start-RedEDR-PPL.ps1"
    $helperScriptContent | Out-File -FilePath $helperScript -Encoding UTF8 -Force
    Write-Success "Created PPL launcher: $helperScript"
} catch {
    Write-Warn "Could not create PPL launcher: $($_.Exception.Message)"
}

# Desktop shortcut pointing to PPL launcher (requires Admin)
try {
    $WScriptShell = New-Object -ComObject WScript.Shell
    $Shortcut = $WScriptShell.CreateShortcut("$([Environment]::GetFolderPath('Desktop'))\Start RedEDR (PPL).lnk")
    $Shortcut.TargetPath = "powershell.exe"
    $Shortcut.Arguments = "-ExecutionPolicy Bypass -NoProfile -File `"$helperScript`""
    $Shortcut.WorkingDirectory = $RedEdrRoot
    $Shortcut.IconLocation = "$rededrExe,0"
    $Shortcut.Save()
    Write-Success "Desktop shortcut created: Start RedEDR (PPL).lnk"
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
$pplRunnerPresent = (Test-Path (Join-Path $RedEdrRoot "ppl_runner.exe"))
$verificationResults += [PSCustomObject]@{
    Component = "RedEdr"
    Status    = if ($rededrPresent -and $pplRunnerPresent) { "OK" } else { "FAIL" }
    Details   = if ($rededrPresent -and $pplRunnerPresent) { "Deployed with PPLRunner" } elseif ($rededrPresent) { "Missing PPLRunner" } else { "Missing RedEdr.exe" }
}

# PPLRunner service
try {
    $pplSvcCheck = sc.exe query ppl_runner 2>&1
    $pplSvcInstalled = ($LASTEXITCODE -eq 0)
    $verificationResults += [PSCustomObject]@{
        Component = "PPLRunner"
        Status    = if ($pplSvcInstalled) { "OK" } else { "WARN" }
        Details   = if ($pplSvcInstalled) { "Service installed (stopped)" } else { "Service not installed (needs test signing + reboot)" }
    }
} catch {
    $verificationResults += [PSCustomObject]@{ Component="PPLRunner"; Status="WARN"; Details="Could not verify service" }
}

# Worker Agent binary
$workerAgentExe = "C:\AutoMutate\worker-agent.exe"
$workerAgentPresent = (Test-Path $workerAgentExe)
$workerAgentDetails = if ($workerAgentPresent) {
    "Built at C:\AutoMutate ($((Get-Item $workerAgentExe).Length / 1MB | ForEach-Object { '{0:N2}' -f $_ }) MB)"
} else {
    "Not built (build manually after reboot)"
}
$verificationResults += [PSCustomObject]@{
    Component = "WorkerAgent"
    Status    = if ($workerAgentPresent) { "OK" } else { "WARN" }
    Details   = $workerAgentDetails
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
Write-Host "|  1. Start RedEDR via PPLRunner (maximum telemetry):           |"
Write-Host "|     Method A: Use desktop shortcut (right-click -> Run as Admin)|"
Write-Host "|       'Start RedEDR (PPL).lnk' on Desktop                     |"
Write-Host "|                                                                |"
Write-Host "|     Method B: Use PowerShell script                           |"
Write-Host "|       cd C:\RedEdr                                             |"
Write-Host "|       .\Start-RedEDR-PPL.ps1                                   |"
Write-Host "|                                                                |"
Write-Host "|     Method C: Start manually via PPLRunner service            |"
Write-Host "|       net start ppl_runner                                     |"
Write-Host "|                                                                |"
Write-Host "|     Web UI: http://localhost:$RedEDRPort".PadRight(64) "|"
Write-Host "|                                                                |"
Write-Host "|  2. Start Worker Agent (connects to controller):              |"
Write-Host "|     cd C:\AutoMutate                                           |"
Write-Host "|     `$env:WORKER_ID='$WorkerName'                               |"
Write-Host "|     `$env:CONTROLLER_ADDR='10.200.200.1:50051'                  |"
Write-Host "|     .\worker-agent.exe                                         |"
Write-Host "+================================================================+" -ForegroundColor Yellow

if (-not $SkipReboot) {
    Write-Host "`nRebooting in 10 seconds (Ctrl+C to cancel)..." -ForegroundColor Yellow
    Start-Sleep -Seconds 10
    Restart-Computer -Force
} else {
    Write-Info "Reboot skipped (-SkipReboot). Manual reboot strongly recommended."
}
