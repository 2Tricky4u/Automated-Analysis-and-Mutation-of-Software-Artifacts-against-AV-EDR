//! Shellcode checkpoint stub (x64 PIC)
//!
//! When prepended to raw shellcode, this stub calls `__artifact_checkpoint("payload_executed")`
//! using a function pointer passed in RCX by the carrier.
//!
//! Only used for instrumented builds (trace_mode != "off").
//! Baseline builds call shellcode with no arguments and no stub.
//!
//! Layout:
//! ```text
//! [stub code]  →  receives fn ptr in RCX, calls it, falls through
//! [string]     →  "payload_executed\0"
//! [original shellcode bytes...]
//! ```

/// Size of the checkpoint stub in bytes (code + string).
pub const STUB_SIZE: usize = STUB_CODE.len() + STUB_MSG.len();

/// The null-terminated checkpoint message embedded in the stub.
const STUB_MSG: &[u8] = b"payload_executed\0";

/// x64 machine code for the checkpoint stub.
///
/// ```asm
/// 0x00  53                 push rbx
/// 0x01  48 89 CB           mov  rbx, rcx           ; save checkpoint fn ptr
/// 0x04  48 83 EC 20        sub  rsp, 0x20          ; shadow space
/// 0x08  48 8D 0D 09 00 00 00  lea rcx, [rip+9]    ; -> "payload_executed"
/// 0x0F  FF D3              call rbx                 ; checkpoint("payload_executed")
/// 0x11  48 83 C4 20        add  rsp, 0x20          ; restore stack
/// 0x15  5B                 pop  rbx                 ; restore rbx
/// 0x16  EB 11              jmp  short +17           ; skip string data
/// 0x18  "payload_executed\0"                        ; 17 bytes
/// 0x29  ... original shellcode
/// ```
const STUB_CODE: &[u8] = &[
    0x53, // push rbx
    0x48, 0x89, 0xCB, // mov rbx, rcx
    0x48, 0x83, 0xEC, 0x20, // sub rsp, 0x20
    0x48, 0x8D, 0x0D, 0x09, 0x00, 0x00, 0x00, // lea rcx, [rip+9]
    0xFF, 0xD3, // call rbx
    0x48, 0x83, 0xC4, 0x20, // add rsp, 0x20
    0x5B, // pop rbx
    0xEB, 0x11, // jmp short +17
];

/// Prepend the checkpoint stub to raw shellcode.
///
/// The resulting bytes expect the carrier to pass `__artifact_checkpoint` as the
/// first argument (RCX on x64 Windows calling convention).
pub fn prepend_checkpoint_stub(shellcode: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(STUB_SIZE + shellcode.len());
    result.extend_from_slice(STUB_CODE);
    result.extend_from_slice(STUB_MSG);
    result.extend_from_slice(shellcode);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stub_size_constant() {
        assert_eq!(STUB_SIZE, STUB_CODE.len() + STUB_MSG.len());
        assert_eq!(STUB_CODE.len(), 24); // 0x18 bytes of code
        assert_eq!(STUB_MSG.len(), 17); // "payload_executed\0"
        assert_eq!(STUB_SIZE, 41); // total stub
    }

    #[test]
    fn test_prepend_preserves_shellcode() {
        let shellcode = vec![0x90, 0x90, 0xCC]; // NOP NOP INT3
        let result = prepend_checkpoint_stub(&shellcode);

        assert_eq!(result.len(), STUB_SIZE + 3);
        // Original shellcode at the end, unchanged
        assert_eq!(&result[STUB_SIZE..], &[0x90, 0x90, 0xCC]);
    }

    #[test]
    fn test_stub_starts_with_push_rbx() {
        let result = prepend_checkpoint_stub(&[0xCC]);
        assert_eq!(result[0], 0x53); // push rbx
    }

    #[test]
    fn test_stub_contains_message() {
        let result = prepend_checkpoint_stub(&[0xCC]);
        let msg_offset = STUB_CODE.len();
        assert_eq!(
            &result[msg_offset..msg_offset + STUB_MSG.len()],
            b"payload_executed\0"
        );
    }

    #[test]
    fn test_jmp_offset_correct() {
        // The jmp short at end of STUB_CODE should skip exactly STUB_MSG bytes
        let jmp_offset = STUB_CODE[STUB_CODE.len() - 1]; // last byte = displacement
        assert_eq!(jmp_offset as usize, STUB_MSG.len());
    }

    #[test]
    fn test_lea_rip_offset_correct() {
        // lea rcx, [rip+N] at offset 0x08, instruction is 7 bytes
        // RIP after instruction = 0x0F, msg at 0x18, so N = 0x09
        assert_eq!(STUB_CODE[11], 0x09); // the displacement byte
        assert_eq!(STUB_CODE[12], 0x00);
        assert_eq!(STUB_CODE[13], 0x00);
        assert_eq!(STUB_CODE[14], 0x00);
    }

    #[test]
    fn test_empty_shellcode() {
        let result = prepend_checkpoint_stub(&[]);
        assert_eq!(result.len(), STUB_SIZE);
    }
}
