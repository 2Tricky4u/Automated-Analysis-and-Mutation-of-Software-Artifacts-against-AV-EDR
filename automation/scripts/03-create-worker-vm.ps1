<#
.SYNOPSIS
    Create Worker VM with TPM + Secure Boot

.PARAMETER WorkerName
    VM name (e.g., "win11-worker-01")

.PARAMETER Os
    "windows10" or "windows11"

.PARAMETER IsoPath
    Path to Windows ISO

.PARAMETER StaticIP
    Worker IP in 192.168.200.0/24

.PARAMETER ConfigPath
    Path to config.yaml

.PARAMETER SkipHashCheck
    Skip SHA256 hash verification (faster but less secure)
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$WorkerName,

    [Parameter(Mandatory)]
    [ValidateSet("windows10", "windows11")]
    [string]$Os,

    [Parameter(Mandatory)]
    [string]$IsoPath,

    [Parameter(Mandatory)]
    [string]$StaticIP,

    [Parameter()]
    [string]$ConfigPath = "..\config.yaml",

    [Parameter()]
    [switch]$SkipHashCheck
)

$ErrorActionPreference = "Stop"
function Write-Success { param($M) Write-Host "[OK] $M" -ForegroundColor Green }
function Write-Info { param($M) Write-Host "[INFO] $M" -ForegroundColor Cyan }
function Get-GDriveFile {
    <#
    .SYNOPSIS
        Download a public Google Drive file
    .PARAMETER IdOrUrl
        File ID (e.g., 1AbC...) or a share URL like:
        https://drive.google.com/file/d/<ID>/view?usp=sharing
    .PARAMETER OutFile
        Destination path.
    .PARAMETER Sha256
        Optional SHA-256 hash to verify integrity.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] [string]$IdOrUrl,
        [Parameter(Mandatory)] [string]$OutFile,
        [string]$Sha256
    )

    $ErrorActionPreference = 'Stop'

    # Extract file ID
    $id = $IdOrUrl
    if ($IdOrUrl -match 'https?://(drive|docs)\.google\.com') {
        if ($IdOrUrl -match '/file/d/([^/]+)/') {
            $id = $Matches[1]
        } elseif ($IdOrUrl -match '(?:\?|&)id=([A-Za-z0-9_-]+)') {
            $id = $Matches[1]
        } else {
            throw "Could not extract Google Drive file ID from URL."
        }
    }

    # Prepare paths/session/headers
    $null = New-Item -ItemType Directory -Force -Path (Split-Path -LiteralPath $OutFile) 2>$null
    $tmp  = "$OutFile.part"

    $headers = @{ 'User-Agent' = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' }
    $baseUri = "https://drive.google.com/uc?export=download&id=$id"

    # First request: capture cookies/session
    $initial = Invoke-WebRequest -Uri $baseUri -SessionVariable sess -Headers $headers

    # Preempt EU consent
    try {
        $consent = New-Object System.Net.Cookie('CONSENT','YES+','/','.google.com')
        $sess.Cookies.Add([Uri]'https://google.com', $consent)
    } catch {}

    # Helper
    function Get-Header {
        param([object]$resp, [string]$name)
        $val = $null
        if ($resp.Headers -and $resp.Headers[$name]) { $val = $resp.Headers[$name] }
        elseif ($resp.BaseResponse -and $resp.BaseResponse.Headers) {
            try { $val = $resp.BaseResponse.Headers[$name] } catch {}
        }
        return $val
    }

    # Detect direct download
    $cd = Get-Header -resp $initial -name 'Content-Disposition'
    $ct = Get-Header -resp $initial -name 'Content-Type'
    $isDirect = ($cd -and $cd -ne '') -or ($ct -and $ct -match 'application/octet-stream')

    # Helper function to download with progress
    function Download-WithProgress {
        param([string]$Uri, [string]$Destination, $WebSession, $Headers)

        Write-Host "Downloading file..." -ForegroundColor Cyan
        Write-Host "  From: $Uri" -ForegroundColor Gray
        Write-Host "  To:   $Destination" -ForegroundColor Gray

        # Download in chunks with progress
        $request = [System.Net.HttpWebRequest]::Create($Uri)
        $request.UserAgent = $Headers['User-Agent']
        $request.Method = "GET"

        # Add cookies from session
        if ($WebSession) {
            $request.CookieContainer = New-Object System.Net.CookieContainer
            $cookies = $WebSession.Cookies.GetCookies([Uri]$Uri)
            foreach ($cookie in $cookies) {
                $request.CookieContainer.Add($cookie)
            }
        }

        try {
            $response = $request.GetResponse()
            $totalLength = $response.ContentLength
            $responseStream = $response.GetResponseStream()
            $fileStream = [System.IO.File]::Create($Destination)

            $buffer = New-Object byte[] 8192
            $totalRead = 0
            $lastPercent = -1

            while (($read = $responseStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                $fileStream.Write($buffer, 0, $read)
                $totalRead += $read

                if ($totalLength -gt 0) {
                    $percent = [int](($totalRead / $totalLength) * 100)
                    if ($percent -ne $lastPercent -and $percent % 5 -eq 0) {
                        $mb = [math]::Round($totalRead / 1MB, 2)
                        $totalMb = [math]::Round($totalLength / 1MB, 2)
                        Write-Host "  Progress: $percent% ($mb MB / $totalMb MB)" -ForegroundColor Cyan
                        $lastPercent = $percent
                    }
                }
            }

            $fileStream.Close()
            $responseStream.Close()
            $response.Close()

            Write-Host "  Download complete!" -ForegroundColor Green
        }
        catch {
            if ($fileStream) { $fileStream.Close() }
            if ($responseStream) { $responseStream.Close() }
            if ($response) { $response.Close() }
            throw
        }
    }

    if ($isDirect) {
        Download-WithProgress -Uri $baseUri -Destination $tmp -WebSession $sess -Headers $headers
        Move-Item -Force -LiteralPath $tmp -Destination $OutFile
    }
    else {
        # Interstitial page like virus scan warning): find download link / token
        Write-Host "Processing download confirmation page..." -ForegroundColor Yellow
        $html  = $initial.Content
        $token = $null
        $uuid  = $null
        $downloadUrl = $null

        # Pattern: <a id="uc-download-link" ... href="/uc?export=download&amp;confirm=...&amp;id=..."
        $linkMatch = [regex]::Match($html, 'href="(/uc\?export=download[^"]+)"')
        if ($linkMatch.Success) {
            $relativeUrl = $linkMatch.Groups[1].Value
            $relativeUrl = $relativeUrl -replace '&amp;', '&'  # Unescape HTML entities
            $downloadUrl = "https://drive.google.com" + $relativeUrl
            Write-Host "  Found download link in HTML" -ForegroundColor Gray
        }

        # Look for confirm token in various formats
        if (-not $downloadUrl) {
            $m = [regex]::Match($html, 'confirm=([^&"''<> ]+)')
            if ($m.Success) { $token = $m.Groups[1].Value }

            if (-not $token) {
                $m = [regex]::Match($html, 'name="confirm"\s+value="([^"]+)"')
                if ($m.Success) { $token = $m.Groups[1].Value }
            }

            if (-not $token) {
                $m = [regex]::Match($html, '"downloadUrl":"([^"]+)"')
                if ($m.Success) {
                    $downloadUrl = $m.Groups[1].Value -replace '\\u003d', '=' -replace '\\u0026', '&'
                }
            }

            # Extract uuid for newer flow
            $m = [regex]::Match($html, 'uuid=([0-9A-Za-z\-_]+)')
            if ($m.Success) { $uuid = $m.Groups[1].Value }

            # Fallback to cookie named download_warning*
            if (-not $token -and -not $downloadUrl) {
                $cookieCol = $sess.Cookies.GetCookies([Uri]'https://drive.google.com')
                $warn = $cookieCol | Where-Object { $_.Name -like 'download_warning*' } | Select-Object -First 1
                if ($warn) {
                    $token = $warn.Value
                    Write-Host "  Found token in cookie: $($warn.Name)" -ForegroundColor Gray
                }
            }
        }

        # Build download URL if we have token or found direct URL
        if (-not $downloadUrl) {
            if ($token) {
                # Use usercontent host
                $downloadUrl = "https://drive.usercontent.google.com/download?export=download&id=$id&confirm=$token"
                if ($uuid) { $downloadUrl += "&uuid=$uuid" }
                Write-Host "  Built download URL with token: $token" -ForegroundColor Gray
            }
            else {
                # try direct download without token
                Write-Host "  Could not find confirmation token, trying direct download..." -ForegroundColor Yellow
                $downloadUrl = "https://drive.usercontent.google.com/download?export=download&id=$id"
            }
        }

        if (-not $downloadUrl) {
            Write-Host "`nDebug: Interstitial page content (first 2000 chars):" -ForegroundColor Magenta
            Write-Host $html.Substring(0, [Math]::Min(2000, $html.Length)) -ForegroundColor Gray
            throw "Could not obtain Google Drive download URL or confirmation token."
        }

        Write-Host "  Final download URL: $downloadUrl" -ForegroundColor Gray
        Download-WithProgress -Uri $downloadUrl -Destination $tmp -WebSession $sess -Headers $headers
        Move-Item -Force -LiteralPath $tmp -Destination $OutFile
    }

    #integrity check
    if ($Sha256) {
        $actual = (Get-FileHash -Path $OutFile -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $Sha256.ToLowerInvariant()) {
            throw "SHA256 mismatch for '$OutFile'. Expected $Sha256, got $actual."
        }
    }

    Write-Host "[OK] Downloaded: $OutFile" -ForegroundColor Green
}




# Load config
$config = @{}
$section = $null
Get-Content $ConfigPath | ForEach-Object {
    if ($_ -match '^(\w+):$') { $section = $matches[1]; $config[$section] = @{} }
    elseif ($_ -match '^\s+(\w+):\s*"?(.+?)"?$' -and $section) { $config[$section][$matches[1]] = $matches[2].Trim('"') }
}

$SwitchName = $config.network.switch_name
$VhdRoot = $config.storage.vhd_root
$Gateway = $config.network.gateway
$Prefix = ($config.network.subnet -split '/')[1]

# Google Drive ISO sources
$DriveSources = @{
    windows10 = @{
        IdOrUrl = "https://drive.google.com/file/d/15eBAxjCTP92Pu7Ru0d1fpVxznj3EtCRM/view?usp=drive_link"
        Sha256  = "a6f470ca6d331eb353b815c043e327a347f594f37ff525f17764738fe812852e"
    }
    windows11 = @{
        IdOrUrl = "https://drive.google.com/file/d/1SDeHFfgoDZAanISyZooNOnpZxwFCzzA9/view?usp=drive_link"
        Sha256  = "d141f6030fed50f75e2b03e1eb2e53646c4b21e5386047cb860af5223f102a32"
    }
}


# Worker config - read from config.yaml
$osTemplate = $config.workers.$Os
if (-not $osTemplate) {
    Write-Error "No worker template found for OS '$Os' in config.yaml"
    exit 1
}

$CpuCount = [int]$osTemplate.cpu_count
$MemoryGB = [int]$osTemplate.memory_gb
$DiskGB = [int]$osTemplate.disk_gb

Write-Info "Creating VM: $WorkerName ($Os) - $StaticIP"

# Validate or fetch ISO
if (-not (Test-Path $IsoPath)) {
    Write-Info "ISO not found locally: $IsoPath"
    if (-not $DriveSources.ContainsKey($Os)) {
        Write-Error "No Google Drive source configured for OS '$Os'"
        exit 1
    }

    $src   = $DriveSources[$Os]
    $urlId = $src.IdOrUrl
    $sha   = if (-not $SkipHashCheck) { $src.Sha256 } else { $null }

    Write-Info "Downloading $Os ISO from Google Drive to $IsoPath ..."
    if ($SkipHashCheck) {
        Write-Warning "Hash verification is DISABLED (-SkipHashCheck)"
    }

    try {
        Get-GDriveFile -IdOrUrl $urlId -OutFile $IsoPath -Sha256 $sha
        Write-Success "Downloaded ISO: $IsoPath"
    }
    catch {
        Write-Error "Failed to download $Os ISO from Google Drive. $_"
        exit 1
    }
} else {
    Write-Success "ISO found: $IsoPath"

    # Verify hash of existing ISO
    if (-not $SkipHashCheck) {
        if (-not $DriveSources.ContainsKey($Os)) {
            Write-Warning "No hash available for OS '$Os' - skipping verification"
        }
        else {
            $expectedHash = $DriveSources[$Os].Sha256
            Write-Info "Verifying ISO integrity (SHA256)..."

            $actualHash = (Get-FileHash -Path $IsoPath -Algorithm SHA256).Hash.ToLowerInvariant()

            if ($actualHash -eq $expectedHash.ToLowerInvariant()) {
                Write-Success "ISO hash verified: $actualHash"
            }
            else {
                Write-Error @"
SHA256 hash mismatch for existing ISO!
Expected: $expectedHash
Got:      $actualHash

The ISO file may be corrupted or tampered with.
Options:
  1. Delete the file and re-download: Remove-Item '$IsoPath'
  2. Skip hash check (unsafe): Add -SkipHashCheck parameter
"@
                exit 1
            }
        }
    }
    else {
        Write-Warning "Hash verification is DISABLED (-SkipHashCheck)"
    }
}

# Create VHD folder
$VhdPath = Join-Path $VhdRoot "$WorkerName.vhdx"
$VhdFolder = Split-Path $VhdPath -Parent
if (-not (Test-Path $VhdFolder)) {
    New-Item -ItemType Directory -Path $VhdFolder -Force | Out-Null
}

# Check if VM already exists
$existingVM = Get-VM -Name $WorkerName -ErrorAction SilentlyContinue
$vmExists = $null -ne $existingVM
$windowsInstalled = $false

if ($vmExists) {
    Write-Info "VM $WorkerName already exists - verifying configuration..."

    # Check if VM is running
    if ($existingVM.State -ne 'Off') {
        Write-Warning "VM is currently $($existingVM.State). Please stop the VM before reconfiguring."
        Write-Info "To stop: Stop-VM -Name '$WorkerName' -Force"
        Write-Info "Skipping configuration changes to avoid breaking running VM."
        Write-Info "VM verification completed (no changes made)"
        exit 0
    }

    # Check if Windows is already installed on the VHD
    $vhdDrive = Get-VMHardDiskDrive -VMName $WorkerName -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($vhdDrive -and $vhdDrive.Path) {
        # Check VHD file size as indicator of Windows installation
        $vhdFile = Get-Item $vhdDrive.Path -ErrorAction SilentlyContinue
        if ($vhdFile -and $vhdFile.Length -gt 5GB) {
            # VHD is larger than 5GB, likely has Windows installed
            $windowsInstalled = $true
            Write-Info "Windows appears to be installed (VHD size: $([math]::Round($vhdFile.Length / 1GB, 2)) GB)"
        }
    }

    # Verify basic settings
    Write-Info "Verifying VM configuration..."
    $currentMemory = $existingVM.MemoryStartup / 1GB
    $currentProc = $existingVM.ProcessorCount

    if ($currentMemory -ne $MemoryGB) {
        Write-Info "  Memory: $currentMemory GB -> $MemoryGB GB (will update)"
    } else {
        Write-Success "  Memory: $MemoryGB GB (OK)"
    }

    if ($currentProc -ne $CpuCount) {
        Write-Info "  CPU: $currentProc cores -> $CpuCount cores (will update)"
    } else {
        Write-Success "  CPU: $CpuCount cores (OK)"
    }

    # Check VHD attachment
    $currentVhd = $vhdDrive.Path
    # Normalize paths for comparison
    $normalizedCurrentVhd = if ($currentVhd) { [System.IO.Path]::GetFullPath($currentVhd) } else { "" }
    $normalizedVhdPath = [System.IO.Path]::GetFullPath($VhdPath)

    if ($normalizedCurrentVhd -ne $normalizedVhdPath) {
        Write-Warning "  VHD: Current=$normalizedCurrentVhd, Expected=$normalizedVhdPath"
        Write-Info "  VHD path differs - keeping existing VHD to avoid data loss"
    } else {
        Write-Success "  VHD: $VhdPath (OK)"
    }

} else {
    # Create new VM
    Write-Info "Creating new VM: $WorkerName"

    # Create VHDX
    if (-not (Test-Path $VhdPath)) {
        New-VHD -Path $VhdPath -SizeBytes ($DiskGB * 1GB) -Dynamic | Out-Null
        Write-Success "Created VHDX: $VhdPath"
    } else {
        Write-Success "VHDX already exists: $VhdPath"
    }

    # Create Gen2 VM
    New-VM -Name $WorkerName -MemoryStartupBytes ($MemoryGB * 1GB) `
        -Generation 2 -VHDPath $VhdPath -SwitchName $SwitchName | Out-Null
    Write-Success "Created VM: $WorkerName"
}

# Configure VM settings
Set-VMProcessor -VMName $WorkerName -Count $CpuCount -ErrorAction SilentlyContinue
Set-VM -VMName $WorkerName -AutomaticCheckpointsEnabled $false -ErrorAction SilentlyContinue

# Memory configuration
$currentMem = (Get-VM -Name $WorkerName).MemoryStartup
$targetMem = $MemoryGB * 1GB
if ($currentMem -ne $targetMem) {
    Set-VM -VMName $WorkerName -MemoryStartupBytes $targetMem -ErrorAction SilentlyContinue
    Write-Success "Updated memory to $MemoryGB GB"
}

# Secure Boot configuration
$firmware = Get-VMFirmware -VMName $WorkerName
$currentSecureBoot = $firmware.SecureBoot

# Only disable Secure Boot if we need to enable TPM
if ($currentSecureBoot -ne 'Off') {
    Write-Info "Disabling Secure Boot temporarily..."
    Set-VMFirmware -VMName $WorkerName -EnableSecureBoot Off -ErrorAction SilentlyContinue
    $firmware = Get-VMFirmware -VMName $WorkerName
    if ($firmware.SecureBoot -eq 'Off') {
        Write-Success "Secure Boot disabled (verified)"
    } else {
        Write-Warning "Secure Boot is still $($firmware.SecureBoot) - TPM may fail"
    }
} else {
    Write-Success "Secure Boot already disabled"
}

# Attach ISO
if ($windowsInstalled) {
    Write-Info "Windows installed - skipping ISO attachment"
} else {
    $dvd = Get-VMDvdDrive -VMName $WorkerName -ErrorAction SilentlyContinue
    if (-not $dvd) {
        Add-VMDvdDrive -VMName $WorkerName -Path $IsoPath -ErrorAction SilentlyContinue
        Write-Success "Added DVD drive with ISO: $IsoPath"
    } else {
        # Check if ISO is already attached
        $currentIso = $dvd.Path
        if ($currentIso -ne $IsoPath) {
            Set-VMDvdDrive -VMName $WorkerName -Path $IsoPath -ErrorAction SilentlyContinue
            Write-Success "Updated ISO: $IsoPath"
        } else {
            Write-Success "ISO already attached: $IsoPath"
        }
    }
}

# Enable TPM with proper key protector
$tpmEnabled = $false

# Check if TPM is already enabled
try {
    # Try to get VM security settings this will fail if TPM isn't enabled
    $vmSecurity = Get-VM -Name $WorkerName | Select-Object -ExpandProperty VMId
    $tpmState = (Get-VM -Name $WorkerName).TpmEnabled 2>$null

    # alter:Try Enable-VMTPM with -Passthru to see current state
    $tpmCheck = Enable-VMTPM -VMName $WorkerName -Passthru -ErrorAction SilentlyContinue 2>$null

    if ($tpmCheck -or $tpmState) {
        Write-Success "TPM 2.0 already enabled"
        $tpmEnabled = $true
    } else {
        # TPM not enabled, need to enable it
        throw "TPM not enabled"
    }
} catch {
    # TPM not enabled or check failed - try to enable it
    Write-Info "Enabling TPM 2.0..."
    try {
        # Create a new local key protector
        Set-VMKeyProtector -VMName $WorkerName -NewLocalKeyProtector -ErrorAction Stop
        Enable-VMTPM -VMName $WorkerName -ErrorAction Stop
        Write-Success "Enabled TPM 2.0"
        $tpmEnabled = $true
    } catch {
        $errorMsg = $_.Exception.Message

        # Check if error is because TPM is already enabled
        if ($errorMsg -like "*already*" -or $errorMsg -like "*enabled*") {
            Write-Success "TPM 2.0 already enabled"
            $tpmEnabled = $true
        } else {
            Write-Warning "TPM enable failed: $errorMsg"
            if ($Os -eq "windows11") {
                Write-Warning "Windows 11 installation may fail without TPM."
                Write-Info "Continuing anyway..."
            }
        }
    }
}

# Configure firmware settings
if ($windowsInstalled) {
    Write-Info "Windows installed - skipping boot order configuration (already configured)"

    # Still verify Secure Boot for installed VMs
    $currentFirmware = Get-VMFirmware -VMName $WorkerName
    $currentSecureBoot = $currentFirmware.SecureBoot
    $targetSecureBoot = if ($Os -eq "windows11") { "On" } else { "Off" }

    if ($currentSecureBoot -ne $targetSecureBoot) {
        Write-Info "Updating Secure Boot: $currentSecureBoot -> $targetSecureBoot"
        if ($Os -eq "windows11") {
            Set-VMFirmware -VMName $WorkerName -EnableSecureBoot On -ErrorAction SilentlyContinue
        } else {
            Set-VMFirmware -VMName $WorkerName -EnableSecureBoot Off -ErrorAction SilentlyContinue
        }
        Write-Success "Updated Secure Boot to $targetSecureBoot"
    } else {
        Write-Success "Secure Boot: $currentSecureBoot (OK)"
    }
} else {
    # Windows not installed: configure boot order for installation
    $dvdDrive = Get-VMDvdDrive -VMName $WorkerName | Select-Object -First 1

    if (-not $dvdDrive) {
        Write-Error "DVD drive not found after attachment"
        exit 1
    }

    # Get current firmware configuration
    $currentFirmware = Get-VMFirmware -VMName $WorkerName
    $currentFirstBoot = $currentFirmware.BootOrder | Select-Object -First 1
    $currentSecureBoot = $currentFirmware.SecureBoot

    # Determine target Secure Boot state
    $targetSecureBoot = if ($Os -eq "windows11") { "On" } else { "Off" }

    # Check if configuration needs updating
    $needsUpdate = $false
    if ($currentFirstBoot.Device.Path -ne $dvdDrive.Path) {
        Write-Info "Boot order needs update (current first boot: $($currentFirstBoot.Device.Path))"
        $needsUpdate = $true
    }

    if ($currentSecureBoot -ne $targetSecureBoot) {
        Write-Info "Secure Boot needs update (current: $currentSecureBoot, target: $targetSecureBoot)"
        $needsUpdate = $true
    }

    if ($needsUpdate) {
        if ($Os -eq "windows11") {
            # Windows 11: Set DVD as first boot + Enable Secure Boot
            Set-VMFirmware -VMName $WorkerName -FirstBootDevice $dvdDrive -EnableSecureBoot On -ErrorAction SilentlyContinue
            Write-Success "Updated firmware: DVD first boot + Secure Boot enabled"
        } else {
            # Windows 10: Set DVD as first boot, keep Secure Boot OFF
            Set-VMFirmware -VMName $WorkerName -FirstBootDevice $dvdDrive -EnableSecureBoot Off -ErrorAction SilentlyContinue
            Write-Success "Updated firmware: DVD first boot, Secure Boot disabled"
        }
    } else {
        Write-Success "Firmware already configured correctly"
    }
}

# Disable Guest Services
$guestService = Get-VMIntegrationService -VMName $WorkerName | Where-Object { $_.Name -eq "Guest Service Interface" }
if ($guestService.Enabled) {
    $guestService | Disable-VMIntegrationService
    Write-Success "Disabled Guest Service Interface"
} else {
    Write-Success "Guest Service Interface already disabled"
}

# Display final status message
if ($windowsInstalled) {
    Write-Success "VM $WorkerName configuration verified (Windows already installed)"
} else {
    Write-Success "VM $WorkerName ready for Windows installation"
}

# Display final configuration
$secureBootStatus = if ($Os -eq "windows11") { "Enabled (MicrosoftWindows)" } else { "Disabled" }
$tpmStatus = if ($tpmEnabled) { "Enabled" } else { "Failed (check logs)" }
$installStatus = if ($windowsInstalled) { "Installed" } else { "Not Installed" }

Write-Host "`n+================================================================+" -ForegroundColor Cyan
Write-Host "|          VM Configuration Summary                              |" -ForegroundColor Cyan
Write-Host "+================================================================+" -ForegroundColor Cyan
Write-Host "| Name:        $WorkerName".PadRight(64) "|" -ForegroundColor White
Write-Host "| OS Type:     $Os".PadRight(64) "|" -ForegroundColor White
Write-Host "| IP Address:  $StaticIP".PadRight(64) "|" -ForegroundColor White
Write-Host "| Generation:  2 (UEFI)".PadRight(64) "|" -ForegroundColor White
Write-Host "| Windows:     $installStatus".PadRight(64) "|" -ForegroundColor $(if ($windowsInstalled) { "Green" } else { "Yellow" })
Write-Host "| Secure Boot: $secureBootStatus".PadRight(64) "|" -ForegroundColor White
Write-Host "| TPM 2.0:     $tpmStatus".PadRight(64) "|" -ForegroundColor White
Write-Host "+================================================================+" -ForegroundColor Cyan

# Only show installation instructions if Windows is not installed
if (-not $windowsInstalled) {
    Write-Host "`n+================================================================+" -ForegroundColor Yellow
    Write-Host "|          MANUAL INSTALLATION REQUIRED                          |" -ForegroundColor Yellow
    Write-Host "+================================================================+" -ForegroundColor Yellow
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Step 1: Start VM and Install Windows                          |" -ForegroundColor White
Write-Host "|  ----------------------------------------------------------    |" -ForegroundColor DarkGray
Write-Host "|    * Open Hyper-V Manager                                      |" -ForegroundColor White
Write-Host "|    * Right-click '$WorkerName' -> Connect -> Start".PadRight(64) "|" -ForegroundColor White
Write-Host "|    * Wait for Windows Setup to boot from ISO                   |" -ForegroundColor White
Write-Host "|    * UEFI fails! Restart will appear-> press tab and then space|" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Step 2: Windows Setup Choices (MINIMAL INPUT REQUIRED)        |" -ForegroundColor White
Write-Host "|  ----------------------------------------------------------    |" -ForegroundColor DarkGray
Write-Host "|    * Language: English (United States) -> Next                 |" -ForegroundColor White
Write-Host "|    * Install now                                               |" -ForegroundColor White
Write-Host "|    * Product key: Skip / I don't have a product key            |" -ForegroundColor White
Write-Host "|    * Edition: Windows 10/11 Pro -> Next                        |" -ForegroundColor White
Write-Host "|    * Accept license -> Next                                    |" -ForegroundColor White
Write-Host "|    * Custom: Install Windows only (advanced)                   |" -ForegroundColor White
Write-Host "|    * Partition: Select 'Drive 0 Unallocated Space' -> Next     |" -ForegroundColor White
Write-Host "|      (Windows will auto-create partitions)                     |" -ForegroundColor DarkGray
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Step 3: OOBE (Out-of-Box Experience)                          |" -ForegroundColor White
Write-Host "|  ----------------------------------------------------------    |" -ForegroundColor DarkGray
Write-Host "|    * Region: United States -> Yes                              |" -ForegroundColor White
Write-Host "|    * Keyboard: US -> Yes                                       |" -ForegroundColor White
Write-Host "|    * Skip second keyboard layout                               |" -ForegroundColor White
Write-Host "|    * Network: 'I don't have internet' (bottom-left)            |" -ForegroundColor White
Write-Host "|      OR 'Skip for now' / 'Continue with limited setup'         |" -ForegroundColor DarkGray
Write-Host "|      If not an option press shift + F10 and type OOBE\BYPASSNRO|" -ForegroundColor Yellow
Write-Host "|    * Account name:  worker-admin                               |" -ForegroundColor Cyan
Write-Host "|    * Password:      AutoMutate!Password                        |" -ForegroundColor Cyan
Write-Host "|    * Security questions: Answer anything (write them down)     |" -ForegroundColor White
Write-Host "|    * Privacy settings: Disable all (faster)                    |" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|  Step 4: After Desktop Appears (5-10 min)                      |" -ForegroundColor White
Write-Host "|  ----------------------------------------------------------    |" -ForegroundColor DarkGray
Write-Host "|    * From HOST PowerShell (Admin), run:                        |" -ForegroundColor White
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|      `$cred = Get-Credential -UserName 'worker-admin'           |" -ForegroundColor Green
Write-Host "|      (Enter password: AutoMutate!Password)                     |" -ForegroundColor DarkGray
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|      Invoke-Command -VMName '$WorkerName' ``".PadRight(64) "|" -ForegroundColor Green
Write-Host "|        -FilePath '.\scripts\04-vm-init.ps1' ``".PadRight(64) "|" -ForegroundColor Green
Write-Host "|        -ArgumentList '$StaticIP', '$WorkerName', '$Gateway', $Prefix ``".PadRight(64) "|" -ForegroundColor Green
Write-Host "|        -Credential `$cred".PadRight(64) "|" -ForegroundColor Green
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "|    The script will configure network, install tools, etc.      |" -ForegroundColor DarkGray
Write-Host "|                                                                |" -ForegroundColor White
Write-Host "+================================================================+" -ForegroundColor Yellow

    Write-Host "`nCredentials Summary:" -ForegroundColor Magenta
    Write-Host "  Username: worker-admin" -ForegroundColor Cyan
    Write-Host "  Password: AutoMutate!Password" -ForegroundColor Cyan
    Write-Host "  IP:       $StaticIP (will be configured by 04-vm-init.ps1)`n" -ForegroundColor Cyan
} else {
    # Windows is installed - show next steps
    Write-Host "`n+================================================================+" -ForegroundColor Green
    Write-Host "|          VM Ready for Use                                      |" -ForegroundColor Green
    Write-Host "+================================================================+" -ForegroundColor Green
    Write-Host "|                                                                |" -ForegroundColor White
    Write-Host "|  Windows is already installed. Next steps:                     |" -ForegroundColor White
    Write-Host "|                                                                |" -ForegroundColor White
    Write-Host "|  1. Start the VM:                                              |" -ForegroundColor White
    Write-Host "|     Start-VM -Name '$WorkerName'".PadRight(64) "|" -ForegroundColor Cyan
    Write-Host "|                                                                |" -ForegroundColor White
    Write-Host "|  2. If needed, run initialization script:                      |" -ForegroundColor White
    Write-Host "|     `$cred = Get-Credential -UserName 'worker-admin'            |" -ForegroundColor Cyan
    Write-Host "|     Invoke-Command -VMName '$WorkerName' ``".PadRight(64) "|" -ForegroundColor Cyan
    Write-Host "|       -FilePath '.\scripts\04-vm-init.ps1' ``".PadRight(64) "|" -ForegroundColor Cyan
    Write-Host "|       -ArgumentList '$StaticIP', '$WorkerName', '$Gateway', $Prefix ``".PadRight(64) "|" -ForegroundColor Cyan
    Write-Host "|       -Credential `$cred".PadRight(64) "|" -ForegroundColor Cyan
    Write-Host "|                                                                |" -ForegroundColor White
    Write-Host "+================================================================+" -ForegroundColor Green
    Write-Host ""
}

exit 0
