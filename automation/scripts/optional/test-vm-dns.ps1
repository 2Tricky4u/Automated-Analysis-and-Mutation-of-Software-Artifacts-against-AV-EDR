# Run this INSIDE the VM to test DNS
Write-Host "=== VM DNS Test ===" -ForegroundColor Cyan

Write-Host "`n[1] DNS Configuration:"
Get-DnsClientServerAddress -AddressFamily IPv4 | Where-Object { $_.ServerAddresses.Count -gt 0 } | Format-Table

Write-Host "[2] Testing DNS to 8.8.8.8 directly:"
try {
    $result = Resolve-DnsName -Name google.com -Server 8.8.8.8 -ErrorAction Stop
    Write-Host "SUCCESS: Resolved to $($result[0].IPAddress)" -ForegroundColor Green
} catch {
    Write-Host "FAILED: $($_.Exception.Message)" -ForegroundColor Red
}

Write-Host "`n[3] Testing DNS via default servers:"
try {
    $result = Resolve-DnsName -Name google.com -ErrorAction Stop
    Write-Host "SUCCESS: Resolved to $($result[0].IPAddress)" -ForegroundColor Green
} catch {
    Write-Host "FAILED: $($_.Exception.Message)" -ForegroundColor Red
}

Write-Host "`n[4] Route table:"
Get-NetRoute -DestinationPrefix "0.0.0.0/0" | Format-Table

Write-Host "[5] Test TCP to 8.8.8.8:53:"
Test-NetConnection -ComputerName 8.8.8.8 -Port 53

Write-Host "`n[6] Windows Firewall Status:"
Get-NetFirewallProfile | Format-Table Name, Enabled
