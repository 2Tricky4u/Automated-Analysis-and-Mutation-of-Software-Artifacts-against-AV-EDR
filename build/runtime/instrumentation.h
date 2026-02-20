/**
 * Instrumentation API Header
 *
 * Provides macros for manual instrumentation that automatically become no-ops
 * when instrumentation is disabled (trace=off).
 *
 * Usage in C artifacts:
 *   #include "instrumentation.h"
 *
 *   int main() {
 *       ARTIFACT_CHECKPOINT("main_entry");
 *
 *       if (!setup()) {
 *           ARTIFACT_FAILURE("Setup failed", GetLastError());
 *           return 1;
 *       }
 *
 *       ARTIFACT_SUCCESS("All stages completed");
 *       __runtime_exit(0);  // Clean exit via direct syscalls
 *   }
 *
 * When compiled with instrumentation (trace != off):
 *   - Macros expand to actual function calls
 *   - Runtime functions are linked automatically
 *   - minimal_runtime.o provides __runtime_exit() (direct syscall termination)
 *
 * When compiled without instrumentation (trace=off):
 *   - Macros expand to no-ops (empty statements)
 *   - No runtime objects linked — baseline binary is artifact-free
 */

#ifndef INSTRUMENTATION_H
#define INSTRUMENTATION_H

// Include minimal runtime header only when instrumentation is enabled.
// For baseline (trace=off) builds, minimal_runtime.o is not linked,
// so we avoid declaring symbols that won't be available.
#ifdef ENABLE_INSTRUMENTATION
#include "minimal_runtime.h"
#endif

#ifdef __cplusplus
extern "C" {
#endif

// ============================================================================
// Instrumentation API (conditional on ENABLE_INSTRUMENTATION)
// ============================================================================

// Check if instrumentation is enabled
// The builder will define ENABLE_INSTRUMENTATION when trace_mode != "off"
#ifdef ENABLE_INSTRUMENTATION

// Forward declarations for runtime functions
extern void __artifact_checkpoint(const char* checkpoint_name);
extern void __artifact_success(const char* message);
extern void __artifact_failure(const char* message, int error_code);

// Macros expand to real function calls
#define ARTIFACT_CHECKPOINT(name) __artifact_checkpoint(name)
#define ARTIFACT_SUCCESS(msg) __artifact_success(msg)
#define ARTIFACT_FAILURE(msg, code) __artifact_failure(msg, code)

#else

// Instrumentation disabled: macros expand to no-ops
#define ARTIFACT_CHECKPOINT(name) ((void)0)
#define ARTIFACT_SUCCESS(msg) ((void)0)
#define ARTIFACT_FAILURE(msg, code) ((void)0)

#endif // ENABLE_INSTRUMENTATION

// Legacy compatibility: also provide lowercase extern declarations
// (for code that uses extern declarations directly)
#ifdef ENABLE_INSTRUMENTATION
// Already declared above
#else
// Provide stub inline functions to avoid linker errors
static inline void __artifact_checkpoint(const char* checkpoint_name) { (void)checkpoint_name; }
static inline void __artifact_success(const char* message) { (void)message; }
static inline void __artifact_failure(const char* message, int error_code) { (void)message; (void)error_code; }
#endif

#ifdef __cplusplus
}
#endif

#endif // INSTRUMENTATION_H
