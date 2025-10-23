<#
.SYNOPSIS
    Comprehensive health check for AutoMutate++ environment

.PARAMETER ConfigPath
    Path to config.yaml (default: ..\config.yaml)

.PARAMETER Quick
    Skip long-running checks (snapshot validation, disk space)

.EXAMPLE
    .\validate-environment.ps1

.EXAMPLE
    .\validate-environment.ps1 -Quick
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$ConfigPath = "..\config.yaml",

    [Parameter()]
    [switch]$Quick
)

$ErrorActionPreference = "Stop"
function Write-Pass { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Fail { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Section { param($M) Write-Host "`n==> $M" -ForegroundColor Magenta }

# Import shared config module
Import-Module "$PSScriptRoot\modules\AutoMutateConfig.psm1" -Force

$FailCount = 0

Write-Host @"

+===============================================================+
|         AutoMutate++ Environment Validation                   |
+===============================================================+

"@ -ForegroundColor Cyan

# Parse config using shared module
$config = Read-AutoMutateConfig -ConfigPath $ConfigPath

# Section 1: Windows Features
Write-Section "Windows Features"

$features = @("Microsoft-Hyper-V-All", "Microsoft-Windows-Subsystem-Linux", "VirtualMachinePlatform")
foreach ($feature in $features) {
    $state = (Get-WindowsOptionalFeature -Online -FeatureName $feature -ErrorAction SilentlyContinue).State
    if ($state -eq "Enabled") {
        Write-Pass "$feature enabled"
    } else {
        Write-Fail "$feature NOT enabled"
        $FailCount++
    }
}

# Section 2: WSL2
Write-Section "WSL2"

# Simple test: try to run a command in Ubuntu
try {
    $ubuntuTest = wsl -d Ubuntu bash -c "echo ok" 2>&1
    if ($ubuntuTest -match "ok") {
        Write-Pass "WSL2 Ubuntu installed and running"
    } else {
        Write-Fail "WSL2 Ubuntu NOT responding"
        $FailCount++
    }
} catch {
    Write-Fail "WSL2 Ubuntu NOT installed or not running"
    $FailCount++
}

# Section 3: Network
Write-Section "Network Configuration"

$SwitchName = $config.network.switch_name
$HostIP = $config.network.host_ip

$switch = Get-VMSwitch -Name $SwitchName -ErrorAction SilentlyContinue
if ($switch) {
    Write-Pass "Switch '$SwitchName' exists"

    if ($switch.SwitchType -eq "Internal") {
        Write-Pass "Switch type: Internal"
    } else {
        Write-Fail "Switch type: $($switch.SwitchType) (expected Internal)"
        $FailCount++
    }
} else {
    Write-Fail "Switch '$SwitchName' NOT found"
    $FailCount++
}

$adapter = Get-NetAdapter | Where-Object { $_.Name -like "*$SwitchName*" } | Select-Object -First 1
if ($adapter) {
    Write-Pass "vEthernet adapter found"

    $ipConfig = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue
    if ($ipConfig -and $ipConfig.IPAddress -eq $HostIP) {
        Write-Pass "Host IP configured: $HostIP"
    } else {
        Write-Fail "Host IP NOT configured correctly (expected $HostIP)"
        $FailCount++
    }
} else {
    Write-Fail "vEthernet adapter NOT found"
    $FailCount++
}

# Section 4: Controller Services
Write-Section "Controller Services"

# Elasticsearch
$esPort = $config.controller.elasticsearch_port
try {
    $response = Invoke-WebRequest -Uri "http://localhost:$esPort" -TimeoutSec 5 -ErrorAction Stop
    if ($response.StatusCode -eq 200) {
        Write-Pass "Elasticsearch responding (http://localhost:$esPort)"
    } else {
        Write-Fail "Elasticsearch unexpected status: $($response.StatusCode)"
        $FailCount++
    }
} catch {
    Write-Fail "Elasticsearch NOT responding"
    $FailCount++
}

# Kibana
$kibanaPort = $config.controller.kibana_port
try {
    $response = Invoke-WebRequest -Uri "http://localhost:$kibanaPort" -TimeoutSec 5 -ErrorAction Stop
    if ($response.StatusCode -eq 200) {
        Write-Pass "Kibana responding (http://localhost:$kibanaPort)"
    } else {
        Write-Fail "Kibana unexpected status: $($response.StatusCode)"
        $FailCount++
    }
} catch {
    Write-Fail "Kibana NOT responding"
    $FailCount++
}

# Section 5: Worker VMs
Write-Section "Worker VMs"

# Get workers using shared module
$Workers = Get-AutoMutateWorkers -ConfigPath $ConfigPath

Write-Info "Found $($Workers.Count) worker(s) in config"

foreach ($worker in $Workers) {
    Write-Host ""
    Write-Host "  Worker: $($worker.Name)" -ForegroundColor Yellow

    $vm = Get-VM -Name $worker.Name -ErrorAction SilentlyContinue
    if ($vm) {
        Write-Pass "  VM exists"

        if ($vm.State -eq "Running") {
            Write-Pass "  VM running"
        } else {
            Write-Fail "  VM NOT running (state: $($vm.State))"
            $FailCount++
        }

        # Check Generation
        if ($vm.Generation -eq 2) {
            Write-Pass "  Generation 2"
        } else {
            Write-Fail "  Generation $($vm.Generation) (expected 2)"
            $FailCount++
        }

        # Check TPM
        $tpmEnabled = (Get-VMSecurity -VMName $worker.Name).TpmEnabled
        if ($tpmEnabled) {
            Write-Pass "  TPM enabled"
        } else {
            Write-Fail "  TPM NOT enabled"
            $FailCount++
        }

        # Check baseline checkpoint
        if (-not $Quick) {
            $baseline = Get-VMSnapshot -VMName $worker.Name -Name "$($worker.Name)-baseline" -ErrorAction SilentlyContinue
            if ($baseline) {
                Write-Pass "  Baseline checkpoint exists"
            } else {
                Write-Fail "  Baseline checkpoint NOT found"
                $FailCount++
            }
        }

    } else {
        Write-Fail "  VM NOT found"
        $FailCount++
    }
}

# Section 6: Storage
if (-not $Quick) {
    Write-Section "Storage"

    $VhdRoot = $config.storage.vhd_root
    if (Test-Path $VhdRoot) {
        Write-Pass "VHD root exists: $VhdRoot"

        $drive = Split-Path $VhdRoot -Qualifier
        $disk = Get-PSDrive $drive.TrimEnd(':')
        $freeGB = [math]::Round($disk.Free / 1GB, 2)
        $usedGB = [math]::Round($disk.Used / 1GB, 2)
        $totalGB = [math]::Round(($disk.Free + $disk.Used) / 1GB, 2)

        Write-Info "  Drive $drive`: $freeGB GB free / $totalGB GB total"

        if ($freeGB -lt 50) {
            Write-Fail "  Low disk space (< 50 GB free)"
            $FailCount++
        } else {
            Write-Pass "  Disk space sufficient"
        }
    } else {
        Write-Fail "VHD root NOT found: $VhdRoot"
        $FailCount++
    }
}

# Section 7: Firewall Rules
Write-Section "Firewall Rules"

$ports = @($config.controller.grpc_port, $config.controller.elasticsearch_port, $config.controller.kibana_port)
foreach ($port in $ports) {
    $ruleName = "AutoMutate-Allow-$port"
    $rule = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
    if ($rule) {
        Write-Pass "Firewall rule: $ruleName"
    } else {
        Write-Fail "Firewall rule NOT found: $ruleName"
        $FailCount++
    }
}

# Check egress filtering status (informational only, not a failure)
$egressBlockRule = Get-NetFirewallRule -DisplayName "AutoMutate-Egress-Block-Internet" -ErrorAction SilentlyContinue
if ($egressBlockRule -and $egressBlockRule.Enabled -eq 'True') {
    Write-Info "Egress filtering: ENABLED (whitelist mode)"

    $whitelistRules = Get-NetFirewallRule -DisplayName "AutoMutate-Egress-Allow-*" -ErrorAction SilentlyContinue
    if ($whitelistRules) {
        Write-Info "  Whitelist rules: $($whitelistRules.Count)"
    }
} else {
    Write-Info "Egress filtering: DISABLED (unrestricted)"
}

# Check VM-to-VM isolation status (informational only, not a failure)
$vmIsolationInbound = Get-NetFirewallRule -DisplayName "AutoMutate-VM-Isolation-Block-Inbound" -ErrorAction SilentlyContinue
$vmIsolationOutbound = Get-NetFirewallRule -DisplayName "AutoMutate-VM-Isolation-Block-Outbound" -ErrorAction SilentlyContinue
if ($vmIsolationInbound -and $vmIsolationOutbound) {
    Write-Info "VM-to-VM isolation: ENABLED (VMs cannot communicate with each other)"
} else {
    Write-Info "VM-to-VM isolation: DISABLED (VMs can communicate)"
}

# Section 8: Port Forwarding
Write-Section "Port Forwarding"

foreach ($port in $ports) {
    $proxy = netsh interface portproxy show v4tov4 | Select-String "$HostIP.*$port"
    if ($proxy) {
        Write-Pass "Port proxy: ${HostIP}:${port} -> 127.0.0.1:${port}"
    } else {
        Write-Fail "Port proxy NOT configured for port $port"
        $FailCount++
    }
}

# Summary
Write-Host ""
Write-Host "="*70 -ForegroundColor Cyan
Write-Host "VALIDATION SUMMARY" -ForegroundColor Magenta
Write-Host "="*70 -ForegroundColor Cyan

if ($FailCount -eq 0) {
    Write-Host ""
    Write-Pass "All checks passed! Environment ready."
    Write-Host ""
    Write-Info "Next steps:"
    Write-Info "  1. Start environment: .\start-environment.ps1"
    Write-Info "  2. Deploy artifacts and run experiments"
    Write-Host ""
    exit 0
} else {
    Write-Host ""
    Write-Fail "$FailCount check(s) failed"
    Write-Host ""
    Write-Info "Troubleshooting:"
    Write-Info "  1. Re-run setup: ..\setup-all.ps1"
    Write-Info "  2. Check logs: Get-Content ..\logs\setup-*.log"
    Write-Host ""
    exit 1
}
