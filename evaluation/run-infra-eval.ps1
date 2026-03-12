<#
.SYNOPSIS
    Run the complete infrastructure evaluation pipeline (I1-I15).
.DESCRIPTION
    Generates all infrastructure-level metrics in 7 stages:
      0. Setup paths and output directory
      1. Build-pipeline benchmarks (synthetic, I1/I2/I3/I5/I9/I14/I15)
      2. Triage-pipeline benchmarks (real GA campaign data, I7/I8/I10/I11/I12/I13)
      3. Merge the two InfraEvalDataset JSONs
      4. Score with infra-eval
      5. Generate figures (plots.py --infra)
      6. Generate LaTeX summary table
      7. Print summary

    Skipped experiments:
      - I4 (binary mutation): requires full Clang/LLVM + xwin SDK at runtime
      - I6 (instrumentation overhead): requires full Clang/LLVM + xwin SDK at runtime
#>

$ErrorActionPreference = "Continue"

# ── Stage 0: Setup ──────────────────────────────────────────────────────

$repoRoot = (git -C $PSScriptRoot rev-parse --show-toplevel).Trim()
$dataDir  = Join-Path (Join-Path $repoRoot "evaluation") "data"
$outDir   = Join-Path $dataDir "infra"
$scriptDir = Join-Path (Join-Path $repoRoot "evaluation") "scripts"

# Real campaign dataset (GA has richest signal: 51 rounds, 6 evasions)
$realDataset = Join-Path $dataDir "ga_eval.json"

# Output paths
$buildJson   = Join-Path $outDir "infra_build.json"
$realJson    = Join-Path $outDir "infra_real.json"
$mergedJson  = Join-Path $outDir "infra_dataset_merged.json"
$reportJson  = Join-Path $outDir "infra_eval_report.json"
$reportCsv   = Join-Path $outDir "infra_eval_metrics.csv"
$summaryTex  = Join-Path $outDir "infra_eval_summary.tex"
$figDir      = Join-Path $outDir "infra_figures"

Write-Host "=== Infrastructure Evaluation Pipeline ===" -ForegroundColor Cyan
Write-Host "  Repo root:    $repoRoot"
Write-Host "  Output dir:   $outDir"
Write-Host "  Real dataset: $realDataset"
Write-Host ""

# Create output directories
if (-not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
    Write-Host "  Created: $outDir" -ForegroundColor Green
}
if (-not (Test-Path $figDir)) {
    New-Item -ItemType Directory -Path $figDir -Force | Out-Null
    Write-Host "  Created: $figDir" -ForegroundColor Green
}

# Check real dataset exists
if (-not (Test-Path $realDataset)) {
    Write-Host "[WARN] Real dataset not found: $realDataset" -ForegroundColor Yellow
    Write-Host "       Stage 2 will be skipped; only synthetic benchmarks will run." -ForegroundColor Yellow
    $hasRealData = $false
} else {
    $hasRealData = $true
}

$failed  = @()
$skipped = @("I4 (binary mutation - requires toolchain)", "I6 (instrumentation overhead - requires toolchain)")

# ── Stage 1: Build-pipeline benchmarks (synthetic) ──────────────────────

Write-Host "=== [1/7] Build-pipeline benchmarks (I1,I2,I3,I5,I9,I14,I15) ===" -ForegroundColor Cyan
Write-Host "  These exercise the build crate APIs with synthetic inputs." -ForegroundColor Gray

$buildArgs = @(
    "run", "-p", "evaluation", "--features", "build-bench", "--bin", "infra-bench", "--",
    "--experiments", "i1,i2,i3,i5,i9,i14,i15",
    "--output", $buildJson
)
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) {
    Write-Host "  [ERR] Stage 1 failed" -ForegroundColor Red
    $failed += "Stage 1 (build benchmarks)"
} else {
    Write-Host "  [OK] Build benchmarks -> $buildJson" -ForegroundColor Green
}
Write-Host ""

# ── Stage 2: Triage-pipeline benchmarks (real data) ─────────────────────

Write-Host "=== [2/7] Triage-pipeline benchmarks (I7,I8,I10,I11,I12,I13) ===" -ForegroundColor Cyan

if ($hasRealData) {
    Write-Host "  Using real GA campaign data for triage experiments." -ForegroundColor Gray

    $realArgs = @(
        "run", "-p", "evaluation", "--features", "build-bench", "--bin", "infra-bench", "--",
        "--dataset", $realDataset,
        "--experiments", "i7,i8,i10,i11,i12,i13",
        "--output", $realJson
    )
    & cargo @realArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] Stage 2 failed" -ForegroundColor Red
        $failed += "Stage 2 (triage benchmarks)"
    } else {
        Write-Host "  [OK] Triage benchmarks -> $realJson" -ForegroundColor Green
    }
} else {
    Write-Host "  [SKIP] No real dataset - running synthetic fallback." -ForegroundColor Yellow

    $realArgs = @(
        "run", "-p", "evaluation", "--features", "build-bench", "--bin", "infra-bench", "--",
        "--experiments", "i7,i8,i10,i11,i12,i13",
        "--output", $realJson
    )
    & cargo @realArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] Stage 2 (synthetic fallback) failed" -ForegroundColor Red
        $failed += "Stage 2 (triage benchmarks - synthetic)"
    } else {
        Write-Host "  [OK] Triage benchmarks (synthetic) -> $realJson" -ForegroundColor Green
    }
}
Write-Host ""

# ── Stage 3: Merge datasets ────────────────────────────────────────────

Write-Host "=== [3/7] Merge InfraEvalDataset JSONs ===" -ForegroundColor Cyan

if ((Test-Path $buildJson) -and (Test-Path $realJson)) {
    $mergePy = @"
import json, sys
a = json.load(open(sys.argv[1], encoding='utf-8'))
b = json.load(open(sys.argv[2], encoding='utf-8'))
a.update({k: v for k, v in b.items() if v is not None})
with open(sys.argv[3], 'w', encoding='utf-8') as f:
    json.dump(a, f, indent=2)
print(f"  Merged {len(a)} fields -> {sys.argv[3]}")
"@
    $mergePy | python - $buildJson $realJson $mergedJson
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] Merge failed" -ForegroundColor Red
        $failed += "Stage 3 (merge)"
    } else {
        Write-Host "  [OK] Merged dataset -> $mergedJson" -ForegroundColor Green
    }
} elseif (Test-Path $buildJson) {
    Write-Host "  [WARN] Only build JSON available, copying as merged." -ForegroundColor Yellow
    Copy-Item $buildJson $mergedJson
} elseif (Test-Path $realJson) {
    Write-Host "  [WARN] Only real JSON available, copying as merged." -ForegroundColor Yellow
    Copy-Item $realJson $mergedJson
} else {
    Write-Host "  [ERR] No input JSONs exist - cannot merge." -ForegroundColor Red
    $failed += "Stage 3 (merge - no inputs)"
}
Write-Host ""

# ── Stage 4: Score with infra-eval ──────────────────────────────────────

Write-Host "=== [4/7] Score with infra-eval ===" -ForegroundColor Cyan

if (Test-Path $mergedJson) {
    $evalArgs = @(
        "run", "-p", "evaluation", "--features", "full", "--bin", "infra-eval", "--",
        "--input", $mergedJson,
        "--output", $reportJson,
        "--csv", $reportCsv
    )
    & cargo @evalArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] infra-eval failed" -ForegroundColor Red
        $failed += "Stage 4 (infra-eval)"
    } else {
        Write-Host "  [OK] Report  -> $reportJson" -ForegroundColor Green
        Write-Host "  [OK] Metrics -> $reportCsv" -ForegroundColor Green
    }
} else {
    Write-Host "  [SKIP] No merged dataset." -ForegroundColor Yellow
    $failed += "Stage 4 (skipped - no input)"
}
Write-Host ""

# ── Stage 5: Generate figures ───────────────────────────────────────────

Write-Host "=== [5/7] Generate infrastructure figures ===" -ForegroundColor Cyan

if (Test-Path $reportJson) {
    $plotScript = Join-Path $scriptDir "plots.py"
    & python $plotScript --infra --input $reportJson --outdir $figDir
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] plots.py --infra failed" -ForegroundColor Red
        $failed += "Stage 5 (plots)"
    } else {
        $figCount = (Get-ChildItem -Path $figDir -File | Measure-Object).Count
        Write-Host "  [OK] $figCount figures -> $figDir" -ForegroundColor Green
    }
} else {
    Write-Host "  [SKIP] No report JSON." -ForegroundColor Yellow
    $failed += "Stage 5 (skipped - no input)"
}
Write-Host ""

# ── Stage 6: Generate LaTeX summary table ───────────────────────────────

Write-Host "=== [6/7] Generate LaTeX summary table ===" -ForegroundColor Cyan

if (Test-Path $reportCsv) {
    $latexPy = @"
import csv, sys

csv_path = sys.argv[1]
tex_path = sys.argv[2]
has_real = sys.argv[3] == 'True'

# Experiment metadata: ID -> (short name, theme group, source)
EXPERIMENT_INFO = {
    'i1':  ('Payload Encoding',       'build',  'Synthetic'),
    'i2':  ('AST Mutation Impact',     'build',  'Synthetic'),
    'i3':  ('IR Mutation Analysis',    'build',  'Synthetic'),
    'i4':  ('Binary Mutation',         'build',  'Skipped'),
    'i5':  ('Template Assembly',       'build',  'Synthetic'),
    'i6':  ('Instrumentation',         'build',  'Skipped'),
    'i7':  ('Token Extraction',        'triage', 'Real' if has_real else 'Synthetic'),
    'i8':  ('Token Scoring',           'triage', 'Real' if has_real else 'Synthetic'),
    'i9':  ('Input Diversity',         'build',  'Synthetic'),
    'i10': ('Oracle Stability',        'triage', 'Real' if has_real else 'Synthetic'),
    'i11': ('Selector Comparison',     'triage', 'Real' if has_real else 'Synthetic'),
    'i12': ('Guidance Utilization',    'triage', 'Real' if has_real else 'Synthetic'),
    'i13': ('Convergence Simulation',  'triage', 'Real' if has_real else 'Synthetic'),
    'i14': ('Line Tracing',           'build',  'Synthetic'),
    'i15': ('Shellcode Checkpoints',  'build',  'Synthetic'),
}

# Read CSV
rows = []
with open(csv_path, encoding='utf-8') as f:
    reader = csv.DictReader(f)
    for row in reader:
        mid = row.get('metric_id', '')
        if not mid.startswith('infra.'):
            continue
        parts = mid.replace('infra.', '').split('.', 1)
        exp_id = parts[0] if parts else ''
        rows.append({
            'exp_id': exp_id,
            'metric_id': mid,
            'label': row.get('label', ''),
            'value': float(row.get('value', 0)),
            'n': int(row.get('n', 0)),
        })

if not rows:
    print("  No infra metrics found in CSV")
    sys.exit(0)

# Group by theme
build_rows  = [r for r in rows if EXPERIMENT_INFO.get(r['exp_id'], ('','build',''))[1] == 'build']
triage_rows = [r for r in rows if EXPERIMENT_INFO.get(r['exp_id'], ('','triage',''))[1] == 'triage']

def tex_escape(s):
    return s.replace('&', r'\&').replace('_', r'\_').replace('%', r'\%').replace('#', r'\#')

def format_value(v):
    if v == int(v) and abs(v) < 1e6:
        return str(int(v))
    if abs(v) < 0.01 and v != 0:
        return f'{v:.6f}'
    return f'{v:.4f}'

def write_rows(f, row_list):
    current_exp = ''
    for r in sorted(row_list, key=lambda x: x['metric_id']):
        info = EXPERIMENT_INFO.get(r['exp_id'], (r['exp_id'], '', 'Unknown'))
        exp_name = info[0]
        source = info[2]
        exp_label = f"{r['exp_id'].upper()} {exp_name}"

        if r['exp_id'] != current_exp:
            current_exp = r['exp_id']

        label = tex_escape(r['label'][:55])
        exp_tex = tex_escape(exp_label)
        f.write(f"{exp_tex} & {label} & {format_value(r['value'])} & {r['n']} & {source} \\\\\n")

with open(tex_path, 'w', encoding='utf-8') as f:
    f.write(r'\begin{table*}[htbp]' + '\n')
    f.write(r'\centering' + '\n')
    f.write(r'\caption{Infrastructure-level evaluation metrics. Build-pipeline experiments (I1--I5, I9, I14--I15) use synthetic benchmarks; triage-pipeline experiments (I7--I8, I10--I13) use GA campaign data ($n=51$ rounds). I4 and I6 are skipped (require full toolchain at runtime).}' + '\n')
    f.write(r'\label{tab:infra-metrics}' + '\n')
    f.write(r'\renewcommand{\arraystretch}{1.25}' + '\n')
    f.write(r'\small' + '\n')
    f.write(r'\begin{tabular}{@{}llrrl@{}}' + '\n')
    f.write(r'\toprule' + '\n')
    f.write(r'\textbf{Experiment} & \textbf{Metric} & \textbf{Value} & $n$ & \textbf{Source} \\' + '\n')
    f.write(r'\midrule' + '\n')

    # Build pipeline section
    if build_rows:
        f.write(r"\multicolumn{5}{@{}l}{\textit{Build Pipeline (synthetic benchmarks)}} \\[2pt]" + '\n')
        write_rows(f, build_rows)

    # Triage pipeline section
    if triage_rows:
        f.write(r'\midrule' + '\n')
        src = 'GA campaign, $n=51$ rounds' if has_real else 'synthetic data'
        f.write(r"\multicolumn{5}{@{}l}{\textit{Triage Pipeline (" + src + r")}} \\[2pt]" + '\n')
        write_rows(f, triage_rows)

    f.write(r'\bottomrule' + '\n')
    f.write(r'\end{tabular}' + '\n')
    f.write(r'\end{table*}' + '\n')

print(f"  LaTeX table -> {tex_path}")
print(f"  {len(rows)} metrics in {len(set(r['exp_id'] for r in rows))} experiments")
"@
    $latexPy | python - $reportCsv $summaryTex $hasRealData
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [ERR] LaTeX generation failed" -ForegroundColor Red
        $failed += "Stage 6 (LaTeX)"
    } else {
        Write-Host "  [OK] LaTeX table -> $summaryTex" -ForegroundColor Green
    }
} else {
    Write-Host "  [SKIP] No metrics CSV." -ForegroundColor Yellow
    $failed += "Stage 6 (skipped - no input)"
}
Write-Host ""

# ── Stage 7: Summary ───────────────────────────────────────────────────

Write-Host "=== [7/7] Summary ===" -ForegroundColor Cyan

Write-Host ""
Write-Host "  Output directory: $outDir" -ForegroundColor Gray

$outputs = @(
    @{ Name = "Build benchmarks (synthetic)"; Path = $buildJson },
    @{ Name = "Triage benchmarks (real)";     Path = $realJson },
    @{ Name = "Merged dataset";               Path = $mergedJson },
    @{ Name = "Evaluation report";            Path = $reportJson },
    @{ Name = "Metrics CSV";                  Path = $reportCsv },
    @{ Name = "LaTeX summary";                Path = $summaryTex }
)

foreach ($o in $outputs) {
    if (Test-Path $o.Path) {
        $size = (Get-Item $o.Path).Length
        $sizeStr = if ($size -gt 1MB) { "{0:N1} MB" -f ($size / 1MB) }
                   elseif ($size -gt 1KB) { "{0:N1} KB" -f ($size / 1KB) }
                   else { "$size B" }
        Write-Host "  [OK] $($o.Name): $($o.Path) ($sizeStr)" -ForegroundColor Green
    } else {
        Write-Host "  [--] $($o.Name): not generated" -ForegroundColor DarkGray
    }
}

if (Test-Path $figDir) {
    $figCount = (Get-ChildItem -Path $figDir -File | Measure-Object).Count
    Write-Host "  [OK] Figures: $figDir ($figCount files)" -ForegroundColor Green
}

Write-Host ""
Write-Host "  Skipped experiments:" -ForegroundColor Yellow
foreach ($s in $skipped) { Write-Host "    - $s" -ForegroundColor Yellow }

Write-Host ""
if ($failed.Count -eq 0) {
    Write-Host "  All stages completed successfully." -ForegroundColor Green
} else {
    Write-Host "  $($failed.Count) failure(s):" -ForegroundColor Red
    foreach ($f in $failed) { Write-Host "    - $f" -ForegroundColor Red }
}
