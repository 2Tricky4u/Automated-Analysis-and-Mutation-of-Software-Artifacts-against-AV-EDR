<#
.SYNOPSIS
    Worker VM initialization (run inside VM or via PowerShell Direct)
    Performs all configuration that autounattend.xml would have done, plus additional setup

.PARAMETER StaticIP
    Worker IP (e.g., "192.168.200.100")

.PARAMETER WorkerName
    VM name for reference

.PARAMETER Gateway
    Gateway IP (default: 192.168.200.1)

.PARAMETER Prefix
    Network prefix length (default: 24)

.PARAMETER SkipReboot
    Skip automatic reboot at the end

.EXAMPLE
    # From host (PowerShell Direct)
    $cred = Get-Credential -UserName 'worker-admin'
    Invoke-Command -VMName "win11-worker-01" -FilePath .\04-vm-init.ps1 `
        -ArgumentList "10.200.200.110", "win11-worker-01" -Credential $cred

.EXAMPLE
    # Inside VM
    .\04-vm-init.ps1 -StaticIP "10.200.200.100" -WorkerName "win10-worker-01"
#>

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
    [switch]$SkipReboot
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

# Network parameters are now passed via function parameters (with defaults)
# $Gateway and $Prefix are defined in the param block above

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Worker VM Initialization                              |" -ForegroundColor Cyan
Write-Host "|          $WorkerName -> $StaticIP".PadRight(64) "|" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

# ===== SECTION 1: System Configuration (autounattend.xml equivalent) =====
Write-Info "[1/8] System-level configuration..."

# 1a) Set computer name
$currentName = $env:COMPUTERNAME
if ($currentName -ne $WorkerName) {
    Write-Info "Renaming computer: $currentName -> $WorkerName"
    Rename-Computer -NewName $WorkerName -Force -ErrorAction SilentlyContinue
    Write-Success "Computer renamed (requires reboot)"
} else {
    Write-Success "Computer name already set: $WorkerName"
}

# 1b) Set timezone to UTC (matches autounattend.xml)
$currentTZ = (Get-TimeZone).Id
if ($currentTZ -ne "UTC") {
    Set-TimeZone -Id "UTC" -ErrorAction SilentlyContinue
    Write-Success "Timezone set to UTC"
} else {
    Write-Success "Timezone already UTC"
}

# 1c) Enable PowerShell script execution (autounattend FirstLogonCommand #1)
$execPolicy = Get-ExecutionPolicy
if ($execPolicy -eq "Restricted" -or $execPolicy -eq "Undefined") {
    Set-ExecutionPolicy RemoteSigned -Scope LocalMachine -Force
    Write-Success "Execution policy set to RemoteSigned"
} else {
    Write-Success "Execution policy already configured: $execPolicy"
}

# 1d) Disable UAC (autounattend FirstLogonCommand #3 - LAB ONLY)
$uacKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"
$currentUAC = (Get-ItemProperty -Path $uacKey -Name "EnableLUA" -ErrorAction SilentlyContinue).EnableLUA
if ($currentUAC -ne 0) {
    Set-ItemProperty -Path $uacKey -Name "EnableLUA" -Value 0 -Force
    Write-Success "UAC disabled (lab environment only)"
} else {
    Write-Success "UAC already disabled"
}

# 1e) Disable Windows Update auto-restart (autounattend FirstLogonCommand #2)
$auKey = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU"
if (-not (Test-Path $auKey)) {
    New-Item -Path $auKey -Force | Out-Null
}
Set-ItemProperty -Path $auKey -Name "NoAutoRebootWithLoggedOnUsers" -Value 1 -Force
Write-Success "Windows Update auto-restart disabled"

# ===== SECTION 2: Network Configuration =====
Write-Info "[2/8] Network configuration..."

$adapter = Get-NetAdapter | Where-Object { $_.Status -eq "Up" -and $_.Name -notlike "*Loopback*" } | Select-Object -First 1
if ($adapter) {
    # Remove existing IP (if DHCP)
    Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue | Remove-NetIPAddress -Confirm:$false -ErrorAction SilentlyContinue
    Get-NetRoute -InterfaceIndex $adapter.ifIndex -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue | Remove-NetRoute -Confirm:$false -ErrorAction SilentlyContinue

    # Configure static IP
    New-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $StaticIP -PrefixLength $Prefix -DefaultGateway $Gateway | Out-Null

    # Configure DNS: Use public DNS directly (8.8.8.8 primary) for reliability
    # Gateway DNS can be slow or unreliable depending on host configuration
    Set-DnsClientServerAddress -InterfaceIndex $adapter.ifIndex -ServerAddresses @("8.8.8.8", "8.8.4.4", $Gateway)
    Write-Success "Static IP configured: $StaticIP/$Prefix (gateway: $Gateway)"
    Write-Info "DNS servers: 8.8.8.8 (primary), 8.8.4.4, $Gateway (fallback)"

    # Flush DNS cache to ensure fresh resolution
    Clear-DnsClientCache -ErrorAction SilentlyContinue
    ipconfig /flushdns | Out-Null

    # Wait for network stack to stabilize
    Start-Sleep -Seconds 3
    Write-Info "Testing network connectivity..."

    # Test 1: Check if gateway route exists
    $route = Get-NetRoute -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue | Where-Object { $_.NextHop -eq $Gateway }
    if ($route) {
        Write-Success "Default route to gateway configured"
    } else {
        Write-Warn "Default route not found (may cause connectivity issues)"
    }

    # Test 2: Try DNS resolution with retry (requires gateway + internet)
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

    # Test 3: Optional ICMP ping (may fail due to firewall, but that's OK)
    $pingTest = Test-Connection -ComputerName $Gateway -Count 1 -Quiet -ErrorAction SilentlyContinue
    if ($pingTest) {
        Write-Success "Gateway responds to ping: $Gateway"
    } else {
        Write-Info "Gateway ping failed (normal - Windows Firewall blocks ICMP by default)"
        Write-Info "TCP/UDP connectivity works even if ping fails"
    }
} else {
    Write-Warn "No active network adapter found"
}

# ===== SECTION 3: Privacy & Telemetry (autounattend OOBE equivalent) =====
Write-Info "[3/8] Privacy & telemetry configuration..."

# Disable telemetry
Set-ItemProperty -Path "HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection" -Name "AllowTelemetry" -Value 0 -Force -ErrorAction SilentlyContinue
Write-Success "Telemetry disabled"

# Disable Cortana
$cortanaKey = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Windows Search"
if (-not (Test-Path $cortanaKey)) { New-Item -Path $cortanaKey -Force | Out-Null }
Set-ItemProperty -Path $cortanaKey -Name "AllowCortana" -Value 0 -Force
Write-Success "Cortana disabled"

# ===== SECTION 4: Windows Defender Configuration =====
Write-Info "[4/8] Windows Defender configuration..."

# Keep Defender ENABLED by default (for EDR testing)
# Users can disable manually for specific experiments
Write-Success "Windows Defender remains ENABLED (EDR testing baseline)"
Write-Info "To disable for specific experiments, run: Set-MpPreference -DisableRealtimeMonitoring `$true"

# ===== SECTION 5: Development Tools Installation =====
Write-Info "[5/8] Installing development tools..."

# 5a) Install Chocolatey
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Write-Info "Installing Chocolatey package manager..."
    Set-ExecutionPolicy Bypass -Scope Process -Force
    [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12

    # Retry loop for Chocolatey download (DNS might still be propagating)
    $chocoInstalled = $false
    $maxChocoRetries = 5
    $chocoRetryCount = 0

    while (-not $chocoInstalled -and $chocoRetryCount -lt $maxChocoRetries) {
        $chocoRetryCount++
        try {
            Write-Info "Attempting to download Chocolatey installer (attempt $chocoRetryCount/$maxChocoRetries)..."
            $webClient = New-Object System.Net.WebClient
            $installScript = $webClient.DownloadString('https://community.chocolatey.org/install.ps1')
            iex $installScript
            $chocoInstalled = $true
            Write-Success "Chocolatey installed successfully"
        } catch {
            Write-Warn "Chocolatey download failed: $($_.Exception.Message)"
            if ($chocoRetryCount -lt $maxChocoRetries) {
                Write-Info "Retrying in 5 seconds..."
                Start-Sleep -Seconds 5
            } else {
                Write-Warn "Chocolatey installation failed after $maxChocoRetries attempts"
                Write-Info "You may need to install manually after reboot when DNS is stable"
                Write-Info "Run: Set-ExecutionPolicy Bypass -Scope Process -Force; iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))"
            }
        }
    }
} else {
    Write-Success "Chocolatey already installed"
}

# 5b) Install Rust toolchain
if (-not (Test-Path "$env:USERPROFILE\.cargo\bin\rustc.exe")) {
    Write-Info "Installing Rust toolchain (this may take 5-10 minutes)..."
    $rustExe = "$env:TEMP\rustup-init.exe"

    # Retry download for Rust installer
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

# 5c) Install protoc (Protocol Buffers compiler)
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

# ===== SECTION 6: AutoMutate Project Structure =====
Write-Info "[6/8] Creating AutoMutate project directories..."

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

# ===== SECTION 7: Optional Tools (commented by default) =====
Write-Info "[7/8] Optional tools (skipped by default)..."

Write-Info "Visual Studio Build Tools NOT installed (large download ~5GB)"
Write-Info "If needed for C++ compilation, run manually:"
Write-Info "  choco install -y visualstudio2022buildtools --params '--add Microsoft.VisualStudio.Workload.VCTools'"

# ===== SECTION 8: Verification & Summary =====
Write-Info "[8/8] Verification..."

$verificationResults = @()

# Check network (using route check instead of ping)
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

# Display results
Write-Host "`n+================================================================+" -ForegroundColor Green
Write-Host "|          Initialization Complete - Verification                |" -ForegroundColor Green
Write-Host "+================================================================+" -ForegroundColor Green

foreach ($result in $verificationResults) {
    $statusColor = switch ($result.Status) {
        "OK" { "Green" }
        "WARN" { "Yellow" }
        "FAIL" { "Red" }
    }
    $line = "| [$($result.Status.PadRight(4))] $($result.Component.PadRight(15)) $($result.Details)"
    Write-Host $line.PadRight(66) + "|" -ForegroundColor $statusColor
}

Write-Host "+================================================================+" -ForegroundColor Green

# Reboot prompt
Write-Host "`n+================================================================+" -ForegroundColor Yellow
Write-Host "|          REBOOT REQUIRED                                       |" -ForegroundColor Yellow
Write-Host "+================================================================+" -ForegroundColor Yellow
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Changes requiring reboot:                                     |" -ForegroundColor White
Write-Host "|    * Computer name change                                      |" -ForegroundColor White
Write-Host "|    * UAC disabled                                              |" -ForegroundColor White
Write-Host "|    * Environment variables (PATH)                              |" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  After reboot, create baseline checkpoint (from host):        |" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|    Checkpoint-VM -VMName '$WorkerName' ``".PadRight(64) "|" -ForegroundColor Cyan
Write-Host "|      -SnapshotName '$WorkerName-baseline'".PadRight(64) "|" -ForegroundColor Cyan
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "+================================================================+" -ForegroundColor Yellow

if (-not $SkipReboot) {
    Write-Host "`nRebooting in 10 seconds (Ctrl+C to cancel)..." -ForegroundColor Yellow
    Start-Sleep -Seconds 10
    Restart-Computer -Force
} else {
    Write-Info "Reboot skipped (use -SkipReboot flag). Manual reboot recommended: Restart-Computer -Force"
}
