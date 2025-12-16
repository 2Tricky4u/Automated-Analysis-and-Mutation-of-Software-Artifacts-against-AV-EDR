<#
.SYNOPSIS
    Deploy worker agent to remote VM (for dynamic registration)

.DESCRIPTION
    Copies worker binary and config to remote VM via WinRM or SSH
    Worker will self-register with controller on startup

.PARAMETER RemoteHost
    IP address or hostname of remote VM

.PARAMETER Username
    Remote admin username

.PARAMETER Password
    Remote admin password (optional - prompts if not provided)

.PARAMETER WorkerConfigPath
    Path to worker TOML config file

.PARAMETER UseSSH
    Use SSH instead of WinRM (for Linux workers)

.EXAMPLE
    .\deploy-remote-worker.ps1 -RemoteHost 20.123.45.67 -Username "vmadmin" -WorkerConfigPath "..\..\generated\win10-azure-worker-01.toml"

.EXAMPLE
    .\deploy-remote-worker.ps1 -RemoteHost 3.145.67.89 -Username "ubuntu" -WorkerConfigPath "..\..\generated\ubuntu-aws-worker-01.toml" -UseSSH
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$RemoteHost,

    [Parameter(Mandatory)]
    [string]$Username,

    [Parameter()]
    [securestring]$Password,

    [Parameter(Mandatory)]
    [string]$WorkerConfigPath,

    [Parameter()]
    [switch]$UseSSH
)

$ErrorActionPreference = "Stop"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info    { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn    { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err     { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  Deploy Worker to Remote VM" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

Write-Info "Target: $RemoteHost"
Write-Info "User: $Username"
Write-Info "Config: $WorkerConfigPath"

# Validate config file exists
if (-not (Test-Path $WorkerConfigPath)) {
    Write-Err "Worker config not found: $WorkerConfigPath"
    exit 1
}

# Get project root
$ProjectRoot = Split-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -Parent

# Build worker binary
Write-Info "[1/6] Building worker agent..."
Push-Location "$ProjectRoot\worker\agent"
try {
    $buildOutput = cargo build --release 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Cargo build failed"
        Write-Host $buildOutput
        exit 1
    }
    Write-Success "Worker binary built successfully"
} finally {
    Pop-Location
}

$workerBinary = Join-Path $ProjectRoot "target\release\worker-agent.exe"
if (-not (Test-Path $workerBinary)) {
    Write-Err "Worker binary not found: $workerBinary"
    Write-Info "Expected location after 'cargo build --release'"
    exit 1
}

# Get credentials
if (-not $Password) {
    $credential = Get-Credential -UserName $Username -Message "Enter password for $Username@$RemoteHost"
} else {
    $credential = New-Object System.Management.Automation.PSCredential($Username, $Password)
}

if ($UseSSH) {
    Write-Info "[2/6] Connecting via SSH..."

    # Test SSH connection
    $sshTest = ssh -o ConnectTimeout=5 -o StrictHostKeyChecking=no "$Username@$RemoteHost" "echo 'SSH connection successful'" 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Err "SSH connection failed: $sshTest"
        exit 1
    }
    Write-Success "SSH connection established"

    # Create remote directories
    Write-Info "[3/6] Creating directories on remote VM..."
    ssh "$Username@$RemoteHost" "mkdir -p /opt/automutate/{artifacts,logs,traces,coverage}"
    Write-Success "Directories created"

    # Copy worker binary
    Write-Info "[4/6] Copying worker binary..."
    scp "$workerBinary" "${Username}@${RemoteHost}:/opt/automutate/worker-agent"
    ssh "$Username@$RemoteHost" "chmod +x /opt/automutate/worker-agent"
    Write-Success "Worker binary deployed"

    # Copy worker config
    Write-Info "[5/6] Copying worker config..."
    scp "$WorkerConfigPath" "${Username}@${RemoteHost}:/opt/automutate/worker.toml"
    Write-Success "Worker config deployed"

    # Start worker agent
    Write-Info "[6/6] Starting worker agent..."
    ssh "$Username@$RemoteHost" "nohup /opt/automutate/worker-agent > /opt/automutate/logs/worker.log 2>&1 &"
    Write-Success "Worker agent started"

} else {
    # Windows remote deployment via WinRM
    Write-Info "[2/6] Connecting via WinRM..."

    try {
        $session = New-PSSession -ComputerName $RemoteHost -Credential $credential -ErrorAction Stop
        Write-Success "WinRM session established"
    } catch {
        Write-Err "Failed to connect to remote host: $_"
        Write-Info "Ensure WinRM is enabled on the remote machine:"
        Write-Info "  Enable-PSRemoting -Force"
        Write-Info "  Set-Item WSMan:\localhost\Client\TrustedHosts -Value '$RemoteHost' -Force"
        exit 1
    }

    try {
        # Create remote directories
        Write-Info "[3/6] Creating directories on remote VM..."
        Invoke-Command -Session $session -ScriptBlock {
            New-Item -ItemType Directory -Path "C:\AutoMutate" -Force | Out-Null
            New-Item -ItemType Directory -Path "C:\AutoMutate\artifacts" -Force | Out-Null
            New-Item -ItemType Directory -Path "C:\AutoMutate\logs" -Force | Out-Null
            New-Item -ItemType Directory -Path "C:\AutoMutate\traces" -Force | Out-Null
            New-Item -ItemType Directory -Path "C:\AutoMutate\coverage" -Force | Out-Null
        }
        Write-Success "Directories created"

        # Copy worker binary
        Write-Info "[4/6] Copying worker binary ($(((Get-Item $workerBinary).Length / 1MB).ToString('0.0')) MB)..."
        Copy-Item -ToSession $session -Path $workerBinary -Destination "C:\AutoMutate\worker-agent.exe" -Force
        Write-Success "Worker binary deployed"

        # Copy worker config
        Write-Info "[5/6] Copying worker config..."
        Copy-Item -ToSession $session -Path $WorkerConfigPath -Destination "C:\AutoMutate\worker.toml" -Force
        Write-Success "Worker config deployed"

        # Configure firewall and start worker
        Write-Info "[6/6] Configuring firewall and starting worker..."
        Invoke-Command -Session $session -ScriptBlock {
            # Create firewall rule for worker port
            New-NetFirewallRule -DisplayName "AutoMutate Worker Agent" `
                -Direction Inbound `
                -LocalPort 50052 `
                -Protocol TCP `
                -Action Allow `
                -ErrorAction SilentlyContinue | Out-Null

            # Start worker as background process
            $startInfo = New-Object System.Diagnostics.ProcessStartInfo
            $startInfo.FileName = "C:\AutoMutate\worker-agent.exe"
            $startInfo.WorkingDirectory = "C:\AutoMutate"
            $startInfo.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
            $startInfo.CreateNoWindow = $true

            $process = [System.Diagnostics.Process]::Start($startInfo)

            if ($process) {
                Write-Host "Worker agent started (PID: $($process.Id))" -ForegroundColor Green
            } else {
                Write-Host "Failed to start worker agent" -ForegroundColor Red
            }
        }
        Write-Success "Worker agent started on remote VM"

    } finally {
        Remove-PSSession -Session $session
    }
}

Write-Host "`n========================================" -ForegroundColor Green
Write-Host "  Deployment Complete!" -ForegroundColor Green
Write-Host "========================================`n" -ForegroundColor Green

Write-Info "Worker deployed to: $RemoteHost"
Write-Info "Worker will self-register with controller on startup"
Write-Info ""
Write-Info "Next steps:"
Write-Info "  1. Verify worker registered:"
Write-Info "     .\scripts\workers\list-workers.ps1"
Write-Info ""
Write-Info "  2. Check worker logs (if needed):"
if ($UseSSH) {
    Write-Info "     ssh $Username@$RemoteHost 'tail -f /opt/automutate/logs/worker.log'"
} else {
    Write-Info "     Invoke-Command -ComputerName $RemoteHost { Get-Content C:\AutoMutate\logs\worker.log -Tail 20 }"
}
Write-Info ""
