//! Tests that simulate the C decoder logic in Rust to verify the
//! encode→decode contract between PayloadEncoder (Rust) and decoder modules (C).
//!
//! These tests replicate the exact algorithms from:
//!   - build/templates/modules/decoder/xor.c   → simulate_c_xor_decode()
//!   - build/templates/modules/decoder/english.c → simulate_c_english_decode()

mod common;

use build::{EncodingType, PayloadEncoder};

// ============================================================================
// C decoder simulation helpers
// ============================================================================

/// Replicates xor.c's decode_payload():
///   for(int i = 0; i < len; i++) { dest[i] = supermega_payload[i] ^ XOR_KEY[i % 2]; }
fn simulate_c_xor_decode(encoded_data: &[u8], xor_key: [u8; 2]) -> Vec<u8> {
    encoded_data
        .iter()
        .enumerate()
        .map(|(i, &b)| b ^ xor_key[i % 2])
        .collect()
}

/// Replicates english.c's decode_payload():
///   tokenize supermega_payload_str by spaces, look up each word in DICTIONARY[0..255]
fn simulate_c_english_decode(payload_str: &str, dictionary: &[&str]) -> Vec<u8> {
    payload_str
        .split_whitespace()
        .map(|word| {
            dictionary
                .iter()
                .position(|&d| d == word)
                .expect(&format!("Word '{}' not found in dictionary", word))
                as u8
        })
        .collect()
}

/// The encoder's built-in 256-word dictionary (must match PayloadEncoder::generate_dictionary)
fn encoder_dictionary() -> Vec<String> {
    let common_words = [
        "the", "be", "to", "of", "and", "a", "in", "that", "have", "i", "it", "for", "not",
        "on", "with", "he", "as", "you", "do", "at", "this", "but", "his", "by", "from",
        "they", "we", "say", "her", "she", "or", "an", "will", "my", "one", "all", "would",
        "there", "their", "what", "so", "up", "out", "if", "about", "who", "get", "which",
        "go", "me", "when", "make", "can", "like", "time", "no", "just", "him", "know", "take",
    ];
    let mut dictionary: Vec<String> = common_words.iter().map(|s| s.to_string()).collect();
    while dictionary.len() < 256 {
        dictionary.push(format!("w{}", dictionary.len()));
    }
    dictionary
}

/// Parse PAYLOAD_LEN from a generated C header: `#define PAYLOAD_LEN <N>`
fn parse_payload_len(header: &str) -> usize {
    for line in header.lines() {
        if let Some(rest) = line.strip_prefix("#define PAYLOAD_LEN ") {
            return rest.trim().parse().expect("PAYLOAD_LEN must be a number");
        }
    }
    panic!("PAYLOAD_LEN not found in header");
}

/// Count hex byte literals (0xNN) in the supermega_payload array
fn count_hex_bytes_in_array(header: &str) -> usize {
    // Find the supermega_payload array between { and };
    let start = header
        .find("supermega_payload[PAYLOAD_LEN] = {")
        .or_else(|| header.find("supermega_payload[1] = {"))
        .expect("supermega_payload array not found");
    let after_brace = header[start..].find('{').unwrap() + start + 1;
    let end_brace = header[after_brace..].find("};").unwrap() + after_brace;
    let array_body = &header[after_brace..end_brace];
    array_body.matches("0x").count()
}

// ============================================================================
// XOR decode simulation tests
// ============================================================================

#[test]
fn test_xor_decode_simulation_typical() {
    let payload = common::payload_typical(); // 256 bytes
    let encoder = PayloadEncoder::new(); // key [0xAA, 0x55]
    let encoded = encoder.encode(&payload, EncodingType::Xor);

    let decoded = simulate_c_xor_decode(&encoded.data, [0xAA, 0x55]);
    assert_eq!(
        payload, decoded,
        "C XOR decode simulation must recover original payload"
    );
}

#[test]
fn test_xor_decode_simulation_all_byte_values() {
    let payload = common::payload_sequential(); // 0x00..=0xFF
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::Xor);

    let decoded = simulate_c_xor_decode(&encoded.data, [0xAA, 0x55]);
    assert_eq!(payload, decoded, "All 256 byte values must round-trip");
}

#[test]
fn test_xor_decode_simulation_boundary_keys() {
    let payload = common::payload_small();
    let boundary_keys: &[[u8; 2]] = &[
        [0x00, 0x00], // Identity XOR
        [0xFF, 0xFF], // Full inversion
        [0x00, 0xFF], // Alternating: identity / invert
        [0xFF, 0x00], // Alternating: invert / identity
    ];

    for key in boundary_keys {
        let encoder = PayloadEncoder::with_xor_key(*key);
        let encoded = encoder.encode(&payload, EncodingType::Xor);
        let decoded = simulate_c_xor_decode(&encoded.data, *key);
        assert_eq!(
            payload, decoded,
            "Round-trip failed for key [{:#04X}, {:#04X}]",
            key[0], key[1]
        );
    }
}

#[test]
fn test_xor_decode_simulation_empty() {
    let payload = common::payload_empty();
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::Xor);

    let decoded = simulate_c_xor_decode(&encoded.data, [0xAA, 0x55]);
    assert_eq!(payload, decoded, "Empty payload must round-trip");
}

#[test]
fn test_xor_decode_simulation_single_byte() {
    let payload = common::payload_tiny(); // [0xCC]
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::Xor);

    let decoded = simulate_c_xor_decode(&encoded.data, [0xAA, 0x55]);
    assert_eq!(payload, decoded, "Single byte must round-trip");
}

#[test]
fn test_xor_decode_all_zeros_payload() {
    let payload = common::payload_all_zeros(); // 64 x 0x00
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::Xor);

    // With key [0xAA, 0x55]: encoded should be [0xAA, 0x55, 0xAA, 0x55, ...]
    assert_eq!(encoded.data[0], 0xAA);
    assert_eq!(encoded.data[1], 0x55);

    let decoded = simulate_c_xor_decode(&encoded.data, [0xAA, 0x55]);
    assert_eq!(payload, decoded);
}

#[test]
fn test_xor_decode_all_ff_payload() {
    let payload = common::payload_all_ff(); // 64 x 0xFF
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::Xor);

    // With key [0xAA, 0x55]: encoded should be [0x55, 0xAA, 0x55, 0xAA, ...]
    assert_eq!(encoded.data[0], 0x55);
    assert_eq!(encoded.data[1], 0xAA);

    let decoded = simulate_c_xor_decode(&encoded.data, [0xAA, 0x55]);
    assert_eq!(payload, decoded);
}

// ============================================================================
// English decode simulation tests
// ============================================================================

#[test]
fn test_english_decode_simulation_typical() {
    let payload = common::payload_small(); // [0x41, 0x42, 0x43, 0x44]
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::English);

    let dict = encoder_dictionary();
    let dict_refs: Vec<&str> = dict.iter().map(|s| s.as_str()).collect();
    let payload_str = String::from_utf8(encoded.data).unwrap();

    let decoded = simulate_c_english_decode(&payload_str, &dict_refs);
    assert_eq!(
        payload, decoded,
        "C English decode simulation must recover original payload"
    );
}

#[test]
fn test_english_decode_simulation_all_byte_values() {
    let payload = common::payload_sequential(); // 0x00..=0xFF
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::English);

    let dict = encoder_dictionary();
    let dict_refs: Vec<&str> = dict.iter().map(|s| s.as_str()).collect();
    let payload_str = String::from_utf8(encoded.data).unwrap();

    let decoded = simulate_c_english_decode(&payload_str, &dict_refs);
    assert_eq!(payload, decoded, "All 256 byte values must round-trip via English");
}

#[test]
fn test_english_dictionary_has_256_unique_entries() {
    let dict = encoder_dictionary();
    assert_eq!(dict.len(), 256, "Dictionary must have exactly 256 entries");

    // All entries should be unique (required for unambiguous decoding)
    let mut sorted = dict.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        256,
        "Dictionary entries must be unique for unambiguous decoding"
    );
}

// ============================================================================
// PAYLOAD_LEN consistency tests
// ============================================================================

#[test]
fn test_payload_len_matches_xor_array_count() {
    let payloads: &[(&str, Vec<u8>)] = &[
        ("empty", common::payload_empty()),
        ("tiny", common::payload_tiny()),
        ("small", common::payload_small()),
        ("typical", common::payload_typical()),
        ("large", common::payload_large()),
    ];

    for (name, payload) in payloads {
        let encoder = PayloadEncoder::new();
        let encoded = encoder.encode(payload, EncodingType::Xor);
        let header = encoder.generate_c_header(&encoded);

        let defined_len = parse_payload_len(&header);
        assert_eq!(
            defined_len,
            payload.len(),
            "PAYLOAD_LEN mismatch for '{}': defined={}, actual={}",
            name,
            defined_len,
            payload.len()
        );

        // Also verify the hex array has the right number of bytes
        if !payload.is_empty() {
            let hex_count = count_hex_bytes_in_array(&header);
            assert_eq!(
                hex_count,
                payload.len(),
                "Hex byte count mismatch for '{}': header has {} hex values, expected {}",
                name,
                hex_count,
                payload.len()
            );
        }
    }
}

#[test]
fn test_payload_len_matches_english_word_count() {
    let payloads: &[(&str, Vec<u8>)] = &[
        ("tiny", common::payload_tiny()),
        ("small", common::payload_small()),
        ("typical", common::payload_typical()),
    ];

    for (name, payload) in payloads {
        let encoder = PayloadEncoder::new();
        let encoded = encoder.encode(payload, EncodingType::English);
        let header = encoder.generate_c_header(&encoded);

        let defined_len = parse_payload_len(&header);
        assert_eq!(
            defined_len,
            payload.len(),
            "PAYLOAD_LEN (word count) mismatch for '{}': defined={}, actual={}",
            name,
            defined_len,
            payload.len()
        );
    }
}

#[test]
fn test_xor_header_hex_values_match_encoded_data() {
    let payload = common::payload_small(); // [0x41, 0x42, 0x43, 0x44]
    let encoder = PayloadEncoder::new(); // key [0xAA, 0x55]
    let encoded = encoder.encode(&payload, EncodingType::Xor);
    let header = encoder.generate_c_header(&encoded);

    // Each encoded byte should appear as 0xNN in the header
    for &byte in &encoded.data {
        let hex = format!("0x{:02X}", byte);
        assert!(
            header.contains(&hex),
            "Encoded byte {} not found in header",
            hex
        );
    }
}
