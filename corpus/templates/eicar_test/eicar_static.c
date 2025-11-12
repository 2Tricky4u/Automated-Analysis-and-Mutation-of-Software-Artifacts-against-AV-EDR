/**
 * EICAR Static Test - Full signature embedded in binary
 *
 * This version has the EICAR string compiled directly into the executable.
 * AV should detect it IMMEDIATELY when the file is written to disk.
 */

#include <windows.h>
#include <stdio.h>

// EICAR string directly embedded (will be in .rdata section)
// AV scanners will find this in the binary file itself
const char EICAR_SIGNATURE[] =
    "X5O!P%@AP[4\\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";

int main(void) {
    printf("========================================\n");
    printf("  EICAR Static Test                     \n");
    printf("========================================\n");
    printf("\n");
    printf("[WARNING] This binary contains the EICAR signature!\n");
    printf("[INFO] If you're reading this, your AV either:\n");
    printf("  1. Is disabled\n");
    printf("  2. Allowed this test file\n");
    printf("  3. Failed to detect it (unlikely!)\n");
    printf("\n");
    printf("[EICAR] Signature: %s\n", EICAR_SIGNATURE);
    printf("\n");
    printf("Press Enter to exit...\n");
    getchar();
    return 0;
}
