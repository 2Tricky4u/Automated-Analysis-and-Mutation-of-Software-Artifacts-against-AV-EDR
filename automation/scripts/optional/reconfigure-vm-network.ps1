# Run this INSIDE the VM to reconfigure network to new subnet
# Or run via PowerShell Direct from host

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$NewIP,  # e.g., "10.200.200.100"

    [Parameter()]
    [string]$Gateway = "10.200.200.1",

    [Parameter()]
    [int]$Prefix = 24
)

Write-Host "=== Reconfiguring VM Network ===" -ForegroundColor Cyan

$adapter = Get-NetAdapter | Where-Object { $_.Status -eq "Up" -and $_.Name -notlike "*Loopback*" } | Select-Object -First 1

if ($adapter) {
    Write-Host "`n[1] Removing old configuration..."
    # Remove ALL old IPs
    Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Remove-NetIPAddress -Confirm:$false -ErrorAction SilentlyContinue
    Get-NetRoute -InterfaceIndex $adapter.ifIndex -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue |
        Remove-NetRoute -Confirm:$false -ErrorAction SilentlyContinue

    Write-Host "[2] Configuring new IP..."
    New-NetIPAddress -InterfaceIndex $adapter.ifIndex -IPAddress $NewIP -PrefixLength $Prefix -DefaultGateway $Gateway | Out-Null
    Set-DnsClientServerAddress -InterfaceIndex $adapter.ifIndex -ServerAddresses @($Gateway, "8.8.8.8", "8.8.4.4")

    Write-Host "[OK] Network reconfigured!" -ForegroundColor Green
    Write-Host "  IP: $NewIP/$Prefix" -ForegroundColor White
    Write-Host "  Gateway: $Gateway" -ForegroundColor White
    Write-Host "  DNS: $Gateway, 8.8.8.8, 8.8.4.4" -ForegroundColor White

    Write-Host "`n[3] Testing connectivity..."
    Start-Sleep -Seconds 2

    try {
        $dns = Resolve-DnsName -Name google.com -ErrorAction Stop
        Write-Host "[OK] DNS resolution works! Resolved to: $($dns[0].IPAddress)" -ForegroundColor Green
        Write-Host "`nInternet access is working!" -ForegroundColor Green
    } catch {
        Write-Host "[WARN] DNS resolution failed (might need a moment to propagate)" -ForegroundColor Yellow
        Write-Host "  Try again in a few seconds: Resolve-DnsName google.com" -ForegroundColor White
    }
} else {
    Write-Host "[ERROR] No active network adapter found!" -ForegroundColor Red
}
