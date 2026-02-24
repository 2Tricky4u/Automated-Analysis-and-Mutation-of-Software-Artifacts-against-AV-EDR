//! Shellcode checkpoint patching via INT3 breakpoints.
//!
//! Inserts evenly-spaced `INT3` (0xCC) breakpoints into shellcode at instruction
//! boundaries, and generates a C header table so the VEH handler can recognize
//! each checkpoint at runtime.
//!
//! Only used for instrumented builds (`trace_mode != "off"`) when the
//! `sc-checkpoints` Cargo feature is enabled.

use anyhow::Result;
use iced_x86::{Decoder, DecoderOptions};

/// A single INT3 breakpoint inserted into the shellcode.
#[derive(Debug, Clone)]
pub struct BreakpointEntry {
    /// Offset from shellcode start (includes stub prefix).
    pub offset: usize,
    /// The original byte replaced by 0xCC.
    pub original_byte: u8,
    /// Human-readable name, e.g. "sc_checkpoint_0".
    pub name: String,
    /// Progress percentage (0–100) through the shellcode body.
    pub progress_pct: u8,
}

/// Result of patching shellcode with INT3 checkpoints.
#[derive(Debug, Clone)]
pub struct PatchedShellcode {
    /// Shellcode bytes with INT3s inserted at selected offsets.
    pub bytes: Vec<u8>,
    /// Metadata for each inserted breakpoint.
    pub table: Vec<BreakpointEntry>,
}

/// Disassemble shellcode after `stub_size` bytes and insert `checkpoint_count`
/// evenly-spaced INT3 breakpoints at instruction boundaries.
///
/// The stub region (`0..stub_size`) is never modified.
///
/// # Errors
/// Returns an error if the shellcode body (after stub) is too small to decode
/// any instructions.
pub fn patch_shellcode(
    shellcode: &mut Vec<u8>,
    checkpoint_count: u32,
    stub_size: usize,
) -> Result<PatchedShellcode> {
    if checkpoint_count == 0 {
        return Ok(PatchedShellcode {
            bytes: shellcode.clone(),
            table: Vec::new(),
        });
    }

    let body = &shellcode[stub_size..];
    if body.is_empty() {
        anyhow::bail!("Shellcode body (after stub) is empty — nothing to checkpoint");
    }

    // Linear disassembly from after the stub to collect instruction boundaries.
    let mut decoder = Decoder::with_ip(64, body, 0, DecoderOptions::NONE);
    let mut boundaries: Vec<usize> = Vec::new();

    for instr in &mut decoder {
        if instr.is_invalid() {
            break;
        }
        // Record the offset relative to the start of shellcode (not body).
        boundaries.push(stub_size + instr.ip() as usize);
    }

    if boundaries.is_empty() {
        anyhow::bail!("No valid x86-64 instructions found in shellcode body");
    }

    // Clamp checkpoint count to number of available boundaries (minus first
    // instruction which is the entry point — we don't want to break entry).
    let usable = if boundaries.len() > 1 {
        boundaries.len() - 1
    } else {
        0
    };

    if usable == 0 {
        // Only one instruction — nothing to checkpoint inside.
        return Ok(PatchedShellcode {
            bytes: shellcode.clone(),
            table: Vec::new(),
        });
    }

    let count = (checkpoint_count as usize).min(usable);
    let interval = (usable + 1) / (count + 1);
    let interval = interval.max(1);

    let mut table = Vec::with_capacity(count);
    let body_len = shellcode.len() - stub_size;

    for i in 0..count {
        let boundary_idx = (i + 1) * interval;
        if boundary_idx >= boundaries.len() {
            break;
        }
        let offset = boundaries[boundary_idx];
        let original_byte = shellcode[offset];
        let progress = ((offset - stub_size) as f64 / body_len as f64 * 100.0) as u8;

        table.push(BreakpointEntry {
            offset,
            original_byte,
            name: format!("sc_checkpoint_{}", i),
            progress_pct: progress.min(100),
        });

        shellcode[offset] = 0xCC;
    }

    Ok(PatchedShellcode {
        bytes: shellcode.clone(),
        table,
    })
}

/// Generate a C header string containing the checkpoint table.
///
/// The header is suitable for `#include`-ing into `sc_checkpoint_runtime.c`.
pub fn generate_c_header(patched: &PatchedShellcode) -> String {
    let count = patched.table.len();
    let mut out = String::with_capacity(512);

    out.push_str("#ifndef SC_CHECKPOINTS_TABLE_H\n");
    out.push_str("#define SC_CHECKPOINTS_TABLE_H\n\n");
    out.push_str(&format!("#define SC_CHECKPOINT_COUNT {}\n\n", count));
    out.push_str("typedef struct {\n");
    out.push_str("    unsigned int offset;\n");
    out.push_str("    unsigned char orig_byte;\n");
    out.push_str("    const char* name;\n");
    out.push_str("} ScCheckpointEntry;\n\n");

    out.push_str("static const ScCheckpointEntry SC_CHECKPOINTS[SC_CHECKPOINT_COUNT] = {\n");
    for entry in &patched.table {
        out.push_str(&format!(
            "    {{ 0x{:04X}, 0x{:02X}, \"{}\" }},\n",
            entry.offset, entry.original_byte, entry.name
        ));
    }
    out.push_str("};\n\n");
    out.push_str("#endif /* SC_CHECKPOINTS_TABLE_H */\n");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NOP sled: every byte is a valid 1-byte instruction boundary.
    #[test]
    fn test_nop_sled_patching() {
        let stub_size = 0;
        let nops = vec![0x90u8; 20]; // 20 NOPs
        let mut buf = nops;
        let patched = patch_shellcode(&mut buf, 3, stub_size).unwrap();

        assert_eq!(patched.table.len(), 3);
        for entry in &patched.table {
            assert_eq!(patched.bytes[entry.offset], 0xCC);
            assert_eq!(entry.original_byte, 0x90);
        }
        // First instruction (offset 0) should NOT be patched.
        assert_eq!(patched.bytes[0], 0x90);
    }

    /// Stub region is never modified.
    #[test]
    fn test_stub_region_skipped() {
        let stub = vec![0x53u8, 0x48, 0x89, 0xCB]; // 4 bytes of stub
        let body = vec![0x90u8; 16]; // 16 NOPs
        let mut buf = [stub.clone(), body].concat();

        let patched = patch_shellcode(&mut buf, 2, 4).unwrap();

        // Stub bytes unchanged.
        assert_eq!(&patched.bytes[..4], &[0x53, 0x48, 0x89, 0xCB]);
        // All checkpoint offsets are >= stub_size.
        for entry in &patched.table {
            assert!(entry.offset >= 4);
        }
    }

    /// Count is clamped when more checkpoints than boundaries requested.
    #[test]
    fn test_count_clamping() {
        let mut buf = vec![0x90u8; 5]; // 5 NOPs = 5 boundaries, 4 usable
        let patched = patch_shellcode(&mut buf, 100, 0).unwrap();

        // Should have at most 4 (usable = len-1 = 4).
        assert!(patched.table.len() <= 4);
        assert!(!patched.table.is_empty());
    }

    /// Empty shellcode body returns an error.
    #[test]
    fn test_empty_shellcode_errors() {
        let mut buf = vec![0x53u8; 4]; // 4 bytes of "stub"
        let result = patch_shellcode(&mut buf, 3, 4);
        assert!(result.is_err());
    }

    /// Zero checkpoint count returns unchanged shellcode.
    #[test]
    fn test_zero_count_noop() {
        let mut buf = vec![0x90u8; 10];
        let patched = patch_shellcode(&mut buf, 0, 0).unwrap();
        assert!(patched.table.is_empty());
        assert_eq!(patched.bytes, vec![0x90u8; 10]);
    }

    /// Single-instruction body can't be checkpointed (no usable boundaries).
    #[test]
    fn test_single_instruction_body() {
        let mut buf = vec![0xC3]; // single RET
        let patched = patch_shellcode(&mut buf, 5, 0).unwrap();
        assert!(patched.table.is_empty());
    }

    /// Generated header has correct format.
    #[test]
    fn test_header_format() {
        let patched = PatchedShellcode {
            bytes: vec![],
            table: vec![
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
            ],
        };

        let header = generate_c_header(&patched);
        assert!(header.contains("#define SC_CHECKPOINT_COUNT 2"));
        assert!(header.contains("0x004A"));
        assert!(header.contains("0x0094"));
        assert!(header.contains("0x48"));
        assert!(header.contains("\"sc_checkpoint_0\""));
        assert!(header.contains("ScCheckpointEntry"));
        assert!(header.starts_with("#ifndef SC_CHECKPOINTS_TABLE_H"));
    }

    /// Payload bytes are bitwise-identical when count=0 — no silent modifications.
    #[test]
    fn test_disabled_payload_bitwise_identical() {
        // Use a realistic-ish payload: stub + multi-byte body
        let original: Vec<u8> = vec![
            0x53, 0x48, 0x89, 0xCB, // stub
            0x48, 0x31, 0xC0, 0x90, 0x90, 0x48, 0x89, 0xC1, // body
            0xC3,
        ];
        let mut buf = original.clone();
        let patched = patch_shellcode(&mut buf, 0, 4).unwrap();

        assert_eq!(
            patched.bytes, original,
            "Disabled patching must return byte-identical payload"
        );
        assert!(patched.table.is_empty());
    }

    /// Boundary correctness: INT3 lands on instruction starts, not mid-instruction.
    #[test]
    fn test_boundary_correctness_multi_byte() {
        // Mix of 1-byte and multi-byte instructions:
        // 0x90       = NOP (1 byte)
        // 0x48 0x89 0xC0 = mov rax, rax (3 bytes)
        // 0x90       = NOP
        // 0x48 0x31 0xC0 = xor rax, rax (3 bytes)
        // 0x90       = NOP
        // 0xC3       = RET
        let mut buf = vec![
            0x90,
            0x48, 0x89, 0xC0,
            0x90,
            0x48, 0x31, 0xC0,
            0x90,
            0xC3,
        ];
        // Instruction boundaries: 0, 1, 4, 5, 8, 9
        // Usable (skip first): 1, 4, 5, 8, 9

        let patched = patch_shellcode(&mut buf, 2, 0).unwrap();

        // Every patched offset should be a valid instruction boundary.
        let valid_boundaries = [0usize, 1, 4, 5, 8, 9];
        for entry in &patched.table {
            assert!(
                valid_boundaries.contains(&entry.offset),
                "Offset {} is not an instruction boundary",
                entry.offset
            );
            assert_ne!(entry.offset, 0, "First instruction should not be patched");
        }
    }
}
