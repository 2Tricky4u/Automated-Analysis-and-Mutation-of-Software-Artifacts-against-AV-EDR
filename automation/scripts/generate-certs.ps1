<#
.SYNOPSIS
    Generate mTLS certificates for Controller and Workers

.PARAMETER OutputPath
    Certificate output directory (default: ..\certs)

.PARAMETER ValidityDays
    Certificate validity period in days (default: 365)

.EXAMPLE
    .\generate-certs.ps1
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string]$OutputPath = "..\certs",

    [Parameter()]
    [int]$ValidityDays = 365,

    [Parameter()]
    [string]$ConfigPath = "..\config.yaml"
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Warn { param($M) Write-Host "[WARN] $M" -ForegroundColor Yellow }

Write-Host @"

╔═══════════════════════════════════════════════════════════════╗
║         mTLS Certificate Generation                           ║
╚═══════════════════════════════════════════════════════════════╝

"@ -ForegroundColor Cyan

# Check for OpenSSL
$opensslPath = Get-Command openssl -ErrorAction SilentlyContinue
if (-not $opensslPath) {
    Write-Warn "OpenSSL not found. Install via Chocolatey: choco install openssl"
    Write-Info "Or download from: https://slproweb.com/products/Win32OpenSSL.html"
    exit 1
}

Write-Success "OpenSSL found: $($opensslPath.Source)"

# Create output directory
if (-not (Test-Path $OutputPath)) {
    New-Item -ItemType Directory -Path $OutputPath -Force | Out-Null
}

Write-Info "Output directory: $(Resolve-Path $OutputPath)"
Write-Info "Validity: $ValidityDays days"

# Parse config to get workers
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)\"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$ConfigContent = Get-Content $ConfigPath -Raw
$WorkersSection = ($ConfigContent -split 'workers:')[1] -split 'storage:' | Select-Object -First 1
$WorkerMatches = [regex]::Matches($WorkersSection, '- name:\s*"([^"]+)"[^\-]*?ip:\s*"([^"]+)"')

$Workers = @()
foreach ($match in $WorkerMatches) {
    $Workers += @{
        Name = $match.Groups[1].Value
        IP = $match.Groups[2].Value
    }
}

Write-Info "Found $($Workers.Count) worker(s) in config"

# Step 1: Generate CA
Write-Info "Generating CA certificate..."

$caKeyPath = Join-Path $OutputPath "ca.key"
$caCrtPath = Join-Path $OutputPath "ca.crt"

& openssl genrsa -out $caKeyPath 4096 2>$null
& openssl req -new -x509 -days $ValidityDays -key $caKeyPath -out $caCrtPath `
    -subj "/C=US/ST=Lab/L=Lab/O=AutoMutate/OU=CA/CN=AutoMutate-CA" 2>$null

Write-Success "CA certificate created"

# Step 2: Generate Controller certificate
Write-Info "Generating Controller certificate..."

$controllerKeyPath = Join-Path $OutputPath "controller.key"
$controllerCsrPath = Join-Path $OutputPath "controller.csr"
$controllerCrtPath = Join-Path $OutputPath "controller.crt"

& openssl genrsa -out $controllerKeyPath 2048 2>$null
& openssl req -new -key $controllerKeyPath -out $controllerCsrPath `
    -subj "/C=US/ST=Lab/L=Lab/O=AutoMutate/OU=Controller/CN=192.168.200.1" 2>$null

$sanConfig = @"
[req]
distinguished_name=req_distinguished_name
[req_distinguished_name]
[v3_req]
subjectAltName=@alt_names
[alt_names]
IP.1=192.168.200.1
IP.2=127.0.0.1
DNS.1=localhost
"@
$sanConfigPath = Join-Path $OutputPath "controller-san.conf"
$sanConfig | Out-File $sanConfigPath -Encoding ASCII

& openssl x509 -req -in $controllerCsrPath -CA $caCrtPath -CAkey $caKeyPath `
    -CAcreateserial -out $controllerCrtPath -days $ValidityDays `
    -extensions v3_req -extfile $sanConfigPath 2>$null

Remove-Item $controllerCsrPath, $sanConfigPath -Force

Write-Success "Controller certificate created"

# Step 3: Generate Worker certificates
foreach ($worker in $Workers) {
    Write-Info "Generating certificate for $($worker.Name)..."

    $workerKeyPath = Join-Path $OutputPath "$($worker.Name).key"
    $workerCsrPath = Join-Path $OutputPath "$($worker.Name).csr"
    $workerCrtPath = Join-Path $OutputPath "$($worker.Name).crt"

    & openssl genrsa -out $workerKeyPath 2048 2>$null
    & openssl req -new -key $workerKeyPath -out $workerCsrPath `
        -subj "/C=US/ST=Lab/L=Lab/O=AutoMutate/OU=Worker/CN=$($worker.IP)" 2>$null

    $workerSanConfig = @"
[req]
distinguished_name=req_distinguished_name
[req_distinguished_name]
[v3_req]
subjectAltName=@alt_names
[alt_names]
IP.1=$($worker.IP)
DNS.1=$($worker.Name)
"@
    $workerSanConfigPath = Join-Path $OutputPath "$($worker.Name)-san.conf"
    $workerSanConfig | Out-File $workerSanConfigPath -Encoding ASCII

    & openssl x509 -req -in $workerCsrPath -CA $caCrtPath -CAkey $caKeyPath `
        -CAcreateserial -out $workerCrtPath -days $ValidityDays `
        -extensions v3_req -extfile $workerSanConfigPath 2>$null

    Remove-Item $workerCsrPath, $workerSanConfigPath -Force

    Write-Success "$($worker.Name) certificate created"
}

# Summary
Write-Host ""
Write-Host "="*70 -ForegroundColor Cyan
Write-Success "Certificate generation complete"
Write-Info "Location: $(Resolve-Path $OutputPath)"
Write-Info "Workers: $($Workers.Count)"
Write-Info "Expires: $((Get-Date).AddDays($ValidityDays).ToString('yyyy-MM-dd'))"
Write-Host ""
Write-Info "Files generated:"
Write-Info "  ca.crt, ca.key (CA - keep ca.key secure!)"
Write-Info "  controller.crt, controller.key"
foreach ($worker in $Workers) {
    Write-Info "  $($worker.Name).crt, $($worker.Name).key"
}
Write-Host "="*70 -ForegroundColor Cyan

exit 0
