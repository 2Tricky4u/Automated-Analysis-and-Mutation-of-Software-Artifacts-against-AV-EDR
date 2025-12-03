/**
 * EICAR Test Template
 *
 * Suspicious Pattern: Contains EICAR anti-malware test signature
 * EDR Trigger: Signature-based detection (not behavioral)
 * Benign Effect: EICAR is a test file, completely harmless
 *
 * Detection likelihood: 100% (EICAR is specifically designed to trigger AVs)
 *
 * Note: EICAR signature is split across multiple strings to avoid
 * immediate detection of the source file itself.
 */

#include <windows.h>
#include <stdio.h>
#include "instrumentation.h"

//extern void __artifact_checkpoint(const char* checkpoint_name);

// EICAR test string (split to avoid detection during compilation)
// Full string: X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*
const char* eicar_part1 = "X5O!P%@AP[4\\PZX54(P^)7CC)7}$";
const char* eicar_part2 = "EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";

// Alternative: execute EICAR from memory
void test_eicar_in_memory() {
    printf("[TEST 1] EICAR Signature in Memory\n");
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
    printf("\n");

    printf("[EICAR] Full string:\n%s\n", eicar_full);
    printf("\n");

    printf("[INFO] If your AV is active, it should quarantine this process NOW\n");
    printf("\n");

    free(eicar_full);
}

// Write EICAR to temp file
void test_eicar_file_write() {
    printf("[TEST 2] EICAR File Write\n");
    printf("====================================\n");

    char temp_path[MAX_PATH];
    GetTempPathA(MAX_PATH, temp_path);

    char eicar_file[MAX_PATH];
    snprintf(eicar_file, MAX_PATH, "%seicar_test.txt", temp_path);

    printf("[INFO] Creating EICAR test file: %s\n", eicar_file);

    FILE* f = fopen(eicar_file, "w");
    if (!f) {
        fprintf(stderr, "[ERROR] Failed to create file: %s\n", eicar_file);
        return;
    }

    // Write EICAR signature to file
    fprintf(f, "%s%s", eicar_part1, eicar_part2);
    fclose(f);

    printf("[SUCCESS] File written\n");
    printf("[WARNING] AV should detect and delete this file immediately!\n");
    printf("\n");

    // Try to read it back (will likely fail due to AV quarantine)
    Sleep(500);  // Give AV time to react

    printf("[INFO] Attempting to read file back...\n");
    f = fopen(eicar_file, "r");
    if (f) {
        char buffer[256];
        fgets(buffer, sizeof(buffer), f);
        fclose(f);
        printf("[RESULT] File still exists (AV may be disabled or slow)\n");
        printf("[CONTENT] %s\n", buffer);

        // Clean up
        DeleteFileA(eicar_file);
    } else {
        printf("[RESULT] File disappeared (AV quarantined it!)\n");
    }
}

// Execute EICAR from RWX memory (combines signature + behavioral detection)
void test_eicar_execute() {
    printf("[TEST 3] EICAR Execution from RWX Memory\n");
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
    printf("\n");

    printf("[INFO] Not actually executing (would crash), but AV should detect the pattern\n");

    VirtualFree(mem, 0, MEM_RELEASE);
    free(eicar_full);
}

int main(void) {
    printf("========================================\n");
    printf("  EICAR Anti-Virus Test File Generator  \n");
    printf("========================================\n");
    printf("\n");
    printf("[INFO] PID: %lu\n", GetCurrentProcessId());
    printf("\n");
    printf("[WARNING] This program contains the EICAR test signature\n");
    printf("[INFO] EICAR is a universal AV test string - completely harmless\n");
    printf("[INFO] See: https://www.eicar.org/\n");
    printf("\n");
    printf("[NOTICE] If your AV doesn't catch this, it may be disabled!\n");
    printf("\n");

    //__artifact_checkpoint("print passed");
    printf("Press Enter to start tests...\n");
    //getchar();
    printf("\n");

    // Test 1: EICAR in memory
    test_eicar_in_memory();
    printf("Press Enter for next test...\n");
    //getchar();
    printf("\n");

    // Test 2: EICAR file write
    test_eicar_file_write();
    printf("Press Enter for next test...\n");
    //getchar();
    printf("\n");

    // Test 3: EICAR in executable memory
    test_eicar_execute();

    ARTIFACT_SUCCESS("Could be executed");

    printf("\n========================================\n");
    printf("[INFO] All tests complete\n");
    printf("[INFO] If you made it here, your AV is either:\n");
    printf("  1. Disabled\n");
    printf("  2. Allowing this test file\n");
    printf("  3. Slow to react\n");
    printf("========================================\n");

    printf("\nPress Enter to exit...\n");
    //getchar();



    return 0;
}
