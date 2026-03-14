/*
 * none_carrier.c — Reference C pseudocode for the None (no-decode) carrier stub
 *
 * NOT COMPILED. Documents the logic matching stubs.rs::NONE_STUB_CODE.
 *
 * Section data layout: [stub_code (37 bytes)] [raw_payload (N bytes)]
 *
 * No decoding — the stub simply jumps to the payload. Used when the payload
 * is injected without encoding (EncodingType::None).
 */

#include <stdint.h>

extern uint8_t* PAYLOAD_ADDR;  // → raw payload (right after stub code)
extern void*    OEP_ADDR;      // optional OEP return (RIP-relative)

void none_carrier_stub(void) {
    // === RSP alignment ===
    // push rsi, rdi, rbx
    // and rsp, -16
    // sub rsp, 0x28

    // === Execute payload directly ===
    ((void(*)(void))PAYLOAD_ADDR)();

    // === Optional OEP return / cleanup ===
}
