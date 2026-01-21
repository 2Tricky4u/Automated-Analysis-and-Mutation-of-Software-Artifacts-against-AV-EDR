<#
.SYNOPSIS
    Deploy worker agent to remote VM (for dynamic registration)

.DESCRIPTION
    Copies worker binary and config to remote Windows VM via WMI/RPC (default) or SSH
    Worker will self-register with controller on startup

    Default: WMI/RPC (no WinRM required, uses standard Windows RPC/DCOM)
    Alternative: SSH with -UseSSH flag (for OpenSSH-enabled Windows)

.PARAMETER RemoteHost
    IP address or hostname of remote VM

.PARAMETER Username
    Remote admin username

.PARAMETER Password
    Remote admin password (optional - prompts if not provided)

.PARAMETER WorkerConfigPath
    Path to worker TOML config file (optional - auto-generated if not provided)

.PARAMETER WorkerId
    Worker ID (default: auto-generated from hostname)

.PARAMETER OsVersion
    OS version: windows10, windows11, ubuntu2204, etc. (default: windows10)

.PARAMETER ControllerAddress
    Controller gRPC address (default: 10.200.200.1:50051)

.PARAMETER UseSSH
    Use SSH instead of WinRM (for Windows workers with OpenSSH enabled)

.EXAMPLE
    .\deploy-remote-worker.ps1 -RemoteHost 20.123.45.67 -Username "vmadmin"

.EXAMPLE
    .\deploy-remote-worker.ps1 -RemoteHost 172.21.107.15 -Username "user" -WorkerId "remote-worker-01"

.EXAMPLE
    .\deploy-remote-worker.ps1 -RemoteHost 20.123.45.67 -Username "vmadmin" -WorkerConfigPath "..\..\generated\win10-azure-worker-01.toml"

.EXAMPLE
    .\deploy-remote-worker.ps1 -RemoteHost 3.145.67.89 -Username "Administrator" -UseSSH
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$RemoteHost,

    [Parameter(Mandatory)]
    [string]$Username,

    [Parameter()]
    [securestring]$Password,

    [Parameter()]
    [string]$WorkerConfigPath,

    [Parameter()]
    [string]$WorkerId,

    [Parameter()]
    [string]$OsVersion = "win10",

    [Parameter()]
    [string]$ControllerAddress = "10.200.200.1:50051",

    [Parameter()]
    [switch]$UseSSH
)

$ErrorActionPreference = "Stop"

function Write-Success
{
    param($M) Write-Host "[OK] $M" -ForegroundColor Green
}
function Write-Info
{
    param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan
}
function Write-Warn
{
    param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow
}
function Write-Err
{
    param($M) Write-Host "[ERROR] $M" -ForegroundColor Red
}

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  Deploy Worker to Remote VM" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

Write-Info "Target: $RemoteHost"
Write-Info "User: $Username"

# Get project root and generated directory (used for both config generation and binary building)
$ScriptDir = $PSScriptRoot
$AutomationDir = Split-Path $ScriptDir -Parent
$ProjectRoot = Split-Path $AutomationDir -Parent
$GeneratedDir = Join-Path $ProjectRoot "generated"

# Auto-generate minimal config if not provided
if (-not $WorkerConfigPath -or -not (Test-Path $WorkerConfigPath))
{
    if (-not $WorkerConfigPath)
    {
        Write-Info "No config provided - generating minimal config automatically"
    }
    else
    {
        Write-Warn "Config not found: $WorkerConfigPath - generating new one"
    }

    # Create generated directory if it doesn't exist
    if (-not (Test-Path $GeneratedDir))
    {
        New-Item -ItemType Directory -Path $GeneratedDir -Force | Out-Null
        Write-Info "Created directory: $GeneratedDir"
    }

    # Auto-generate worker ID if not provided, following naming convention
    if (-not $WorkerId)
    {
        # Find next available worker number for this OS
        $existingWorkers = Get-ChildItem -Path $GeneratedDir -Filter "$OsVersion-worker-*.toml" -ErrorAction SilentlyContinue
        $maxNumber = 0
        foreach ($file in $existingWorkers)
        {
            if ($file.Name -match "$OsVersion-worker-(\d+)\.toml")
            {
                $number = [int]$matches[1]
                if ($number -gt $maxNumber)
                {
                    $maxNumber = $number
                }
            }
        }
        $nextNumber = $maxNumber + 1
        $WorkerId = "$OsVersion-worker-{0:D2}" -f $nextNumber
        Write-Info "Auto-generated Worker ID: $WorkerId (next available for $OsVersion)"
    }

    # Create config file in generated directory
    $WorkerConfigPath = Join-Path $GeneratedDir "$WorkerId.toml"

    # Minimal worker config template
    $configTemplate = @"
# Auto-generated minimal worker configuration for Phase 1 architecture
# Generated: $( Get-Date -Format "yyyy-MM-dd HH:mm:ss" )
# Phase 1: Controller dials worker (no worker->controller connections)

[worker]
worker_id = "$WorkerId"
ip_address = "$RemoteHost"
os_version = "$OsVersion"
listen_port = 50052  # Phase 1: Worker listens for controller connections

[telemetry.rededr]
base_url = "http://localhost:8081"

[storage]
artifacts_path = "C:\\AutoMutate\\artifacts"
results_path = "C:\\AutoMutate\\results"
logs_path = "C:\\AutoMutate\\logs"

[logging]
level = "INFO"
"@

    Set-Content -Path $WorkerConfigPath -Value $configTemplate -NoNewline
    Write-Success "Generated minimal config: $WorkerConfigPath"
}

Write-Info "Config: $WorkerConfigPath"

# Build worker binary
Write-Info "[1/6] Building worker agent..."
$RepoRoot = (git rev-parse --show-toplevel).Trim()
Push-Location $RepoRoot
try
{

    $BuildWin = Join-Path $RepoRoot "build.ps1"
    $BuildWin = Resolve-Path $BuildWin

    Write-Host "[deploy] REPO_ROOT = $RepoRoot"
    Write-Host "[deploy] invoking $BuildWin"

    # Execute the build script in a clean PowerShell process
    $buildOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $BuildWin | Out-String
    $exitCode = $LASTEXITCODE

    if ($exitCode -ne 0)
    {
        Write-Err "Build script failed with exit code $exitCode"
        Write-Host $buildOutput
        exit 1
    }

    Write-Success "Worker binary built successfully"
}
finally
{
    Pop-Location
}

$workerBinary = Join-Path $RepoRoot "target-win\release\worker-agent.exe"
if (-not (Test-Path $workerBinary))
{
    Write-Err "Worker binary not found: $workerBinary"
    Write-Info "Expected location after 'cargo build --release'"
    exit 1
}

# Initialize deployment success flag
$deploymentSucceeded = $false
$deploymentMethod = "Unknown"

# Get credentials only for WMI (SSH will use its own auth)
if (-not $UseSSH)
{
    if (-not $Password)
    {
        $credential = Get-Credential -UserName $Username -Message "Enter password for $($Username)@$($RemoteHost)"
    }
    else
    {
        $credential = New-Object System.Management.Automation.PSCredential($Username, $Password)
    }
}

if ( -not $UseSSH -and $false) # Deactivate this check as slow down too much and not able to activate it on remote
{
    # Windows remote deployment via WMI/RPC (no WinRM required)
    Write-Info "[2/6] Connecting via WMI/RPC..."

    # Test WMI connectivity
    $wmiConnected = $false
    try
    {
        $null = Get-WmiObject -Class Win32_OperatingSystem -ComputerName $RemoteHost -Credential $credential -ErrorAction Stop
        Write-Success "WMI/RPC connection established"
        $wmiConnected = $true
    }
    catch
    {
        Write-Warn "Failed to connect via WMI/RPC: $_"
        Write-Info ""
        Write-Info "Possible causes:"
        Write-Info "  1. Firewall blocking RPC ports (135 + ephemeral). On remote machine, run:"
        Write-Info "     Enable-NetFirewallRule -DisplayGroup 'Windows Management Instrumentation (WMI)'"
        Write-Info "  2. Remote Registry service not running. On remote machine, run:"
        Write-Info "     Set-Service RemoteRegistry -StartupType Automatic; Start-Service RemoteRegistry"
        Write-Info "  3. Wrong credentials. Verify username/password are correct."
        Write-Info "  4. User account needs to be in Administrators group on remote machine."
        Write-Info ""
        Write-Info "Falling back to SSH deployment..."
    }

    if ($wmiConnected)
    {
        try
        {
            # Create remote directories using WMI
            Write-Info "[3/6] Creating directories on remote VM..."
            $directories = @(
                "C:\AutoMutate",
                "C:\AutoMutate\artifacts",
                "C:\AutoMutate\logs",
                "C:\AutoMutate\traces",
                "C:\AutoMutate\coverage"
            )

            foreach ($dir in $directories)
            {
                $result = Invoke-WmiMethod -Class Win32_Process -Name Create `
                    -ArgumentList "cmd.exe /c `"if not exist `"$dir`" mkdir `"$dir`"`"" `
                    -ComputerName $RemoteHost -Credential $credential -ErrorAction Stop

                if ($result.ReturnValue -ne 0)
                {
                    Write-Err "Failed to create directory $dir (return code: $( $result.ReturnValue ))"
                    exit 1
                }
            }
            Write-Success "Directories created"

            # Copy worker binary via SMB admin share
            Write-Info "[4/6] Copying worker binary ($(((Get-Item $workerBinary).Length / 1MB).ToString('0.0') ) MB)..."
            $remotePath = "\\$RemoteHost\C$\AutoMutate\worker-agent.exe"

            # Create PSDrive for credential-based copy
            $driveName = "DeployDrive$( Get-Random )"
            try
            {
                New-PSDrive -Name $driveName -PSProvider FileSystem -Root "\\$RemoteHost\C$" -Credential $credential -ErrorAction Stop | Out-Null
                Copy-Item -Path $workerBinary -Destination "${driveName}:\AutoMutate\worker-agent.exe" -Force -ErrorAction Stop
                Write-Success "Worker binary deployed"
            }
            finally
            {
                Remove-PSDrive -Name $driveName -Force -ErrorAction SilentlyContinue
            }

            # Copy worker config via SMB admin share
            Write-Info "[5/6] Copying worker config..."
            try
            {
                New-PSDrive -Name $driveName -PSProvider FileSystem -Root "\\$RemoteHost\C$" -Credential $credential -ErrorAction Stop | Out-Null
                Copy-Item -Path $WorkerConfigPath -Destination "${driveName}:\AutoMutate\worker.toml" -Force -ErrorAction Stop
                Write-Success "Worker config deployed"
            }
            finally
            {
                Remove-PSDrive -Name $driveName -Force -ErrorAction SilentlyContinue
            }

            # Configure firewall using WMI
            Write-Info "[6/6] Configuring firewall and starting worker..."

            # Add firewall rule
            $firewallCmd = 'netsh advfirewall firewall add rule name="AutoMutate Worker Agent" dir=in action=allow protocol=TCP localport=50052'
            $result = Invoke-WmiMethod -Class Win32_Process -Name Create `
                -ArgumentList "cmd.exe /c `"$firewallCmd 2>nul`"" `
                -ComputerName $RemoteHost -Credential $credential -ErrorAction SilentlyContinue

            # Start worker agent in background using WMI
            $startCmd = 'cmd.exe /c "start /B C:\AutoMutate\worker-agent.exe > C:\AutoMutate\logs\worker.log 2>&1"'
            $result = Invoke-WmiMethod -Class Win32_Process -Name Create `
                -ArgumentList $startCmd `
                -ComputerName $RemoteHost -Credential $credential -ErrorAction Stop

            if ($result.ReturnValue -eq 0)
            {
                Write-Success "Worker agent started on remote VM (PID: $( $result.ProcessId ))"
                $deploymentSucceeded = $true
                $deploymentMethod = "WMI/RPC"
            }
            else
            {
                Write-Err "Failed to start worker agent (return code: $( $result.ReturnValue ))"
                exit 1
            }

        }
        catch
        {
            Write-Err "WMI deployment failed: $_"
            exit 1
        }
    }
}

# Try SSH if WMI failed or if -UseSSH was specified
if (-not $deploymentSucceeded)
{
    Write-Info "[2/6] Setting up SSH authentication..."

    # Set up worker-specific SSH key directory
    # Note: $ProjectRoot is actually automation/, not the real project root
    $sshKeysDir = Join-Path $ProjectRoot "ssh-keys"
    $workerKeyDir = Join-Path $sshKeysDir $WorkerId
    $privateKeyPath = Join-Path $workerKeyDir "id_ed25519"
    $publicKeyPath = Join-Path $workerKeyDir "id_ed25519.pub"

    # Check if SSH key already exists for this worker
    if (-not (Test-Path $privateKeyPath))
    {
        Write-Info "Generating new SSH key for worker: $WorkerId"
        New-Item -ItemType Directory -Path $workerKeyDir -Force | Out-Null

        # Generate SSH key (no passphrase for automation)
        ssh-keygen -t ed25519 -f $privateKeyPath -N '""' -C "automutate-$WorkerId" -q
        if ($LASTEXITCODE -ne 0)
        {
            Write-Err "Failed to generate SSH key"
            exit 1
        }
        Write-Success "SSH key generated: $workerKeyDir"

        # Test basic SSH connectivity first
        Write-Info "Testing SSH password authentication to $RemoteHost..."
        Write-Info "Please enter your password when prompted (you may be asked multiple times)"
        $sshTarget = "$Username@$RemoteHost"
        $testResult = ssh -o ConnectTimeout=10 -o StrictHostKeyChecking=no -o NumberOfPasswordPrompts=1 $sshTarget "echo 'SSH_OK'" 2>&1
        if ($LASTEXITCODE -ne 0 -or $testResult -notmatch "SSH_OK")
        {
            Write-Err "SSH password authentication test failed"
            Write-Host "Error: $testResult" -ForegroundColor Red
            Write-Info ""
            Write-Info "Please verify:"
            Write-Info "  1. Username is correct: $Username"
            Write-Info "  2. Password is correct for user: $Username"
            Write-Info "  3. SSH server is running on remote machine: $RemoteHost"
            Write-Info "  4. Password authentication is enabled in SSH config"
            Write-Info "     Check: C:\ProgramData\ssh\sshd_config on remote machine"
            Write-Info "     Ensure: PasswordAuthentication yes"
            exit 1
        }
        Write-Success "SSH password authentication working"

        # Copy public key to remote machine (requires password prompts)
        Write-Info "Installing SSH public key on remote machine..."

        # Step 1: Copy public key file to remote temp location (password prompt 1)
        Write-Host "Copying public key file (password 1/2):" -ForegroundColor Yellow
        $remoteTempKey = "C:/Windows/Temp/automutate_key_$WorkerId.pub"
        scp -o ConnectTimeout=10 -o StrictHostKeyChecking=no "$publicKeyPath" "${sshTarget}:$remoteTempKey" 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0)
        {
            Write-Err "Failed to copy public key file to remote machine"
            exit 1
        }

        # Step 2: Install the key with proper permissions (password prompt 2)
        Write-Host "Installing key with permissions (password 2/2):" -ForegroundColor Yellow

        # Create PowerShell script to install the key (avoids quoting issues by using Base64 encoding)
        $installScript = @"
New-Item -ItemType Directory -Path `$env:USERPROFILE\.ssh -Force -ErrorAction SilentlyContinue | Out-Null
Get-Content '$remoteTempKey' | Add-Content -Path `$env:USERPROFILE\.ssh\authorized_keys -Force
icacls `$env:USERPROFILE\.ssh\authorized_keys /inheritance:r /grant "`${env:USERNAME}:(F)" | Out-Null
icacls `$env:USERPROFILE\.ssh /inheritance:r /grant "`${env:USERNAME}:(F)" | Out-Null
Remove-Item '$remoteTempKey' -Force -ErrorAction SilentlyContinue
"@

        # Encode script as Base64 to avoid all quoting issues
        $scriptBytes = [System.Text.Encoding]::Unicode.GetBytes($installScript)
        $encodedScript = [Convert]::ToBase64String($scriptBytes)

        # Execute with EncodedCommand parameter
        $installCmd = "powershell.exe -NoProfile -NonInteractive -EncodedCommand $encodedScript"
        $sshOutput = ssh -o ConnectTimeout=10 -o StrictHostKeyChecking=no $sshTarget $installCmd 2>&1
        $sshExitCode = $LASTEXITCODE

        if ($sshExitCode -ne 0)
        {
            Write-Err "Failed to install SSH public key on remote machine"
            Write-Info ""
            Write-Host "Error details:" -ForegroundColor Red
            Write-Host ($sshOutput | Out-String) -ForegroundColor Red
            Write-Info ""
            $publicKey = (Get-Content $publicKeyPath -Raw).Trim()
            Write-Info "Manual setup required on remote machine ($RemoteHost):"
            Write-Info "  1. Create directory: mkdir `$env:USERPROFILE\.ssh"
            Write-Info "  2. Add this key to: `$env:USERPROFILE\.ssh\authorized_keys"
            Write-Info "     Key: $publicKey"
            Write-Info "  3. Set permissions: icacls `$env:USERPROFILE\.ssh\authorized_keys /inheritance:r /grant `"`$env:USERNAME:(F)`""
            exit 1
        }
        Write-Success "SSH key installed in user profile"

        # Special handling for Administrator users on Windows OpenSSH
        Write-Info "Checking if Administrator-specific setup is needed..."
        $checkAdminScript = @"
`$adminKeyFile = 'C:\ProgramData\ssh\administrators_authorized_keys'
`$userKeyFile  = "`$env:USERPROFILE\.ssh\authorized_keys"

if (Test-Path `$userKeyFile) {
    New-Item -ItemType Directory -Path 'C:\ProgramData\ssh' -Force | Out-Null
    Get-Content `$userKeyFile | Set-Content `$adminKeyFile -Force

    cmd.exe /c 'icacls "C:\ProgramData\ssh\administrators_authorized_keys" /inheritance:r /grant "SYSTEM:(F)" /grant "Administrators:(F)"'
    Write-Host 'ADMIN_KEY_INSTALLED'
} else {
    Write-Host 'NO_USER_KEY'
}
"@

        $adminCheckBytes = [System.Text.Encoding]::Unicode.GetBytes($checkAdminScript)
        $adminCheckEncoded = [Convert]::ToBase64String($adminCheckBytes)
        $adminCheckCmd = "powershell.exe -NoProfile -NonInteractive -EncodedCommand $adminCheckEncoded"

        $adminCheckResult = ssh -o ConnectTimeout=10 -o StrictHostKeyChecking=no $sshTarget $adminCheckCmd 2>&1
        if ($adminCheckResult -match "ADMIN_KEY_INSTALLED")
        {
            Write-Success "Administrator-specific key file created"
        }
        elseif ($adminCheckResult -match "NOT_ADMIN")
        {
            Write-Info "User is not an administrator, standard key location used"
        }

        Write-Success "SSH key setup complete"
        Write-Info "Future deployments to this worker will not require password!"
    }
    else
    {
        Write-Success "Using existing SSH key: $privateKeyPath"
        Write-Info "No password required for this deployment"
    }

    # Common SSH options (use worker-specific key)
    $sshOpts = @(
        "-i", $privateKeyPath
        "-o", "ConnectTimeout=10"
        "-o", "StrictHostKeyChecking=no"
        "-o", "IdentitiesOnly=yes"
        "-o", "PreferredAuthentications=publickey"
        "-o", "PasswordAuthentication=no"
        "-o", "KbdInteractiveAuthentication=no"
        "-o", "NumberOfPasswordPrompts=0"
    )

    Write-Info "[2.5/6] Ensuring C:\AutoMutate exists on remote..."
    $sshTarget = "$Username@$RemoteHost"

    $mkAutoMutateCmd = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "New-Item -ItemType Directory -Force -Path ''C:\AutoMutate''" >NUL 2>NUL'


    ssh @sshOpts $sshTarget $mkAutoMutateCmd
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Failed to create C:\AutoMutate on remote host"
        exit 1
    }
    Write-Success "Remote directory ensured: C:\AutoMutate"

    # Copy worker binary (using SSH key - no password needed)
    Write-Info "[3/6] Copying worker binary ($(((Get-Item $workerBinary).Length / 1MB).ToString('0.0')) MB)..."
    $remoteBinaryPath = "C:/AutoMutate/worker-agent.exe"
    $sshTarget = "$Username@$RemoteHost"
    scp @sshOpts "$workerBinary" "${sshTarget}:$remoteBinaryPath"
    if ($LASTEXITCODE -ne 0)
    {
        Write-Err "Failed to copy worker binary via SCP"
        exit 1
    }
    Write-Success "Worker binary copied"

    # Copy worker config (using SSH key - no password needed)
    Write-Info "[4/6] Copying worker config..."
    $remoteConfigPath = "C:/AutoMutate/worker.toml"
    scp @sshOpts "$WorkerConfigPath" "${sshTarget}:$remoteConfigPath"
    if ($LASTEXITCODE -ne 0)
    {
        Write-Err "Failed to copy worker config via SCP"
        exit 1
    }
    Write-Success "Worker config copied"

    # --- Copy worker-init.ps1 and telemetry zip, then run worker-init.ps1 as Admin on remote ---

    $WorkerInitLocal = Join-Path $RepoRoot "automation\scripts\worker-init.ps1"
    $RedEdrLocal     = Join-Path $RepoRoot "telemetry\RedEdr_no_stack_trace_dll.zip"

    $RemoteAutoMutateRoot = "C:/AutoMutate"
    $RemoteWorkerInit     = "$RemoteAutoMutateRoot/worker-init.ps1"
    $RemoteRedEdrDir      = "$RemoteAutoMutateRoot/build/telemetry"
    $RemoteRedEdrZip      = "$RemoteRedEdrDir/RedEdr.zip"

    Write-Info "[X/6] Copying worker-init.ps1..."
    scp @sshOpts "$WorkerInitLocal" "${sshTarget}:$RemoteWorkerInit"
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Failed to copy worker-init.ps1 via SCP"
        exit 1
    }
    Write-Success "worker-init.ps1 copied to $RemoteWorkerInit"

    Write-Info "[X/6] Ensuring telemetry directory exists..."

    $RemoteRedEdrDirWin = 'C:\AutoMutate\build\telemetry'

    # No pipe, so cmd.exe cannot misinterpret anything.
    # [void](...) discards output inside PowerShell.
    $RemoteMkDirCmd = @(
        "powershell.exe",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy", "Bypass",
        "-Command",
        """[void](New-Item -ItemType Directory -Force -Path '$RemoteRedEdrDirWin')"""
    ) -join ' '

    ssh @sshOpts $sshTarget $RemoteMkDirCmd
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Failed to create remote directory: $RemoteRedEdrDirWin"
        exit 1
    }

    Write-Success "Remote directory ensured: $RemoteRedEdrDirWin"

    Write-Info "[X/6] Copying RedEdr telemetry zip..."
    scp @sshOpts "$RedEdrLocal" "${sshTarget}:$RemoteRedEdrZip"
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Failed to copy RedEdr.zip via SCP"
        exit 1
    }
    Write-Success "Telemetry zip copied to $RemoteRedEdrZip"

    Write-Info "[X/6] Executing worker-init.ps1 with admin privileges..."
    # This launches an elevated PowerShell on the remote host via scheduled task (works non-interactively).
    $remoteCmd = @"
`$Action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -ExecutionPolicy Bypass -File `"$RemoteWorkerInit`"'
`$Principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
`$Task = New-ScheduledTask -Action `$Action -Principal `$Principal
Register-ScheduledTask -TaskName 'AutoMutate-WorkerInit' -InputObject `$Task -Force | Out-Null
Start-ScheduledTask -TaskName 'AutoMutate-WorkerInit'
Start-Sleep -Seconds 2
# Optional: wait for completion and propagate exit code
`$state = (Get-ScheduledTask -TaskName 'AutoMutate-WorkerInit').State
while (`$state -eq 'Running') { Start-Sleep -Seconds 2; `$state = (Get-ScheduledTask -TaskName 'AutoMutate-WorkerInit').State }
`$last = (Get-ScheduledTaskInfo -TaskName 'AutoMutate-WorkerInit').LastTaskResult
Unregister-ScheduledTask -TaskName 'AutoMutate-WorkerInit' -Confirm:`$false | Out-Null
exit `$last
"@

    ssh @sshOpts $sshTarget "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command `"$remoteCmd`""
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Remote worker-init.ps1 (elevated) failed with exit code $LASTEXITCODE"
        exit 1
    }
    Write-Success "worker-init.ps1 executed successfully (elevated)"

    # Create directories and start worker (using SSH key - no password needed)
    Write-Info "[5/6] Setting up directories and starting worker..."
    $setupScript = 'cmd.exe /c "mkdir C:\AutoMutate\artifacts 2>nul & mkdir C:\AutoMutate\logs 2>nul & mkdir C:\AutoMutate\traces 2>nul & mkdir C:\AutoMutate\coverage 2>nul & start /B C:\AutoMutate\worker-agent.exe > C:\AutoMutate\logs\worker.log 2>&1"'
    ssh @sshOpts $sshTarget $setupScript

    if ($LASTEXITCODE -eq 0)
    {
        Write-Success "Worker agent started"
        $deploymentSucceeded = $true
        $deploymentMethod = "SSH"
    }
    else
    {
        Write-Err "Failed to start worker agent via SSH"
        exit 1
    }

    Write-Info "[6/6] Deployment via SSH complete"
}

# Verify deployment succeeded
if (-not $deploymentSucceeded)
{
    Write-Err "Deployment failed - neither WMI nor SSH method succeeded"
    Write-Info ""
    Write-Info "Troubleshooting:"
    Write-Info "  - For WMI: Ensure RPC/DCOM ports are open and Remote Registry service is running"
    Write-Info "  - For SSH: Ensure OpenSSH server is installed and running on remote machine"
    Write-Info "  - Verify credentials and network connectivity"
    exit 1
}

Write-Host "`n========================================" -ForegroundColor Green
Write-Host "  Deployment Complete!" -ForegroundColor Green
Write-Host "========================================`n" -ForegroundColor Green

Write-Info "Worker deployed to: $RemoteHost"
Write-Info "Deployment method: $deploymentMethod"
if ($WorkerId)
{
    Write-Info "Worker ID: $WorkerId"
}
Write-Info "Worker will self-register with controller on startup"
Write-Info ""
Write-Info "Next steps:"
Write-Info "  1. Verify worker registered:"
Write-Info "     .\scripts\workers\list-workers.ps1"
Write-Info ""
Write-Info "  2. Check worker logs (if needed):"
$sshLogCmd = "ssh $Username@$RemoteHost `"powershell Get-Content C:\AutoMutate\logs\worker.log -Tail 20`""
if ($deploymentMethod -eq "SSH")
{
    Write-Info "     $sshLogCmd"
}
else
{
    Write-Info "     Via SMB share: type \\$RemoteHost\C$\AutoMutate\logs\worker.log"
    Write-Info "     Via RDP: Open C:\AutoMutate\logs\worker.log on remote machine"
    Write-Info "     Via SSH: $sshLogCmd"
}
Write-Info ""
Write-Info "Tip: Deploy more workers without pre-generating configs:"
Write-Info "  .\deploy-remote-worker.ps1 -RemoteHost <IP> -Username <user> -WorkerId <custom-id>"
Write-Info ""
