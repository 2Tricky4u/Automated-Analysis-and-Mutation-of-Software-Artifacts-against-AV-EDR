/**
 * Artifact Status API Test
 *
 * Demonstrates the new instrumentation API for C programmers:
 * - __artifact_checkpoint(name)     - Mark progress checkpoints
 * - __artifact_success(message)     - Signal successful completion
 * - __artifact_failure(msg, code)   - Signal failure with error code
 *
 * These functions are automatically available when building with --trace=all or --trace=lines
 */

#include <stdio.h>
#include <windows.h>

// Forward declarations (provided by instrumentation runtime)
extern void __artifact_checkpoint(const char* checkpoint_name);
extern void __artifact_success(const char* message);
extern void __artifact_failure(const char* message, int error_code);

int main() {
    printf("========================================\n");
    printf("  Artifact Status API Test\n");
    printf("========================================\n\n");

    // Stage 1: Initialization
    printf("[Stage 1] Initialization...\n");
    __artifact_checkpoint("init_start");

    Sleep(100);  // Simulate work
    printf("  -> Allocating resources\n");

    __artifact_checkpoint("init_complete");
    printf("  -> Initialization complete\n\n");

    // Stage 2: Main work
    printf("[Stage 2] Performing main task...\n");
    __artifact_checkpoint("work_start");

    Sleep(200);  // Simulate work
    printf("  -> Processing data\n");

    __artifact_checkpoint("work_50pct");
    printf("  -> 50%% complete\n");

    Sleep(200);  // More work
    __artifact_checkpoint("work_complete");
    printf("  -> Main task complete\n\n");

    // Stage 3: Cleanup
    printf("[Stage 3] Cleanup...\n");
    __artifact_checkpoint("cleanup_start");

    Sleep(100);  // Simulate cleanup
    printf("  -> Releasing resources\n");

    __artifact_checkpoint("cleanup_complete");
    printf("  -> Cleanup complete\n\n");

    // Signal overall success
    printf("========================================\n");
    printf("All stages completed successfully!\n");
    printf("========================================\n\n");

    __artifact_success("All 3 stages completed without EDR intervention");

    printf("Press Enter to exit...\n");
    getchar();

    return 0;
}
