//! Post-link binary PE transforms
//!
//! Operates on compiled PE bytes after linking. Each transform modifies
//! the PE structure to shift its feature vector toward benign ML classification.
//!
//! ## Transforms
//!
//! | ID                         | Effect                                          |
//! |----------------------------|-------------------------------------------------|
//! | `binary.rich_header`       | Inject MSVC-style Rich header                   |
//! | `binary.import_pad`        | Add benign imports to dilute suspicious API ratio |
//! | `binary.resource_inject`   | Add version info + manifest resources            |
//! | `binary.section_rename`    | Rename sections to MSVC defaults                 |
//! | `binary.entropy_normalize` | Add low-entropy padding section                  |
//! | `binary.string_inject`     | Append benign strings section                    |
//! | `binary.size_pad`          | Pad PE to target file size                       |
//! | `binary.debug_dir`         | Add fake PDB debug directory                     |

use anyhow::{Context, Result};
use tracing::{debug, warn};

use super::binary_data::{
    BENIGN_IMPORTS, BENIGN_STRINGS, DEFAULT_PDB_PATH, MSVC_STANDARD_SECTIONS, build_manifest,
    build_version_info, encode_rich_header, generate_low_entropy_padding, get_rich_profile,
};
use crate::mutator::MutationSpec;

// ============================================================================
// PE Constants
// ============================================================================

const SECTION_HEADER_SIZE: usize = 40;

// Section characteristics
const IMAGE_SCN_CNT_CODE: u32 = 0x0000_0020;
const IMAGE_SCN_CNT_INITIALIZED_DATA: u32 = 0x0000_0040;
const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

// Data directory indices (offset from start of data directory array)
const DD_IMPORT: usize = 1;
const DD_RESOURCE: usize = 2;
const DD_DEBUG: usize = 6;

// Debug types
const IMAGE_DEBUG_TYPE_CODEVIEW: u32 = 2;

// IMAGE_IMPORT_DESCRIPTOR size
const IMPORT_DESC_SIZE: usize = 20;

// ============================================================================
// BinaryMutator
// ============================================================================

/// Post-link binary PE mutator
///
/// Takes ownership of PE bytes, applies requested transforms, returns modified bytes.
pub struct BinaryMutator {
    pe_bytes: Vec<u8>,
}

impl BinaryMutator {
    pub fn new(pe_bytes: Vec<u8>) -> Self {
        Self { pe_bytes }
    }

    /// Apply all requested binary mutations in order.
    ///
    /// Returns `(modified_pe_bytes, list_of_applied_mutation_ids)`.
    pub fn apply(mut self, mutations: &[&MutationSpec]) -> Result<(Vec<u8>, Vec<String>)> {
        // Validate this is actually a PE
        if self.pe_bytes.len() < 64 || &self.pe_bytes[0..2] != b"MZ" {
            anyhow::bail!("Not a valid PE file (missing MZ signature)");
        }

        let mut applied = Vec::new();

        for spec in mutations {
            let (_, name) = spec.parse();
            let result = match name {
                "rich_header" => self.apply_rich_header(spec),
                "import_pad" => self.apply_import_pad(spec),
                "resource_inject" => self.apply_resource_inject(spec),
                "section_rename" => self.apply_section_rename(spec),
                "entropy_normalize" => self.apply_entropy_normalize(spec),
                "string_inject" => self.apply_string_inject(spec),
                "size_pad" => self.apply_size_pad(spec),
                "debug_dir" => self.apply_debug_dir(spec),
                other => {
                    warn!("Unknown binary mutation: binary.{}", other);
                    continue;
                }
            };

            match result {
                Ok(()) => {
                    debug!("Applied binary mutation: {}", spec.id);
                    applied.push(spec.id.clone());
                }
                Err(e) => {
                    return Err(e).context(format!("Binary mutation {} failed", spec.id));
                }
            }
        }

        // Zero PE checksum (optional for user-mode executables, avoids stale value)
        self.zero_pe_checksum();

        // Final validation: re-parse with goblin to ensure we produced a valid PE
        goblin::pe::PE::parse(&self.pe_bytes)
            .map_err(|e| anyhow::anyhow!("Binary mutations produced invalid PE: {}", e))?;

        Ok((self.pe_bytes, applied))
    }

    // ========================================================================
    // PE byte helpers
    // ========================================================================

    fn read_u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes(self.pe_bytes[offset..offset + 2].try_into().unwrap())
    }

    fn read_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.pe_bytes[offset..offset + 4].try_into().unwrap())
    }

    fn write_u16(&mut self, offset: usize, val: u16) {
        self.pe_bytes[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
    }

    fn write_u32(&mut self, offset: usize, val: u32) {
        self.pe_bytes[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
    }

    /// Get the offset of the PE optional header in the file
    fn optional_header_offset(&self) -> usize {
        let e_lfanew = self.read_u32(0x3C) as usize;
        e_lfanew + 4 + 20 // PE sig (4) + COFF header (20)
    }

    /// Get the offset of the data directory array
    fn data_directory_offset(&self) -> usize {
        // For PE32+: data directories start at optional header + 112 (0x70)
        self.optional_header_offset() + 0x70
    }

    /// Read a data directory entry (RVA, Size)
    fn read_data_dir(&self, index: usize) -> (u32, u32) {
        let dd_offset = self.data_directory_offset() + index * 8;
        (self.read_u32(dd_offset), self.read_u32(dd_offset + 4))
    }

    /// Write a data directory entry
    fn write_data_dir(&mut self, index: usize, rva: u32, size: u32) {
        let dd_offset = self.data_directory_offset() + index * 8;
        self.write_u32(dd_offset, rva);
        self.write_u32(dd_offset + 4, size);
    }

    /// Get section table offset and count
    fn section_table_info(&self) -> (usize, usize) {
        let e_lfanew = self.read_u32(0x3C) as usize;
        let coff_offset = e_lfanew + 4;
        let num_sections = self.read_u16(coff_offset + 2) as usize;
        let opt_header_size = self.read_u16(coff_offset + 16) as usize;
        let section_table_offset = coff_offset + 20 + opt_header_size;
        (section_table_offset, num_sections)
    }

    /// Get file alignment from optional header
    fn file_alignment(&self) -> u32 {
        self.read_u32(self.optional_header_offset() + 0x24)
    }

    /// Get section alignment from optional header
    fn section_alignment(&self) -> u32 {
        self.read_u32(self.optional_header_offset() + 0x20)
    }

    /// Get SizeOfImage from optional header
    fn size_of_image(&self) -> u32 {
        self.read_u32(self.optional_header_offset() + 0x38)
    }

    /// Set SizeOfImage in optional header
    fn set_size_of_image(&mut self, val: u32) {
        let off = self.optional_header_offset() + 0x38;
        self.write_u32(off, val);
    }

    /// Get SizeOfHeaders from optional header
    fn size_of_headers(&self) -> u32 {
        self.read_u32(self.optional_header_offset() + 0x3C)
    }

    /// Set NumberOfSections in COFF header
    fn set_num_sections(&mut self, val: u16) {
        let e_lfanew = self.read_u32(0x3C) as usize;
        self.write_u16(e_lfanew + 4 + 2, val);
    }

    /// Zero the PE checksum field
    fn zero_pe_checksum(&mut self) {
        let off = self.optional_header_offset() + 0x40;
        if off + 4 <= self.pe_bytes.len() {
            self.write_u32(off, 0);
        }
    }

    /// Convert RVA to file offset using section table
    fn rva_to_offset(&self, rva: u32) -> Option<usize> {
        let (sec_off, num_sec) = self.section_table_info();
        for i in 0..num_sec {
            let so = sec_off + i * SECTION_HEADER_SIZE;
            let va = self.read_u32(so + 12);
            let vs = self.read_u32(so + 8);
            let raw_size = self.read_u32(so + 16);
            let raw_ptr = self.read_u32(so + 20);
            let section_size = std::cmp::max(vs, raw_size);
            if rva >= va && rva < va + section_size {
                return Some((raw_ptr + (rva - va)) as usize);
            }
        }
        None
    }

    /// Add a new section to the PE. Returns the section's RVA.
    ///
    /// Appends the section data (padded to FileAlignment) at the end of the file,
    /// writes a new section header entry, and updates NumberOfSections + SizeOfImage.
    fn add_section(&mut self, name: &[u8; 8], data: &[u8], characteristics: u32) -> Result<u32> {
        let (sec_table_off, num_sec) = self.section_table_info();
        let file_align = self.file_alignment();
        let sec_align = self.section_alignment();
        let size_of_headers = self.size_of_headers();

        // Check there's room for another section header entry
        let new_header_end = sec_table_off + (num_sec + 1) * SECTION_HEADER_SIZE;
        if new_header_end as u32 > size_of_headers {
            anyhow::bail!(
                "No room in section table for new entry (need {:#x}, headers end at {:#x})",
                new_header_end,
                size_of_headers
            );
        }

        // Find the last section to determine new VA and file offset
        let (last_va, last_vs, last_raw_ptr, last_raw_size) = if num_sec > 0 {
            let last = sec_table_off + (num_sec - 1) * SECTION_HEADER_SIZE;
            (
                self.read_u32(last + 12),
                self.read_u32(last + 8),
                self.read_u32(last + 20),
                self.read_u32(last + 16),
            )
        } else {
            (0, 0, size_of_headers, 0)
        };

        // New section VA: aligned after last section
        let new_va = align_up(last_va + std::cmp::max(last_vs, last_raw_size), sec_align);

        // New section file offset: aligned after last section's raw data
        let new_raw_ptr = align_up(last_raw_ptr + last_raw_size, file_align);

        // Pad the section data to FileAlignment
        let padded_size = align_up(data.len() as u32, file_align);

        // Extend file to fit (may need to fill gap between current EOF and new_raw_ptr)
        let required_file_size = new_raw_ptr as usize + padded_size as usize;
        if self.pe_bytes.len() < required_file_size {
            self.pe_bytes.resize(required_file_size, 0);
        }

        // Write section data
        self.pe_bytes[new_raw_ptr as usize..new_raw_ptr as usize + data.len()]
            .copy_from_slice(data);

        // Write section header
        let hdr_off = sec_table_off + num_sec * SECTION_HEADER_SIZE;

        // Name (8 bytes)
        self.pe_bytes[hdr_off..hdr_off + 8].copy_from_slice(name);
        // VirtualSize
        self.write_u32(hdr_off + 8, data.len() as u32);
        // VirtualAddress
        self.write_u32(hdr_off + 12, new_va);
        // SizeOfRawData
        self.write_u32(hdr_off + 16, padded_size);
        // PointerToRawData
        self.write_u32(hdr_off + 20, new_raw_ptr);
        // PointerToRelocations, PointerToLinenumbers, NumberOfRelocations, NumberOfLinenumbers = 0
        // (already zero from the header area)
        // Characteristics
        self.write_u32(hdr_off + 36, characteristics);

        // Update NumberOfSections
        self.set_num_sections(num_sec as u16 + 1);

        // Update SizeOfImage
        let new_image_end = align_up(new_va + data.len() as u32, sec_align);
        if new_image_end > self.size_of_image() {
            self.set_size_of_image(new_image_end);
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

    // ========================================================================
    // Transform: binary.rich_header
    // ========================================================================

    /// Inject an MSVC-style Rich header between DOS stub and PE signature
    ///
    /// Params:
    /// - `donor`: "notepad" | "calc" | "explorer" (default: "notepad")
    fn apply_rich_header(&mut self, spec: &MutationSpec) -> Result<()> {
        let donor = spec
            .params
            .get("donor")
            .map(|s| s.as_str())
            .unwrap_or("notepad");
        let profile = get_rich_profile(donor);

        let e_lfanew = self.read_u32(0x3C) as usize;
        const DOS_HEADER_END: usize = 0x40; // Standard DOS header is 64 bytes

        if e_lfanew <= DOS_HEADER_END {
            anyhow::bail!(
                "e_lfanew too small for Rich header injection: {:#x}",
                e_lfanew
            );
        }

        let available = e_lfanew - DOS_HEADER_END;

        // Calculate max records that fit: overhead = 16 (DanS+pad) + 8 (Rich+cs)
        let overhead = 24usize;
        if available < overhead {
            anyhow::bail!(
                "Not enough space for Rich header ({} bytes available, need >= {})",
                available,
                overhead
            );
        }
        let max_records = (available - overhead) / 8;
        let records_to_use = std::cmp::min(profile.records.len(), max_records);

        let rich_bytes = encode_rich_header(&profile.records[..records_to_use], profile.checksum);

        // Zero-fill the gap
        for byte in self.pe_bytes[DOS_HEADER_END..e_lfanew].iter_mut() {
            *byte = 0;
        }

        // Write Rich header right-aligned before PE signature
        let start = e_lfanew - rich_bytes.len();
        self.pe_bytes[start..start + rich_bytes.len()].copy_from_slice(&rich_bytes);

        debug!(
            "Injected Rich header: donor={}, {} records, {} bytes at offset {:#x}",
            donor,
            records_to_use,
            rich_bytes.len(),
            start
        );

        Ok(())
    }

    // ========================================================================
    // Transform: binary.import_pad
    // ========================================================================

    /// Add benign imports to dilute the suspicious-import ratio
    ///
    /// Params:
    /// - `count`: max number of benign functions to add (default: 30)
    fn apply_import_pad(&mut self, spec: &MutationSpec) -> Result<()> {
        let max_count: usize = spec
            .params
            .get("count")
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        // Parse existing imports to know what DLLs are already present
        let existing_dlls: Vec<String> = {
            let pe = goblin::pe::PE::parse(&self.pe_bytes)
                .map_err(|e| anyhow::anyhow!("Failed to parse PE for import analysis: {}", e))?;
            pe.imports
                .iter()
                .map(|imp| imp.dll.to_lowercase())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect()
        };

        // Select benign DLLs not already imported
        let mut new_dlls: Vec<(&str, Vec<&str>)> = Vec::new();
        let mut total_funcs = 0;

        for import in BENIGN_IMPORTS.iter() {
            if total_funcs >= max_count {
                break;
            }
            if existing_dlls
                .iter()
                .any(|d| d == &import.dll.to_lowercase())
            {
                debug!("Skipping {} (already imported)", import.dll);
                continue;
            }

            let remaining = max_count - total_funcs;
            let funcs: Vec<&str> = import.functions.iter().take(remaining).copied().collect();
            total_funcs += funcs.len();
            new_dlls.push((import.dll, funcs));
        }

        if new_dlls.is_empty() {
            debug!("No new benign imports to add (all DLLs already imported)");
            return Ok(());
        }

        // Read existing IDT bytes
        let (import_rva, import_size) = self.read_data_dir(DD_IMPORT);
        let existing_idt_bytes = if import_rva > 0 && import_size > 0 {
            let offset = self
                .rva_to_offset(import_rva)
                .context("Failed to resolve import table RVA")?;

            // Count existing descriptors (stop at null terminator)
            let mut count = 0;
            let mut pos = offset;
            loop {
                if pos + IMPORT_DESC_SIZE > self.pe_bytes.len() {
                    break;
                }
                // Check if this is the null terminator (all zeros)
                let is_null = self.pe_bytes[pos..pos + IMPORT_DESC_SIZE]
                    .iter()
                    .all(|&b| b == 0);
                if is_null {
                    break;
                }
                count += 1;
                pos += IMPORT_DESC_SIZE;
            }
            self.pe_bytes[offset..offset + count * IMPORT_DESC_SIZE].to_vec()
        } else {
            Vec::new()
        };

        // Calculate where new section will go
        let (sec_table_off, num_sec) = self.section_table_info();
        let sec_align = self.section_alignment();

        let new_section_va = if num_sec > 0 {
            let last = sec_table_off + (num_sec - 1) * SECTION_HEADER_SIZE;
            let last_va = self.read_u32(last + 12);
            let last_vs = self.read_u32(last + 8);
            let last_rs = self.read_u32(last + 16);
            align_up(last_va + std::cmp::max(last_vs, last_rs), sec_align)
        } else {
            align_up(self.size_of_headers(), sec_align)
        };

        // Build the import section data
        let section_data = build_import_section(&existing_idt_bytes, &new_dlls, new_section_va);

        // Add the section
        let actual_va = self.add_section(
            b".idata\0\0",
            &section_data,
            IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
        )?;

        // If actual_va differs from predicted, we need to rebuild (shouldn't happen)
        if actual_va != new_section_va {
            warn!(
                "Section VA mismatch: predicted {:#x}, got {:#x}. Rebuilding import data.",
                new_section_va, actual_va
            );
            // Rebuild with correct RVA
            let section_data = build_import_section(&existing_idt_bytes, &new_dlls, actual_va);

            // Overwrite the section data we just wrote
            let (sec_off, num) = self.section_table_info();
            let last = sec_off + (num - 1) * SECTION_HEADER_SIZE;
            let raw_ptr = self.read_u32(last + 20) as usize;
            self.pe_bytes[raw_ptr..raw_ptr + section_data.len()].copy_from_slice(&section_data);
        }

        // Update DataDirectory[1] (Import Table) to point to our new combined IDT
        let idt_size = (existing_idt_bytes.len() / IMPORT_DESC_SIZE
            + new_dlls.len()
            + 1) // +1 for null terminator
            * IMPORT_DESC_SIZE;
        self.write_data_dir(DD_IMPORT, actual_va, idt_size as u32);

        debug!(
            "Added {} benign imports from {} DLLs ({} functions total)",
            new_dlls.len(),
            new_dlls.len(),
            total_funcs
        );

        Ok(())
    }

    // ========================================================================
    // Transform: binary.resource_inject
    // ========================================================================

    /// Add version info + manifest resources
    ///
    /// Params:
    /// - `product_name`: (default: "System Configuration Utility")
    /// - `company`: (default: "Microsoft Corporation")
    fn apply_resource_inject(&mut self, spec: &MutationSpec) -> Result<()> {
        let product_name = spec
            .params
            .get("product_name")
            .map(|s| s.as_str())
            .unwrap_or("System Configuration Utility");
        let company = spec
            .params
            .get("company")
            .map(|s| s.as_str())
            .unwrap_or("Microsoft Corporation");

        // Build resource data payloads
        let version_data = build_version_info(product_name, company, "1.0.0.0");
        let manifest_data = build_manifest(product_name).into_bytes();

        // Calculate where new section will go
        let (sec_table_off, num_sec) = self.section_table_info();
        let sec_align = self.section_alignment();

        let new_section_va = if num_sec > 0 {
            let last = sec_table_off + (num_sec - 1) * SECTION_HEADER_SIZE;
            let last_va = self.read_u32(last + 12);
            let last_vs = self.read_u32(last + 8);
            let last_rs = self.read_u32(last + 16);
            align_up(last_va + std::cmp::max(last_vs, last_rs), sec_align)
        } else {
            align_up(self.size_of_headers(), sec_align)
        };

        // Build resource directory + data
        let section_data = build_resource_section(&version_data, &manifest_data, new_section_va);

        let actual_va = self.add_section(
            b".rsrc\0\0\0",
            &section_data,
            IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ,
        )?;

        if actual_va != new_section_va {
            warn!(
                "Resource section VA mismatch: predicted {:#x}, got {:#x}. Rebuilding.",
                new_section_va, actual_va
            );
            let section_data = build_resource_section(&version_data, &manifest_data, actual_va);
            let (sec_off, num) = self.section_table_info();
            let last = sec_off + (num - 1) * SECTION_HEADER_SIZE;
            let raw_ptr = self.read_u32(last + 20) as usize;
            self.pe_bytes[raw_ptr..raw_ptr + section_data.len()].copy_from_slice(&section_data);
        }

        // Update DataDirectory[2] (Resource Table)
        self.write_data_dir(DD_RESOURCE, actual_va, section_data.len() as u32);

        debug!(
            "Injected resources: version info ({} bytes) + manifest ({} bytes)",
            version_data.len(),
            manifest_data.len()
        );

        Ok(())
    }

    // ========================================================================
    // Transform: binary.section_rename
    // ========================================================================

    /// Rename non-standard sections to MSVC defaults based on characteristics.
    ///
    /// Params: none (deterministic)
    fn apply_section_rename(&mut self, _spec: &MutationSpec) -> Result<()> {
        let (sec_table_off, num_sec) = self.section_table_info();
        let mut renamed = 0u32;

        for i in 0..num_sec {
            let so = sec_table_off + i * SECTION_HEADER_SIZE;
            let name: [u8; 8] = self.pe_bytes[so..so + 8].try_into().unwrap();

            // Skip if already a standard MSVC section name
            if MSVC_STANDARD_SECTIONS
                .iter()
                .any(|std_name| **std_name == name)
            {
                continue;
            }

            let chars = self.read_u32(so + 36);
            let new_name: &[u8; 8] = if chars & IMAGE_SCN_CNT_CODE != 0
                && chars & IMAGE_SCN_MEM_EXECUTE != 0
                && chars & IMAGE_SCN_MEM_READ != 0
            {
                b".text\0\0\0"
            } else if chars & IMAGE_SCN_CNT_INITIALIZED_DATA != 0
                && chars & IMAGE_SCN_MEM_READ != 0
                && chars & IMAGE_SCN_MEM_WRITE != 0
            {
                b".data\0\0\0"
            } else if chars & IMAGE_SCN_CNT_INITIALIZED_DATA != 0 && chars & IMAGE_SCN_MEM_READ != 0
            {
                b".rdata\0\0"
            } else {
                // Unknown characteristics, leave unchanged
                continue;
            };

            self.pe_bytes[so..so + 8].copy_from_slice(new_name);
            renamed += 1;
            debug!(
                "Renamed section '{}' → '{}'",
                String::from_utf8_lossy(&name).trim_end_matches('\0'),
                String::from_utf8_lossy(new_name).trim_end_matches('\0'),
            );
        }

        debug!("Section rename: {} sections renamed", renamed);
        Ok(())
    }

    // ========================================================================
    // Transform: binary.entropy_normalize
    // ========================================================================

    /// Add low-entropy padding section to dilute overall file entropy.
    ///
    /// Params:
    /// - `target`: target file entropy in bits/byte (default: "6.0")
    fn apply_entropy_normalize(&mut self, spec: &MutationSpec) -> Result<()> {
        let target: f64 = spec
            .params
            .get("target")
            .and_then(|s| s.parse().ok())
            .unwrap_or(6.0);

        let current = compute_entropy(&self.pe_bytes);
        if current <= target {
            debug!(
                "Entropy already at {:.2} (target {:.2}), skipping",
                current, target
            );
            return Ok(());
        }

        // Binary search for the padding size needed.
        // Adding N bytes of ~0 entropy data to a file of size L with entropy H:
        // We estimate conservatively and iterate.
        let file_len = self.pe_bytes.len();
        let mut lo: usize = 0;
        let mut hi: usize = file_len * 4; // max 4x file size padding
        let mut best_size = hi;

        for _ in 0..30 {
            let mid = (lo + hi) / 2;
            if mid == 0 {
                break;
            }
            // Estimate: padding has entropy ~3.5 bits/byte
            let pad_entropy = 3.5;
            let combined =
                (current * file_len as f64 + pad_entropy * mid as f64) / (file_len + mid) as f64;
            if combined <= target {
                best_size = mid;
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }

        // Minimum 512 bytes to be meaningful
        let pad_size = best_size.max(512);
        let padding = generate_low_entropy_padding(pad_size);

        self.add_section(
            b".reloc2\0",
            &padding,
            IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ,
        )?;

        let new_entropy = compute_entropy(&self.pe_bytes);
        debug!(
            "Entropy normalized: {:.2} → {:.2} (target {:.2}, added {} bytes padding)",
            current, new_entropy, target, pad_size
        );

        Ok(())
    }

    // ========================================================================
    // Transform: binary.string_inject
    // ========================================================================

    /// Append benign strings as a new section.
    ///
    /// Params:
    /// - `count`: number of strings to include (default: "20")
    fn apply_string_inject(&mut self, spec: &MutationSpec) -> Result<()> {
        let count: usize = spec
            .params
            .get("count")
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);

        let count = count.min(BENIGN_STRINGS.len());

        // Concatenate null-terminated ASCII strings
        let mut string_data = Vec::new();
        for s in BENIGN_STRINGS.iter().take(count) {
            string_data.extend_from_slice(s.as_bytes());
            string_data.push(0);
        }

        self.add_section(
            b".rdata2\0",
            &string_data,
            IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ,
        )?;

        debug!(
            "Injected {} benign strings ({} bytes) into .rdata2 section",
            count,
            string_data.len()
        );

        Ok(())
    }

    // ========================================================================
    // Transform: binary.size_pad
    // ========================================================================

    /// Pad PE to a target file size.
    ///
    /// Params:
    /// - `target_kb`: target file size in KB (default: "256")
    fn apply_size_pad(&mut self, spec: &MutationSpec) -> Result<()> {
        let target_kb: usize = spec
            .params
            .get("target_kb")
            .and_then(|s| s.parse().ok())
            .unwrap_or(256);

        let target_bytes = target_kb * 1024;
        let current_size = self.pe_bytes.len();

        if current_size >= target_bytes {
            debug!(
                "File already {} bytes (target {} bytes), skipping size pad",
                current_size, target_bytes
            );
            return Ok(());
        }

        let pad_needed = target_bytes - current_size;
        let padding = generate_low_entropy_padding(pad_needed);

        self.add_section(
            b".data2\0\0",
            &padding,
            IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ,
        )?;

        debug!(
            "Size padded: {} → {} bytes (target {} KB)",
            current_size,
            self.pe_bytes.len(),
            target_kb
        );

        Ok(())
    }

    // ========================================================================
    // Transform: binary.debug_dir
    // ========================================================================

    /// Add a fake PDB debug directory entry (CodeView/RSDS).
    ///
    /// Params:
    /// - `pdb_path`: fake PDB path (default: DEFAULT_PDB_PATH)
    fn apply_debug_dir(&mut self, spec: &MutationSpec) -> Result<()> {
        let pdb_path = spec
            .params
            .get("pdb_path")
            .map(|s| s.as_str())
            .unwrap_or(DEFAULT_PDB_PATH);

        // Read TimeDateStamp from COFF header for the debug directory entry
        let e_lfanew = self.read_u32(0x3C) as usize;
        let timestamp = self.read_u32(e_lfanew + 4 + 4); // COFF header + 4 = TimeDateStamp

        // Build CodeView (RSDS) debug info:
        //   "RSDS" (4) + GUID (16) + Age (4) + PDB path (null-terminated)
        let pdb_path_bytes = pdb_path.as_bytes();
        let codeview_size = 4 + 16 + 4 + pdb_path_bytes.len() + 1;
        let mut codeview = Vec::with_capacity(codeview_size);
        codeview.extend_from_slice(b"RSDS");
        // GUID: deterministic but looks random (derived from timestamp)
        let guid_base = timestamp.wrapping_mul(0x01000193);
        codeview.extend_from_slice(&guid_base.to_le_bytes());
        codeview.extend_from_slice(&guid_base.wrapping_add(1).to_le_bytes());
        codeview.extend_from_slice(&guid_base.wrapping_add(2).to_le_bytes());
        codeview.extend_from_slice(&guid_base.wrapping_add(3).to_le_bytes());
        // Age = 1
        codeview.extend_from_slice(&1u32.to_le_bytes());
        // PDB path (null-terminated)
        codeview.extend_from_slice(pdb_path_bytes);
        codeview.push(0);

        // Build IMAGE_DEBUG_DIRECTORY (28 bytes)
        // Will be filled with correct offsets after we know the section VA
        let debug_dir_size: usize = 28;
        let total_size = debug_dir_size + codeview.len();

        // Pre-calculate section VA
        let (sec_table_off, num_sec) = self.section_table_info();
        let sec_align = self.section_alignment();
        let new_section_va = if num_sec > 0 {
            let last = sec_table_off + (num_sec - 1) * SECTION_HEADER_SIZE;
            let last_va = self.read_u32(last + 12);
            let last_vs = self.read_u32(last + 8);
            let last_rs = self.read_u32(last + 16);
            align_up(last_va + std::cmp::max(last_vs, last_rs), sec_align)
        } else {
            align_up(self.size_of_headers(), sec_align)
        };

        // Build section data: [debug_directory (28 bytes)] + [codeview_data]
        let codeview_rva = new_section_va + debug_dir_size as u32;
        let mut section_data = vec![0u8; total_size];

        // IMAGE_DEBUG_DIRECTORY fields:
        // Characteristics (4) = 0
        write_u32_at(&mut section_data, 0, 0);
        // TimeDateStamp (4)
        write_u32_at(&mut section_data, 4, timestamp);
        // MajorVersion (2) + MinorVersion (2) = 0
        write_u16_at(&mut section_data, 8, 0);
        write_u16_at(&mut section_data, 10, 0);
        // Type (4) = IMAGE_DEBUG_TYPE_CODEVIEW
        write_u32_at(&mut section_data, 12, IMAGE_DEBUG_TYPE_CODEVIEW);
        // SizeOfData (4)
        write_u32_at(&mut section_data, 16, codeview.len() as u32);
        // AddressOfRawData (RVA) (4) — will be fixed up after add_section
        write_u32_at(&mut section_data, 20, codeview_rva);
        // PointerToRawData (4) — will be fixed up after add_section
        // (leave as 0 for now, fix below)
        write_u32_at(&mut section_data, 24, 0);

        // Copy CodeView data
        section_data[debug_dir_size..].copy_from_slice(&codeview);

        // Add the section
        let actual_va = self.add_section(
            b".debug\0\0",
            &section_data,
            IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ,
        )?;

        // Fix up RVAs and file offsets if actual_va differs from predicted
        let actual_codeview_rva = actual_va + debug_dir_size as u32;

        // Find the section we just added to get its raw pointer
        let (sec_off, num) = self.section_table_info();
        let last_sec = sec_off + (num - 1) * SECTION_HEADER_SIZE;
        let raw_ptr = self.read_u32(last_sec + 20);

        // Fix AddressOfRawData (RVA to codeview data)
        let debug_dir_file_off = raw_ptr as usize;
        write_u32_at(
            &mut self.pe_bytes,
            debug_dir_file_off + 20,
            actual_codeview_rva,
        );
        // Fix PointerToRawData (file offset to codeview data)
        write_u32_at(
            &mut self.pe_bytes,
            debug_dir_file_off + 24,
            raw_ptr + debug_dir_size as u32,
        );

        // Update DataDirectory[6] (Debug) → RVA of debug directory, Size = 28
        self.write_data_dir(DD_DEBUG, actual_va, debug_dir_size as u32);

        debug!(
            "Injected debug directory: PDB='{}', VA={:#x}, codeview {} bytes",
            pdb_path,
            actual_va,
            codeview.len()
        );

        Ok(())
    }
}

// ============================================================================
// Import Section Builder
// ============================================================================

/// Build complete import section data with combined IDT + new ILTs/IATs/names
fn build_import_section(
    existing_idt_bytes: &[u8],
    new_dlls: &[(&str, Vec<&str>)],
    base_rva: u32,
) -> Vec<u8> {
    let existing_desc_count = existing_idt_bytes.len() / IMPORT_DESC_SIZE;

    // ── Phase 1: Calculate layout offsets ──

    // IDT: existing descriptors + new descriptors + null terminator
    let total_descs = existing_desc_count + new_dlls.len() + 1;
    let idt_size = total_descs * IMPORT_DESC_SIZE;
    let mut offset = idt_size;

    // ILT arrays (one per new DLL, null-terminated)
    let mut ilt_offsets = Vec::new();
    for (_, funcs) in new_dlls {
        ilt_offsets.push(offset);
        offset += (funcs.len() + 1) * 8; // +1 for null terminator
    }

    // IAT arrays (one per new DLL, null-terminated, same content as ILT on disk)
    let mut iat_offsets = Vec::new();
    for (_, funcs) in new_dlls {
        iat_offsets.push(offset);
        offset += (funcs.len() + 1) * 8;
    }

    // Hint/Name entries (per function)
    let mut hint_name_offsets: Vec<Vec<usize>> = Vec::new();
    for (_, funcs) in new_dlls {
        let mut dll_offsets = Vec::new();
        for func in funcs {
            dll_offsets.push(offset);
            // Hint (u16) + name + null, padded to WORD boundary
            let entry_size = 2 + func.len() + 1;
            offset += (entry_size + 1) & !1;
        }
        hint_name_offsets.push(dll_offsets);
    }

    // DLL name strings
    let mut dll_name_offsets = Vec::new();
    for (dll_name, _) in new_dlls {
        dll_name_offsets.push(offset);
        offset += dll_name.len() + 1; // null-terminated
    }

    let total_size = offset;

    // ── Phase 2: Build section data ──

    let mut data = vec![0u8; total_size];

    // Copy existing IDT descriptors
    if !existing_idt_bytes.is_empty() {
        data[..existing_idt_bytes.len()].copy_from_slice(existing_idt_bytes);
    }

    // Write new import descriptors
    for (i, _) in new_dlls.iter().enumerate() {
        let desc_off = existing_idt_bytes.len() + i * IMPORT_DESC_SIZE;

        // OriginalFirstThunk (ILT RVA)
        write_u32_at(&mut data, desc_off, base_rva + ilt_offsets[i] as u32);
        // TimeDateStamp = 0 (already zero)
        // ForwarderChain = 0 (already zero)
        // Name (DLL name RVA)
        write_u32_at(
            &mut data,
            desc_off + 12,
            base_rva + dll_name_offsets[i] as u32,
        );
        // FirstThunk (IAT RVA)
        write_u32_at(&mut data, desc_off + 16, base_rva + iat_offsets[i] as u32);
    }
    // Null terminator descriptor is already zeros

    // Write ILT and IAT entries
    for (i, (_, funcs)) in new_dlls.iter().enumerate() {
        for (j, _) in funcs.iter().enumerate() {
            let hint_name_rva = base_rva + hint_name_offsets[i][j] as u32;

            // ILT entry: 8 bytes, bit 63 = 0 → import by name, bits 0-30 = RVA
            let ilt_off = ilt_offsets[i] + j * 8;
            write_u64_at(&mut data, ilt_off, hint_name_rva as u64);

            // IAT entry: same content on disk (loader overwrites at runtime)
            let iat_off = iat_offsets[i] + j * 8;
            write_u64_at(&mut data, iat_off, hint_name_rva as u64);
        }
        // Null terminators already zero
    }

    // Write Hint/Name entries
    for (i, (_, funcs)) in new_dlls.iter().enumerate() {
        for (j, func) in funcs.iter().enumerate() {
            let off = hint_name_offsets[i][j];
            // Hint = 0 (already zero, let loader resolve)
            // Function name (after 2-byte hint)
            data[off + 2..off + 2 + func.len()].copy_from_slice(func.as_bytes());
            // Null terminator already zero
        }
    }

    // Write DLL name strings
    for (i, (dll_name, _)) in new_dlls.iter().enumerate() {
        let off = dll_name_offsets[i];
        data[off..off + dll_name.len()].copy_from_slice(dll_name.as_bytes());
        // Null terminator already zero
    }

    data
}

// ============================================================================
// Resource Section Builder
// ============================================================================

// Resource types
const RT_VERSION: u32 = 16;
const RT_MANIFEST: u32 = 24;

/// Build a .rsrc section with RT_VERSION and RT_MANIFEST resources
fn build_resource_section(version_data: &[u8], manifest_data: &[u8], section_rva: u32) -> Vec<u8> {
    // Layout:
    //
    // [Root Dir]       16 bytes + 2 entries (16 bytes)  = 32 bytes  @ 0x00
    // [Version Dir]    16 bytes + 1 entry  (8 bytes)    = 24 bytes  @ 0x20
    // [Manifest Dir]   16 bytes + 1 entry  (8 bytes)    = 24 bytes  @ 0x38
    // [Version Lang]   16 bytes + 1 entry  (8 bytes)    = 24 bytes  @ 0x50
    // [Manifest Lang]  16 bytes + 1 entry  (8 bytes)    = 24 bytes  @ 0x68
    // [Version DataE]  16 bytes                                     @ 0x80
    // [Manifest DataE] 16 bytes                                     @ 0x90
    // [Version Data]   variable                                     @ 0xA0
    // [Manifest Data]  variable                                     @ 0xA0 + ver_len_aligned

    let directories_end: usize = 0xA0; // after all directories + data entries
    let ver_data_offset = directories_end;
    let ver_data_aligned = align_up_u32(version_data.len() as u32, 4) as usize;
    let manifest_data_offset = ver_data_offset + ver_data_aligned;
    let total_size = manifest_data_offset + manifest_data.len();

    let mut data = vec![0u8; total_size];

    // ── Root Directory @ 0x00 ──
    // NumberOfIdEntries = 2
    write_u16_at(&mut data, 0x0C, 0); // NumberOfNamedEntries
    write_u16_at(&mut data, 0x0E, 2); // NumberOfIdEntries

    // Entry[0]: RT_VERSION → subdirectory at 0x20
    write_u32_at(&mut data, 0x10, RT_VERSION); // ID
    write_u32_at(&mut data, 0x14, 0x20 | 0x8000_0000); // Offset (high bit = subdirectory)

    // Entry[1]: RT_MANIFEST → subdirectory at 0x38
    write_u32_at(&mut data, 0x18, RT_MANIFEST);
    write_u32_at(&mut data, 0x1C, 0x38 | 0x8000_0000);

    // ── Version Name Directory @ 0x20 ──
    write_u16_at(&mut data, 0x2C, 0); // Named
    write_u16_at(&mut data, 0x2E, 1); // ID entries
    // Entry: ID=1 → subdirectory at 0x50
    write_u32_at(&mut data, 0x30, 1); // Resource Name/ID = 1
    write_u32_at(&mut data, 0x34, 0x50 | 0x8000_0000);

    // ── Manifest Name Directory @ 0x38 ──
    write_u16_at(&mut data, 0x48, 0);
    write_u16_at(&mut data, 0x4A, 1);
    // Entry: ID=1 → subdirectory at 0x68
    write_u32_at(&mut data, 0x4C, 1);
    write_u32_at(&mut data, 0x50 - 4, 0x68 | 0x8000_0000); // 0x4C + 4 = 0x50

    // Wait, let me recalculate offsets more carefully.
    // The directory entries start after the directory header.
    // At 0x38: directory header (16 bytes) → entries start at 0x48
    // But I wrote NumberOfIdEntries at 0x48/0x4A. That's wrong.
    // Let me redo with explicit offset tracking.

    // Actually let me just zero the buffer and write everything at explicit offsets.
    data.fill(0);

    // Root directory (0x00..0x10)
    write_u16_at(&mut data, 0x0E, 2); // 2 ID entries
    // Root entries (0x10..0x20)
    write_u32_at(&mut data, 0x10, RT_VERSION);
    write_u32_at(&mut data, 0x14, 0x8000_0020); // → subdir at 0x20
    write_u32_at(&mut data, 0x18, RT_MANIFEST);
    write_u32_at(&mut data, 0x1C, 0x8000_0038); // → subdir at 0x38

    // Version name dir (0x20..0x30)
    write_u16_at(&mut data, 0x2E, 1); // 1 ID entry
    // Version name entry (0x30..0x38)
    write_u32_at(&mut data, 0x30, 1); // ID = 1
    write_u32_at(&mut data, 0x34, 0x8000_0050); // → lang dir at 0x50

    // Manifest name dir (0x38..0x48)
    write_u16_at(&mut data, 0x46, 1); // 1 ID entry
    // Manifest name entry (0x48..0x50)
    write_u32_at(&mut data, 0x48, 1); // ID = 1 (for exe manifest)
    write_u32_at(&mut data, 0x4C, 0x8000_0068); // → lang dir at 0x68

    // Version language dir (0x50..0x60)
    write_u16_at(&mut data, 0x5E, 1); // 1 ID entry
    // Version language entry (0x60..0x68)
    write_u32_at(&mut data, 0x60, 0x0409); // English US
    write_u32_at(&mut data, 0x64, 0x80); // → data entry at 0x80 (NOT subdirectory, no high bit)

    // Manifest language dir (0x68..0x78)
    write_u16_at(&mut data, 0x76, 1); // 1 ID entry
    // Manifest language entry (0x78..0x80)
    write_u32_at(&mut data, 0x78, 0x0409);
    write_u32_at(&mut data, 0x7C, 0x90); // → data entry at 0x90

    // Version data entry (0x80..0x90)
    write_u32_at(&mut data, 0x80, section_rva + ver_data_offset as u32); // RVA to data
    write_u32_at(&mut data, 0x84, version_data.len() as u32); // Size
    // CodePage = 0, Reserved = 0 (already zero)

    // Manifest data entry (0x90..0xA0)
    write_u32_at(&mut data, 0x90, section_rva + manifest_data_offset as u32);
    write_u32_at(&mut data, 0x94, manifest_data.len() as u32);

    // Version data
    data[ver_data_offset..ver_data_offset + version_data.len()].copy_from_slice(version_data);

    // Manifest data
    data[manifest_data_offset..manifest_data_offset + manifest_data.len()]
        .copy_from_slice(manifest_data);

    data
}

// ============================================================================
// Utility functions
// ============================================================================

/// Compute Shannon entropy of a byte slice in bits per byte.
fn compute_entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let len = data.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

fn align_up(val: u32, align: u32) -> u32 {
    if align == 0 {
        return val;
    }
    (val + align - 1) & !(align - 1)
}

fn align_up_u32(val: u32, align: u32) -> u32 {
    align_up(val, align)
}

fn write_u16_at(buf: &mut [u8], offset: usize, val: u16) {
    buf[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
}

fn write_u32_at(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

fn write_u64_at(buf: &mut [u8], offset: usize, val: u64) {
    buf[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::binary_data;
    use std::collections::HashMap;

    /// Build a minimal valid PE32+ (x64) for testing
    ///
    /// Layout:
    /// - DOS header at 0x00 (64 bytes), e_lfanew = 0x80
    /// - Gap for Rich header at 0x40..0x80
    /// - PE signature at 0x80
    /// - COFF header at 0x84
    /// - Optional header (PE32+) at 0x98 (240 bytes)
    /// - Section table at 0x188 (1 section, 40 bytes)
    /// - Padding to 0x200
    /// - .text section data at 0x200 (512 bytes)
    fn create_test_pe() -> Vec<u8> {
        let file_alignment: u32 = 0x200;
        let section_alignment: u32 = 0x1000;
        let headers_size: u32 = 0x200;
        let text_va: u32 = 0x1000;
        let text_raw_size: u32 = 0x200;
        let size_of_image: u32 = 0x2000;

        let mut pe = vec![0u8; (headers_size + text_raw_size) as usize];

        // ── DOS Header ──
        pe[0] = b'M';
        pe[1] = b'Z';
        // e_lfanew at 0x3C
        write_u32_at(&mut pe, 0x3C, 0x80);

        // ── PE Signature at 0x80 ──
        pe[0x80] = b'P';
        pe[0x81] = b'E';
        // 0x82, 0x83 = 0x00

        // ── COFF Header at 0x84 ──
        write_u16_at(&mut pe, 0x84, 0x8664); // Machine = AMD64
        write_u16_at(&mut pe, 0x86, 1); // NumberOfSections
        write_u32_at(&mut pe, 0x88, 0x6789_ABCD); // TimeDateStamp
        // PointerToSymbolTable, NumberOfSymbols = 0
        write_u16_at(&mut pe, 0x94, 0xF0); // SizeOfOptionalHeader = 240
        write_u16_at(&mut pe, 0x96, 0x0022); // Characteristics (EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE)

        // ── Optional Header at 0x98 ──
        let opt = 0x98;
        write_u16_at(&mut pe, opt, 0x020B); // Magic = PE32+
        pe[opt + 2] = 14; // MajorLinkerVersion
        pe[opt + 3] = 0; // MinorLinkerVersion

        // SizeOfCode
        write_u32_at(&mut pe, opt + 4, text_raw_size);
        // AddressOfEntryPoint
        write_u32_at(&mut pe, opt + 16, text_va);
        // ImageBase (PE32+: 8 bytes at offset 24)
        write_u64_at(&mut pe, opt + 24, 0x0000_0001_4000_0000);
        // SectionAlignment
        write_u32_at(&mut pe, opt + 32, section_alignment);
        // FileAlignment
        write_u32_at(&mut pe, opt + 36, file_alignment);

        // MajorOperatingSystemVersion
        write_u16_at(&mut pe, opt + 40, 6);
        // MinorOperatingSystemVersion
        write_u16_at(&mut pe, opt + 42, 0);
        // MajorSubsystemVersion
        write_u16_at(&mut pe, opt + 48, 6);

        // SizeOfImage
        write_u32_at(&mut pe, opt + 56, size_of_image);
        // SizeOfHeaders
        write_u32_at(&mut pe, opt + 60, headers_size);
        // Subsystem = WINDOWS_CUI (3)
        write_u16_at(&mut pe, opt + 68, 3);
        // DllCharacteristics
        write_u16_at(&mut pe, opt + 70, 0x8160); // NX | DYNAMIC_BASE | HIGH_ENTROPY_VA | TERMINAL_SERVER_AWARE

        // SizeOfStackReserve (8 bytes at offset 72)
        write_u64_at(&mut pe, opt + 72, 0x100000);
        // SizeOfStackCommit
        write_u64_at(&mut pe, opt + 80, 0x1000);
        // SizeOfHeapReserve
        write_u64_at(&mut pe, opt + 88, 0x100000);
        // SizeOfHeapCommit
        write_u64_at(&mut pe, opt + 96, 0x1000);

        // NumberOfRvaAndSizes
        write_u32_at(&mut pe, opt + 108, 16);

        // DataDirectories: 16 * 8 = 128 bytes (all zeros for now)
        // offset 112..240

        // ── Section Table at 0x188 ──
        let sec = 0x188;
        // .text section
        pe[sec..sec + 5].copy_from_slice(b".text");
        write_u32_at(&mut pe, sec + 8, 0x10); // VirtualSize
        write_u32_at(&mut pe, sec + 12, text_va); // VirtualAddress
        write_u32_at(&mut pe, sec + 16, text_raw_size); // SizeOfRawData
        write_u32_at(&mut pe, sec + 20, headers_size); // PointerToRawData
        write_u32_at(&mut pe, sec + 36, 0x6000_0020); // CODE | EXECUTE | READ

        // .text section data: just a "ret" instruction
        pe[headers_size as usize] = 0xC3;

        pe
    }

    #[test]
    fn test_create_test_pe_valid() {
        let pe = create_test_pe();
        let parsed = goblin::pe::PE::parse(&pe).expect("Test PE should be valid");
        assert_eq!(parsed.header.coff_header.number_of_sections, 1);
        assert!(parsed.header.optional_header.is_some());
    }

    #[test]
    fn test_rich_header_injection() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.rich_header".to_string(),
            params: [("donor".to_string(), "notepad".to_string())]
                .into_iter()
                .collect(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.rich_header".to_string()));

        // Verify "Rich" marker exists in the result
        let rich_pos = result
            .windows(4)
            .position(|w| w == b"Rich")
            .expect("Rich marker should be present");

        // Rich marker should be before PE signature (at e_lfanew)
        let e_lfanew = u32::from_le_bytes(result[0x3C..0x40].try_into().unwrap()) as usize;
        assert!(rich_pos < e_lfanew);

        // Result should still be a valid PE
        goblin::pe::PE::parse(&result).expect("PE should remain valid after Rich header");
    }

    #[test]
    fn test_rich_header_calc_profile() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.rich_header".to_string(),
            params: [("donor".to_string(), "calc".to_string())]
                .into_iter()
                .collect(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert_eq!(applied.len(), 1);

        // Verify checksum matches calc profile
        let rich_pos = result.windows(4).position(|w| w == b"Rich").unwrap();
        let checksum = u32::from_le_bytes(result[rich_pos + 4..rich_pos + 8].try_into().unwrap());
        assert_eq!(checksum, binary_data::PROFILE_CALC.checksum);
    }

    #[test]
    fn test_import_pad() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.import_pad".to_string(),
            params: [("count".to_string(), "10".to_string())]
                .into_iter()
                .collect(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.import_pad".to_string()));

        // Parse result and check imports
        let parsed = goblin::pe::PE::parse(&result).expect("PE should be valid after import pad");

        // Should have new imports
        assert!(
            !parsed.imports.is_empty(),
            "Should have imports after padding"
        );

        // Check that at least one of our benign DLLs is present
        let dll_names: Vec<String> = parsed
            .imports
            .iter()
            .map(|imp| imp.dll.to_lowercase())
            .collect();
        let has_benign = dll_names
            .iter()
            .any(|d| d == "user32.dll" || d == "advapi32.dll" || d == "shell32.dll");
        assert!(
            has_benign,
            "Should contain benign DLL imports, got: {:?}",
            dll_names
        );
    }

    #[test]
    fn test_resource_inject() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.resource_inject".to_string(),
            params: HashMap::new(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.resource_inject".to_string()));

        // Result should be valid PE
        let parsed =
            goblin::pe::PE::parse(&result).expect("PE should be valid after resource inject");

        // Should have 2 sections now (.text + .rsrc)
        assert_eq!(parsed.header.coff_header.number_of_sections, 2);

        // Check that .rsrc section exists
        let has_rsrc = parsed
            .sections
            .iter()
            .any(|s| String::from_utf8_lossy(&s.name).starts_with(".rsrc"));
        assert!(has_rsrc, "Should have .rsrc section");

        // Verify DataDirectory[2] (Resource) is set
        if let Some(opt) = parsed.header.optional_header {
            let res_dir = opt
                .data_directories
                .get_resource_table()
                .expect("Should have resource data directory");
            assert!(res_dir.virtual_address > 0);
            assert!(res_dir.size > 0);
        }
    }

    #[test]
    fn test_all_three_transforms() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe);

        let specs = vec![
            MutationSpec {
                id: "binary.rich_header".to_string(),
                params: HashMap::new(),
            },
            MutationSpec {
                id: "binary.import_pad".to_string(),
                params: [("count".to_string(), "5".to_string())]
                    .into_iter()
                    .collect(),
            },
            MutationSpec {
                id: "binary.resource_inject".to_string(),
                params: HashMap::new(),
            },
        ];
        let spec_refs: Vec<&MutationSpec> = specs.iter().collect();

        let (result, applied) = mutator.apply(&spec_refs).unwrap();
        assert_eq!(applied.len(), 3);

        // Validate final PE
        let parsed =
            goblin::pe::PE::parse(&result).expect("PE should be valid after all transforms");

        // 3 sections: .text + .idata + .rsrc
        assert_eq!(parsed.header.coff_header.number_of_sections, 3);

        // Rich header present
        assert!(result.windows(4).any(|w| w == b"Rich"));

        // Has imports
        assert!(!parsed.imports.is_empty());
    }

    #[test]
    fn test_unknown_mutation_skipped() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe.clone());

        let spec = MutationSpec {
            id: "binary.unknown_thing".to_string(),
            params: HashMap::new(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.is_empty());
        // PE should be unchanged (except zeroed checksum)
        assert_eq!(result.len(), pe.len());
    }

    #[test]
    fn test_invalid_pe_rejected() {
        let mutator = BinaryMutator::new(vec![0; 100]);

        let spec = MutationSpec {
            id: "binary.rich_header".to_string(),
            params: HashMap::new(),
        };

        let result = mutator.apply(&[&spec]);
        assert!(result.is_err());
    }

    /// Build a test PE with a non-standard code section name for section_rename tests
    fn create_test_pe_with_custom_section() -> Vec<u8> {
        let mut pe = create_test_pe();
        // Overwrite the .text section name with ".foo\0\0\0\0"
        let sec = 0x188;
        pe[sec..sec + 8].copy_from_slice(b".foo\0\0\0\0");
        pe
    }

    /// Build a test PE with large headers (room for many section entries)
    ///
    /// SizeOfHeaders = 0x600, which gives room for (0x600 - 0x188) / 40 = ~28 sections.
    fn create_test_pe_large_headers() -> Vec<u8> {
        let file_alignment: u32 = 0x200;
        let section_alignment: u32 = 0x1000;
        let headers_size: u32 = 0x600; // 1536 bytes — room for many section headers
        let text_va: u32 = 0x1000;
        let text_raw_size: u32 = 0x200;
        let size_of_image: u32 = 0x2000;

        let mut pe = vec![0u8; (headers_size + text_raw_size) as usize];

        pe[0] = b'M';
        pe[1] = b'Z';
        write_u32_at(&mut pe, 0x3C, 0x80);

        pe[0x80] = b'P';
        pe[0x81] = b'E';

        write_u16_at(&mut pe, 0x84, 0x8664);
        write_u16_at(&mut pe, 0x86, 1);
        write_u32_at(&mut pe, 0x88, 0x6789_ABCD);
        write_u16_at(&mut pe, 0x94, 0xF0);
        write_u16_at(&mut pe, 0x96, 0x0022);

        let opt = 0x98;
        write_u16_at(&mut pe, opt, 0x020B);
        pe[opt + 2] = 14;
        write_u32_at(&mut pe, opt + 4, text_raw_size);
        write_u32_at(&mut pe, opt + 16, text_va);
        write_u64_at(&mut pe, opt + 24, 0x0000_0001_4000_0000);
        write_u32_at(&mut pe, opt + 32, section_alignment);
        write_u32_at(&mut pe, opt + 36, file_alignment);
        write_u16_at(&mut pe, opt + 40, 6);
        write_u16_at(&mut pe, opt + 42, 0);
        write_u16_at(&mut pe, opt + 48, 6);
        write_u32_at(&mut pe, opt + 56, size_of_image);
        write_u32_at(&mut pe, opt + 60, headers_size);
        write_u16_at(&mut pe, opt + 68, 3);
        write_u16_at(&mut pe, opt + 70, 0x8160);
        write_u64_at(&mut pe, opt + 72, 0x100000);
        write_u64_at(&mut pe, opt + 80, 0x1000);
        write_u64_at(&mut pe, opt + 88, 0x100000);
        write_u64_at(&mut pe, opt + 96, 0x1000);
        write_u32_at(&mut pe, opt + 108, 16);

        let sec = 0x188;
        pe[sec..sec + 8].copy_from_slice(b".foo\0\0\0\0");
        write_u32_at(&mut pe, sec + 8, 0x10);
        write_u32_at(&mut pe, sec + 12, text_va);
        write_u32_at(&mut pe, sec + 16, text_raw_size);
        write_u32_at(&mut pe, sec + 20, headers_size);
        write_u32_at(&mut pe, sec + 36, 0x6000_0020); // CODE | EXECUTE | READ

        pe[headers_size as usize] = 0xC3;

        pe
    }

    #[test]
    fn test_section_rename() {
        let pe = create_test_pe_with_custom_section();
        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.section_rename".to_string(),
            params: HashMap::new(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.section_rename".to_string()));

        // Section should be renamed from .foo to .text (CODE | EXECUTE | READ)
        let parsed =
            goblin::pe::PE::parse(&result).expect("PE should be valid after section rename");
        let name = &parsed.sections[0].name;
        assert_eq!(
            name, b".text\0\0\0",
            "Code section should be renamed to .text"
        );
    }

    #[test]
    fn test_section_rename_standard_unchanged() {
        // .text is already standard — should not be renamed
        let pe = create_test_pe();
        let original_name = {
            let parsed = goblin::pe::PE::parse(&pe).unwrap();
            parsed.sections[0].name
        };

        let mutator = BinaryMutator::new(pe);
        let spec = MutationSpec {
            id: "binary.section_rename".to_string(),
            params: HashMap::new(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.section_rename".to_string()));

        let parsed = goblin::pe::PE::parse(&result).unwrap();
        assert_eq!(parsed.sections[0].name, original_name);
    }

    #[test]
    fn test_entropy_normalize() {
        // Fill .text section with high-entropy data so the transform actually triggers
        let mut pe = create_test_pe();
        let text_start = 0x200usize; // PointerToRawData for .text
        for i in text_start..pe.len() {
            pe[i] = ((i * 137 + 97) % 251) as u8; // pseudo-random, high entropy
        }
        let initial_entropy = compute_entropy(&pe);
        assert!(
            initial_entropy > 5.0,
            "Setup: PE should have high entropy, got {:.2}",
            initial_entropy
        );

        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.entropy_normalize".to_string(),
            params: [("target".to_string(), "4.0".to_string())]
                .into_iter()
                .collect(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.entropy_normalize".to_string()));

        // Verify new section was added
        let parsed =
            goblin::pe::PE::parse(&result).expect("PE should be valid after entropy normalize");
        assert!(parsed.header.coff_header.number_of_sections >= 2);

        // Entropy should have decreased
        let new_entropy = compute_entropy(&result);
        assert!(
            new_entropy < initial_entropy,
            "Entropy should decrease: {:.2} → {:.2}",
            initial_entropy,
            new_entropy
        );
    }

    #[test]
    fn test_string_inject() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.string_inject".to_string(),
            params: [("count".to_string(), "5".to_string())]
                .into_iter()
                .collect(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.string_inject".to_string()));

        // Verify PE is valid
        let parsed =
            goblin::pe::PE::parse(&result).expect("PE should be valid after string inject");
        assert_eq!(parsed.header.coff_header.number_of_sections, 2);

        // Verify at least 5 benign strings are present in the result
        let mut found = 0;
        for s in binary_data::BENIGN_STRINGS.iter().take(5) {
            if result.windows(s.len()).any(|w| w == s.as_bytes()) {
                found += 1;
            }
        }
        assert!(found >= 5, "Expected 5 benign strings, found {}", found);
    }

    #[test]
    fn test_size_pad() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.size_pad".to_string(),
            params: [("target_kb".to_string(), "4".to_string())]
                .into_iter()
                .collect(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.size_pad".to_string()));

        // File should be at least 4KB
        assert!(
            result.len() >= 4096,
            "Expected >= 4096 bytes, got {}",
            result.len()
        );

        // Verify PE is valid
        goblin::pe::PE::parse(&result).expect("PE should be valid after size pad");
    }

    #[test]
    fn test_debug_dir() {
        let pe = create_test_pe();
        let mutator = BinaryMutator::new(pe);

        let spec = MutationSpec {
            id: "binary.debug_dir".to_string(),
            params: HashMap::new(),
        };

        let (result, applied) = mutator.apply(&[&spec]).unwrap();
        assert!(applied.contains(&"binary.debug_dir".to_string()));

        // Verify PE is valid
        let parsed =
            goblin::pe::PE::parse(&result).expect("PE should be valid after debug dir inject");

        // DataDirectory[6] (Debug) should be set
        if let Some(opt) = parsed.header.optional_header {
            let debug_dir = opt
                .data_directories
                .get_debug_table()
                .expect("Should have debug data directory");
            assert!(debug_dir.virtual_address > 0, "Debug VA should be non-zero");
            assert_eq!(debug_dir.size, 28, "Debug dir size should be 28 bytes");
        }

        // "RSDS" marker should be present
        assert!(
            result.windows(4).any(|w| w == b"RSDS"),
            "RSDS CodeView signature should be present"
        );

        // Default PDB path should be present
        assert!(
            result
                .windows(binary_data::DEFAULT_PDB_PATH.len())
                .any(|w| w == binary_data::DEFAULT_PDB_PATH.as_bytes()),
            "Default PDB path should be present"
        );
    }

    #[test]
    fn test_all_eight_transforms() {
        let pe = create_test_pe_large_headers();
        let mutator = BinaryMutator::new(pe);

        let specs = vec![
            MutationSpec {
                id: "binary.rich_header".to_string(),
                params: HashMap::new(),
            },
            MutationSpec {
                id: "binary.import_pad".to_string(),
                params: [("count".to_string(), "5".to_string())]
                    .into_iter()
                    .collect(),
            },
            MutationSpec {
                id: "binary.resource_inject".to_string(),
                params: HashMap::new(),
            },
            MutationSpec {
                id: "binary.section_rename".to_string(),
                params: HashMap::new(),
            },
            MutationSpec {
                id: "binary.entropy_normalize".to_string(),
                params: [("target".to_string(), "5.0".to_string())]
                    .into_iter()
                    .collect(),
            },
            MutationSpec {
                id: "binary.string_inject".to_string(),
                params: [("count".to_string(), "10".to_string())]
                    .into_iter()
                    .collect(),
            },
            MutationSpec {
                id: "binary.size_pad".to_string(),
                params: [("target_kb".to_string(), "8".to_string())]
                    .into_iter()
                    .collect(),
            },
            MutationSpec {
                id: "binary.debug_dir".to_string(),
                params: HashMap::new(),
            },
        ];
        let spec_refs: Vec<&MutationSpec> = specs.iter().collect();

        let (result, applied) = mutator.apply(&spec_refs).unwrap();
        assert_eq!(applied.len(), 8, "All 8 transforms should apply");

        // Validate final PE
        let parsed =
            goblin::pe::PE::parse(&result).expect("PE should be valid after all 8 transforms");

        // Should have multiple sections
        assert!(
            parsed.header.coff_header.number_of_sections >= 6,
            "Expected >= 6 sections, got {}",
            parsed.header.coff_header.number_of_sections
        );

        // Rich header present
        assert!(result.windows(4).any(|w| w == b"Rich"));

        // Has imports
        assert!(!parsed.imports.is_empty());

        // RSDS present
        assert!(result.windows(4).any(|w| w == b"RSDS"));

        // File size >= 8KB
        assert!(
            result.len() >= 8192,
            "Expected >= 8192 bytes, got {}",
            result.len()
        );
    }
}
