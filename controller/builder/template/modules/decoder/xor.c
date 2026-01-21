/* MODULE: DECODER
 * TYPE: xor
 * DESC: Rolling 2-byte XOR key
 * MUTATIONS: key_upgrade, control_flow_flattening, loop_restructuring
 */
#include "../header/definitions.h"

extern unsigned char XOR_KEY[2];

void decode_payload(char *dest, int len) {
    // @MUTATE:key_upgrade(rc4|aes|rolling_xor|multi_stage)
    // @MUTATE:control_flow_flattening
    // @MUTATE:loop_restructuring(for→while|unroll|switch_dispatch)
    // @MUTATE:syntactic_substitution(for→while)
    for(int i = 0; i < len; i++) {
        // @MUTATE:literal_encoding
        // @MUTATE:opaque_predicate
        dest[i] = supermega_payload[i] ^ XOR_KEY[i % 2];
    }
}
