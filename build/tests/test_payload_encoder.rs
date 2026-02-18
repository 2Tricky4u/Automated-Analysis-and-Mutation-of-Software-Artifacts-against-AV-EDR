mod common;

use build::{EncodingType, PayloadEncoder, generate_test_payload};

#[test]
fn test_xor_roundtrip_multiple_payloads() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", common::payload_empty()),
        ("tiny", common::payload_tiny()),
        ("small", common::payload_small()),
        ("typical", common::payload_typical()),
        ("large", common::payload_large()),
        ("all_zeros", common::payload_all_zeros()),
        ("all_ff", common::payload_all_ff()),
        ("sequential", common::payload_sequential()),
    ];
    let encoder = PayloadEncoder::with_xor_key([0xAA, 0x55]);
    for (name, payload) in &cases {
        let encoded = encoder.encode(payload, EncodingType::Xor);
        let decoded: Vec<u8> = encoded
            .data
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ [0xAA, 0x55][i % 2])
            .collect();
        assert_eq!(
            payload,
            &decoded,
            "FAILED roundtrip for '{}' (len={})",
            name,
            payload.len()
        );
    }
}

#[test]
fn test_xor_roundtrip_multiple_keys() {
    let keys: &[[u8; 2]] = &[[0, 0], [0xFF, 0xFF], [0x01, 0x80], [0xDE, 0xAD]];
    let payload = common::payload_small();

    for key in keys {
        let encoder = PayloadEncoder::with_xor_key(*key);
        let encoded = encoder.encode(&payload, EncodingType::Xor);
        let decoded: Vec<u8> = encoded
            .data
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ key[i % 2])
            .collect();
        assert_eq!(
            payload, decoded,
            "FAILED roundtrip for key [{:#04X}, {:#04X}]",
            key[0], key[1]
        );
    }
}

#[test]
fn test_xor_empty_payload() {
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&[], EncodingType::Xor);
    assert!(encoded.data.is_empty());
    assert_eq!(encoded.encoding, EncodingType::Xor);
}

#[test]
fn test_english_encoding_word_count() {
    let encoder = PayloadEncoder::new();
    let sizes = [1, 3, 10, 64, 256];

    for &sz in &sizes {
        let payload: Vec<u8> = (0..sz).map(|i| (i % 256) as u8).collect();
        let encoded = encoder.encode(&payload, EncodingType::English);
        let word_string = String::from_utf8(encoded.data.clone()).unwrap();
        let word_count = word_string.split_whitespace().count();
        assert_eq!(
            word_count, sz as usize,
            "Expected {} words for {}-byte payload, got {}",
            sz, sz, word_count
        );
    }
}

#[test]
fn test_english_all_256_bytes() {
    let encoder = PayloadEncoder::new();
    let payload = common::payload_sequential();
    let encoded = encoder.encode(&payload, EncodingType::English);
    let word_string = String::from_utf8(encoded.data).unwrap();
    let words: Vec<&str> = word_string.split_whitespace().collect();

    assert_eq!(words.len(), 256);
    // First few words should be the common dictionary words
    assert_eq!(words[0], "the"); // byte 0x00
    assert_eq!(words[1], "be"); // byte 0x01
    assert_eq!(words[2], "to"); // byte 0x02
}

#[test]
fn test_c_header_xor_structure() {
    let encoder = PayloadEncoder::with_xor_key([0xDE, 0xAD]);
    let payload = common::payload_small();
    let encoded = encoder.encode(&payload, EncodingType::Xor);
    let header = encoder.generate_c_header(&encoded);

    assert!(
        header.contains(&format!("#define PAYLOAD_LEN {}", payload.len())),
        "Missing PAYLOAD_LEN define"
    );
    assert!(header.contains("XOR_KEY[2]"), "Missing XOR_KEY array");
    assert!(
        header.contains("supermega_payload[PAYLOAD_LEN]"),
        "Missing supermega_payload"
    );
    assert!(header.contains("0xDE"), "Missing key byte 0");
    assert!(header.contains("0xAD"), "Missing key byte 1");
}

#[test]
fn test_c_header_english_structure() {
    let encoder = PayloadEncoder::new();
    let payload = vec![0u8, 1, 2];
    let encoded = encoder.encode(&payload, EncodingType::English);
    let header = encoder.generate_c_header(&encoded);

    assert!(header.contains("DICTIONARY[]"), "Missing DICTIONARY array");
    assert!(
        header.contains("supermega_payload_str[]"),
        "Missing supermega_payload_str"
    );
    assert!(
        header.contains("#define PAYLOAD_LEN"),
        "Missing PAYLOAD_LEN"
    );
}

#[test]
fn test_encoding_type_from_str() {
    assert_eq!(EncodingType::from_str("xor").unwrap(), EncodingType::Xor);
    assert_eq!(EncodingType::from_str("XOR").unwrap(), EncodingType::Xor);
    assert_eq!(EncodingType::from_str("Xor").unwrap(), EncodingType::Xor);
    assert_eq!(
        EncodingType::from_str("english").unwrap(),
        EncodingType::English
    );
    assert_eq!(
        EncodingType::from_str("ENGLISH").unwrap(),
        EncodingType::English
    );
    assert!(EncodingType::from_str("aes").is_err());
    assert!(EncodingType::from_str("").is_err());
}

#[test]
fn test_encoding_type_decoder_module() {
    assert_eq!(EncodingType::Xor.decoder_module(), "xor");
    assert_eq!(EncodingType::English.decoder_module(), "english");
}

#[test]
fn test_generate_test_payload_sizes() {
    // Size 0 — all NOPs, no INT3 trailer
    let p0 = generate_test_payload(0);
    assert_eq!(p0.len(), 0);

    // Size 1 — too small for 2-byte INT3 trailer, stays all NOPs
    let p1 = generate_test_payload(1);
    assert_eq!(p1.len(), 1);
    assert_eq!(p1[0], 0x90);

    // Size 2 — exactly the INT3 trailer
    let p2 = generate_test_payload(2);
    assert_eq!(p2.len(), 2);
    assert_eq!(p2[0], 0xCC);
    assert_eq!(p2[1], 0xCC);

    // Size 10 — 8 NOPs + 2 INT3
    let p10 = generate_test_payload(10);
    assert_eq!(p10.len(), 10);
    for i in 0..8 {
        assert_eq!(p10[i], 0x90, "Expected NOP at index {}", i);
    }
    assert_eq!(p10[8], 0xCC);
    assert_eq!(p10[9], 0xCC);

    // Size 1000
    let p1000 = generate_test_payload(1000);
    assert_eq!(p1000.len(), 1000);
    assert_eq!(p1000[0], 0x90);
    assert_eq!(p1000[997], 0x90);
    assert_eq!(p1000[998], 0xCC);
    assert_eq!(p1000[999], 0xCC);
}

// ── C header validation tests ───────────────────────────────────────────────

#[test]
fn test_xor_header_array_count_matches_payload_len() {
    let sizes = [1, 4, 12, 13, 24, 100, 256, 1000];

    for &sz in &sizes {
        let payload: Vec<u8> = (0..sz).map(|i| (i % 256) as u8).collect();
        let encoder = PayloadEncoder::new();
        let encoded = encoder.encode(&payload, EncodingType::Xor);
        let header = encoder.generate_c_header(&encoded);

        let defined_len = common::parse_payload_len(&header);
        assert_eq!(
            defined_len, sz as usize,
            "PAYLOAD_LEN mismatch for size {}",
            sz
        );

        let hex_count = common::count_hex_bytes_in_array(&header);
        assert_eq!(
            hex_count, sz as usize,
            "Hex byte count in array doesn't match PAYLOAD_LEN for size {}",
            sz
        );
    }
}

#[test]
fn test_english_header_word_count_matches_payload_len() {
    let sizes = [1, 4, 50, 256];

    for &sz in &sizes {
        let payload: Vec<u8> = (0..sz).map(|i| (i % 256) as u8).collect();
        let encoder = PayloadEncoder::new();
        let encoded = encoder.encode(&payload, EncodingType::English);
        let header = encoder.generate_c_header(&encoded);

        let defined_len = common::parse_payload_len(&header);
        assert_eq!(
            defined_len, sz as usize,
            "English PAYLOAD_LEN mismatch for size {}",
            sz
        );
    }
}

#[test]
fn test_c_header_balanced_braces() {
    let encoder = PayloadEncoder::new();

    // XOR header
    let payload = common::payload_typical();
    let encoded = encoder.encode(&payload, EncodingType::Xor);
    let header = encoder.generate_c_header(&encoded);

    let open = header.matches('{').count();
    let close = header.matches('}').count();
    assert_eq!(
        open, close,
        "XOR header has unbalanced braces: {} open, {} close",
        open, close
    );

    // English header
    let encoded_en = encoder.encode(&payload, EncodingType::English);
    let header_en = encoder.generate_c_header(&encoded_en);

    let open_en = header_en.matches('{').count();
    let close_en = header_en.matches('}').count();
    assert_eq!(
        open_en, close_en,
        "English header has unbalanced braces: {} open, {} close",
        open_en, close_en
    );
}

#[test]
fn test_c_header_has_include_guard() {
    let encoder = PayloadEncoder::new();
    let payload = common::payload_small();

    for encoding in &[EncodingType::Xor, EncodingType::English] {
        let encoded = encoder.encode(&payload, *encoding);
        let header = encoder.generate_c_header(&encoded);

        assert!(
            header.contains("#ifndef PAYLOAD_H"),
            "{:?} header missing #ifndef guard",
            encoding
        );
        assert!(
            header.contains("#define PAYLOAD_H"),
            "{:?} header missing #define guard",
            encoding
        );
        assert!(
            header.contains("#endif"),
            "{:?} header missing #endif",
            encoding
        );
    }
}

#[test]
fn test_c_header_large_payload_valid() {
    let payload = vec![0xABu8; 16384]; // 16KB
    let encoder = PayloadEncoder::new();
    let encoded = encoder.encode(&payload, EncodingType::Xor);
    let header = encoder.generate_c_header(&encoded);

    assert!(
        header.contains("#define PAYLOAD_LEN 16384"),
        "Large payload PAYLOAD_LEN incorrect"
    );
    assert!(
        header.len() > 16384,
        "Header should be larger than raw payload (hex formatting)"
    );

    // Braces should still be balanced
    assert_eq!(header.matches('{').count(), header.matches('}').count());
}

#[test]
fn test_c_header_deterministic() {
    let encoder = PayloadEncoder::new();
    let payload = common::payload_typical();

    let encoded1 = encoder.encode(&payload, EncodingType::Xor);
    let header1 = encoder.generate_c_header(&encoded1);

    let encoded2 = encoder.encode(&payload, EncodingType::Xor);
    let header2 = encoder.generate_c_header(&encoded2);

    assert_eq!(
        header1, header2,
        "Same payload should produce identical headers"
    );
}
