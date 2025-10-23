<#
.SYNOPSIS
    AutoMutate Environment Setup

.DESCRIPTION
    Master script that orchestrates the complete infrastructure setup:
    1. Host configuration (Hyper-V, WSL2, networking)
    2. WSL2 bootstrap (Rust, Elasticsearch, Controller)
    3. Worker VM creation (Windows 10/11 with TPM)
    4. Baseline checkpoint creation

.PARAMETER ConfigPath
    Path to config.yaml (default: .\config.yaml)

.PARAMETER SkipRebootCheck
    Skip reboot check (use after manual reboot)

.EXAMPLE
    .\setup-all.ps1

.EXAMPLE
    .\setup-all.ps1 -ConfigPath ".\custom-config.yaml" -SkipRebootCheck
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$ConfigPath = ".\config.yaml",

    [Parameter()]
    [switch]$SkipRebootCheck
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Import shared config module
Import-Module "$PSScriptRoot\scripts\modules\AutoMutateConfig.psm1" -Force

# Color output functions
function Write-Success { param($Message) Write-Host "[OK] $Message" -ForegroundColor Green }
function Write-Info { param($Message) Write-Host "[INFO] $Message" -ForegroundColor Cyan }
function Write-Warning { param($Message) Write-Host "[WARN] $Message" -ForegroundColor Yellow }
function Write-Error { param($Message) Write-Host "[ERROR] $Message" -ForegroundColor Red }
function Write-Step { param($Message) Write-Host "`n==> $Message" -ForegroundColor Magenta }

# Check admin privileges
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "This script must be run as Administrator"
    exit 1
}

Write-Host @"

+=================================================================+
|                                                                 |
|                  A U T O M U T A T E ++                         |
|                 Environment Setup                               |
|                                                                 |
|                 EDR Triage & Mutation Framework                 |
|                                                                 |
+=================================================================+


"@ -ForegroundColor Cyan

# Validate config file
if (-not (Test-Path $ConfigPath)) {
    Write-Error "Config file not found: $ConfigPath"
    Write-Info "Expected location: $(Resolve-Path '.\config.yaml' -ErrorAction SilentlyContinue)"
    exit 1
}

Write-Info "Using configuration: $ConfigPath"

# Validate config can be parsed
try {
    $Config = Read-AutoMutateConfig -ConfigPath $ConfigPath
    Write-Success "Configuration loaded successfully"
} catch {
    Write-Error "Failed to parse config: $_"
    exit 1
}

# Setup logging
$LogDir = Join-Path $PSScriptRoot "logs"
if (-not (Test-Path $LogDir)) {
    New-Item -ItemType Directory -Path $LogDir -Force | Out-Null
}
$LogFile = Join-Path $LogDir "setup-$(Get-Date -Format 'yyyyMMdd-HHmmss').log"
Start-Transcript -Path $LogFile -Append

Write-Info "Log file: $LogFile"

# Step 1: Host Setup
Write-Step "Step 1/4: Host Configuration"
Write-Info "Enabling Hyper-V, WSL2, and configuring network isolation..."

$HostSetupScript = Join-Path $PSScriptRoot "scripts\01-host-setup.ps1"
if (-not (Test-Path $HostSetupScript)) {
    Write-Error "Host setup script not found: $HostSetupScript"
    exit 1
}

& $HostSetupScript -ConfigPath $ConfigPath

if ($LASTEXITCODE -eq 3010) {
    Write-Warning "Reboot required to complete feature installation"
    Write-Info "After reboot, run: .\setup-all.ps1 -SkipRebootCheck"
    Stop-Transcript
    exit 3010
}

Write-Success "Host configuration complete"

# Step 2: WSL2 Bootstrap
Write-Step "Step 2/4: WSL2 Controller Bootstrap"
Write-Info "Installing Rust toolchain, Elasticsearch, and building Controller..."

# Detect WSL Ubuntu home directory
$WslUser = wsl -d Ubuntu bash -c "whoami" 2>$null
if (-not $WslUser) {
    Write-Error "WSL Ubuntu not found. Re-run Step 1."
    exit 1
}

$ProjectRoot = Split-Path $PSScriptRoot -Parent
$WslProjectRoot = $ProjectRoot -replace '\\', '/' -replace 'C:', '/mnt/c'

Write-Info "WSL User: $WslUser"
Write-Info "Project root (WSL): $WslProjectRoot"

$WslBootstrapScript = Join-Path $PSScriptRoot "scripts\02-wsl-bootstrap.sh"
if (-not (Test-Path $WslBootstrapScript)) {
    Write-Error "WSL bootstrap script not found: $WslBootstrapScript"
    exit 1
}

# Make script executable and run
wsl -d Ubuntu bash -c "chmod +x '$WslProjectRoot/automation/scripts/02-wsl-bootstrap.sh'"
wsl -d Ubuntu bash -c "cd '$WslProjectRoot/automation' && ./scripts/02-wsl-bootstrap.sh '$WslProjectRoot'"

if ($LASTEXITCODE -ne 0) {
    Write-Error "WSL bootstrap failed. Check logs in WSL: $WslProjectRoot/automation/logs/"
    exit 1
}

Write-Success "WSL2 Controller bootstrap complete"
Write-Info "Elasticsearch: http://localhost:9200" #TODO make the port read from config
Write-Info "Kibana: http://localhost:5601"

# Step 3: Create Worker VMs
Write-Step "Step 3/4: Worker VM Creation"

# Get workers from template config using shared module
$WorkerConfigs = Get-AutoMutateWorkers -ConfigPath $ConfigPath

Write-Info "Found $($WorkerConfigs.Count) worker(s) in configuration"

$CreateVmScript = Join-Path $PSScriptRoot "scripts\03-create-worker-vm.ps1"
if (-not (Test-Path $CreateVmScript)) {
    Write-Error "Create VM script not found: $CreateVmScript"
    exit 1
}

foreach ($worker in $WorkerConfigs) {
    Write-Info "Creating VM: $($worker.Name) ($($worker.Os)) at $($worker.IP)"

    & $CreateVmScript `
        -WorkerName $worker.Name `
        -Os $worker.Os `
        -IsoPath $worker.IsoPath `
        -StaticIP $worker.IP `
        -ConfigPath $ConfigPath

    if ($LASTEXITCODE -ne 0) {
        Write-Error "Failed to create VM: $($worker.Name)"
        continue
    }

    Write-Success "VM created: $($worker.Name)"
}

# Step 4: Manual Instructions
Write-Step "Step 4/4: Manual Configuration Required"

Write-Host @"

+=================================================================+
|                                                                 |
|              Automated Setup Complete!                          |
|                                                                 |
+=================================================================+

Next steps (MANUAL):

1. Install Windows on each Worker VM:
"@ -ForegroundColor Green

foreach ($worker in $WorkerConfigs) {
    Write-Host "   - Start VM: $($worker.Name) in Hyper-V Manager" -ForegroundColor Yellow
    Write-Host "   - Boot from ISO: $($worker.IsoPath)" -ForegroundColor Gray
}

Write-Host @"

2. Complete Windows Setup manually:
   - During installation, create local account: worker-admin
   - Recommended password: AutoMutate!Password (or choose your own)
   - Complete OOBE (Out-of-Box Experience)
   - Installation takes ~10-20 minutes depending on hardware

3. After Windows desktop loads, configure each VM:

"@ -ForegroundColor Green

foreach ($worker in $WorkerConfigs) {
    Write-Host @"
   VM: $($worker.Name) (IP: $($worker.IP))
   Run via PowerShell Direct (from host):

   `$cred = Get-Credential -UserName 'worker-admin' -Message 'Enter VM password'
   Invoke-Command -VMName "$($worker.Name)" -FilePath ".\scripts\04-vm-init.ps1" ``
     -ArgumentList "$($worker.IP)", "$($worker.Name)" -Credential `$cred

"@ -ForegroundColor Cyan
}

Write-Host @"
4. Create baseline checkpoints for all VMs:
   .\scripts\create-all-baselines.ps1

   OR manually for each VM:
"@ -ForegroundColor Green

foreach ($worker in $WorkerConfigs) {
    Write-Host @"
   .\scripts\05-create-baseline.ps1 -WorkerName "$($worker.Name)"
"@ -ForegroundColor Cyan
}
Write-Host ""

Write-Host @"

5. Optional: Isolate VMs from internet:
   Use the internet kill switch to air-gap VMs during artifact testing:

   To disable internet access (air-gap mode):
   .\scripts\toggle-vm-internet.ps1 -Action Disable

   To re-enable internet access:
   .\scripts\toggle-vm-internet.ps1 -Action Enable

   To check current status:
   .\scripts\toggle-vm-internet.ps1 -Action Status

6. Validate installation:
   .\validate-environment.ps1

7. Start environment:
   .\scripts\start-environment.ps1

"@ -ForegroundColor Green

Write-Success "Setup log saved to: $LogFile"

Stop-Transcript

# Summary
Write-Host "`n" + "="*70 + "`n"
Write-Host "SUMMARY:" -ForegroundColor Magenta
Write-Success "Host configured (Hyper-V, WSL2, NAT for VM internet)"
Write-Success "Controller bootstrapped (Rust, Docker, Elasticsearch, Kibana)"
Write-Success "Worker VM shells created ($($WorkerConfigs.Count) total - awaiting Windows install)"
Write-Host ""
Write-Info "NEXT: Install Windows on each VM, then run 04-vm-init.ps1 for configuration"
Write-Info "THEN: Create baselines with 05-create-baseline.ps1"
Write-Info "NOTE: Use .\scripts\toggle-vm-internet.ps1 to control VM internet access"
