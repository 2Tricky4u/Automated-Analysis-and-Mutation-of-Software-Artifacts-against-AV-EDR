// Test program for artifact status instrumentation API
// Demonstrates usage of ARTIFACT_CHECKPOINT, ARTIFACT_SUCCESS, ARTIFACT_FAILURE
//
// These macros allow C code writers to manually instrument their artifacts
// to track execution progress and outcomes.
//
// When trace_mode != "off": macros expand to real function calls
// When trace_mode == "off": macros expand to no-ops (no linker errors)

#include <windows.h>
#include <stdio.h>
#include "instrumentation.h"

// Simulated artifact operations
int setup_environment(void) {
    ARTIFACT_CHECKPOINT("setup_start");

    printf("[Artifact] Setting up environment...\n");
    Sleep(100);

    // Simulate some setup work
    HANDLE hMutex = CreateMutexA(NULL, FALSE, "TestArtifactMutex");
    if (hMutex == NULL) {
        ARTIFACT_FAILURE("Failed to create mutex", GetLastError());
        return 0;
    }
    CloseHandle(hMutex);

    ARTIFACT_CHECKPOINT("setup_complete");
    return 1;
}

int allocate_resources(void) {
    ARTIFACT_CHECKPOINT("allocation_start");

    printf("[Artifact] Allocating resources...\n");
    Sleep(100);

    // Simulate memory allocation
    LPVOID memory = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (memory == NULL) {
        ARTIFACT_FAILURE("Memory allocation failed", GetLastError());
        return 0;
    }

    // Write some data
    memcpy(memory, "Test data", 10);

    // Clean up
    VirtualFree(memory, 0, MEM_RELEASE);

    ARTIFACT_CHECKPOINT("allocation_complete");
    return 1;
}

int perform_operation(void) {
    ARTIFACT_CHECKPOINT("operation_start");

    printf("[Artifact] Performing main operation...\n");
    Sleep(100);

    // Simulate file operation
    HANDLE hFile = CreateFileA(
        "test_artifact_output.tmp",
        GENERIC_WRITE,
        0,
        NULL,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE,
        NULL
    );

    if (hFile == INVALID_HANDLE_VALUE) {
        ARTIFACT_FAILURE("Failed to create temp file", GetLastError());
        return 0;
    }

    // Write some data
    const char* data = "Artifact execution trace\n";
    DWORD written;
    WriteFile(hFile, data, (DWORD)strlen(data), &written, NULL);

    CloseHandle(hFile);  // Auto-deleted due to FILE_FLAG_DELETE_ON_CLOSE

    ARTIFACT_CHECKPOINT("operation_complete");
    return 1;
}

int cleanup(void) {
    ARTIFACT_CHECKPOINT("cleanup_start");

    printf("[Artifact] Cleaning up...\n");
    Sleep(50);

    // Simulate cleanup
    // (Nothing to fail here in this test)

    ARTIFACT_CHECKPOINT("cleanup_complete");
    return 1;
}

int main(int argc, char* argv[]) {
    printf("=== Artifact Status Instrumentation Test ===\n");
    printf("This artifact demonstrates checkpoint/success/failure tracking\n\n");

    ARTIFACT_CHECKPOINT("main_entry");

    // Simulate multi-stage artifact execution
    if (!setup_environment()) {
        printf("[Artifact] Setup failed!\n");
        return 1;
    }

    if (!allocate_resources()) {
        printf("[Artifact] Resource allocation failed!\n");
        return 2;
    }

    if (!perform_operation()) {
        printf("[Artifact] Main operation failed!\n");
        return 3;
    }

    if (!cleanup()) {
        printf("[Artifact] Cleanup failed!\n");
        return 4;
    }

    printf("\n[Artifact] All stages completed successfully!\n");
    ARTIFACT_SUCCESS("All stages completed: setup, allocation, operation, cleanup");

    return 0;
}
