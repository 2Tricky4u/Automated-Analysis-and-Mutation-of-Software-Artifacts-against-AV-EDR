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

    # Import powershell-yaml module (install if missing)
    if (-not (Get-Module -ListAvailable -Name powershell-yaml)) {
        Write-Warning "Installing powershell-yaml module..."
        try {
            Install-Module -Name powershell-yaml -Scope CurrentUser -Force -ErrorAction Stop
        } catch {
            throw "Failed to install powershell-yaml module. Please install manually: Install-Module powershell-yaml"
        }
    }
    Import-Module powershell-yaml -ErrorAction Stop

    # Parse YAML using proper parser
    $yamlContent = Get-Content $ConfigPath -Raw
    $config = ConvertFrom-Yaml $yamlContent

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

    if (-not $config.workers) {
        throw "Config file missing 'workers' section"
    }

    # Use ArrayList for proper Count behavior
    $workers = New-Object System.Collections.ArrayList

    # Generate Windows 10 workers
    if ($config.workers.windows10 -and $config.workers.windows10.count -gt 0) {
        $template = $config.workers.windows10
        $count = $template.count

        for ($i = 0; $i -lt $count; $i++) {
            $workerNumber = $i + 1
            $workerName = "$($template.name_prefix)-{0:D2}" -f $workerNumber
            $workerIP = Get-IncrementedIP -IPAddress $template.ip_start -Offset $i

            $null = $workers.Add(@{
                Name = $workerName
                Os = "windows10"
                Edition = $template.edition
                IsoPath = $template.iso_path
                IP = $workerIP
                CpuCount = $template.cpu_count
                MemoryGB = $template.memory_gb
                DiskGB = $template.disk_gb
            })
        }
    }

    # Generate Windows 11 workers
    if ($config.workers.windows11 -and $config.workers.windows11.count -gt 0) {
        $template = $config.workers.windows11
        $count = $template.count

        for ($i = 0; $i -lt $count; $i++) {
            $workerNumber = $i + 1
            $workerName = "$($template.name_prefix)-{0:D2}" -f $workerNumber
            $workerIP = Get-IncrementedIP -IPAddress $template.ip_start -Offset $i

            $null = $workers.Add(@{
                Name = $workerName
                Os = "windows11"
                Edition = $template.edition
                IsoPath = $template.iso_path
                IP = $workerIP
                CpuCount = $template.cpu_count
                MemoryGB = $template.memory_gb
                DiskGB = $template.disk_gb
            })
        }
    }

    if ($workers.Count -eq 0) {
        throw "No workers defined in config. Set 'count' > 0 for windows10 or windows11 templates."
    }

    # Use comma operator to prevent PowerShell from unwrapping single-element arrays
    # Without this, a single worker returns as hashtable instead of array[1]
    return , @($workers)
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
