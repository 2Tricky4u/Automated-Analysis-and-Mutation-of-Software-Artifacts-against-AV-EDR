#!/usr/bin/env bash
set -euo pipefail

# Ensure Rust env is loaded (important when called via non-interactive shell)
if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
fi

# Resolve repo root reliably (works even if script is called from elsewhere)
REPO_ROOT="$(git rev-parse --show-toplevel)"

echo "[build-linux] REPO_ROOT = $REPO_ROOT"

# Per-OS target directory
export CARGO_TARGET_DIR="$REPO_ROOT/target-wsl"

# Build + run scheduler
cargo build
cargo run --release -p scheduler