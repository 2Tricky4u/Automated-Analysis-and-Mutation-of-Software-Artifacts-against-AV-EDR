#!/usr/bin/env bash
# Setup LLVM/Clang for cross-compiling C/C++ to Windows MSVC from WSL
# Part of AutoMutate++ build pipeline
#
# Usage:
#   ./setup-llvm-crossbuild.sh
#
# This script:
#   1. Installs LLVM 17 toolchain (clang, lld, llc, opt)
#   2. Installs xwin for Microsoft CRT/SDK
#   3. Downloads Windows SDK files (~300MB)
#   4. Verifies cross-compilation works

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=========================================="
echo "AutoMutate++ LLVM Cross-Build Setup"
echo -e "==========================================${NC}"
echo ""

# Check if running in WSL
if ! grep -qi microsoft /proc/version 2>/dev/null; then
    echo -e "${RED}Error: This script must run in WSL or Linux${NC}"
    exit 1
fi

echo -e "${GREEN} Running in WSL/Linux${NC}"

# Update package list
echo ""
echo -e "${YELLOW}Updating package lists...${NC}"
sudo apt update

# Install LLVM 17 toolchain
LLVM_VERSION=17
echo ""
echo -e "${YELLOW}Installing LLVM ${LLVM_VERSION} toolchain...${NC}"

# Check if already installed
if command -v clang-${LLVM_VERSION} &> /dev/null; then
    echo -e "${GREEN} LLVM ${LLVM_VERSION} already installed${NC}"
else
    # Add LLVM apt repository
    wget -O - https://apt.llvm.org/llvm-snapshot.gpg.key | sudo apt-key add -
    sudo add-apt-repository -y "deb http://apt.llvm.org/$(lsb_release -cs)/ llvm-toolchain-$(lsb_release -cs)-${LLVM_VERSION} main"
    sudo apt update

    # Install LLVM packages
    sudo apt install -y \
        clang-${LLVM_VERSION} \
        lld-${LLVM_VERSION} \
        llvm-${LLVM_VERSION} \
        llvm-${LLVM_VERSION}-dev \
        libclang-${LLVM_VERSION}-dev \
        libc++-${LLVM_VERSION}-dev \
        libc++abi-${LLVM_VERSION}-dev

    echo -e "${GREEN} LLVM ${LLVM_VERSION} installed${NC}"
fi

# Setup symlinks for convenience
echo ""
echo -e "${YELLOW}Setting up symlinks...${NC}"
sudo update-alternatives --install /usr/bin/clang clang /usr/bin/clang-${LLVM_VERSION} 100
sudo update-alternatives --install /usr/bin/clang++ clang++ /usr/bin/clang++-${LLVM_VERSION} 100
sudo update-alternatives --install /usr/bin/lld lld /usr/bin/lld-${LLVM_VERSION} 100
sudo update-alternatives --install /usr/bin/ld.lld ld.lld /usr/bin/ld.lld-${LLVM_VERSION} 100
sudo update-alternatives --install /usr/bin/llc llc /usr/bin/llc-${LLVM_VERSION} 100
sudo update-alternatives --install /usr/bin/opt opt /usr/bin/opt-${LLVM_VERSION} 100
sudo update-alternatives --install /usr/bin/llvm-config llvm-config /usr/bin/llvm-config-${LLVM_VERSION} 100

# Install Rust if needed (for xwin)
if ! command -v cargo &> /dev/null; then
    echo ""
    echo -e "${YELLOW}Installing Rust...${NC}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
else
    echo -e "${GREEN} Rust already installed${NC}"
fi

 # Install xwin for Windows SDK
 echo ""
 echo -e "${YELLOW}Installing xwin (Microsoft CRT/SDK downloader)...${NC}"
 if command -v xwin &> /dev/null; then
     echo -e "${GREEN} xwin already installed${NC}"
 else
     cargo install xwin --locked
 fi

 # Ensure xwin cache stays on Linux filesystem (avoid /mnt/c)
 XWIN_CACHE_DIR="${HOME}/.xwin-cache"
 export XWIN_CACHE_DIR

 # If a Windows-side cache exists from previous runs, remove it to avoid EXDEV issues
 WIN_CACHE_CANDIDATE="/mnt/c/Users/${USER:-$LOGNAME}/.xwin-cache"
 if [ -d "$WIN_CACHE_CANDIDATE" ]; then
     echo -e "${YELLOW}Found Windows-side xwin cache at $WIN_CACHE_CANDIDATE — removing to prevent cross-device moves...${NC}"
     rm -rf "$WIN_CACHE_CANDIDATE"
 fi

 # Gentle warning if running under /mnt/*
 case "$PWD" in
   /mnt/*)
     echo -e "${YELLOW}Note: you're running from $PWD (Windows filesystem). We'll force xwin cache/output to Linux home to avoid EXDEV.${NC}"
     ;;
 esac

# Download Windows SDK
XWIN_DIR="$HOME/.xwin"
if [ -d "$XWIN_DIR" ]; then
    echo -e "${GREEN} Windows SDK already downloaded${NC}"
else
    echo ""
    echo -e "${YELLOW}Downloading Windows SDK (~1GB)...${NC}"
    xwin --accept-license splat \
         --cache-dir "$XWIN_CACHE_DIR" \
         --output "$XWIN_DIR" \
         --include-arch x86_64 \
         --include-debug-libs false \
         --include crt,ucrt,sdk
    echo -e "${GREEN} Windows SDK downloaded to $XWIN_DIR${NC}"
fi
# Verify installation
echo ""
echo -e "${BLUE}=========================================="
echo "Verifying Installation"
echo -e "==========================================${NC}"

echo ""
echo "LLVM Version:"
clang --version | head -n 1

echo ""
echo "LLD Version:"
ld.lld --version | head -n 1

echo ""
echo "xwin SDK Location:"
ls -lh "$XWIN_DIR" | head -n 5

# Create test file
TEST_DIR=$(mktemp -d)
TEST_FILE="$TEST_DIR/test.c"

cat > "$TEST_FILE" << 'EOF'
#include <windows.h>
#include <stdio.h>

int main(void) {
    printf("Hello from Windows PE (built on WSL)!\n");
    DWORD pid = GetCurrentProcessId();
    printf("PID: %lu\n", pid);
    return 0;
}
EOF

echo ""
echo -e "${YELLOW}Testing cross-compilation...${NC}"
echo "Test source: $TEST_FILE"

# Compile test
TEST_EXE="$TEST_DIR/test.exe"
clang -target x86_64-pc-windows-msvc \
      --sysroot "$XWIN_DIR" \
      -fuse-ld=lld \
      -o "$TEST_EXE" \
      "$TEST_FILE"

if [ -f "$TEST_EXE" ]; then
    echo -e "${GREEN} Cross-compilation successful!${NC}"
    echo "Test executable: $TEST_EXE"
    ls -lh "$TEST_EXE"

    # Check if it's a valid PE
    file "$TEST_EXE" | grep -q "PE32+" && echo -e "${GREEN} Valid PE32+ executable${NC}"
else
    echo -e "${RED} Cross-compilation failed${NC}"
    exit 1
fi

# Cleanup test
rm -rf "$TEST_DIR"

echo ""
echo -e "${GREEN}=========================================="
echo "Setup Complete!"
echo -e "==========================================${NC}"
echo ""
echo "You can now build Windows artifacts from C/C++ source:"
echo ""
echo -e "${BLUE}Basic build:${NC}"
echo "  clang -target x86_64-pc-windows-msvc \\"
echo "        --sysroot ~/.xwin \\"
echo "        -fuse-ld=lld \\"
echo "        -O2 \\"
echo "        -o artifact.exe \\"
echo "        source.c"
echo ""
echo -e "${BLUE}Build with LLVM IR (for mutations):${NC}"
echo "  # Step 1: Compile to LLVM IR"
echo "  clang -target x86_64-pc-windows-msvc \\"
echo "        -emit-llvm -S -O2 \\"
echo "        -o artifact.ll \\"
echo "        source.c"
echo ""
echo "  # Step 2: Apply mutations (custom LLVM passes)"
echo "  opt -load-pass-plugin=libMutationPass.so \\"
echo "      -passes='opaque-predicates,cfg-flatten' \\"
echo "      -o mutated.ll \\"
echo "      artifact.ll"
echo ""
echo "  # Step 3: Compile IR to object file"
echo "  llc -mtriple=x86_64-pc-windows-msvc \\"
echo "      -filetype=obj \\"
echo "      -o artifact.obj \\"
echo "      mutated.ll"
echo ""
echo "  # Step 4: Link to PE executable"
echo "  ld.lld -flavor link \\"
echo "         -subsystem:console \\"
echo "         -entry:mainCRTStartup \\"
echo "         -libpath:~/.xwin/crt/lib/x86_64 \\"
echo "         -libpath:~/.xwin/sdk/lib/um/x86_64 \\"
echo "         -out:artifact.exe \\"
echo "         artifact.obj \\"
echo "         libcmt.lib kernel32.lib"
echo ""
echo -e "${BLUE}Next steps:${NC}"
echo "  1. Review BUILD-PIPELINE.md for architecture details"
echo "  2. Implement build/emitter crate: cargo new build/emitter --lib"
echo "  3. Create artifact templates in: automation/corpus/templates/"
echo ""
