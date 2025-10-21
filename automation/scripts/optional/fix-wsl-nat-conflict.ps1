<#
.SYNOPSIS
    Fix WSL2 internet access broken by Hyper-V NAT conflict

.DESCRIPTION
    Windows only supports ONE NetNat at a time. If IsolationNAT was created,
    it breaks WSL2's internet access. This script removes the conflicting NAT
    and restores WSL2 connectivity.

    NOTE: As of the updated setup, IsolationNAT should NOT be created at all.
    This script is a recovery tool for systems where it was incorrectly created.

.EXAMPLE
    .\fix-wsl-nat-conflict.ps1
#>

$ErrorActionPreference = "Stop"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }
function Write-Err { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

Write-Host "`n+================================================================+" -ForegroundColor Yellow
Write-Host "|          Fix WSL2 NAT Conflict (Recovery Tool)                 |" -ForegroundColor Yellow
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

if (-not $isolationNat) {
    Write-Success "No IsolationNAT found - system is correctly configured"
    Write-Info "The updated 01-host-setup.ps1 uses IP forwarding (no conflicting NAT)"

    if ($wslNat) {
        Write-Success "WSL NAT is active: $($wslNat.InternalIPInterfaceAddressPrefix)"
    }

    Write-Host ""
    Write-Info "Testing WSL connectivity..."
    $wslTest = wsl -- timeout 2 ping -c 1 8.8.8.8 2>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Success "WSL internet access works!"
        exit 0
    } else {
        Write-Err "WSL cannot reach internet - NAT is not the issue"
        Write-Info "Try: .\fix-wsl-dns.ps1 (DNS configuration issue)"
        exit 1
    }
}

# IsolationNAT exists - this is the problem
Write-Warn "CONFLICT DETECTED: IsolationNAT exists (should not be present)"
Write-Info "This NAT was created by an older version of 01-host-setup.ps1"

if ($wslNat) {
    Write-Info "WSL NAT also exists: $($wslNat.InternalIPInterfaceAddressPrefix)"
    Write-Warn "Multiple NATs cause routing conflicts (Windows limitation)"
} else {
    Write-Warn "WSL NAT missing - may have been overridden by IsolationNAT"
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
    Write-Host "  1. Re-run 01-host-setup.ps1 (updated version uses IP forwarding)" -ForegroundColor White
    Write-Host "  2. IP forwarding preserves WSL NAT (no conflicts)" -ForegroundColor White
    Write-Host "  3. VMs will route through host's default gateway" -ForegroundColor White
} else {
    Write-Warn "WSL still cannot reach internet"
    Write-Info "Additional troubleshooting needed:"
    Write-Info "  1. Check Windows firewall (may block WSL vEthernet)"
    Write-Info "  2. Run: wsl -- ip route show (verify default via 172.x.x.1)"
    Write-Info "  3. Run: wsl -- cat /etc/resolv.conf (should NOT be 10.255.255.254)"
    Write-Info "  4. Try: .\fix-wsl-dns.ps1 (DNS-specific fixes)"
}

Write-Host ""
