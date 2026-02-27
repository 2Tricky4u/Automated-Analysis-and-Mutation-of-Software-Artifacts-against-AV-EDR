/* MODULE: ANTIEMULATION
 * TYPE: cpuburn
 * DESC: Pure CPU-intensive computation to exhaust emulator instruction budgets.
 *       No API calls, no memory permissions — just math the emulator must execute.
 *       Real CPU: ~50ms. Emulator: instruction budget exceeded or timeout.
 * MUTATIONS: loop_mutation, literal_encoding, loop_restructuring
 */
#include "../header/definitions.h"

// @MUTATE:literal_encoding
#define BURN_OUTER  800
#define BURN_INNER  1000

FORCE_INLINE void antiemulation() {
    volatile unsigned int acc = 0x5A3C1E0F;

    // @MUTATE:loop_mutation(fixed->GetTickCount_modulo)
    // @MUTATE:loop_restructuring(for->while|unroll)
    for (int i = 0; i < BURN_OUTER; i++) {
        // @MUTATE:loop_restructuring(for->while|unroll)
        // @MUTATE:literal_encoding
        for (int j = 0; j < BURN_INNER; j++) {
            // Mix function — cheap on real CPU, expensive instruction count for emulator
            // @MUTATE:literal_encoding
            acc ^= (acc << 13);
            acc ^= (acc >> 17);
            acc ^= (acc << 5);
            acc += (unsigned int)(i * j + 0x9E3779B9);
        }
    }

    // Use the result so the compiler can't optimize away the loop
    // @MUTATE:opaque_predicate
    if (acc == 0xDEADBEEF) {
        // Unreachable in practice — but prevents dead-code elimination
        return;
    }
}
