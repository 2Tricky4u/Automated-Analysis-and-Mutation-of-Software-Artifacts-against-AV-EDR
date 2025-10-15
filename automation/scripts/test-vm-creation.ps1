<#
.SYNOPSIS
    Quick test of VM creation with TPM + Secure Boot ordering

.DESCRIPTION
    Creates a single test VM to verify the TPM/Secure Boot fix works
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Write-Error { param($M) Write-Host "[ERROR] $M" -ForegroundColor Red }

$TestVMName = "test-win11-tpm"
$ConfigPath = "..\config.yaml"

# Load config
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$SwitchName = $config.network.switch_name
$VhdRoot = $config.storage.vhd_root
$VhdPath = Join-Path $VhdRoot "$TestVMName.vhdx"

Write-Info "Testing VM creation with TPM + Secure Boot ordering fix..."

# Cleanup if exists
$existing = Get-VM -Name $TestVMName -ErrorAction SilentlyContinue
if ($existing) {
    Write-Info "Removing existing test VM..."
    if ($existing.State -eq 'Running') { Stop-VM -Name $TestVMName -Force -TurnOff }
    Remove-VM -Name $TestVMName -Force
}
if (Test-Path $VhdPath) { Remove-Item $VhdPath -Force }

# Create VHDX
New-VHD -Path $VhdPath -SizeBytes (20GB) -Dynamic | Out-Null
Write-Success "Created test VHDX"

# Create Gen2 VM
New-VM -Name $TestVMName -MemoryStartupBytes 2GB `
    -Generation 2 -VHDPath $VhdPath -SwitchName $SwitchName | Out-Null
Write-Success "Created Gen2 VM"

# CRITICAL FIX: Disable default Secure Boot IMMEDIATELY
Set-VMFirmware -VMName $TestVMName -EnableSecureBoot Off
Write-Success "Disabled default Secure Boot"

# Enable TPM (should work now)
try {
    Enable-VMTPM -VMName $TestVMName -ErrorAction Stop
    Write-Success "Enabled TPM 2.0 (WITHOUT key protector error!)"
} catch {
    Write-Error "TPM enable failed: $_"
    exit 1
}

# Now enable Secure Boot
Set-VMFirmware -VMName $TestVMName -EnableSecureBoot On `
    -SecureBootTemplate "MicrosoftWindows"
Write-Success "Enabled Secure Boot"

# Verify final state
$firmware = Get-VMFirmware -VMName $TestVMName
$tpm = Get-VMTPM -VMName $TestVMName

Write-Host "`n=== Final VM State ===" -ForegroundColor Magenta
Write-Host "Secure Boot: $($firmware.SecureBoot)" -ForegroundColor Cyan
Write-Host "TPM Enabled: $($tpm.Enabled)" -ForegroundColor Cyan

if ($firmware.SecureBoot -eq 'On' -and $tpm.Enabled) {
    Write-Host "`n[SUCCESS] VM creation test PASSED!" -ForegroundColor Green
    Write-Host "Both TPM and Secure Boot are enabled correctly." -ForegroundColor Green

    # Cleanup
    Write-Info "Cleaning up test VM..."
    Remove-VM -Name $TestVMName -Force
    Remove-Item $VhdPath -Force
    Write-Success "Test VM removed"

    exit 0
} else {
    Write-Error "Test FAILED - VM state incorrect"
    exit 1
}
