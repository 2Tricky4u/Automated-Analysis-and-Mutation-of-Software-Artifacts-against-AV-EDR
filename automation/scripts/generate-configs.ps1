<#
.SYNOPSIS
    Generate TOML runtime configs from automation/config.yaml

.DESCRIPTION
    This script reads the master config.yaml and generates:
    - controller.toml for the WSL2 controller
    - worker-XX.toml for each worker VM

.PARAMETER ConfigPath
    Path to the YAML config file (default: automation/config.yaml)

.PARAMETER OutputDir
    Directory for generated TOML files (default: automation/generated)

.PARAMETER Force
    Overwrite existing generated files

.PARAMETER DynamicRegistration
    Generate configs for dynamic worker registration (workers self-register on startup)
#>

param(
    [string]$ConfigPath = "",
    [string]$OutputDir = "",
    [switch]$Force,
    [switch]$DynamicRegistration
)

$ErrorActionPreference = "Stop"

# Determine project root (script is in automation/scripts/)
$ScriptDir = $PSScriptRoot
$AutomationDir = Split-Path $ScriptDir -Parent
$ProjectRoot = Split-Path $AutomationDir -Parent

# Set default paths relative to project root
if (-not $ConfigPath) {
    $ConfigPath = Join-Path $AutomationDir "config.yaml"
}
if (-not $OutputDir) {
    $OutputDir = Join-Path $AutomationDir "generated"
}

# Resolve to absolute paths
$ConfigPath = Resolve-Path $ConfigPath -ErrorAction Stop
$TemplatesDir = Join-Path $AutomationDir "templates"

# Import PowerShell-Yaml module (install if missing)
if (-not (Get-Module -ListAvailable -Name powershell-yaml)) {
    Write-Host "Installing powershell-yaml module..." -ForegroundColor Yellow
    try {
        Install-Module -Name powershell-yaml -Scope CurrentUser -Force -ErrorAction Stop
    } catch {
        Write-Error "Failed to install powershell-yaml module. Please install manually: Install-Module powershell-yaml"
        exit 1
    }
}
Import-Module powershell-yaml -ErrorAction Stop

# Helper: Replace {{TOKENS}} in template
function Expand-Template {
    param(
        [string]$Template,
        [hashtable]$Values
    )

    $result = $Template
    foreach ($key in $Values.Keys) {
        $value = $Values[$key]
        # Convert booleans to lowercase strings for TOML
        if ($value -is [bool]) {
            $value = $value.ToString().ToLower()
        }
        # Escape backslashes for TOML paths (\ -> \\)
        # In regex replacement, we need to double-escape: \ -> \\\\
        if ($value -is [string]) {
            $value = $value -replace '\\', '\\\\'
        }
        $result = $result -replace [regex]::Escape("{{$key}}"), $value
    }
    return $result
}

# Helper: Calculate worker IP from base + offset
function Get-WorkerIp {
    param(
        [string]$BaseIp,
        [int]$Offset
    )

    $parts = $BaseIp.Split('.')
    if ($parts.Count -ne 4) {
        throw "Invalid IP address: $BaseIp"
    }

    $lastOctet = [int]$parts[3] + $Offset
    return "$($parts[0]).$($parts[1]).$($parts[2]).$lastOctet"
}

# Helper: Convert bool to TOML format
function ConvertTo-TomlBool {
    param([object]$Value)
    if ($Value -is [bool]) {
        return $Value.ToString().ToLower()
    }
    return $Value
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "AutoMutate++ Config Generator" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "Project root: $ProjectRoot" -ForegroundColor Gray
Write-Host "Config path: $ConfigPath" -ForegroundColor Gray
Write-Host "Templates dir: $TemplatesDir" -ForegroundColor Gray
Write-Host "Output dir: $OutputDir" -ForegroundColor Gray
Write-Host ""

# Validate input
if (-not (Test-Path $ConfigPath)) {
    Write-Error "Config file not found: $ConfigPath"
    exit 1
}

# Validate templates exist
$controllerTemplatePath = Join-Path $TemplatesDir "controller.toml.template"
$workerTemplatePath = Join-Path $TemplatesDir "worker.toml.template"

if (-not (Test-Path $controllerTemplatePath)) {
    Write-Error "Controller template not found: $controllerTemplatePath"
    exit 1
}

if (-not (Test-Path $workerTemplatePath)) {
    Write-Error "Worker template not found: $workerTemplatePath"
    exit 1
}

# Create output directory
if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
    Write-Host "Created output directory: $OutputDir" -ForegroundColor Green
} elseif (-not $Force) {
    $existing = Get-ChildItem $OutputDir -Filter "*.toml" -ErrorAction SilentlyContinue
    if ($existing) {
        Write-Warning "Output directory already contains TOML files."
        $response = Read-Host "Overwrite? (y/N)"
        if ($response -ne 'y' -and $response -ne 'Y') {
            Write-Host "Aborted by user." -ForegroundColor Yellow
            exit 0
        }
    }
}

# Load YAML config
Write-Host "Loading config from: $ConfigPath" -ForegroundColor White
$yamlContent = Get-Content $ConfigPath -Raw
$config = ConvertFrom-Yaml $yamlContent

# Generate controller.toml
Write-Host ""
Write-Host "Generating controller.toml..." -ForegroundColor Cyan

$controllerTemplate = Get-Content $controllerTemplatePath -Raw

$controllerValues = @{
    "NETWORK_HOST_IP" = $config.network.host_ip
    "CONTROLLER_GRPC_PORT" = $config.controller.grpc_port
    "CONTROLLER_ELASTICSEARCH_PORT" = $config.controller.elasticsearch_port
    "CONTROLLER_MAX_CONNECTIONS" = $config.controller.max_connections
    "CONTROLLER_REQUEST_TIMEOUT_SECS" = $config.controller.request_timeout_secs
    "TRIAGE_MODEL_TYPE" = $config.controller.triage.model_type
    "TRIAGE_CONFIDENCE_THRESHOLD" = $config.controller.triage.confidence_threshold
    "TRIAGE_MIN_SUPPORT_COUNT" = $config.controller.triage.min_support_count
    "TRIAGE_MAX_HYPOTHESES" = $config.controller.triage.max_hypotheses
    "TRIAGE_FEATURE_IMPORTANCE_METHOD" = $config.controller.triage.feature_importance_method
    "MUTATOR_MAX_MUTATIONS" = $config.controller.mutator.max_mutations_per_artifact
    "MUTATOR_WEIGHT_COVERAGE" = $config.controller.mutator.selector_weights.coverage
    "MUTATOR_WEIGHT_EVASION" = $config.controller.mutator.selector_weights.evasion
    "MUTATOR_WEIGHT_SIMILARITY" = $config.controller.mutator.selector_weights.similarity
    "MUTATOR_PROB_AST" = $config.controller.mutator.probabilities.ast_transform
    "MUTATOR_PROB_BINARY" = $config.controller.mutator.probabilities.binary_transform
    "MUTATOR_PROB_BEHAVIORAL" = $config.controller.mutator.probabilities.behavioral_transform
    "SCHEDULER_QUEUE_CAPACITY" = $config.controller.scheduler.queue_capacity
    "SCHEDULER_MAX_CONCURRENT" = $config.controller.scheduler.max_concurrent_runs_per_worker
    "SCHEDULER_RUN_TIMEOUT_SECS" = $config.controller.scheduler.run_timeout_secs
    "SCHEDULER_POLL_INTERVAL" = $config.controller.scheduler.poll_interval_ms
    "SCHEDULER_HEALTH_TIMEOUT" = $config.controller.scheduler.health_timeout_seconds
    "SCHEDULER_MAX_RETRIES" = $config.controller.scheduler.max_retries
    "SCHEDULER_RETRY_BACKOFF_SECS" = $config.controller.scheduler.retry_backoff_secs
    "CORPUS_STORAGE_PATH" = $config.controller.corpus.storage_path
    "CORPUS_MAX_SIZE" = $config.controller.corpus.max_size
    "CORPUS_PRIORITIZATION" = $config.controller.corpus.prioritization
    "CORPUS_NOVELTY_THRESHOLD" = $config.controller.corpus.novelty_threshold
    "LOGGING_LEVEL" = $config.logging.level
    "TELEMETRY_COLLECTION_INTERVAL_MS" = $config.controller.telemetry.collection_interval_ms
    "TELEMETRY_API_TRACING_ENABLED" = (ConvertTo-TomlBool $config.controller.telemetry.api_tracing_enabled)
    "TELEMETRY_BB_COVERAGE_ENABLED" = (ConvertTo-TomlBool $config.controller.telemetry.bb_coverage_enabled)
    "TELEMETRY_LINE_TRACING_ENABLED" = (ConvertTo-TomlBool $config.controller.telemetry.line_tracing_enabled)
    "REDEDR_ENABLE_ETW" = (ConvertTo-TomlBool $config.rededr.enable_etw)
    "REDEDR_DATA_DIR" = $config.rededr.data_dir
    "REDEDR_API_PORT" = $config.rededr.api_port
    "DIFFERENTIAL_ENABLED" = (ConvertTo-TomlBool $config.controller.differential.enabled)
    "DIFFERENTIAL_LIFT_THRESHOLD" = $config.controller.differential.lift_threshold
    "DIFFERENTIAL_MIN_CONFIDENCE" = $config.controller.differential.min_confidence
}

$controllerToml = Expand-Template -Template $controllerTemplate -Values $controllerValues
$controllerOutput = Join-Path $OutputDir "controller.toml"
Set-Content -Path $controllerOutput -Value $controllerToml -NoNewline
Write-Host "  Created: $controllerOutput" -ForegroundColor Green

# Generate worker.toml for each VM
Write-Host ""
Write-Host "Generating worker configs..." -ForegroundColor Cyan

$workerTemplate = Get-Content $workerTemplatePath -Raw
$workerIndex = 0

foreach ($osType in @('windows10', 'windows11')) {
    $workerConfig = $config.workers.$osType

    if ($null -eq $workerConfig -or $workerConfig.count -eq 0) {
        Write-Host "  Skipping $osType (count = 0)" -ForegroundColor Gray
        continue
    }

    Write-Host "  Processing $osType workers (count: $($workerConfig.count))..." -ForegroundColor White

    for ($i = 0; $i -lt $workerConfig.count; $i++) {
        $workerIp = Get-WorkerIp -BaseIp $workerConfig.ip_start -Offset $i

        # Use actual VM hostname as worker ID (e.g., "win10-worker-01")
        $workerNumber = $i + 1
        $vmName = "$($workerConfig.name_prefix)-{0:D2}" -f $workerNumber
        $workerId = $vmName

        $workerValues = @{
            "WORKER_ID" = $workerId
            "WORKER_IP" = $workerIp
            "WORKER_OS_VERSION" = $osType
            "NETWORK_HOST_IP" = $config.network.host_ip
            "WORKER_LISTEN_PORT" = $config.listen_port
            "CONTROLLER_GRPC_PORT" = $config.controller.grpc_port
            "HARNESS_WORKING_DIRECTORY" = $workerConfig.harness.working_directory
            "HARNESS_EXECUTION_TIMEOUT_SECS" = $workerConfig.harness.execution_timeout_secs
            "HARNESS_CLEANUP_ENABLED" = (ConvertTo-TomlBool $workerConfig.harness.cleanup_enabled)
            "HARNESS_SANDBOX_ENABLED" = (ConvertTo-TomlBool $workerConfig.harness.sandbox_enabled)
            "HARNESS_SANDBOX_LOW_INTEGRITY" = (ConvertTo-TomlBool $workerConfig.harness.sandbox_low_integrity)
            "HARNESS_SANDBOX_JOB_OBJECT" = (ConvertTo-TomlBool $workerConfig.harness.sandbox_job_object)
            "HARNESS_MONITOR_CHILDREN" = (ConvertTo-TomlBool $workerConfig.harness.monitor_children)
            "HARNESS_MAX_CHILD_DEPTH" = $workerConfig.harness.max_child_depth
            "TELEMETRY_STREAM_BUFFER_SIZE" = $workerConfig.telemetry.stream_buffer_size
            "TELEMETRY_FLUSH_INTERVAL_MS" = $workerConfig.telemetry.flush_interval_ms
            "TELEMETRY_ETW_ENABLED" = (ConvertTo-TomlBool $workerConfig.telemetry.etw_enabled)
            "TELEMETRY_ETW_BUFFER_SIZE_KB" = $workerConfig.telemetry.etw_buffer_size_kb
            "TELEMETRY_API_TRACING_ENABLED" = (ConvertTo-TomlBool $workerConfig.telemetry.api_tracing_enabled)
            "TELEMETRY_BB_COVERAGE_ENABLED" = (ConvertTo-TomlBool $workerConfig.telemetry.bb_coverage_enabled)
            "TELEMETRY_LINE_TRACING_ENABLED" = (ConvertTo-TomlBool $workerConfig.telemetry.line_tracing_enabled)
            "BUILD_RUST_TOOLCHAIN" = $workerConfig.build.rust_toolchain
            "BUILD_DEFAULT_TRACE_MODE" = $workerConfig.build.default_trace_mode
            "BUILD_OPTIMIZATION_LEVEL" = $workerConfig.build.optimization_level
            "REDEDR_ENABLE_ETW" = (ConvertTo-TomlBool $config.rededr.enable_etw)
            "REDEDR_API_PORT" = $config.rededr.api_port
            "REDEDR_DATA_DIR" = $config.rededr.data_dir
            "LOGGING_LEVEL" = $config.logging.level
        }

        $workerToml = Expand-Template -Template $workerTemplate -Values $workerValues

        # Use VM hostname as filename (e.g., "win10-worker-00.toml")
        $workerOutput = Join-Path $OutputDir "$vmName.toml"

        # Add header comment based on registration mode
        if ($DynamicRegistration) {
            $header = @"
# Worker Runtime Configuration
# Generated from automation/config.yaml
# Do not edit manually - changes will be overwritten
#
# =============================================================================
# Registration Mode: DYNAMIC (worker self-registers on startup)
# =============================================================================
# This worker will auto-register with the controller when it starts.
# The controller does NOT need to read this file at startup.
#
# To deploy this worker to a remote VM:
#   .\scripts\workers\deploy-remote-worker.ps1 ``
#       -RemoteHost <IP> ``
#       -Username <user> ``
#       -WorkerConfigPath "$workerOutput"
#
# The worker will:
#   1. Auto-detect capabilities (RedEDR, Defender, ETW, GPU)
#   2. Register with controller via gRPC
#   3. Appear in worker pool immediately (no controller restart needed!)
# =============================================================================

"@
            Set-Content -Path $workerOutput -Value ($header + $workerToml) -NoNewline
        } else {
            Set-Content -Path $workerOutput -Value $workerToml -NoNewline
        }

        Write-Host "    Created: $workerOutput (IP: $workerIp)" -ForegroundColor Green

        $workerIndex++
    }
}

# Summary
Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Config Generation Complete!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "Generated files in: $OutputDir" -ForegroundColor White
Write-Host "  - controller.toml (1 file)" -ForegroundColor White
Write-Host "  - <hostname>.toml ($workerIndex worker files)" -ForegroundColor White
Write-Host ""
if ($DynamicRegistration) {
    Write-Host "Registration Mode: DYNAMIC (workers self-register on startup)" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Next steps:" -ForegroundColor Yellow
    Write-Host "  1. Review generated configs: ls $OutputDir" -ForegroundColor White
    Write-Host "  2. Deploy controller to WSL2: .\scripts\02-wsl-bootstrap.sh" -ForegroundColor White
    Write-Host "  3. Deploy workers using automated script:" -ForegroundColor White
    Write-Host "     .\scripts\workers\deploy-remote-worker.ps1 -RemoteHost <IP> -Username <user> -WorkerConfigPath <path>" -ForegroundColor Gray
    Write-Host "     Example: .\scripts\workers\deploy-remote-worker.ps1 -RemoteHost 20.123.45.67 -Username vmadmin -WorkerConfigPath $OutputDir\win10-worker-01.toml" -ForegroundColor Gray
    Write-Host "  4. Verify workers registered: .\scripts\workers\list-workers.ps1" -ForegroundColor White
    Write-Host ""
    Write-Host "Benefits of dynamic registration:" -ForegroundColor Yellow
    Write-Host "  - No controller restart needed when adding workers" -ForegroundColor Gray
    Write-Host "  - Workers auto-detect capabilities (RedEDR, Defender, ETW, GPU)" -ForegroundColor Gray
    Write-Host "  - Deploy to cloud VMs (Azure, AWS, etc.) with single command" -ForegroundColor Gray
} else {
    Write-Host "Registration Mode: STATIC (controller reads worker configs at startup)" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Next steps:" -ForegroundColor Yellow
    Write-Host "  1. Review generated configs: ls $OutputDir" -ForegroundColor White
    Write-Host "  2. Deploy controller to WSL2: .\scripts\02-wsl-bootstrap.sh" -ForegroundColor White
    Write-Host "  3. Deploy workers to VMs: Copy <hostname>.toml to C:\AutoMutate\worker.toml" -ForegroundColor White
    Write-Host "  4. Restart controller to load new worker configs" -ForegroundColor White
    Write-Host ""
    Write-Host "Tip: Use -DynamicRegistration flag for zero-downtime worker deployment:" -ForegroundColor Yellow
    Write-Host "  .\generate-configs.ps1 -DynamicRegistration -Force" -ForegroundColor Gray
}
Write-Host ""
