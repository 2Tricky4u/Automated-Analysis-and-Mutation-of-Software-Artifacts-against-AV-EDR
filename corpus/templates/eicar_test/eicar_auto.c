/**
 * EICAR Automated Test - Non-Interactive Version
 *
 * Suspicious Pattern: Contains EICAR anti-malware test signature
 * EDR Trigger: Signature-based detection (not behavioral)
 * Benign Effect: EICAR is a test file, completely harmless
 *
 * Detection likelihood: 100% (EICAR is specifically designed to trigger AVs)
 *
 * This version runs WITHOUT user input prompts, suitable for automated testing.
 */

#include <windows.h>
#include <stdio.h>

// EICAR test string (split to avoid detection during compilation)
// Full string: X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*
const char* eicar_part1 = "X5O!P%@AP[4\\PZX54(P^)7CC)7}$";
const char* eicar_part2 = "EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";

// Simulate benign activity before triggering detection
void benign_preamble() {
    printf("[BENIGN] Performing normal operations...\n");

    // Query system info
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    printf("[BENIGN] Processor count: %lu\n", si.dwNumberOfProcessors);

    // Query username
    char username[256];
    DWORD username_len = sizeof(username);
    if (GetUserNameA(username, &username_len)) {
        printf("[BENIGN] Current user: %s\n", username);
    }

    // Sleep briefly
    Sleep(100);
}

// Test 1: EICAR in memory
void test_eicar_in_memory() {
    printf("\n[TEST 1] EICAR Signature in Memory\n");
    printf("====================================\n");

    // Allocate memory for EICAR string
    size_t eicar_len = strlen(eicar_part1) + strlen(eicar_part2) + 1;
    char* eicar_full = (char*)malloc(eicar_len);

    if (!eicar_full) {
        fprintf(stderr, "[ERROR] Memory allocation failed\n");
        return;
    }

    // Concatenate parts to form full EICAR string
    printf("[INFO] Assembling EICAR test signature...\n");
    strcpy(eicar_full, eicar_part1);
    strcat(eicar_full, eicar_part2);

    printf("[INFO] EICAR string assembled (%zu bytes)\n", eicar_len - 1);
    printf("[WARNING] This signature triggers ALL anti-virus products!\n");

    printf("[EICAR] Full string: %s\n", eicar_full);

    printf("[INFO] If your AV is active, it should quarantine this process NOW\n");

    // Give AV time to react
    Sleep(500);

    free(eicar_full);
}

// Test 2: EICAR in executable memory (behavioral trigger)
void test_eicar_execute() {
    printf("\n[TEST 2] EICAR in RWX Memory\n");
    printf("====================================\n");

    // Assemble EICAR string
    size_t eicar_len = strlen(eicar_part1) + strlen(eicar_part2) + 1;
    char* eicar_full = (char*)malloc(eicar_len);

    strcpy(eicar_full, eicar_part1);
    strcat(eicar_full, eicar_part2);

    printf("[SUSPICIOUS] Allocating RWX memory...\n");
    LPVOID mem = VirtualAlloc(NULL, eicar_len, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);

    if (!mem) {
        fprintf(stderr, "[ERROR] VirtualAlloc failed: %lu\n", GetLastError());
        free(eicar_full);
        return;
    }

    printf("[SUSPICIOUS] Copying EICAR signature to RWX memory at %p...\n", mem);
    memcpy(mem, eicar_full, eicar_len);

    printf("[INFO] EICAR is now in executable memory\n");
    printf("[WARNING] This combines signature + behavioral detection!\n");

    // Give AV time to scan RWX region
    Sleep(500);

    printf("[INFO] Cleaning up (not executing - would crash)\n");

    VirtualFree(mem, 0, MEM_RELEASE);
    free(eicar_full);
}

int main(void) {
    printf("========================================\n");
    printf("  EICAR Automated Test (Non-Interactive)\n");
    printf("========================================\n");
    printf("\n");
    printf("[INFO] PID: %lu\n", GetCurrentProcessId());
    printf("[INFO] EICAR is a universal AV test - completely harmless\n");
    printf("[INFO] See: https://www.eicar.org/\n");
    printf("\n");

    // Benign preamble to establish baseline telemetry
    benign_preamble();

    // Test 1: EICAR in memory
    test_eicar_in_memory();
    Sleep(500);  // Give AV time to react

    // Test 2: EICAR in executable memory
    test_eicar_execute();
    Sleep(500);  // Give AV time to react

    printf("\n========================================\n");
    printf("[INFO] All tests complete\n");
    printf("[INFO] If you made it here, your AV is either:\n");
    printf("  1. Disabled\n");
    printf("  2. Allowing this test file\n");
    printf("  3. Slow to react\n");
    printf("========================================\n");

    // Auto-exit (no user input required)
    printf("\n[INFO] Exiting in 1 second...\n");
    Sleep(1000);

    return 0;
}
