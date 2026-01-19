/* MODULE: DECODER
 * TYPE: xor
 * DESC: Decodes payload using a 2-byte rolling XOR key.
 * DATA: Expects 'XOR_KEY' in global scope (from payload.h).
 */
#include "../header/definitions.h"

// Defined in payload.h
extern unsigned char XOR_KEY[2];

void decode_payload(char *dest, int len) {
    for(int i = 0; i < len; i++) {
        dest[i] = supermega_payload[i] ^ XOR_KEY[i % 2];
    }
}
