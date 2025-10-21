<#
.SYNOPSIS
    Fix WSL2 internet access broken by Hyper-V NAT conflict

.DESCRIPTION
    Windows only supports ONE NetNat at a time. If IsolationNAT was created,
    it breaks WSL2's internet access. This script removes the conflicting NAT
    and restores WSL2 connectivity.

.EXAMPLE
    .\fix-wsl-nat-conflict.ps1
#>

$ErrorActionPreference = "Stop"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

Write-Host "`n+================================================================+" -ForegroundColor Yellow
Write-Host "|          Fix WSL2 NAT Conflict                                 |" -ForegroundColor Yellow
Write-Host "+================================================================+" -ForegroundColor Yellow
Write-Host ""

# Check current NAT configuration
Write-Info "Checking current NAT configuration..."
$allNats = Get-NetNat -ErrorAction SilentlyContinue

if ($allNats) {
    Write-Info "Found $($allNats.Count) NAT(s):"
    foreach ($nat in $allNats) {
        Write-Host "  - $($nat.Name): $($nat.InternalIPInterfaceAddressPrefix)" -ForegroundColor White
    }
} else {
    Write-Info "No NATs configured"
}

# Identify the problem
$isolationNat = $allNats | Where-Object { $_.Name -eq "IsolationNAT" }
$wslNat = $allNats | Where-Object { $_.InternalIPInterfaceAddressPrefix -like "172.*" }

if ($isolationNat -and $wslNat) {
    Write-Warn "CONFLICT DETECTED: Both IsolationNAT and WSL NAT exist!"
    Write-Info "This breaks WSL2 internet access (Windows only supports 1 NetNat)"
} elseif ($isolationNat -and -not $wslNat) {
    Write-Warn "IsolationNAT exists but no WSL NAT found"
    Write-Info "WSL may have lost its NAT due to conflict"
}

# Test WSL connectivity before fix
Write-Info "Testing WSL connectivity BEFORE fix..."
$beforeTest = wsl -- timeout 2 ping -c 1 8.8.8.8 2>$null
$wslWorksBefore = ($LASTEXITCODE -eq 0)

if ($wslWorksBefore) {
    Write-Success "WSL already has internet access - no fix needed"
    exit 0
} else {
    Write-Warn "WSL cannot reach internet (as expected)"
}

# Apply fix: Remove IsolationNAT
if ($isolationNat) {
    Write-Info "Removing IsolationNAT to restore WSL2 connectivity..."
    Remove-NetNat -Name "IsolationNAT" -Confirm:$false
    Write-Success "Removed IsolationNAT"
}

# Restart WSL to recreate its NAT
Write-Info "Restarting WSL to recreate default NAT..."
wsl --shutdown
Start-Sleep -Seconds 5

# Test WSL connectivity after fix
Write-Info "Testing WSL connectivity AFTER fix..."
$afterTest = wsl -- timeout 3 ping -c 1 8.8.8.8 2>$null
$wslWorksAfter = ($LASTEXITCODE -eq 0)

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Results                                               |" -ForegroundColor Cyan
Write-Host "+================================================================+" -ForegroundColor Cyan

if ($wslWorksAfter) {
    Write-Success "WSL2 internet access RESTORED!"
    Write-Host ""
    Write-Info "Next steps:"
    Write-Host "  1. Re-run 01-host-setup.ps1 (it will now use IP forwarding instead of NAT)" -ForegroundColor White
    Write-Host "  2. The new approach preserves WSL NAT and shares it with VMs" -ForegroundColor White
} else {
    Write-Warn "WSL still cannot reach internet"
    Write-Info "Additional troubleshooting needed:"
    Write-Info "  1. Check Windows firewall"
    Write-Info "  2. Run: wsl -- ip route show"
    Write-Info "  3. Run: wsl -- cat /etc/resolv.conf"
}

Write-Host ""
