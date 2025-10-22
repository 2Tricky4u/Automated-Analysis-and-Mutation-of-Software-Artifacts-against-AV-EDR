# Quick NAT check
Write-Host "=== NAT Status ===" -ForegroundColor Cyan

$nats = Get-NetNat
if ($nats) {
    $nats | Format-Table Name, InternalIPInterfaceAddressPrefix, Active -AutoSize

    $vmNat = $nats | Where-Object { $_.Name -eq "AutoMutateVMNAT" }
    if ($vmNat) {
        Write-Host "[OK] AutoMutateVMNAT exists and is active" -ForegroundColor Green
    } else {
        Write-Host "[ERROR] AutoMutateVMNAT not found!" -ForegroundColor Red
        Write-Host "Create it by running (as Admin):" -ForegroundColor Yellow
        Write-Host "  New-NetNat -Name AutoMutateVMNAT -InternalIPInterfaceAddressPrefix 192.168.200.0/24" -ForegroundColor Cyan
    }
} else {
    Write-Host "[ERROR] No NAT configurations found!" -ForegroundColor Red
    Write-Host "Create one by running (as Admin):" -ForegroundColor Yellow
    Write-Host "  New-NetNat -Name AutoMutateVMNAT -InternalIPInterfaceAddressPrefix 192.168.200.0/24" -ForegroundColor Cyan
}

Write-Host "`n=== Quick Fix Command ===" -ForegroundColor Yellow
Write-Host "If NAT is missing, run this in Administrator PowerShell:" -ForegroundColor White
Write-Host "  New-NetNat -Name 'AutoMutateVMNAT' -InternalIPInterfaceAddressPrefix '192.168.200.0/24'" -ForegroundColor Cyan
