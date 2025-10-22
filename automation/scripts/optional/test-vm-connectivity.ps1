<#
.SYNOPSIS
    Test VM network connectivity (run inside VM)

.DESCRIPTION
    Comprehensive connectivity test that doesn't rely on ICMP ping.
    Tests routing, DNS resolution, HTTP connectivity, and port reachability.

.EXAMPLE
    # From inside a worker VM
    .\test-vm-connectivity.ps1
#>

$ErrorActionPreference = "Continue"

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          VM Network Connectivity Test                          |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

$results = @()

# 1. Check network adapter configuration
Write-Host "[1] Network Adapter Configuration:" -ForegroundColor Yellow
$adapter = Get-NetAdapter | Where-Object { $_.Status -eq "Up" -and $_.Name -notlike "*Loopback*" } | Select-Object -First 1
if ($adapter) {
    $ip = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue
    Write-Host "  Adapter: $($adapter.Name)" -ForegroundColor White
    Write-Host "  Status: $($adapter.Status)" -ForegroundColor Green
    Write-Host "  IP Address: $($ip.IPAddress)/$($ip.PrefixLength)" -ForegroundColor Green
    $results += [PSCustomObject]@{ Test = "Network Adapter"; Result = "PASS" }
} else {
    Write-Host "  ERROR: No active adapter found!" -ForegroundColor Red
    $results += [PSCustomObject]@{ Test = "Network Adapter"; Result = "FAIL" }
}

# 2. Check default gateway
Write-Host "`n[2] Default Gateway:" -ForegroundColor Yellow
$gateway = Get-NetRoute -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($gateway) {
    Write-Host "  Gateway IP: $($gateway.NextHop)" -ForegroundColor Green
    Write-Host "  Interface: $($gateway.InterfaceAlias)" -ForegroundColor White
    $results += [PSCustomObject]@{ Test = "Default Gateway"; Result = "PASS" }
} else {
    Write-Host "  ERROR: No default gateway configured!" -ForegroundColor Red
    $results += [PSCustomObject]@{ Test = "Default Gateway"; Result = "FAIL" }
}

# 3. Check DNS servers
Write-Host "`n[3] DNS Configuration:" -ForegroundColor Yellow
$dns = Get-DnsClientServerAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
       Where-Object { $_.ServerAddresses.Count -gt 0 } | Select-Object -First 1
if ($dns) {
    Write-Host "  DNS Servers: $($dns.ServerAddresses -join ', ')" -ForegroundColor Green
    $results += [PSCustomObject]@{ Test = "DNS Configuration"; Result = "PASS" }
} else {
    Write-Host "  WARNING: No DNS servers configured!" -ForegroundColor Yellow
    $results += [PSCustomObject]@{ Test = "DNS Configuration"; Result = "WARN" }
}

# 4. Test DNS resolution
Write-Host "`n[4] DNS Resolution Test:" -ForegroundColor Yellow
try {
    $dnsTest = Resolve-DnsName -Name "google.com" -Type A -ErrorAction Stop
    if ($dnsTest) {
        Write-Host "  google.com resolves to: $($dnsTest[0].IPAddress)" -ForegroundColor Green
        $results += [PSCustomObject]@{ Test = "DNS Resolution"; Result = "PASS" }
    }
} catch {
    Write-Host "  ERROR: Cannot resolve google.com" -ForegroundColor Red
    Write-Host "  Details: $($_.Exception.Message)" -ForegroundColor Red
    $results += [PSCustomObject]@{ Test = "DNS Resolution"; Result = "FAIL" }
}

# 5. Test HTTP connectivity (port 80)
Write-Host "`n[5] HTTP Connectivity Test (Port 80):" -ForegroundColor Yellow
try {
    $httpTest = Test-NetConnection -ComputerName "google.com" -Port 80 -WarningAction SilentlyContinue -ErrorAction Stop
    if ($httpTest.TcpTestSucceeded) {
        Write-Host "  Connection to google.com:80 succeeded" -ForegroundColor Green
        $results += [PSCustomObject]@{ Test = "HTTP (Port 80)"; Result = "PASS" }
    } else {
        Write-Host "  Connection to google.com:80 failed" -ForegroundColor Red
        $results += [PSCustomObject]@{ Test = "HTTP (Port 80)"; Result = "FAIL" }
    }
} catch {
    Write-Host "  ERROR: $($_.Exception.Message)" -ForegroundColor Red
    $results += [PSCustomObject]@{ Test = "HTTP (Port 80)"; Result = "FAIL" }
}

# 6. Test HTTPS connectivity (port 443)
Write-Host "`n[6] HTTPS Connectivity Test (Port 443):" -ForegroundColor Yellow
try {
    $httpsTest = Test-NetConnection -ComputerName "google.com" -Port 443 -WarningAction SilentlyContinue -ErrorAction Stop
    if ($httpsTest.TcpTestSucceeded) {
        Write-Host "  Connection to google.com:443 succeeded" -ForegroundColor Green
        $results += [PSCustomObject]@{ Test = "HTTPS (Port 443)"; Result = "PASS" }
    } else {
        Write-Host "  Connection to google.com:443 failed" -ForegroundColor Red
        $results += [PSCustomObject]@{ Test = "HTTPS (Port 443)"; Result = "FAIL" }
    }
} catch {
    Write-Host "  ERROR: $($_.Exception.Message)" -ForegroundColor Red
    $results += [PSCustomObject]@{ Test = "HTTPS (Port 443)"; Result = "FAIL" }
}

# 7. Test gateway connectivity (using TCP, not ping)
Write-Host "`n[7] Gateway TCP Connectivity:" -ForegroundColor Yellow
if ($gateway) {
    # Try to connect to a common port on the gateway (we know DNS port 53 should be open)
    try {
        $gwTest = Test-NetConnection -ComputerName $gateway.NextHop -Port 53 -WarningAction SilentlyContinue -ErrorAction Stop
        if ($gwTest.TcpTestSucceeded) {
            Write-Host "  Gateway reachable on port 53 (DNS)" -ForegroundColor Green
            $results += [PSCustomObject]@{ Test = "Gateway Reachable"; Result = "PASS" }
        } else {
            Write-Host "  Gateway not reachable on port 53" -ForegroundColor Yellow
            Write-Host "  (This may be normal - gateway might not have DNS port open)" -ForegroundColor White
            $results += [PSCustomObject]@{ Test = "Gateway Reachable"; Result = "WARN" }
        }
    } catch {
        Write-Host "  Gateway test inconclusive" -ForegroundColor Yellow
        $results += [PSCustomObject]@{ Test = "Gateway Reachable"; Result = "WARN" }
    }
}

# 8. Optional: Test ICMP ping (expected to fail)
Write-Host "`n[8] ICMP Ping Test (optional - may fail):" -ForegroundColor Yellow
if ($gateway) {
    $pingResult = Test-Connection -ComputerName $gateway.NextHop -Count 1 -Quiet -ErrorAction SilentlyContinue
    if ($pingResult) {
        Write-Host "  Gateway responds to ping" -ForegroundColor Green
    } else {
        Write-Host "  Ping failed (this is NORMAL - Windows Firewall blocks ICMP)" -ForegroundColor White
        Write-Host "  Network connectivity is still functional via TCP/UDP" -ForegroundColor White
    }
}

# Summary
Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          Test Summary                                          |" -ForegroundColor Cyan
Write-Host "+================================================================+`n" -ForegroundColor Cyan

$passed = ($results | Where-Object { $_.Result -eq "PASS" }).Count
$warned = ($results | Where-Object { $_.Result -eq "WARN" }).Count
$failed = ($results | Where-Object { $_.Result -eq "FAIL" }).Count

foreach ($result in $results) {
    $color = switch ($result.Result) {
        "PASS" { "Green" }
        "WARN" { "Yellow" }
        "FAIL" { "Red" }
    }
    Write-Host "  [$($result.Result.PadRight(4))] $($result.Test)" -ForegroundColor $color
}

Write-Host "`n  Total: $passed passed, $warned warnings, $failed failed" -ForegroundColor White

if ($failed -eq 0 -and $passed -ge 5) {
    Write-Host "`n  Internet connectivity is working correctly!`n" -ForegroundColor Green
} elseif ($failed -gt 0) {
    Write-Host "`n  Some tests failed - check network configuration`n" -ForegroundColor Red
} else {
    Write-Host "`n  Connectivity tests partially successful`n" -ForegroundColor Yellow
}
