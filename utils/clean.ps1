$ErrorActionPreference = "Stop"

# Resolve repo root reliably
$RepoRoot = (git rev-parse --show-toplevel).Trim()

Write-Host "[build-win] REPO_ROOT = $RepoRoot"

# Per-OS target directory
$env:CARGO_TARGET_DIR = "$RepoRoot\target-win"

# Clean
cargo clean