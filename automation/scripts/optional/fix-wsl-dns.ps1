<#
.SYNOPSIS
    Fix WSL2 DNS resolution issues

.DESCRIPTION
    WSL2 sometimes generates incorrect DNS servers (10.255.255.254)
    This script fixes DNS by creating /etc/wsl.conf and /etc/resolv.conf

.EXAMPLE
    .\fix-wsl-dns.ps1
#>

$ErrorActionPreference = "Stop"

function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

Write-Info "Fixing WSL2 DNS configuration..."

# Test current connectivity
Write-Info "Testing current DNS resolution..."
$canPing = wsl -- ping -c 1 -W 1 8.8.8.8 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Success "IP connectivity works (can reach 8.8.8.8)"
} else {
    Write-Warn "Cannot reach 8.8.8.8 - may be a routing issue"
}

$canResolve = wsl -- ping -c 1 -W 1 google.com 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Success "DNS already works! No fix needed."
    exit 0
} else {
    Write-Warn "DNS resolution failed - applying fix..."
}

# Fix 1: Create /etc/wsl.conf to disable auto-generated resolv.conf
Write-Info "Step 1: Configuring /etc/wsl.conf..."
wsl -- bash -c @"
sudo mkdir -p /etc
sudo tee /etc/wsl.conf > /dev/null << 'EOF'
[network]
generateResolvConf = false

[boot]
systemd = false
EOF
"@
Write-Success "Created /etc/wsl.conf"

# Fix 2: Create static /etc/resolv.conf with working DNS servers
Write-Info "Step 2: Creating static /etc/resolv.conf..."
wsl -- bash -c @"
sudo rm -f /etc/resolv.conf
sudo tee /etc/resolv.conf > /dev/null << 'EOF'
# Static DNS configuration (fixed by fix-wsl-dns.ps1)
nameserver 8.8.8.8
nameserver 8.8.4.4
nameserver 1.1.1.1
EOF
sudo chattr +i /etc/resolv.conf
"@
Write-Success "Created static /etc/resolv.conf with Google/Cloudflare DNS"

# Fix 3: Restart WSL
Write-Info "Step 3: Restarting WSL to apply changes..."
wsl --shutdown
Start-Sleep -Seconds 3
Write-Success "WSL restarted"

# Test connectivity
Write-Info "Step 4: Testing connectivity..."
Start-Sleep -Seconds 2

$testIP = wsl -- ping -c 1 -W 2 8.8.8.8 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Success "IP connectivity: OK (can reach 8.8.8.8)"
} else {
    Write-Warn "IP connectivity: FAILED"
}

$testDNS = wsl -- ping -c 1 -W 2 google.com 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Success "DNS resolution: OK (can resolve google.com)"
} else {
    Write-Warn "DNS resolution: FAILED"
}

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          WSL2 DNS Fix Summary                                  |" -ForegroundColor Cyan
Write-Host "+================================================================+" -ForegroundColor Cyan
Write-Host "| Changes made:                                                  |" -ForegroundColor White
Write-Host "|   1. Created /etc/wsl.conf (disable auto-generated DNS)       |" -ForegroundColor White
Write-Host "|   2. Created static /etc/resolv.conf (8.8.8.8, 1.1.1.1)       |" -ForegroundColor White
Write-Host "|   3. Made resolv.conf immutable (chattr +i)                   |" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "| Test connectivity:                                             |" -ForegroundColor White
Write-Host "|   wsl -- ping -c 3 google.com                                 |" -ForegroundColor Cyan
Write-Host "|   wsl -- curl -I https://google.com                           |" -ForegroundColor Cyan
Write-Host "+================================================================+" -ForegroundColor Cyan
Write-Host ""

if ($LASTEXITCODE -eq 0) {
    Write-Success "WSL2 DNS fix complete!"
    exit 0
} else {
    Write-Warn "DNS still not working. Additional troubleshooting needed:"
    Write-Info "  1. Check Windows firewall rules for WSL"
    Write-Info "  2. Check antivirus/VPN interference"
    Write-Info "  3. Try: netsh winsock reset (then reboot)"
    exit 1
}
