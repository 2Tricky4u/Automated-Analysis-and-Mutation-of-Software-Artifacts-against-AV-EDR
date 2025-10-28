<#
.SYNOPSIS
    Initialize Worker VM after manual Windows installation

.DESCRIPTION
    Disables Secure Boot and runs 04-vm-init.ps1 inside the VM.
    Run this after Windows installation is complete and VM has been shut down.

.PARAMETER VMName
    Name of the worker VM (e.g., "win11-worker-01")

.PARAMETER IPAddress
    Static IP for the worker (e.g., "10.200.200.110")

.PARAMETER Username
    VM admin username (default: "worker-admin")

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.EXAMPLE
    .\initialize-worker.ps1 -VMName "win11-worker-01" -IPAddress "10.200.200.110"

.NOTES
    Prerequisites:
    - Windows installation completed
    - VM must be STOPPED (Secure Boot can only be disabled while VM is off)
    - PowerShell Direct enabled (happens automatically in 04-vm-init.ps1)
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [Parameter(Mandatory)]
    [string]$IPAddress,

    [Parameter()]
    [string]$Username = "worker-admin",

    [Parameter()]
    [string]$ConfigPath = "..\config.yaml",

    [Parameter()]
    [int]$RedEDRPort = 8080
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Err { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

Write-Host @"

+================================================================+
|          Worker VM Initialization Wrapper                      |
+================================================================+

"@ -ForegroundColor Cyan

# Check if VM exists
$vm = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if (-not $vm) {
    Write-Err "VM not found: $VMName"
    Write-Info "Available VMs:"
    Get-VM | Select-Object Name, State | Format-Table
    exit 1
}

Write-Info "VM: $VMName"
Write-Info "IP: $IPAddress"
Write-Info ""

# Step 1: Ensure VM is stopped
if ($vm.State -ne "Off") {
    Write-Info "VM is currently $($vm.State). Stopping VM..."
    Stop-VM -Name $VMName -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 5
    Write-Success "VM stopped"
} else {
    Write-Success "VM already stopped"
}

# Step 2: Disable Secure Boot
Write-Info "Disabling Secure Boot..."
try {
    Set-VMFirmware -VMName $VMName -EnableSecureBoot Off
    Write-Success "Secure Boot disabled"
} catch {
    Write-Err "Failed to disable Secure Boot: $($_.Exception.Message)"
    exit 1
}

# Verify Secure Boot is disabled
$firmware = Get-VMFirmware -VMName $VMName
if ($firmware.SecureBoot -eq "Off") {
    Write-Success "Verified: Secure Boot is OFF"
} else {
    Write-Err "Secure Boot is still enabled: $($firmware.SecureBoot)"
    exit 1
}

# Step 3: Start VM
Write-Info "Starting VM..."
Start-VM -Name $VMName
Write-Success "VM started"

# Wait for VM to boot and be ready for PowerShell Direct
Write-Info "Waiting for VM to boot (30 seconds)..."
Start-Sleep -Seconds 30

# Wait for heartbeat
Write-Info "Waiting for VM heartbeat..."
$maxWait = 60
$waited = 0
while ($waited -lt $maxWait) {
    $vm = Get-VM -Name $VMName
    if ($vm.Heartbeat -eq "OkApplicationsHealthy" -or $vm.Heartbeat -eq "OkApplicationsUnknown") {
        Write-Success "VM heartbeat detected"
        break
    }
    Start-Sleep -Seconds 2
    $waited += 2
}

if ($waited -ge $maxWait) {
    Write-Err "VM heartbeat not detected after $maxWait seconds"
    Write-Info "VM may still be booting. You can try running 04-vm-init.ps1 manually."
    exit 1
}

# Step 4: Prepare build package for VM
Write-Host ""
Write-Info "Preparing worker agent build package..."

# Detect project root (2 levels up from scripts/)
$ProjectRoot = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent

if (-not (Test-Path "$ProjectRoot\Cargo.toml")) {
    Write-Err "Cannot find project root with Cargo.toml"
    Write-Info "Expected: $ProjectRoot\Cargo.toml"
    exit 1
}

# Create temporary build package
$BuildPackage = "$env:TEMP\worker-build-$VMName"
if (Test-Path $BuildPackage) {
    Remove-Item $BuildPackage -Recurse -Force
}
New-Item -ItemType Directory -Path $BuildPackage -Force | Out-Null

Write-Info "Project root: $ProjectRoot"
Write-Info "Build package: $BuildPackage"

# Copy minimal files needed for worker agent build
try {
    # Create minimal workspace Cargo.toml (only worker/agent member)
    # Read original to extract workspace.package and workspace.dependencies
    $originalCargoToml = Get-Content "$ProjectRoot\Cargo.toml" -Raw

    # Create minimal workspace config for VM build
    $minimalWorkspace = @"
[workspace]
resolver = "2"
members = [
    "worker/agent",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
authors = ["2Tricky4u"]

[workspace.dependencies]
tokio = { version = "1.48.0", features = ["full"] }
tonic = "0.14.2"
prost = "0.14.1"
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.145"
tracing = "0.1.41"
tracing-subscriber = "0.3.20"
anyhow = "1.0.100"
thiserror = "2.0.17"
"@

    $minimalWorkspace | Out-File -FilePath "$BuildPackage\Cargo.toml" -Encoding UTF8 -Force
    Write-Success "Created minimal workspace Cargo.toml"

    # Copy Cargo.lock if exists (speeds up builds)
    Copy-Item "$ProjectRoot\Cargo.lock" "$BuildPackage\" -Force -ErrorAction SilentlyContinue

    # Worker agent source
    $workerSrc = "$ProjectRoot\worker\agent"
    $workerDest = "$BuildPackage\worker\agent"
    if (Test-Path $workerSrc) {
        New-Item -ItemType Directory -Path $workerDest -Force | Out-Null
        Copy-Item "$workerSrc\*" "$workerDest\" -Recurse -Force
        Write-Success "Copied worker agent source"
    } else {
        Write-Err "Worker agent source not found: $workerSrc"
        exit 1
    }

    # Proto definitions (needed by build.rs)
    $protoSrc = "$ProjectRoot\controller\proto"
    $protoDest = "$BuildPackage\controller\proto"
    if (Test-Path $protoSrc) {
        New-Item -ItemType Directory -Path $protoDest -Force | Out-Null
        Copy-Item "$protoSrc\*.proto" "$protoDest\" -Force
        Write-Success "Copied proto definitions"
    } else {
        Write-Err "Proto files not found: $protoSrc"
        exit 1
    }

    # Config crate (if exists and referenced by worker)
    $configSrc = "$ProjectRoot\config"
    if (Test-Path $configSrc) {
        $configDest = "$BuildPackage\config"
        New-Item -ItemType Directory -Path $configDest -Force | Out-Null
        Copy-Item "$configSrc\*" "$configDest\" -Recurse -Force
        Write-Info "Copied config crate"
    }

    Write-Success "Build package prepared: $(Get-ChildItem $BuildPackage -Recurse | Measure-Object -Property Length -Sum | Select-Object -ExpandProperty Sum) bytes"

} catch {
    Write-Err "Failed to prepare build package: $($_.Exception.Message)"
    exit 1
}

# Step 5: Get credentials
Write-Host ""
Write-Info "Enter VM credentials for PowerShell Direct..."
$cred = Get-Credential -UserName $Username -Message "Enter password for $Username"

# Step 6: Copy build package to VM
Write-Host ""
Write-Info "Copying build package to VM..."

try {
    # Create destination directory in VM
    Invoke-Command -VMName $VMName -Credential $cred -ScriptBlock {
        $buildDir = "C:\AutoMutate\build"
        if (Test-Path $buildDir) {
            Remove-Item $buildDir -Recurse -Force
        }
        New-Item -ItemType Directory -Path $buildDir -Force | Out-Null
    }

    # Copy files to VM using PowerShell Direct
    # Note: Copy-Item with -ToSession is the most reliable method
    $session = New-PSSession -VMName $VMName -Credential $cred
    Copy-Item "$BuildPackage\*" -Destination "C:\AutoMutate\build\" -ToSession $session -Recurse -Force
    Remove-PSSession $session

    Write-Success "Build package copied to VM"

} catch {
    Write-Err "Failed to copy build package: $($_.Exception.Message)"
    exit 1
}

# Step 7: Run 04-vm-init.ps1 via PowerShell Direct
Write-Host ""
Write-Info "Running 04-vm-init.ps1 inside VM via PowerShell Direct..."
Write-Info "This will take several minutes (dependencies, RedEDR, configuration)..."
Write-Host ""

try {
    # Resolve config path (works from automation/ or scripts/ folder)
    $ResolvedConfigPath = $ConfigPath
    if (-not [System.IO.Path]::IsPathRooted($ConfigPath)) {
        # Relative path - try to find config.yaml
        if (Test-Path $ConfigPath) {
            $ResolvedConfigPath = $ConfigPath
        } elseif (Test-Path (Join-Path $PSScriptRoot "..\config.yaml")) {
            # Running from scripts folder
            $ResolvedConfigPath = Join-Path $PSScriptRoot "..\config.yaml"
        } elseif (Test-Path ".\config.yaml") {
            # Running from automation folder
            $ResolvedConfigPath = ".\config.yaml"
        } else {
            Write-Err "Cannot find config.yaml. Tried: $ConfigPath"
            exit 1
        }
    }

    # Load config to get network settings
    $config = @{}
    $section = $null
    Get-Content $ResolvedConfigPath | ForEach-Object {
        if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
        elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
    }

    $Gateway = $config.network.gateway
    $Prefix = ($config.network.subnet -split '/')[1]

    # Resolve 04-vm-init.ps1 path (always in scripts folder)
    $InitScriptPath = Join-Path $PSScriptRoot "04-vm-init.ps1"
    if (-not (Test-Path $InitScriptPath)) {
        Write-Err "Script not found: $InitScriptPath"
        exit 1
    }

    Invoke-Command -VMName $VMName -FilePath $InitScriptPath `
        -ArgumentList $IPAddress, $VMName, $Gateway, $Prefix, $RedEDRPort `
        -Credential $cred

    Write-Host ""
    Write-Success "Initialization complete!"
} catch {
    Write-Err "Failed to run 04-vm-init.ps1: $($_.Exception.Message)"
    Write-Info "Troubleshooting:"
    Write-Info "  1. Verify PowerShell Direct is enabled (check VM Integration Services)"
    Write-Info "  2. Verify credentials are correct"
    Write-Info "  3. Check VM is fully booted (may need more time)"
    Write-Info "  4. Try running manually from within the VM"
    exit 1
}

Write-Host ""
Write-Host "+================================================================+" -ForegroundColor Green
Write-Host "|          Worker VM Ready                                       |" -ForegroundColor Green
Write-Host "+================================================================+" -ForegroundColor Green
Write-Host ""
Write-Info "VM: $VMName"
Write-Info "IP: $IPAddress"
Write-Info "Secure Boot: DISABLED"
Write-Host ""

exit 0
