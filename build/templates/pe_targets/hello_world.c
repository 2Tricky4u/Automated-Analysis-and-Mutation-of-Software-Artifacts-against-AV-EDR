/*
 * hello_world.c — Minimal target PE for injection testing
 *
 * Pre-compile with:
 *   clang --target=x86_64-pc-windows-msvc -o hello_world.exe hello_world.c \
 *     -Wl,/entry:main,/subsystem:console -fuse-ld=lld \
 *     -I/root/.xwin/crt/include -I/root/.xwin/sdk/include/ucrt \
 *     -I/root/.xwin/sdk/include/um -I/root/.xwin/sdk/include/shared \
 *     -L/root/.xwin/crt/lib/x86_64 -L/root/.xwin/sdk/lib/um/x86_64 \
 *     -L/root/.xwin/sdk/lib/ucrt/x86_64 -lkernel32 -luser32
 *
 * Or cross-compile from WSL2 using the existing Clang+xwin pipeline.
 * Source kept for reproducibility; the pre-compiled .exe is the actual asset.
 */
#include <windows.h>

int main(void) {
    MessageBoxA(NULL, "Hello", "World", MB_OK);
    return 0;
}
