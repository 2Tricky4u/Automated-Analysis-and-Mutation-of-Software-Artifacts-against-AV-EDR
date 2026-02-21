//! Embedded donor data for binary PE transforms
//!
//! Contains pre-built data structures used by `BinaryMutator`:
//! - Rich header donor profiles (MSVC compiler metadata)
//! - Benign import pool (DLL names + function names)
//! - Application manifest template
//! - Version info helpers

// ============================================================================
// Rich Header Donor Profiles
// ============================================================================

/// A single Rich header compiler/tool record
pub struct RichRecord {
    /// Product ID (tool type): linker, C compiler, MASM, etc.
    pub product_id: u16,
    /// Minor build number of the tool
    pub build_number: u16,
    /// Number of objects/modules compiled with this tool
    pub count: u32,
}

/// Complete Rich header donor profile
pub struct RichProfile {
    pub name: &'static str,
    /// XOR checksum for encoding (arbitrary but fixed per profile)
    pub checksum: u32,
    /// Compiler/linker records (decoded, before XOR)
    pub records: &'static [RichRecord],
}

/// MSVC 2022 v17.8 — small console application (like notepad.exe)
pub static PROFILE_NOTEPAD: RichProfile = RichProfile {
    name: "notepad",
    checksum: 0x7B3D_1A2E,
    records: &[
        RichRecord {
            product_id: 0x0001,
            build_number: 0,
            count: 132,
        }, // Import0 (linker objects)
        RichRecord {
            product_id: 0x0104,
            build_number: 30704,
            count: 1,
        }, // Linker14 (link.exe)
        RichRecord {
            product_id: 0x0093,
            build_number: 30704,
            count: 23,
        }, // Utc1900_C (cl.exe x64)
        RichRecord {
            product_id: 0x0105,
            build_number: 30704,
            count: 1,
        }, // Masm1400 (ml64.exe)
        RichRecord {
            product_id: 0x00E3,
            build_number: 30704,
            count: 1,
        }, // Implib1400
    ],
};

/// MSVC 2019 v16.11 — medium application (like calc.exe)
pub static PROFILE_CALC: RichProfile = RichProfile {
    name: "calc",
    checksum: 0x4A5C_2B3D,
    records: &[
        RichRecord {
            product_id: 0x0001,
            build_number: 0,
            count: 96,
        },
        RichRecord {
            product_id: 0x0102,
            build_number: 29395,
            count: 1,
        }, // Linker14 (older)
        RichRecord {
            product_id: 0x0093,
            build_number: 29395,
            count: 15,
        },
        RichRecord {
            product_id: 0x0103,
            build_number: 29395,
            count: 1,
        }, // Masm (older)
        RichRecord {
            product_id: 0x00E1,
            build_number: 29395,
            count: 1,
        }, // Implib (older)
    ],
};

/// MSVC 2022 v17.4 — large C++ application (like explorer.exe)
pub static PROFILE_EXPLORER: RichProfile = RichProfile {
    name: "explorer",
    checksum: 0x5D6E_3C4F,
    records: &[
        RichRecord {
            product_id: 0x0001,
            build_number: 0,
            count: 248,
        },
        RichRecord {
            product_id: 0x0104,
            build_number: 30249,
            count: 1,
        },
        RichRecord {
            product_id: 0x0093,
            build_number: 30249,
            count: 89,
        }, // C x64
        RichRecord {
            product_id: 0x0095,
            build_number: 30249,
            count: 42,
        }, // C++ x64
        RichRecord {
            product_id: 0x0105,
            build_number: 30249,
            count: 3,
        },
        RichRecord {
            product_id: 0x00E3,
            build_number: 30249,
            count: 2,
        },
        RichRecord {
            product_id: 0x00FF,
            build_number: 0,
            count: 1,
        }, // Export0
    ],
};

/// Look up a Rich header donor profile by name
pub fn get_rich_profile(name: &str) -> &'static RichProfile {
    match name {
        "calc" => &PROFILE_CALC,
        "explorer" => &PROFILE_EXPLORER,
        _ => &PROFILE_NOTEPAD, // default
    }
}

/// Encode a Rich header from records + checksum into raw bytes
///
/// Format (all little-endian):
/// ```text
/// XOR(DanS, cs) | XOR(0,cs) | XOR(0,cs) | XOR(0,cs)  ← 16-byte header
/// XOR(comp_id, cs) | XOR(count, cs)                    ← 8 bytes per record
/// ...
/// "Rich"          | cs                                  ← 8-byte footer (plaintext)
/// ```
pub fn encode_rich_header(records: &[RichRecord], checksum: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity(16 + records.len() * 8 + 8);

    // DanS signature XOR checksum
    let dans: u32 = 0x536E_6144; // "DanS" as LE u32
    data.extend_from_slice(&(dans ^ checksum).to_le_bytes());

    // 3 padding DWORDs (zeros XOR checksum = checksum)
    for _ in 0..3 {
        data.extend_from_slice(&checksum.to_le_bytes());
    }

    // Records
    for rec in records {
        let comp_id = ((rec.product_id as u32) << 16) | (rec.build_number as u32);
        data.extend_from_slice(&(comp_id ^ checksum).to_le_bytes());
        data.extend_from_slice(&(rec.count ^ checksum).to_le_bytes());
    }

    // "Rich" marker (plaintext)
    data.extend_from_slice(b"Rich");
    // Checksum (plaintext)
    data.extend_from_slice(&checksum.to_le_bytes());

    data
}

// ============================================================================
// Benign Import Pool
// ============================================================================

/// A DLL and its benign exported functions
pub struct BenignImport {
    pub dll: &'static str,
    pub functions: &'static [&'static str],
}

pub static BENIGN_IMPORTS: &[BenignImport] = &[
    // UI / Window management
    BenignImport {
        dll: "user32.dll",
        functions: &[
            "GetSystemMetrics",
            "GetDesktopWindow",
            "GetWindowTextW",
            "MessageBoxA",
        ],
    },
    // Registry / Security
    BenignImport {
        dll: "advapi32.dll",
        functions: &[
            "RegOpenKeyExW",
            "RegQueryValueExW",
            "GetUserNameW",
            "OpenProcessToken",
        ],
    },
    // Shell
    BenignImport {
        dll: "shell32.dll",
        functions: &[
            "SHGetFolderPathW",
            "ShellExecuteW",
            "SHGetSpecialFolderPathW",
        ],
    },
    // COM
    BenignImport {
        dll: "ole32.dll",
        functions: &["CoInitializeEx", "CoCreateInstance", "CoUninitialize"],
    },
    // Graphics
    BenignImport {
        dll: "gdi32.dll",
        functions: &["GetDeviceCaps", "CreateCompatibleDC", "DeleteDC"],
    },
    // File version queries
    BenignImport {
        dll: "version.dll",
        functions: &["GetFileVersionInfoW", "VerQueryValueW"],
    },
    // Networking (normal for any app that talks to the internet)
    BenignImport {
        dll: "winhttp.dll",
        functions: &[
            "WinHttpOpen",
            "WinHttpConnect",
            "WinHttpCloseHandle",
        ],
    },
    BenignImport {
        dll: "ws2_32.dll",
        functions: &[
            "WSAStartup",
            "WSACleanup",
            "getaddrinfo",
            "freeaddrinfo",
        ],
    },
    // Crypto (normal for apps handling config/updates/TLS)
    BenignImport {
        dll: "bcrypt.dll",
        functions: &[
            "BCryptOpenAlgorithmProvider",
            "BCryptCloseAlgorithmProvider",
            "BCryptGenRandom",
        ],
    },
    BenignImport {
        dll: "crypt32.dll",
        functions: &[
            "CertOpenStore",
            "CertCloseStore",
            "CertFreeCertificateContext",
        ],
    },
    // Path / string utilities
    BenignImport {
        dll: "shlwapi.dll",
        functions: &["PathFileExistsW", "PathCombineW", "StrCmpIW"],
    },
    // Common controls
    BenignImport {
        dll: "comctl32.dll",
        functions: &["InitCommonControlsEx", "ImageList_Create"],
    },
    // C runtime (common in MSVC-compiled apps)
    BenignImport {
        dll: "msvcrt.dll",
        functions: &["malloc", "free", "memcpy", "sprintf"],
    },
    // Security / identity
    BenignImport {
        dll: "secur32.dll",
        functions: &["GetUserNameExW", "LsaEnumerateLogonSessions"],
    },
    // Terminal services
    BenignImport {
        dll: "wtsapi32.dll",
        functions: &[
            "WTSQuerySessionInformationW",
            "WTSFreeMemory",
        ],
    },
];

// ============================================================================
// Application Manifest
// ============================================================================

pub static MANIFEST_TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="1.0.0.0" processorArchitecture="amd64" name="{COMPANY}.{PRODUCT}" type="win32"/>
  <description>{DESCRIPTION}</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
      <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/>
      <supportedOS Id="{e2011457-1546-43c5-a5fe-008deee3d3f0}"/>
    </application>
  </compatibility>
</assembly>"#;

/// Build a manifest XML string with the given product and company name
pub fn build_manifest(product_name: &str, company_name: &str) -> String {
    MANIFEST_TEMPLATE
        .replace("{COMPANY}", &company_name.replace(' ', ""))
        .replace("{PRODUCT}", &product_name.replace(' ', ""))
        .replace("{DESCRIPTION}", product_name)
}

// ============================================================================
// VS_VERSIONINFO Builder
// ============================================================================

/// Build a complete VS_VERSIONINFO resource structure
///
/// Returns raw bytes ready to embed in a .rsrc section
pub fn build_version_info(product_name: &str, company_name: &str, file_version: &str) -> Vec<u8> {
    let mut buf = Vec::new();

    // We'll build the structure bottom-up, then assemble

    // ── VS_FIXEDFILEINFO (52 bytes) ──
    let mut fixed = Vec::with_capacity(52);
    fixed.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes()); // dwSignature
    fixed.extend_from_slice(&0x0001_0000u32.to_le_bytes()); // dwStrucVersion
    fixed.extend_from_slice(&0x0001_0000u32.to_le_bytes()); // dwFileVersionMS (1.0)
    fixed.extend_from_slice(&0x0000_0001u32.to_le_bytes()); // dwFileVersionLS (0.1)
    fixed.extend_from_slice(&0x0001_0000u32.to_le_bytes()); // dwProductVersionMS
    fixed.extend_from_slice(&0x0000_0001u32.to_le_bytes()); // dwProductVersionLS
    fixed.extend_from_slice(&0x0000_003Fu32.to_le_bytes()); // dwFileFlagsMask
    fixed.extend_from_slice(&0x0000_0000u32.to_le_bytes()); // dwFileFlags
    fixed.extend_from_slice(&0x0004_0004u32.to_le_bytes()); // dwFileOS (VOS_NT_WINDOWS32)
    fixed.extend_from_slice(&0x0000_0001u32.to_le_bytes()); // dwFileType (VFT_APP)
    fixed.extend_from_slice(&0x0000_0000u32.to_le_bytes()); // dwFileSubtype
    fixed.extend_from_slice(&0x0000_0000u32.to_le_bytes()); // dwFileDateMS
    fixed.extend_from_slice(&0x0000_0000u32.to_le_bytes()); // dwFileDateLS

    // ── String entries for StringTable ──
    let string_pairs = [
        ("CompanyName", company_name),
        ("FileDescription", product_name),
        ("FileVersion", file_version),
        ("InternalName", product_name),
        (
            "LegalCopyright",
            &format!("Copyright (C) {} 2024", company_name),
        ),
        ("OriginalFilename", "app.exe"),
        ("ProductName", product_name),
        ("ProductVersion", file_version),
    ];

    let mut string_entries = Vec::new();
    for (key, value) in &string_pairs {
        let entry = build_version_string(key, value);
        string_entries.extend_from_slice(&entry);
    }

    // ── StringTable (wraps string entries) ──
    let string_table_key = utf16_bytes("040904B0"); // English US, Unicode
    let string_table_header_size = 6 + string_table_key.len(); // wLength + wValueLength + wType + szKey
    let string_table_size = align_up_usize(string_table_header_size, 4) + string_entries.len();

    let mut string_table = Vec::new();
    string_table.extend_from_slice(&(string_table_size as u16).to_le_bytes()); // wLength
    string_table.extend_from_slice(&0u16.to_le_bytes()); // wValueLength
    string_table.extend_from_slice(&1u16.to_le_bytes()); // wType (text)
    string_table.extend_from_slice(&string_table_key);
    pad_to_dword(&mut string_table);
    string_table.extend_from_slice(&string_entries);

    // ── StringFileInfo (wraps StringTable) ──
    let sfi_key = utf16_bytes("StringFileInfo");
    let sfi_header_size = 6 + sfi_key.len();
    let sfi_size = align_up_usize(sfi_header_size, 4) + string_table.len();

    let mut sfi = Vec::new();
    sfi.extend_from_slice(&(sfi_size as u16).to_le_bytes()); // wLength
    sfi.extend_from_slice(&0u16.to_le_bytes()); // wValueLength
    sfi.extend_from_slice(&1u16.to_le_bytes()); // wType (text)
    sfi.extend_from_slice(&sfi_key);
    pad_to_dword(&mut sfi);
    sfi.extend_from_slice(&string_table);

    // ── VarFileInfo → Var "Translation" ──
    let translation_key = utf16_bytes("Translation");
    let translation_value: u32 = 0x04B0_0409; // langID=0x0409 (English US), codepage=0x04B0 (Unicode)
    let var_header_size = 6 + translation_key.len();
    let var_size = align_up_usize(var_header_size, 4) + 4;

    let mut var = Vec::new();
    var.extend_from_slice(&(var_size as u16).to_le_bytes()); // wLength
    var.extend_from_slice(&4u16.to_le_bytes()); // wValueLength (4 bytes)
    var.extend_from_slice(&0u16.to_le_bytes()); // wType (binary)
    var.extend_from_slice(&translation_key);
    pad_to_dword(&mut var);
    var.extend_from_slice(&translation_value.to_le_bytes());

    let vfi_key = utf16_bytes("VarFileInfo");
    let vfi_header_size = 6 + vfi_key.len();
    let vfi_size = align_up_usize(vfi_header_size, 4) + var.len();

    let mut vfi = Vec::new();
    vfi.extend_from_slice(&(vfi_size as u16).to_le_bytes());
    vfi.extend_from_slice(&0u16.to_le_bytes());
    vfi.extend_from_slice(&1u16.to_le_bytes()); // wType (text)
    vfi.extend_from_slice(&vfi_key);
    pad_to_dword(&mut vfi);
    vfi.extend_from_slice(&var);

    // ── Top-level VS_VERSIONINFO ──
    let vs_key = utf16_bytes("VS_VERSION_INFO");
    let vs_header_size = 6 + vs_key.len(); // wLength + wValueLength + wType + szKey
    let after_key_aligned = align_up_usize(vs_header_size, 4);
    let after_fixed_aligned = align_up_usize(after_key_aligned + fixed.len(), 4);
    let vs_total = after_fixed_aligned + sfi.len() + vfi.len();

    // wLength
    buf.extend_from_slice(&(vs_total as u16).to_le_bytes());
    // wValueLength (size of VS_FIXEDFILEINFO = 52)
    buf.extend_from_slice(&(fixed.len() as u16).to_le_bytes());
    // wType (0 = binary)
    buf.extend_from_slice(&0u16.to_le_bytes());
    // szKey
    buf.extend_from_slice(&vs_key);
    // Padding1
    pad_to_dword(&mut buf);
    // VS_FIXEDFILEINFO
    buf.extend_from_slice(&fixed);
    // Padding2
    pad_to_dword(&mut buf);
    // Children
    buf.extend_from_slice(&sfi);
    buf.extend_from_slice(&vfi);

    buf
}

/// Build a single String entry for VS_VERSIONINFO StringTable
fn build_version_string(key: &str, value: &str) -> Vec<u8> {
    let key_bytes = utf16_bytes(key);
    let value_bytes = utf16_bytes(value);
    let value_chars = value.encode_utf16().count() + 1; // chars including null

    let header_size = 6 + key_bytes.len();
    let after_key_aligned = align_up_usize(header_size, 4);
    let total = after_key_aligned + value_bytes.len();

    let mut entry = Vec::new();
    entry.extend_from_slice(&(total as u16).to_le_bytes()); // wLength
    entry.extend_from_slice(&(value_chars as u16).to_le_bytes()); // wValueLength (in WCHAR)
    entry.extend_from_slice(&1u16.to_le_bytes()); // wType (text)
    entry.extend_from_slice(&key_bytes);
    pad_to_dword(&mut entry);
    entry.extend_from_slice(&value_bytes);
    pad_to_dword(&mut entry);

    entry
}

/// Convert a &str to UTF-16LE bytes with null terminator
fn utf16_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(std::iter::once(0u16))
        .flat_map(|w| w.to_le_bytes())
        .collect()
}

/// Pad a Vec<u8> to the next DWORD (4-byte) boundary
fn pad_to_dword(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

fn align_up_usize(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

// ============================================================================
// MSVC Standard Section Names
// ============================================================================

/// Standard MSVC section names — don't rename these
pub static MSVC_STANDARD_SECTIONS: &[&[u8; 8]] = &[
    b".text\0\0\0",
    b".data\0\0\0",
    b".rdata\0\0",
    b".bss\0\0\0\0",
    b".rsrc\0\0\0",
    b".reloc\0\0",
    b".pdata\0\0",
    b".xdata\0\0",
    b".idata\0\0",
    b".edata\0\0",
    b".tls\0\0\0\0",
    b".CRT\0\0\0\0",
];

// ============================================================================
// Debug Directory Defaults
// ============================================================================

/// Default fake PDB path for binary.debug_dir
pub const DEFAULT_PDB_PATH: &str = "C:\\src\\obj\\x64\\Release\\app.pdb";

// ============================================================================
// Resource Inject Defaults
// ============================================================================

/// Default company name for resource_inject — generic, non-impersonating
pub const DEFAULT_COMPANY: &str = "Application Software Inc.";

/// Default product name for resource_inject — bland, nondescript
pub const DEFAULT_PRODUCT: &str = "Application Service";

// ============================================================================
// Benign Strings
// ============================================================================

pub static BENIGN_STRINGS: &[&str] = &[
    "The operation completed successfully.",
    "Access is denied.",
    "The system cannot find the file specified.",
    "Not enough memory resources are available to process this command.",
    "The process cannot access the file because it is being used by another process.",
    "The parameter is incorrect.",
    "The specified path is invalid.",
    "Initializing application components...",
    "Loading configuration settings...",
    "Checking for updates...",
    "Starting background services...",
    "Application started successfully.",
    "Shutting down...",
    "Saving user preferences...",
    "Connecting to network...",
    "Operation timed out. Please try again.",
    "An unexpected error occurred. Please restart the application.",
    "Unable to load the specified resource.",
    "The requested feature is not available in this version.",
    "Please wait while the operation completes...",
    "Verifying system requirements...",
    "Updating registry settings...",
    "Cleaning up temporary files...",
    "Restoring default settings...",
    "Configuration saved successfully.",
    "The file format is not supported.",
    "Network connection lost. Attempting to reconnect...",
    "Insufficient disk space for this operation.",
    "The specified user account has expired.",
    "Windows is checking for a solution to the problem...",
];

// ============================================================================
// Low-Entropy Padding Generator
// ============================================================================

/// Format strings that appear in typical compiled applications
static FORMAT_STRINGS: &[&str] = &[
    "%s: %d\n",
    "%s\\%s",
    "%d.%d.%d.%d",
    "Error: %s (0x%08x)\n",
    "%s=%s\r\n",
    "\\\\?\\%s",
    "%s [%d]\n",
    "(%d, %d)",
];

/// Generate low-entropy padding that looks like real compiled PE data.
///
/// Mixes multiple data types found in real `.rdata`/`.data` sections:
/// - Null-terminated strings (error messages, status text)
/// - Aligned integer sequences (enum/flag tables)
/// - Pointer-sized zero blocks (uninitialized pointer arrays)
/// - Short format strings
/// - 8-byte aligned non-zero values (vtable-like pointers)
///
/// Produces H ≈ 3.5–4.5 bits/byte, resembling natural initialized data.
pub fn generate_low_entropy_padding(size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size);
    let mut string_idx = 0;
    let mut fmt_idx = 0;
    let mut block_type = 0u8;

    while buf.len() < size {
        let remaining = size - buf.len();
        if remaining < 8 {
            buf.extend(std::iter::repeat_n(0u8, remaining));
            break;
        }

        match block_type % 6 {
            0 => {
                // Benign string (null-terminated, like error messages in .rdata)
                let s = BENIGN_STRINGS[string_idx % BENIGN_STRINGS.len()];
                string_idx += 1;
                let s_bytes = s.as_bytes();
                if remaining > s_bytes.len() + 1 {
                    buf.extend_from_slice(s_bytes);
                    buf.push(0);
                    // Align to 4 bytes (like MSVC string pooling)
                    while buf.len() % 4 != 0 && buf.len() < size {
                        buf.push(0);
                    }
                } else {
                    buf.extend(std::iter::repeat_n(0u8, remaining));
                }
            }
            1 => {
                // Pointer-sized zero block (like uninitialized pointer arrays)
                let count = remaining.min(48); // 6 pointers worth
                buf.extend(std::iter::repeat_n(0u8, count));
            }
            2 => {
                // Small aligned integer sequence (like enum tables / flag arrays)
                let count = (remaining / 4).min(8);
                for i in 0..count {
                    buf.extend_from_slice(&(i as u32).to_le_bytes());
                }
            }
            3 => {
                // Short format string
                let s = FORMAT_STRINGS[fmt_idx % FORMAT_STRINGS.len()];
                fmt_idx += 1;
                if remaining > s.len() + 1 {
                    buf.extend_from_slice(s.as_bytes());
                    buf.push(0);
                    while buf.len() % 4 != 0 && buf.len() < size {
                        buf.push(0);
                    }
                } else {
                    buf.extend(std::iter::repeat_n(0u8, remaining));
                }
            }
            4 => {
                // 8-byte aligned non-zero values (like vtable pointers / RVA tables)
                let count = (remaining / 8).min(4);
                for i in 0..count {
                    // Values that look like RVAs: 0x1000-range addresses
                    let fake_rva = 0x0000_0001_4000_1000u64 + (i as u64 * 0x100);
                    buf.extend_from_slice(&fake_rva.to_le_bytes());
                }
            }
            5 => {
                // Another benign string for variety
                let s = BENIGN_STRINGS[(string_idx + 7) % BENIGN_STRINGS.len()];
                string_idx += 1;
                let s_bytes = s.as_bytes();
                if remaining > s_bytes.len() + 1 {
                    buf.extend_from_slice(s_bytes);
                    buf.push(0);
                    while buf.len() % 4 != 0 && buf.len() < size {
                        buf.push(0);
                    }
                } else {
                    buf.extend(std::iter::repeat_n(0u8, remaining));
                }
            }
            _ => unreachable!(),
        }

        block_type = block_type.wrapping_add(1);
    }

    buf.truncate(size);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_rich_header_structure() {
        let records = &PROFILE_NOTEPAD.records[..2]; // Use first 2 records
        let cs = PROFILE_NOTEPAD.checksum;
        let data = encode_rich_header(records, cs);

        // Size: 16 (header) + 2*8 (records) + 8 (footer) = 40 bytes
        assert_eq!(data.len(), 40);

        // "Rich" marker at offset -8 from end (plaintext)
        assert_eq!(&data[data.len() - 8..data.len() - 4], b"Rich");

        // Checksum at end (plaintext LE)
        let stored_cs = u32::from_le_bytes(data[data.len() - 4..].try_into().unwrap());
        assert_eq!(stored_cs, cs);

        // Decode DanS signature
        let first_dword = u32::from_le_bytes(data[0..4].try_into().unwrap());
        assert_eq!(first_dword ^ cs, 0x536E_6144); // "DanS"
    }

    #[test]
    fn test_build_manifest() {
        let xml = build_manifest("System Configuration Utility", "Application Software Inc.");
        assert!(xml.contains("asInvoker"));
        assert!(xml.contains("ApplicationSoftwareInc."));
        assert!(xml.contains("SystemConfigurationUtility"));
        assert!(xml.contains("System Configuration Utility"));
        assert!(!xml.contains("Microsoft.Windows."));
    }

    #[test]
    fn test_build_version_info_not_empty() {
        let vi = build_version_info("Test Application", "Microsoft Corporation", "1.0.0.0");
        // Should be non-trivial size (typically 500+ bytes)
        assert!(vi.len() > 100);

        // First two bytes = wLength (total size)
        let total_len = u16::from_le_bytes(vi[0..2].try_into().unwrap()) as usize;
        assert_eq!(total_len, vi.len());

        // VS_FIXEDFILEINFO signature should be present after the key
        // Search for 0xFEEF04BD
        let sig_bytes = 0xFEEF_04BDu32.to_le_bytes();
        assert!(vi.windows(4).any(|w| w == sig_bytes));
    }

    #[test]
    fn test_utf16_bytes() {
        let bytes = utf16_bytes("AB");
        // 'A'=0x0041 'B'=0x0042 null=0x0000 → 6 bytes
        assert_eq!(bytes, vec![0x41, 0x00, 0x42, 0x00, 0x00, 0x00]);
    }
}
