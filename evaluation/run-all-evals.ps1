<#
.SYNOPSIS
    Run evaluation pipeline for all *_eval.json files in evaluation/data/.
.DESCRIPTION
    For each {name}_eval.json, runs:
      1. evaluate          -> {name}_eval_report.json, {name}_eval_metrics.csv, {name}_eval_summary.json
      2. component-eval    -> component_{name}_eval_report.json, component_{name}_eval_metrics.csv
      3. plots.py          -> {name}_figures/
    All outputs are placed in evaluation/data/{name}/.
#>

$ErrorActionPreference = "Stop"

$repoRoot = (git -C $PSScriptRoot rev-parse --show-toplevel).Trim()
$dataDir = Join-Path $repoRoot "evaluation" "data"

$evalFiles = Get-ChildItem -Path $dataDir -Filter "*_eval.json" -File
if ($evalFiles.Count -eq 0) {
    Write-Host "[ERR] No *_eval.json files found in $dataDir" -ForegroundColor Red
    exit 1
}

Write-Host "Found $($evalFiles.Count) evaluation dataset(s):" -ForegroundColor Cyan
foreach ($f in $evalFiles) { Write-Host "  - $($f.Name)" -ForegroundColor Gray }
Write-Host ""

$failed = @()

foreach ($file in $evalFiles) {
    $name = $file.BaseName -replace '_eval$', ''
    $inputFile = $file.Name
    $outDir = Join-Path $dataDir $name

    Write-Host "=== [$name] ===" -ForegroundColor Cyan

    # Create output directory
    if (-not (Test-Path $outDir)) {
        New-Item -ItemType Directory -Path $outDir -Force | Out-Null
        Write-Host "  Created: $outDir" -ForegroundColor Green
    }

    # 1. evaluate
    Write-Host "  [1/3] evaluate..." -ForegroundColor Yellow
    $evalReport = Join-Path $outDir "${name}_eval_report.json"
    $evalCsv = Join-Path $outDir "${name}_eval_metrics.csv"
    $evalSummary = Join-Path $outDir "${name}_eval_summary.json"

    $evalArgs = @(
        "run", "-p", "evaluation", "--features", "full", "--bin", "evaluate", "--",
        "--input", $file.FullName,
        "--json", $evalReport,
        "--csv", $evalCsv,
        "--summary", $evalSummary
    )
    & cargo @evalArgs 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] evaluate failed for $name" -ForegroundColor Red
        $failed += "$name/evaluate"
        continue
    }
    Write-Host "  [OK] evaluate" -ForegroundColor Green

    # 2. component-eval
    Write-Host "  [2/3] component-eval..." -ForegroundColor Yellow
    $compReport = Join-Path $outDir "component_${name}_eval_report.json"
    $compCsv = Join-Path $outDir "component_${name}_eval_metrics.csv"

    $compArgs = @(
        "run", "-p", "evaluation", "--features", "full", "--bin", "component-eval", "--",
        "--input", $file.FullName,
        "--output", $compReport,
        "--csv", $compCsv
    )
    & cargo @compArgs 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] component-eval failed for $name" -ForegroundColor Red
        $failed += "$name/component-eval"
        continue
    }
    Write-Host "  [OK] component-eval" -ForegroundColor Green

    # 3. plots.py
    Write-Host "  [3/3] plots.py..." -ForegroundColor Yellow
    $figDir = Join-Path $outDir "${name}_figures"
    if (-not (Test-Path $figDir)) {
        New-Item -ItemType Directory -Path $figDir -Force | Out-Null
    }

    $plotScript = Join-Path $repoRoot "evaluation" "scripts" "plots.py"
    & python $plotScript --input $compReport --outdir $figDir 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] plots.py failed for $name" -ForegroundColor Red
        $failed += "$name/plots"
        continue
    }
    Write-Host "  [OK] plots.py" -ForegroundColor Green

    Write-Host ""
}

# Summary
Write-Host "=== Done ===" -ForegroundColor Cyan
if ($failed.Count -eq 0) {
    Write-Host "All $($evalFiles.Count) evaluations completed successfully." -ForegroundColor Green
} else {
    Write-Host "$($failed.Count) failure(s):" -ForegroundColor Red
    foreach ($f in $failed) { Write-Host "  - $f" -ForegroundColor Red }
}
