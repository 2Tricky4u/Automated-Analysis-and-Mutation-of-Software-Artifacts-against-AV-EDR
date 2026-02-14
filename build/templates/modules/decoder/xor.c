/* MODULE: DECODER
 * TYPE: xor
 * DESC: Rolling 2-byte XOR key
 * MUTATIONS: key_upgrade, control_flow_flattening, loop_restructuring
 */
#include "../header/definitions.h"

extern unsigned char XOR_KEY[2];

void decode_payload(char *dest, int len) {
    for(int i = 0; i < len; i++) {
        dest[i] = supermega_payload[i] ^ XOR_KEY[i % 2];
    }
}
