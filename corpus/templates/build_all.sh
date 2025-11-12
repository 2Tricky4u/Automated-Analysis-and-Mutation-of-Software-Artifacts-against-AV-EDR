#\!/usr/bin/env bash
set -euo pipefail

GREEN="\033[0;32m"
RED="\033[0;31m"
YELLOW="\033[1;33m"
BLUE="\033[0;34m"
NC="\033[0m"

echo -e "Building All Templates (LLVM/Clang)"
echo ""

XWIN_DIR="/root/.xwin"
if [ \! -d "" ]; then
    echo -e "Error: xwin SDK not found"
    exit 1
fi

COMMON_FLAGS="-target x86_64-pc-windows-msvc   -isystem /crt/include   -isystem /sdk/include/ucrt   -isystem /sdk/include/shared   -isystem /sdk/include/um   -isystem /sdk/include/winrt   -L/crt/lib/x86_64   -L/sdk/lib/ucrt/x86_64   -L/sdk/lib/um/x86_64   -fuse-ld=lld -Wl,/subsystem:console -O2"

BASE_LIBS="-Wl,-defaultlib:libcmt -Wl,-defaultlib:kernel32"

SUCCESS=0
FAILED=0

build_template() {
    local dir=
    local source=
    local output=
    local extra_libs=

    echo ""
    echo -e "Building: /"

    if [ \! -f "/" ]; then
        echo -e "✗ Not found"
        FAILED=1
        return 1
    fi

    cd ""
    if clang    -o "" "" 2>&1 | tail -10; then
        if [ -f "" ]; then
            echo -e "✓ Built:  (
1.7K
3.5K
512
512
512
512
512
512)"
            SUCCESS=1
        else
            echo -e "✗ File not created"
            FAILED=1
        fi
    else
        echo -e "✗ Build failed"
        FAILED=1
    fi
    cd - > /dev/null
}

build_template "loader_v1" "loader.c" "loader.exe" ""
build_template "rwx_direct" "rwx_direct.c" "rwx_direct.exe" "-Wl,-defaultlib:advapi32 -Wl,-defaultlib:wininet"
build_template "process_injection" "process_injection.c" "process_injection.exe" "-Wl,-defaultlib:user32"
build_template "network_beacon" "network_beacon.c" "network_beacon.exe" "-Wl,-defaultlib:ws2_32"
build_template "eicar_test" "eicar_test.c" "eicar_test.exe" ""
build_template "eicar_test" "eicar_static.c" "eicar_static.exe" ""

echo ""
echo -e "======================================"
echo -e "Success:  | Failed: "
[  -eq 0 ] && echo -e "All builds successful\!" || exit 1
