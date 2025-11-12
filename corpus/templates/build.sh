#!/usr/bin/env bash
# Build script for loader_v1 using LLVM/Clang cross-compilation from WSL
# Based on setup-llvm-crossbuild.sh approach

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}=========================================="
echo "Building loader_v1 (LLVM/Clang)"
echo -e "==========================================${NC}"

# Check if xwin SDK is installed
XWIN_DIR="${HOME}/.xwin"
if [ ! -d "$XWIN_DIR" ]; then
    echo -e "${RED}Error: xwin SDK not found at $XWIN_DIR${NC}"
    echo "Run: automation/scripts/setup-llvm-crossbuild.sh"
    exit 1
fi

# Source file
SOURCE="loader.c"
OUTPUT="loader_clang.exe"

# Compile with clang (cross-compile to Windows MSVC)
echo ""
echo -e "${YELLOW}Compiling $SOURCE -> $OUTPUT${NC}"

clang -target x86_64-pc-windows-msvc \
  -isystem "$XWIN_DIR/crt/include" \
  -isystem "$XWIN_DIR/sdk/include/ucrt" \
  -isystem "$XWIN_DIR/sdk/include/shared" \
  -isystem "$XWIN_DIR/sdk/include/um" \
  -isystem "$XWIN_DIR/sdk/include/winrt" \
  -L"$XWIN_DIR/crt/lib/x86_64" \
  -L"$XWIN_DIR/sdk/lib/ucrt/x86_64" \
  -L"$XWIN_DIR/sdk/lib/um/x86_64" \
  -Wl,-defaultlib:libcmt -Wl,-defaultlib:kernel32 \
  -fuse-ld=lld \
  -Wl,/subsystem:console \
  -O2 \
  -o "$OUTPUT" \
  "$SOURCE"

if [ -f "$OUTPUT" ]; then
    echo -e "${GREEN}✓ Build successful!${NC}"
    ls -lh "$OUTPUT"
    file "$OUTPUT"
else
    echo -e "${RED}✗ Build failed${NC}"
    exit 1
fi

echo ""
echo -e "${GREEN}=========================================="
echo "Build Complete!"
echo -e "==========================================${NC}"
echo ""
echo "Run: ./$OUTPUT"
