/* MODULE: DECODER
 * TYPE: none
 * DESC: No encoding — raw payload bytes, direct copy.
 */
#include "../header/definitions.h"

FORCE_INLINE void decode_payload(char *dest, int len) {
    __builtin_memcpy(dest, supermega_payload, len);
}
