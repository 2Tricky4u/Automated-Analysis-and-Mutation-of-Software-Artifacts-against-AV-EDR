/* MODULE: DECODER
 * TYPE: xor
 * DESC: Rolling 2-byte XOR key — widened to 8-byte XOR for throughput
 * MUTATIONS: key_upgrade, control_flow_flattening, loop_restructuring
 */
#include "../header/definitions.h"

extern unsigned char XOR_KEY[2];

FORCE_INLINE void decode_payload(char *dest, int len) {
    const unsigned char *src = supermega_payload;

    /* Broadcast 2-byte key into 8-byte mask: [k0,k1,k0,k1,k0,k1,k0,k1] */
    unsigned __int64 k16 = (unsigned __int64)XOR_KEY[0] | ((unsigned __int64)XOR_KEY[1] << 8);
    unsigned __int64 mask = k16 | (k16 << 16);
    mask = mask | (mask << 32);

    /* XOR 8 bytes at a time */
    int i = 0;
    int bulk = len & ~7; /* round down to multiple of 8 */
    for (; i < bulk; i += 8) {
        unsigned __int64 block;
        __builtin_memcpy(&block, src + i, 8);
        block ^= mask;
        __builtin_memcpy(dest + i, &block, 8);
    }

    /* Tail: remaining 0-7 bytes */
    for (; i < len; i++) {
        dest[i] = src[i] ^ XOR_KEY[i & 1];
    }
}
