/**
 * Minimal Runtime Header
 *
 * Provides __runtime_exit() for clean process termination.
 * This function is ALWAYS available, regardless of instrumentation mode.
 *
 * Include this header in artifacts that need clean exit via direct syscalls.
 */

#ifndef MINIMAL_RUNTIME_H
#define MINIMAL_RUNTIME_H

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Clean exit using direct syscalls (bypasses all hooks including RedEDR)
 *
 * USE THIS INSTEAD OF: return 0, exit(), ExitProcess()
 *
 * Benefits:
 * - Bypasses hooked exit functions (NtTerminateProcess, ExitProcess, etc.)
 * - Prevents deadlocks with RedEDR detours or other EDR hooks
 * - Works in both instrumented and non-instrumented modes
 * - Safe for artifacts that need reliable exit under observation
 *
 * Usage:
 *   #include "minimal_runtime.h"
 *
 *   int main() {
 *       // ... artifact code ...
 *       printf("Done\n");
 *       __runtime_exit(0);  // Instead of: return 0;
 *   }
 */
extern void __runtime_exit(int exit_code) __attribute__((noreturn));

// Convenience macro
#define RUNTIME_EXIT(code) __runtime_exit(code)

#ifdef __cplusplus
}
#endif

#endif // MINIMAL_RUNTIME_H
