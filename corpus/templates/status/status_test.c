/**
 * Artifact Status API Test
 *
 * Demonstrates the new instrumentation API for C programmers:
 * - ARTIFACT_CHECKPOINT(name)     - Mark progress checkpoints
 * - ARTIFACT_SUCCESS(message)     - Signal successful completion
 * - ARTIFACT_FAILURE(msg, code)   - Signal failure with error code
 *
 * These macros work with both trace=on and trace=off modes:
 * - trace=on:  Macros expand to real function calls
 * - trace=off: Macros expand to no-ops (zero overhead, no linker errors)
 */

#include <stdio.h>
#include <windows.h>
#include "instrumentation.h"  // Use the header with macros

int main() {
    printf("========================================\n");
    printf("  Artifact Status API Test\n");
    printf("========================================\n\n");

    // Stage 1: Initialization
    printf("[Stage 1] Initialization...\n");
    ARTIFACT_CHECKPOINT("init_start");

    Sleep(100);  // Simulate work
    printf("  -> Allocating resources\n");

    ARTIFACT_CHECKPOINT("init_complete");
    printf("  -> Initialization complete\n\n");

    // Stage 2: Main work
    printf("[Stage 2] Performing main task...\n");
    ARTIFACT_CHECKPOINT("work_start");

    Sleep(200);  // Simulate work
    printf("  -> Processing data\n");

    ARTIFACT_CHECKPOINT("work_50pct");
    printf("  -> 50%% complete\n");

    Sleep(200);  // More work
    ARTIFACT_CHECKPOINT("work_complete");
    printf("  -> Main task complete\n\n");

    // Stage 3: Cleanup
    printf("[Stage 3] Cleanup...\n");
    ARTIFACT_CHECKPOINT("cleanup_start");

    Sleep(100);  // Simulate cleanup
    printf("  -> Releasing resources\n");

    ARTIFACT_CHECKPOINT("cleanup_complete");
    printf("  -> Cleanup complete\n\n");

    // Signal overall success
    printf("========================================\n");
    printf("All stages completed successfully!\n");
    printf("========================================\n\n");

    ARTIFACT_SUCCESS("All 3 stages completed without EDR intervention");

    printf("Press Enter to exit...\n");
    getchar();

    return 0;
}
