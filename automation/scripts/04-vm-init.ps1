<#
.SYNOPSIS
    Worker VM initialization (run inside VM or via PowerShell Direct)

.PARAMETER StaticIP
    Worker IP (e.g., "192.168.200.100")

.PARAMETER WorkerName
    VM name for reference
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$StaticIP,

    [Parameter(Mandatory)]
    [string]$WorkerName
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }

$Gateway = "192.168.200.1"
$Prefix = 24

Write-Info "Configuring $WorkerName with IP: $StaticIP"

# 1) Configure static IP
$adapter = Get-NetAdapter | Where-Object { $_.Status -eq "Up" -and $_.Name -notlike "*Loopback*" } | Select-Object -First 1
if ($adapter) {
    Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue | Remove-NetIPAddress -Confirm:$false -ErrorAction SilentlyContinue
    New-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $StaticIP -PrefixLength $Prefix -DefaultGateway $Gateway | Out-Null
    Set-DnsClientServerAddress -InterfaceIndex $adapter.ifIndex -ServerAddresses $Gateway
    Write-Success "Static IP configured: $StaticIP/$Prefix"
}

# 2) Disable Windows Update auto-restart
Set-ItemProperty -Path "HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU" -Name "NoAutoRebootWithLoggedOnUsers" -Value 1 -Force -ErrorAction SilentlyContinue
Write-Success "Windows Update auto-restart disabled"

# 3) Install Chocolatey
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Set-ExecutionPolicy Bypass -Scope Process -Force
    [System.Net.ServicePointManager]::SecurityProtocol = 3072
    iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
    Write-Success "Chocolatey installed"
}

# 4) Install Rust
if (-not (Test-Path "$env:USERPROFILE\.cargo\bin\rustc.exe")) {
    $rustExe = "$env:TEMP\rustup-init.exe"
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustExe
    Start-Process -FilePath $rustExe -ArgumentList "-y" -NoNewWindow -Wait
    $env:Path += ";$env:USERPROFILE\.cargo\bin"
    Write-Success "Rust installed"
}

# 5) Install protoc
if (-not (Test-Path "C:\protoc\bin\protoc.exe")) {
    $zip = "$env:TEMP\protoc.zip"
    Invoke-WebRequest -Uri "https://github.com/protocolbuffers/protobuf/releases/download/v25.1/protoc-25.1-win64.zip" -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath "C:\protoc" -Force
    $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    if ($machinePath -notlike "*C:\protoc\bin*") {
        [Environment]::SetEnvironmentVariable("Path", "$machinePath;C:\protoc\bin", "Machine")
    }
    Write-Success "protoc installed"
}

# 6) Create AutoMutate directory
New-Item -ItemType Directory -Path "C:\AutoMutate" -Force | Out-Null

# 7) Install Visual Studio Build Tools (minimal C++ workload)
# Commented out by default (large download, ~5GB). Uncomment if needed:
# choco install -y visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --quiet"

Write-Success "VM initialization complete"
Write-Info "Restart recommended: Restart-Computer -Force"
Write-Info "After restart, create baseline: Checkpoint-VM -VMName '$WorkerName' -SnapshotName '$WorkerName-baseline' (on host)"
