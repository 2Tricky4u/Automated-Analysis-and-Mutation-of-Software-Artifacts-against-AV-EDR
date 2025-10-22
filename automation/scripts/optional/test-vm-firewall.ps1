# Run INSIDE VM to test if firewall is blocking
Write-Host "=== Testing VM Firewall Impact ===" -ForegroundColor Cyan

Write-Host "`n[1] Current Firewall Status:"
Get-NetFirewallProfile | Format-Table Name, Enabled, DefaultOutboundAction

Write-Host "`n[2] Testing DNS with firewall ON..."
$test1 = $null
try {
    $test1 = Resolve-DnsName -Name google.com -Server 8.8.8.8 -ErrorAction Stop -DnsOnly
    Write-Host "  SUCCESS: DNS works!" -ForegroundColor Green
} catch {
    Write-Host "  FAILED: $($_.Exception.Message)" -ForegroundColor Red
}

if (-not $test1) {
    Write-Host "`n[3] Temporarily disabling firewall to test..."
    Set-NetFirewallProfile -Profile Domain,Public,Private -Enabled False
    Write-Host "  Firewall disabled" -ForegroundColor Yellow

    Start-Sleep -Seconds 2

    Write-Host "`n[4] Testing DNS with firewall OFF..."
    try {
        $test2 = Resolve-DnsName -Name google.com -Server 8.8.8.8 -ErrorAction Stop -DnsOnly
        if ($test2) {
            Write-Host "  SUCCESS: DNS works without firewall!" -ForegroundColor Green
            Write-Host "`n  DIAGNOSIS: VM firewall was blocking outbound DNS" -ForegroundColor Yellow
            Write-Host "  SOLUTION: Need to add firewall rules to allow outbound traffic" -ForegroundColor Yellow
        }
    } catch {
        Write-Host "  FAILED: Still doesn't work - firewall is NOT the issue" -ForegroundColor Red
        Write-Host "  Error: $($_.Exception.Message)" -ForegroundColor Red
    }

    Write-Host "`n[5] Re-enabling firewall..."
    Set-NetFirewallProfile -Profile Domain,Public,Private -Enabled True
    Write-Host "  Firewall re-enabled" -ForegroundColor Green
}

Write-Host "`n=== Summary ===" -ForegroundColor Cyan
if ($test1) {
    Write-Host "DNS works - no firewall issue" -ForegroundColor Green
} elseif ($test2) {
    Write-Host "VM firewall is blocking outbound traffic!" -ForegroundColor Red
    Write-Host "Fix: Disable firewall or add outbound allow rules" -ForegroundColor Yellow
} else {
    Write-Host "Problem is NOT the VM firewall" -ForegroundColor Red
    Write-Host "Check: NAT configuration, routing, host firewall" -ForegroundColor Yellow
}
