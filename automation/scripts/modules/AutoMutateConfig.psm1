<#
.SYNOPSIS
    Shared configuration module for AutoMutate++ automation scripts

.DESCRIPTION
    Provides functions to parse config.yaml and generate worker VM configurations
    from templates. All scripts should use this module instead of manually parsing YAML.
#>

function Read-AutoMutateConfig {
    <#
    .SYNOPSIS
        Parse config.yaml into a hashtable

    .PARAMETER ConfigPath
        Path to config.yaml file

    .OUTPUTS
        Hashtable with parsed configuration
    #>
    param(
        [Parameter(Mandatory)]
        [string]$ConfigPath
    )

    if (-not (Test-Path $ConfigPath)) {
        throw "Config file not found: $ConfigPath"
    }

    $config = @{}
    $section = $null
    $subsection = $null

    Get-Content $ConfigPath | ForEach-Object {
        $line = $_

        # Skip comments and empty lines
        if ($line -match '^\s*#' -or $line -match '^\s*$') {
            return
        }

        # Top-level section (e.g., "network:")
        if ($line -match '^(\w+):$') {
            $section = $matches[1]
            $config[$section] = @{}
            $subsection = $null
        }
        # Subsection (e.g., "  windows10:")
        elseif ($line -match '^\s{2}(\w+):$' -and $section) {
            $subsection = $matches[1]
            if ($subsection -eq 'windows10' -or $subsection -eq 'windows11') {
                # Worker template subsection
                if (-not $config[$section].ContainsKey($subsection)) {
                    $config[$section][$subsection] = @{}
                }
            } else {
                # Regular subsection
                $config[$section][$subsection] = @{}
            }
        }
        # Key-value pair in subsection
        elseif ($line -match '^\s{4}(\w+):\s*(.+)$' -and $section -and $subsection) {
            $key = $matches[1]
            $value = $matches[2].Trim()

            # Strip inline comments
            if ($value -match '^([^#]+?)\s*#') {
                $value = $matches[1].Trim()
            }

            # Remove quotes
            $value = $value.Trim('"')

            # Convert numbers
            if ($value -match '^\d+$') {
                $value = [int]$value
            }
            $config[$section][$subsection][$key] = $value
        }
        # Key-value pair in top-level section
        elseif ($line -match '^\s{2}(\w+):\s*(.+)$' -and $section) {
            $key = $matches[1]
            $value = $matches[2].Trim()

            # Strip inline comments
            if ($value -match '^([^#]+?)\s*#') {
                $value = $matches[1].Trim()
            }

            # Remove quotes
            $value = $value.Trim('"')

            # Convert numbers
            if ($value -match '^\d+$') {
                $value = [int]$value
            }
            # Convert booleans
            if ($value -eq 'true') { $value = $true }
            if ($value -eq 'false') { $value = $false }
            $config[$section][$key] = $value
        }
    }

    return $config
}

function Get-IncrementedIP {
    <#
    .SYNOPSIS
        Increment an IP address by a given offset

    .PARAMETER IPAddress
        Starting IP address (e.g., "10.200.200.100")

    .PARAMETER Offset
        Number to add to the last octet

    .OUTPUTS
        String with incremented IP address
    #>
    param(
        [Parameter(Mandatory)]
        [string]$IPAddress,

        [Parameter(Mandatory)]
        [int]$Offset
    )

    $octets = $IPAddress.Split('.')
    $lastOctet = [int]$octets[3] + $Offset

    if ($lastOctet -gt 255) {
        throw "IP address overflow: $IPAddress + $Offset results in octet > 255"
    }

    return "$($octets[0]).$($octets[1]).$($octets[2]).$lastOctet"
}

function Get-AutoMutateWorkers {
    <#
    .SYNOPSIS
        Generate list of worker VMs from template configuration

    .PARAMETER ConfigPath
        Path to config.yaml file

    .OUTPUTS
        Array of hashtables, each representing a worker VM with properties:
        - Name: VM name (e.g., "win10-worker-01")
        - Os: Operating system ("windows10" or "windows11")
        - Edition: Windows edition ("Pro", "Enterprise", etc.)
        - IsoPath: Path to Windows ISO
        - IP: Static IP address
        - CpuCount: Number of CPU cores
        - MemoryGB: Memory in gigabytes
        - DiskGB: Disk size in gigabytes
    #>
    param(
        [Parameter(Mandatory)]
        [string]$ConfigPath
    )

    $config = Read-AutoMutateConfig -ConfigPath $ConfigPath

    if (-not $config.ContainsKey('workers')) {
        throw "Config file missing 'workers' section"
    }

    $workers = @()

    # Generate Windows 10 workers
    if ($config.workers.ContainsKey('windows10')) {
        $template = $config.workers.windows10
        $count = $template.count

        for ($i = 0; $i -lt $count; $i++) {
            $workerNumber = $i + 1
            $workerName = "$($template.name_prefix)-$($workerNumber.ToString('D2'))"
            $workerIP = Get-IncrementedIP -IPAddress $template.ip_start -Offset $i

            $workers += @{
                Name = $workerName
                Os = "windows10"
                Edition = $template.edition
                IsoPath = $template.iso_path
                IP = $workerIP
                CpuCount = $template.cpu_count
                MemoryGB = $template.memory_gb
                DiskGB = $template.disk_gb
            }
        }
    }

    # Generate Windows 11 workers
    if ($config.workers.ContainsKey('windows11')) {
        $template = $config.workers.windows11
        $count = $template.count

        for ($i = 0; $i -lt $count; $i++) {
            $workerNumber = $i + 1
            $workerName = "$($template.name_prefix)-$($workerNumber.ToString('D2'))"
            $workerIP = Get-IncrementedIP -IPAddress $template.ip_start -Offset $i

            $workers += @{
                Name = $workerName
                Os = "windows11"
                Edition = $template.edition
                IsoPath = $template.iso_path
                IP = $workerIP
                CpuCount = $template.cpu_count
                MemoryGB = $template.memory_gb
                DiskGB = $template.disk_gb
            }
        }
    }

    if ($workers.Count -eq 0) {
        throw "No workers defined in config. Set 'count' > 0 for windows10 or windows11 templates."
    }

    return $workers
}

function Get-AutoMutateWorkerByName {
    <#
    .SYNOPSIS
        Get a specific worker by name

    .PARAMETER ConfigPath
        Path to config.yaml file

    .PARAMETER WorkerName
        Name of the worker VM

    .OUTPUTS
        Hashtable representing the worker, or $null if not found
    #>
    param(
        [Parameter(Mandatory)]
        [string]$ConfigPath,

        [Parameter(Mandatory)]
        [string]$WorkerName
    )

    $workers = Get-AutoMutateWorkers -ConfigPath $ConfigPath
    return $workers | Where-Object { $_.Name -eq $WorkerName } | Select-Object -First 1
}

function Get-AutoMutateWorkerNames {
    <#
    .SYNOPSIS
        Get list of worker names only

    .PARAMETER ConfigPath
        Path to config.yaml file

    .OUTPUTS
        Array of worker names
    #>
    param(
        [Parameter(Mandatory)]
        [string]$ConfigPath
    )

    $workers = Get-AutoMutateWorkers -ConfigPath $ConfigPath
    return $workers | ForEach-Object { $_.Name }
}

# Export module functions
Export-ModuleMember -Function @(
    'Read-AutoMutateConfig',
    'Get-IncrementedIP',
    'Get-AutoMutateWorkers',
    'Get-AutoMutateWorkerByName',
    'Get-AutoMutateWorkerNames'
)
