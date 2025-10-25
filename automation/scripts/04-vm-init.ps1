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
    [int]$RedEDRPort = 8080,

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
Write-Info "[7/10] RedEdr setup (download, extract, layout)..."

$RedEdrUrl  = "https://github.com/dobin/RedEdr/releases/download/v0.3/RedEdr_0.3.zip"
$RedEdrZip  = "$env:TEMP\RedEdr_0.3.zip"
$RedEdrRoot = "C:\RedEdr"   # only this path is supported
try {
    if (-not (Test-Path $RedEdrZip)) {
        Write-Info "Downloading RedEdr release..."
        Invoke-WebRequest -Uri $RedEdrUrl -OutFile $RedEdrZip -UseBasicParsing -ErrorAction Stop
        Write-Success "Downloaded: $RedEdrZip"
    } else { Write-Info "Zip already present: $RedEdrZip" }

    if (Test-Path $RedEdrRoot) { Write-Info "Clearing existing $RedEdrRoot"; Remove-Item $RedEdrRoot -Recurse -Force -ErrorAction SilentlyContinue }
    New-Item -ItemType Directory -Path $RedEdrRoot -Force | Out-Null
    Expand-Archive -Path $RedEdrZip -DestinationPath $RedEdrRoot -Force
    Write-Success "Extracted to $RedEdrRoot (required by RedEdr)"
} catch {
    Write-Err "Failed to install RedEdr: $($_.Exception.Message)"
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
    Start RedEDR as SYSTEM (required for ETW/ETW-TI or kernel hooking) and trace a chosen process.

.DESCRIPTION
    Prompts you to choose:
      - Mode: "hooking" (ntdll.dll hooking via KAPC DLL injection) OR "etw" (ETW + ETW-TI)
      - Target process to observe (e.g., notepad.exe)
      - Whether to enable the Web UI

    It then creates/starts a Scheduled Task that runs RedEdr.exe as NT AUTHORITY\SYSTEM
    with the appropriate arguments.

    Notes from RedEDR docs:
      - Hooking: `.\RedEdr.exe --kernel --inject --trace <proc>`
        • Requires self-signed kernel modules to load.

      - ETW & ETW-TI: `.\RedEdr.exe --etw --etwti --trace <proc>`
        • ETW-TI requires an ELAM driver to start RedEdrPplService (self-signed kernel driver).
          Make a VM snapshot first; PPL service removal is not currently possible.
        • For Microsoft-Windows-Security-Auditing ETW, run as SYSTEM and configure advanced audit policy.

.PARAMETER StopOnly
    Stop running instance and remove the task without starting a new one.
#>
param([switch]$StopOnly)

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

# --- Interactive choices ---
# Mode
Write-Host ""
Write-Host "Select mode:" -ForegroundColor Cyan
Write-Host "  [1] hooking  -> --kernel --inject --trace <proc> (ntdll.dll hooking via KAPC; requires self-signed kernel modules)"
Write-Host "  [2] etw      -> --etw --etwti --trace <proc> (ETW + ETW-TI; ELAM/PPL required; snapshot VM first)"
$modeSel = Read-Host "Enter 1 or 2"

switch ($modeSel) {
    "1" { $Mode = "hooking" }
    "2" { $Mode = "etw" }
    default {
        Write-Err "Invalid selection. Choose 1 or 2."
        exit 1
    }
}

# Target process
$TraceTarget = Read-Host "Enter process to observe (e.g., notepad.exe)"
if (-not $TraceTarget -or [string]::IsNullOrWhiteSpace($TraceTarget)) {
    Write-Err "A target process is required."
    exit 1
}
$TraceTarget = $TraceTarget.Trim()

# Web UI (no port option in RedEDR CLI)
$webAns = Read-Host "Enable Web UI? [Y/n] (default Y)"
$EnableWeb = if ($webAns -match '^(n|no)$') { $false } else { $true }

# --- Build argument string ---
$argList = @()
switch ($Mode) {
    "hooking" {
        Write-Info "Mode: hooking (kernel + APC DLL injection)"
        Write-Warn "Requires self-signed kernel modules to load."
        $argList += @("--kernel","--inject","--trace",$TraceTarget)
    }
    "etw" {
        Write-Info "Mode: etw (ETW + ETW-TI)"
        Write-Warn "ETW-TI requires ELAM and RedEdrPplService (snapshot VM; not easily removable)."
        $argList += @("--etw","--etwti","--trace",$TraceTarget)
    }
}

if ($EnableWeb) {
    $argList += @("--web")
    Write-Info "Web UI enabled (--web)."
}

# IMPORTANT: Do NOT add --hide; scheduled tasks run headless already.
$rededrArgs = ($argList | ForEach-Object {
    if ($_ -match '\s') { '"' + $_ + '"' } else { $_ }
}) -join ' '

Write-Host ""
Write-Info "Creating SYSTEM scheduled task with command:"
Write-Host "  $RedEdrExe $rededrArgs" -ForegroundColor Gray

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
    Write-Info    "Mode   : $Mode"
    Write-Info    "Target : $TraceTarget"
    if ($EnableWeb) { Write-Info "Web UI : enabled (--web)" }
    Write-Info    "Stop   : .\Start-RedEDR-SYSTEM.ps1 -StopOnly"
} else {
    Write-Warn "RedEDR process not detected."
    Write-Info "Open Task Scheduler (taskschd.msc) → Task '$TaskName' → History for details."
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
Write-Host "|  After reboot, try:                                            |"
Write-Host "|    cd C:\RedEdr                                                |"
Write-Host "|    .\RedEdr.exe --all --web --hide --trace notepad.exe         |"
Write-Host "|  Then open http://localhost:$RedEDRPort".PadRight(64) "|"
Write-Host "+================================================================+" -ForegroundColor Yellow

if (-not $SkipReboot) {
    Write-Host "`nRebooting in 10 seconds (Ctrl+C to cancel)..." -ForegroundColor Yellow
    Start-Sleep -Seconds 10
    Restart-Computer -Force
} else {
    Write-Info "Reboot skipped (-SkipReboot). Manual reboot strongly recommended."
}
