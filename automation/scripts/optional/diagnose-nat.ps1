# Diagnose NAT functionality
Write-Host "=== NAT Diagnostics ===" -ForegroundColor Cyan

Write-Host "`n[1] NAT Configuration:"
$nat = Get-NetNat -Name AutoMutateVMNAT -ErrorAction SilentlyContinue
if ($nat) {
    $nat | Format-List Name, InternalIPInterfaceAddressPrefix, Active
} else {
    Write-Host "ERROR: NAT not found!" -ForegroundColor Red
}

Write-Host "`n[2] NAT Statistics:"
Get-NetNatStaticMapping -ErrorAction SilentlyContinue

Write-Host "`n[3] NAT Sessions (active translations):"
$sessions = Get-NetNatSession -ErrorAction SilentlyContinue
if ($sessions) {
    $sessions | Select-Object -First 10 | Format-Table
} else {
    Write-Host "No active NAT sessions (VM may not be sending traffic)" -ForegroundColor Yellow
}

Write-Host "`n[4] Network Interfaces NAT is using:"
$adapters = Get-NetAdapter | Where-Object { $_.Status -eq "Up" }
foreach ($adapter in $adapters) {
    $ip = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue
    if ($ip -and $ip.IPAddress -like "192.168.200.*") {
        Write-Host "  NAT Interface: $($adapter.Name) - $($ip.IPAddress)" -ForegroundColor Green
    }
}

Write-Host "`n[5] CRITICAL TEST - Can HOST reach 8.8.8.8?"
$hostTest = Test-NetConnection -ComputerName 8.8.8.8 -Port 53 -WarningAction SilentlyContinue
Write-Host "  Host -> 8.8.8.8:53 : $($hostTest.TcpTestSucceeded)" -ForegroundColor $(if ($hostTest.TcpTestSucceeded) { "Green" } else { "Red" })

Write-Host "`n[6] Diagnosis:"
if (-not $nat) {
    Write-Host "  ERROR: NAT doesn't exist - create it first" -ForegroundColor Red
} elseif (-not $nat.Active) {
    Write-Host "  ERROR: NAT exists but is NOT active" -ForegroundColor Red
} elseif (-not $hostTest.TcpTestSucceeded) {
    Write-Host "  ERROR: Host itself cannot reach internet!" -ForegroundColor Red
    Write-Host "  Check your host's internet connection" -ForegroundColor Yellow
} else {
    Write-Host "  NAT is configured and active" -ForegroundColor Green
    Write-Host "  Host has internet access" -ForegroundColor Green
    Write-Host "`n  Possible issues:" -ForegroundColor Yellow
    Write-Host "    1. Windows NAT might not work properly (known limitation)" -ForegroundColor White
    Write-Host "    2. Need to restart Hyper-V Virtual Switch service" -ForegroundColor White
    Write-Host "    3. May need to use ICS instead of NetNat" -ForegroundColor White
}

Write-Host "`n=== Recommended Fix ===" -ForegroundColor Cyan
Write-Host "Try restarting the networking stack:" -ForegroundColor White
Write-Host "  Restart-Service vmms -Force" -ForegroundColor Yellow
Write-Host "  Restart-Service hns -Force" -ForegroundColor Yellow
Write-Host "Then test from VM again" -ForegroundColor White
