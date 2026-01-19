#!/bin/bash
# Build script for SuperMega Loader Integration
# Targets Windows x64 using Clang on Linux (WSL2)

# Adjust these paths to your xwin SDK location
XWIN_DIR=~/.xwin

clang -target x86_64-pc-windows-msvc \
    -isystem $XWIN_DIR/crt/include \
    -isystem $XWIN_DIR/sdk/include/ucrt \
    -isystem $XWIN_DIR/sdk/include/shared \
    -isystem $XWIN_DIR/sdk/include/um \
    -isystem $XWIN_DIR/sdk/include/winrt \
    -L $XWIN_DIR/crt/lib/x86_64 \
    -L $XWIN_DIR/sdk/lib/x86_64 \
    -fuse-ld=lld \
    -Wl,/subsystem:console \
    -fno-stack-protector \
    -o loader.exe \
    loader.c

echo "Build complete: loader.exe"
