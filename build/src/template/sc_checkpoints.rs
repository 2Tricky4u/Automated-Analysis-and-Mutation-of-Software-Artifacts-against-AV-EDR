//! Shellcode checkpoint patching via INT3 breakpoints.
//!
//! Inserts evenly-spaced `INT3` (0xCC) breakpoints into shellcode at instruction
//! boundaries, and generates a C header table so the VEH handler can recognize
//! each checkpoint at runtime.
//!
//! Only used for instrumented builds (`trace_mode != "off"`) when the
//! `sc-checkpoints` Cargo feature is enabled.

use anyhow::Result;
use iced_x86::{Decoder, DecoderOptions, FlowControl};
use std::collections::{HashSet, VecDeque};

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

/// Collect instruction boundaries reachable via control flow (recursive descent).
///
/// Starting from offset 0 in `body`, follows branches and fall-throughs to
/// discover all reachable instruction offsets. Returns sorted, deduplicated
/// offsets relative to the full shellcode (i.e. each offset has `stub_size` added).
fn collect_reachable_boundaries(body: &[u8], stub_size: usize) -> Vec<usize> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut boundaries = Vec::new();

    queue.push_back(0usize); // entry point into body

    while let Some(start) = queue.pop_front() {
        if start >= body.len() || visited.contains(&start) {
            continue;
        }

        let mut decoder = Decoder::with_ip(64, &body[start..], start as u64, DecoderOptions::NONE);

        for instr in &mut decoder {
            let ip = instr.ip() as usize;

            if visited.contains(&ip) || instr.is_invalid() {
                break;
            }

            visited.insert(ip);
            boundaries.push(stub_size + ip);

            match instr.flow_control() {
                FlowControl::Next => { /* continue linear decode */ }
                FlowControl::UnconditionalBranch => {
                    let target = instr.near_branch_target() as usize;
                    if target < body.len() {
                        queue.push_back(target);
                    }
                    break; // stop linear decode on this path
                }
                FlowControl::ConditionalBranch => {
                    let target = instr.near_branch_target() as usize;
                    if target < body.len() {
                        queue.push_back(target);
                    }
                    // fall-through is also reachable — continue linear decode
                }
                FlowControl::Call => {
                    let target = instr.near_branch_target() as usize;
                    if target < body.len() {
                        queue.push_back(target);
                    }
                    // fall-through = return address — continue linear decode
                }
                FlowControl::Return | FlowControl::IndirectBranch | FlowControl::IndirectCall => {
                    break; // can't follow statically
                }
                FlowControl::Interrupt => { /* continue — INT3 etc. */ }
                _ => {
                    break; // XbeginXabortXend, Exception, etc.
                }
            }
        }
    }

    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

/// Disassemble shellcode after `stub_size` bytes and insert `checkpoint_count`
/// evenly-spaced INT3 breakpoints at instruction boundaries.
///
/// The stub region (`0..stub_size`) is never modified.
///
/// Uses recursive descent disassembly to follow control flow and avoid placing
/// INT3 bytes on inline data (hashes, strings, config blobs) that would be
/// misidentified as instructions by linear decoding.
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

    // Recursive descent disassembly: follow control flow to find only reachable
    // instruction boundaries, avoiding inline data that linear decode would
    // misidentify as instructions.
    let boundaries = collect_reachable_boundaries(body, stub_size);

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
        let mut buf = vec![0x90, 0x48, 0x89, 0xC0, 0x90, 0x48, 0x31, 0xC0, 0x90, 0xC3];
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

    // ======================================================================
    // Even spacing verification
    // ======================================================================

    /// Core promise: checkpoints are evenly spaced, not clustered at the start.
    #[test]
    fn test_even_spacing_nop_sled() {
        // 40 NOPs → boundaries 0..39, usable 1..39 (39 usable)
        // 3 checkpoints → interval = (39+1)/(3+1) = 10
        // Expected indices into boundaries: 10, 20, 30
        let mut buf = vec![0x90u8; 40];
        let patched = patch_shellcode(&mut buf, 3, 0).unwrap();

        assert_eq!(patched.table.len(), 3);
        let offsets: Vec<usize> = patched.table.iter().map(|e| e.offset).collect();
        assert_eq!(
            offsets,
            vec![10, 20, 30],
            "Checkpoints should be evenly spaced at interval=10"
        );
    }

    /// Even spacing with 1 checkpoint should land near the midpoint.
    #[test]
    fn test_single_checkpoint_midpoint() {
        // 20 NOPs → 19 usable → interval = (19+1)/(1+1) = 10
        // Expected: boundary index 10 → offset 10
        let mut buf = vec![0x90u8; 20];
        let patched = patch_shellcode(&mut buf, 1, 0).unwrap();

        assert_eq!(patched.table.len(), 1);
        let offset = patched.table[0].offset;
        // Should be roughly in the middle (10 out of 0..19)
        assert_eq!(offset, 10, "Single checkpoint should land at midpoint");
    }

    /// Even spacing with stub: offsets are relative to full shellcode, not body.
    #[test]
    fn test_even_spacing_with_stub() {
        let stub = vec![0x53u8, 0x48, 0x89, 0xCB]; // 4-byte stub
        let body = vec![0x90u8; 20]; // 20 NOPs
        let mut buf = [stub, body].concat();

        let patched = patch_shellcode(&mut buf, 2, 4).unwrap();

        assert_eq!(patched.table.len(), 2);
        // Body boundaries at offsets 4..23 (20 boundaries), usable 5..23 (19 usable)
        // interval = (19+1)/(2+1) = 6 (rounded)
        // boundary_idx 6 → offset 4+6=10, boundary_idx 12 → offset 4+12=16
        // (indices into body boundaries, then add stub_size)
        for entry in &patched.table {
            assert!(
                entry.offset >= 4,
                "Checkpoint offset {} is inside stub region",
                entry.offset
            );
            assert!(
                entry.offset < 24,
                "Checkpoint offset {} is past end",
                entry.offset
            );
        }
        // Verify spacing between checkpoints is roughly equal
        let gap = patched.table[1].offset - patched.table[0].offset;
        assert!(
            gap >= 4 && gap <= 8,
            "Gap between checkpoints should be ~6, got {}",
            gap
        );
    }

    // ======================================================================
    // progress_pct correctness
    // ======================================================================

    /// progress_pct should reflect position within the body (0–100).
    #[test]
    fn test_progress_pct_values() {
        // 100 NOPs, 3 checkpoints → should be ~25%, ~50%, ~75%
        let mut buf = vec![0x90u8; 100];
        let patched = patch_shellcode(&mut buf, 3, 0).unwrap();

        assert_eq!(patched.table.len(), 3);
        for entry in &patched.table {
            assert!(
                entry.progress_pct <= 100,
                "progress_pct {} > 100",
                entry.progress_pct
            );
        }
        // First checkpoint should be in ~20-30% range
        assert!(
            patched.table[0].progress_pct >= 15 && patched.table[0].progress_pct <= 35,
            "First checkpoint progress_pct should be ~25%, got {}",
            patched.table[0].progress_pct
        );
        // Last checkpoint should be in ~65-85% range
        assert!(
            patched.table[2].progress_pct >= 60 && patched.table[2].progress_pct <= 85,
            "Last checkpoint progress_pct should be ~75%, got {}",
            patched.table[2].progress_pct
        );
        // Progress should be monotonically increasing
        for i in 1..patched.table.len() {
            assert!(
                patched.table[i].progress_pct >= patched.table[i - 1].progress_pct,
                "progress_pct not monotonic: {} then {}",
                patched.table[i - 1].progress_pct,
                patched.table[i].progress_pct
            );
        }
    }

    /// progress_pct with stub should be relative to body, not total shellcode.
    #[test]
    fn test_progress_pct_with_stub() {
        let stub = vec![0x53u8; 10]; // 10-byte stub
        let body = vec![0x90u8; 100]; // 100 NOPs
        let mut buf = [stub, body].concat();

        let patched = patch_shellcode(&mut buf, 1, 10).unwrap();

        assert_eq!(patched.table.len(), 1);
        // Single checkpoint near midpoint of body → ~50%
        let pct = patched.table[0].progress_pct;
        assert!(
            pct >= 40 && pct <= 60,
            "Single checkpoint with stub: progress_pct should be ~50%, got {}",
            pct
        );
    }

    // ======================================================================
    // No duplicate offsets
    // ======================================================================

    /// No two checkpoints should land on the same offset.
    #[test]
    fn test_no_duplicate_offsets() {
        // Request many checkpoints on a small body to stress the dedup
        let mut buf = vec![0x90u8; 10]; // 10 NOPs → 9 usable
        let patched = patch_shellcode(&mut buf, 8, 0).unwrap();

        let mut seen = std::collections::HashSet::new();
        for entry in &patched.table {
            assert!(
                seen.insert(entry.offset),
                "Duplicate checkpoint offset: {}",
                entry.offset
            );
        }
    }

    /// Offsets must be strictly increasing (implies no duplicates + ordered).
    #[test]
    fn test_offsets_strictly_increasing() {
        let mut buf = vec![0x90u8; 50];
        let patched = patch_shellcode(&mut buf, 5, 0).unwrap();

        for i in 1..patched.table.len() {
            assert!(
                patched.table[i].offset > patched.table[i - 1].offset,
                "Offsets not strictly increasing: {} then {}",
                patched.table[i - 1].offset,
                patched.table[i].offset
            );
        }
    }

    // ======================================================================
    // count = usable (max saturation)
    // ======================================================================

    /// When count equals usable boundaries, every usable slot is patched.
    #[test]
    fn test_max_count_equals_usable() {
        // 6 NOPs → boundaries [0,1,2,3,4,5], usable = 5
        // Request exactly 5 → should patch all usable boundaries
        let mut buf = vec![0x90u8; 6];
        let patched = patch_shellcode(&mut buf, 5, 0).unwrap();

        // interval = (5+1)/(5+1) = 1 → every usable boundary patched
        assert_eq!(patched.table.len(), 5);
        let offsets: Vec<usize> = patched.table.iter().map(|e| e.offset).collect();
        assert_eq!(offsets, vec![1, 2, 3, 4, 5]);
        // Entry point (0) still untouched
        assert_eq!(patched.bytes[0], 0x90);
    }

    // ======================================================================
    // Realistic multi-byte instruction mix
    // ======================================================================

    /// Realistic x64 prologue + body: variable-length instructions with
    /// RIP-relative LEA, branches, and multi-byte MOV.
    #[test]
    fn test_realistic_instruction_mix() {
        #[rustfmt::skip]
        let mut buf: Vec<u8> = vec![
            // Typical function prologue
            0x55,                               // push rbp           (1 byte)  off=0
            0x48, 0x89, 0xE5,                   // mov rbp, rsp       (3 bytes) off=1
            0x48, 0x83, 0xEC, 0x20,             // sub rsp, 0x20      (4 bytes) off=4
            // Body
            0x48, 0x31, 0xC0,                   // xor rax, rax       (3 bytes) off=8
            0x48, 0x89, 0x45, 0xF8,             // mov [rbp-8], rax   (4 bytes) off=11
            0x48, 0x8D, 0x0D, 0x10, 0x00, 0x00, 0x00, // lea rcx,[rip+0x10] (7 bytes) off=15
            0xB8, 0x01, 0x00, 0x00, 0x00,       // mov eax, 1         (5 bytes) off=22
            0x90,                               // nop                (1 byte)  off=27
            0x48, 0x83, 0xC4, 0x20,             // add rsp, 0x20      (4 bytes) off=28
            0x5D,                               // pop rbp            (1 byte)  off=32
            0xC3,                               // ret                (1 byte)  off=33
        ];
        // Boundaries: 0, 1, 4, 8, 11, 15, 22, 27, 28, 32, 33  (11 total)
        // Usable (skip 0): 1, 4, 8, 11, 15, 22, 27, 28, 32, 33  (10 usable)

        let original = buf.clone();
        let patched = patch_shellcode(&mut buf, 3, 0).unwrap();

        assert_eq!(patched.table.len(), 3);

        let valid_boundaries: Vec<usize> = vec![0, 1, 4, 8, 11, 15, 22, 27, 28, 32, 33];

        for entry in &patched.table {
            // Must be a valid instruction boundary
            assert!(
                valid_boundaries.contains(&entry.offset),
                "Offset {} is not a valid instruction boundary (valid: {:?})",
                entry.offset,
                valid_boundaries
            );
            // Must not be the entry point
            assert_ne!(entry.offset, 0, "Entry point should never be patched");
            // Original byte must match what was there before patching
            assert_eq!(
                entry.original_byte, original[entry.offset],
                "original_byte at offset {} doesn't match: expected 0x{:02X}, got 0x{:02X}",
                entry.offset, original[entry.offset], entry.original_byte
            );
            // Patched byte must be INT3
            assert_eq!(
                patched.bytes[entry.offset], 0xCC,
                "Patched byte at offset {} should be 0xCC",
                entry.offset
            );
        }

        // Verify even spacing: checkpoints should span the full body, not cluster
        let first_off = patched.table[0].offset;
        let last_off = patched.table[2].offset;
        assert!(
            last_off - first_off >= 15,
            "3 checkpoints across 34-byte body should span at least 15 bytes, got {}",
            last_off - first_off
        );

        // Bytes NOT in the table should be unmodified
        let patched_offsets: std::collections::HashSet<usize> =
            patched.table.iter().map(|e| e.offset).collect();
        for (i, &byte) in patched.bytes.iter().enumerate() {
            if !patched_offsets.contains(&i) {
                assert_eq!(
                    byte, original[i],
                    "Byte at offset {} was modified but is not a checkpoint",
                    i
                );
            }
        }
    }

    /// Names follow the expected "sc_checkpoint_N" pattern with correct indices.
    #[test]
    fn test_checkpoint_naming() {
        let mut buf = vec![0x90u8; 30];
        let patched = patch_shellcode(&mut buf, 4, 0).unwrap();

        for (i, entry) in patched.table.iter().enumerate() {
            assert_eq!(
                entry.name,
                format!("sc_checkpoint_{}", i),
                "Checkpoint {} has wrong name",
                i
            );
        }
    }

    // ======================================================================
    // Recursive descent behavior
    // ======================================================================

    /// Inline data after an unconditional jump is NOT treated as instructions.
    #[test]
    fn test_recursive_descent_skips_inline_data() {
        // nop           ; off=0, reachable
        // jmp short +2  ; off=1, reachable, target=5 (skip 2 data bytes)
        // db 0xAA, 0xBB ; off=3,4 — inline DATA, NOT reachable
        // nop           ; off=5, reachable (jump target)
        // ret           ; off=6, reachable
        #[rustfmt::skip]
        let code: Vec<u8> = vec![
            0x90,             // nop
            0xEB, 0x02,       // jmp short +2 (target = 1+2+2 = 5)
            0xAA, 0xBB,       // inline data
            0x90,             // nop
            0xC3,             // ret
        ];

        let boundaries = collect_reachable_boundaries(&code, 0);

        // Offsets 0, 1, 5, 6 are reachable; 3, 4 are data
        assert!(boundaries.contains(&0), "Entry point should be a boundary");
        assert!(
            boundaries.contains(&1),
            "jmp instruction should be a boundary"
        );
        assert!(
            boundaries.contains(&5),
            "Jump target (nop) should be a boundary"
        );
        assert!(boundaries.contains(&6), "ret should be a boundary");
        assert!(
            !boundaries.contains(&3),
            "Inline data byte 0xAA should NOT be a boundary"
        );
        assert!(
            !boundaries.contains(&4),
            "Inline data byte 0xBB should NOT be a boundary"
        );
        assert_eq!(
            boundaries.len(),
            4,
            "Should have exactly 4 reachable boundaries"
        );

        // Also verify via patch_shellcode that INT3 never lands on data bytes
        let mut buf = code;
        let patched = patch_shellcode(&mut buf, 10, 0).unwrap();
        for entry in &patched.table {
            assert!(
                entry.offset != 3 && entry.offset != 4,
                "INT3 must not land on inline data at offset {}",
                entry.offset
            );
        }
    }

    /// Conditional branches explore both fall-through and branch target paths.
    #[test]
    fn test_recursive_descent_conditional_branch_both_paths() {
        // xor eax,eax   ; off=0 (2 bytes), reachable
        // je +2          ; off=2 (2 bytes), target=6, fall-through=4
        // nop            ; off=4, reachable (fall-through)
        // nop            ; off=5, reachable
        // nop            ; off=6, reachable (branch target)
        // ret            ; off=7, reachable
        #[rustfmt::skip]
        let code: Vec<u8> = vec![
            0x31, 0xC0,       // xor eax, eax
            0x74, 0x02,       // je +2 (target = 2+2+2 = 6)
            0x90,             // nop (fall-through)
            0x90,             // nop
            0x90,             // nop (branch target)
            0xC3,             // ret
        ];

        let boundaries = collect_reachable_boundaries(&code, 0);

        // All 6 instruction offsets should be reachable: {0, 2, 4, 5, 6, 7}
        let expected = vec![0usize, 2, 4, 5, 6, 7];
        for &off in &expected {
            assert!(
                boundaries.contains(&off),
                "Offset {} should be reachable",
                off
            );
        }
        assert_eq!(
            boundaries.len(),
            expected.len(),
            "Should have exactly {} reachable boundaries",
            expected.len()
        );
    }

    /// Indirect jump stops analysis — code after it is NOT reachable.
    #[test]
    fn test_recursive_descent_indirect_jump_stops() {
        // nop           ; off=0, reachable
        // mov rax,rcx   ; off=1 (3 bytes), reachable
        // jmp rax       ; off=4 (2 bytes), reachable, INDIRECT → stop
        // nop           ; off=6, NOT reachable
        // ret           ; off=7, NOT reachable
        #[rustfmt::skip]
        let code: Vec<u8> = vec![
            0x90,                   // nop
            0x48, 0x89, 0xC8,       // mov rax, rcx
            0xFF, 0xE0,             // jmp rax (indirect)
            0x90,                   // nop (unreachable)
            0xC3,                   // ret (unreachable)
        ];

        let boundaries = collect_reachable_boundaries(&code, 0);

        // Only offsets 0, 1, 4 are reachable
        assert!(boundaries.contains(&0), "Entry point should be reachable");
        assert!(boundaries.contains(&1), "mov rax,rcx should be reachable");
        assert!(boundaries.contains(&4), "jmp rax should be reachable");
        assert!(
            !boundaries.contains(&6),
            "nop after indirect jmp should NOT be reachable"
        );
        assert!(
            !boundaries.contains(&7),
            "ret after indirect jmp should NOT be reachable"
        );
        assert_eq!(
            boundaries.len(),
            3,
            "Should have exactly 3 reachable boundaries"
        );
    }
}
