//! Payload Encoder
//!
//! Encodes raw shellcode/payloads into C header format compatible with decoder modules.
//! Supports multiple encoding schemes:
//!   - None: Raw bytes, no encoding (direct copy)
//!   - XOR: Rolling 2-byte XOR key
//!   - English: Dictionary-based word mapping (low entropy)

use anyhow::{Result, bail};
use std::collections::HashMap;

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
}

impl std::str::FromStr for EncodingType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "xor" => Ok(Self::Xor),
            "english" => Ok(Self::English),
            "none" => Ok(Self::None),
            other => bail!(
                "Unknown encoding type: '{}'. Valid: xor, english, none",
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
        }
    }
}

/// Payload encoder that generates C header code
pub struct PayloadEncoder {
    /// XOR key (for XOR encoding)
    xor_key: [u8; 2],
    /// Dictionary words (for English encoding)
    dictionary: Vec<String>,
}

impl PayloadEncoder {
    /// Create a new encoder with random XOR key
    pub fn new() -> Self {
        // Use a simple deterministic seed for reproducibility
        // In production, you'd want actual randomness
        Self {
            xor_key: [0xAA, 0x55],
            dictionary: Self::generate_dictionary(),
        }
    }

    /// Create encoder with specific XOR key
    pub fn with_xor_key(key: [u8; 2]) -> Self {
        Self {
            xor_key: key,
            dictionary: Self::generate_dictionary(),
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

    /// Encode payload using specified encoding type
    pub fn encode(&self, payload: &[u8], encoding: EncodingType) -> EncodedPayload {
        match encoding {
            EncodingType::Xor => self.encode_xor(payload),
            EncodingType::English => self.encode_english(payload),
            EncodingType::None => Self::encode_none(payload),
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

    /// Generate C header code for the encoded payload
    pub fn generate_c_header(&self, encoded: &EncodedPayload) -> String {
        match encoded.encoding {
            EncodingType::Xor => self.generate_xor_header(encoded),
            EncodingType::English => self.generate_english_header(encoded),
            EncodingType::None => Self::generate_none_header(encoded),
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

/// Generate a dummy/test payload (NOPs + INT3)
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
}
