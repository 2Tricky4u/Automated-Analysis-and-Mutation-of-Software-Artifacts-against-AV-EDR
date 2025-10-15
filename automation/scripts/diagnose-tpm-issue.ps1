<#
.SYNOPSIS
    Diagnose TPM enablement issue with detailed logging
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

Write-Host "`n=== TPM Issue Diagnostic ===" -ForegroundColor Cyan

$TestVMName = "test-tpm-debug"
$VhdPath = "C:\HyperV\VHDs\$TestVMName.vhdx"

# Cleanup if exists
$existing = Get-VM -Name $TestVMName -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "[INFO] Removing existing test VM..." -ForegroundColor Yellow
    if ($existing.State -ne 'Off') { Stop-VM -Name $TestVMName -Force -TurnOff }
    Remove-VM -Name $TestVMName -Force
}
if (Test-Path $VhdPath) { Remove-Item $VhdPath -Force }

# Step 1: Create VM
Write-Host "`n[Step 1] Creating Gen2 VM..." -ForegroundColor Cyan
New-VHD -Path $VhdPath -SizeBytes 20GB -Dynamic | Out-Null
New-VM -Name $TestVMName -MemoryStartupBytes 2GB -Generation 2 -VHDPath $VhdPath | Out-Null
Write-Host "  [OK] VM created" -ForegroundColor Green

# Check initial state
$firmware = Get-VMFirmware -VMName $TestVMName
Write-Host "`n[Step 2] Initial firmware state (after New-VM):" -ForegroundColor Cyan
Write-Host "  Secure Boot: $($firmware.SecureBoot)" -ForegroundColor White

# Step 3: Disable Secure Boot
Write-Host "`n[Step 3] Disabling Secure Boot..." -ForegroundColor Cyan
Set-VMFirmware -VMName $TestVMName -EnableSecureBoot Off
$firmware = Get-VMFirmware -VMName $TestVMName
Write-Host "  Secure Boot after disable: $($firmware.SecureBoot)" -ForegroundColor White

if ($firmware.SecureBoot -eq 'On') {
    Write-Host "  [ERROR] Secure Boot is still ON! This is the problem." -ForegroundColor Red
    Write-Host "  Hyper-V may be ignoring the -EnableSecureBoot Off command." -ForegroundColor Red
} else {
    Write-Host "  [OK] Secure Boot successfully disabled" -ForegroundColor Green
}

# Step 4: Try to enable TPM
Write-Host "`n[Step 4] Attempting to enable TPM..." -ForegroundColor Cyan
try {
    Enable-VMTPM -VMName $TestVMName -ErrorAction Stop
    Write-Host "  [SUCCESS] TPM enabled without errors!" -ForegroundColor Green

    $tpm = Get-VMTPM -VMName $TestVMName
    Write-Host "  TPM Enabled: $($tpm.Enabled)" -ForegroundColor White
} catch {
    Write-Host "  [ERROR] TPM enable failed:" -ForegroundColor Red
    Write-Host "  $_" -ForegroundColor Red

    Write-Host "`n[Diagnosis] This suggests one of:" -ForegroundColor Yellow
    Write-Host "  1. Secure Boot is still ON (check Step 3 output above)" -ForegroundColor Yellow
    Write-Host "  2. Host doesn't support TPM for VMs (check: Get-VMHost | Select VirtualMachineMigrationEnabled)" -ForegroundColor Yellow
    Write-Host "  3. Hyper-V version has a bug with TPM + Gen2 VMs" -ForegroundColor Yellow
}

# Step 5: Check host capabilities
Write-Host "`n[Step 5] Checking host TPM capabilities..." -ForegroundColor Cyan
$vmHost = Get-VMHost
Write-Host "  VM Migration Enabled: $($vmHost.VirtualMachineMigrationEnabled)" -ForegroundColor White
Write-Host "  Hyper-V Version: $($vmHost.HyperVVersion)" -ForegroundColor White

# Check if host has TPM
$hostTpm = Get-Tpm -ErrorAction SilentlyContinue
if ($hostTpm) {
    Write-Host "  Host TPM Present: $($hostTpm.TpmPresent)" -ForegroundColor White
    Write-Host "  Host TPM Enabled: $($hostTpm.TpmEnabled)" -ForegroundColor White
} else {
    Write-Host "  [WARN] Could not check host TPM status" -ForegroundColor Yellow
}

# Cleanup
Write-Host "`n[Cleanup] Removing test VM..." -ForegroundColor Cyan
Remove-VM -Name $TestVMName -Force
Remove-Item $VhdPath -Force
Write-Host "  [OK] Cleanup complete" -ForegroundColor Green

Write-Host "`n=== Diagnostic Complete ===" -ForegroundColor Cyan
Write-Host "Review the output above to identify the issue.`n" -ForegroundColor Cyan
