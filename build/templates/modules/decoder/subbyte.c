/* MODULE: DECODER
 * TYPE: subbyte
 * DESC: Nibble-based sub-byte mapping decoder. Each original byte was split
 *       into two 4-bit nibbles, each mapped through SUBBYTE_MAPPING[16].
 *       Decoder builds reverse lookup, then reconstructs original bytes.
 *       Payload size: ENCODED_PAYLOAD_LEN = 2 * PAYLOAD_LEN.
 * MUTATIONS: literal_encoding
 */
#include "../header/definitions.h"

extern unsigned char SUBBYTE_MAPPING[16];

FORCE_INLINE void decode_payload(char *dest, int len) {
    // Build reverse lookup: reverse[mapped_value] = nibble_index
    unsigned char reverse[256];
    for (int i = 0; i < 256; i++) reverse[i] = 0;
    for (int i = 0; i < 16; i++) {
        reverse[SUBBYTE_MAPPING[i]] = (unsigned char)i;
    }

    // Decode: every 2 encoded bytes -> 1 original byte
    unsigned char *src = (unsigned char *)supermega_payload;
    for (int i = 0; i < len; i++) {
        unsigned char high = reverse[src[i * 2]];
        unsigned char low  = reverse[src[i * 2 + 1]];
        dest[i] = (char)((high << 4) | low);
    }
}
