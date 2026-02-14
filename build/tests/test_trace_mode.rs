use build::TraceMode;

#[test]
fn test_serde_roundtrip_all_variants() {
    let modes = vec![
        TraceMode::Off,
        TraceMode::Api,
        TraceMode::BB,
        TraceMode::ApiPlusBB,
        TraceMode::Lines,
        TraceMode::LinesAroundBB(0),
        TraceMode::LinesAroundBB(42),
        TraceMode::LinesAroundBB(u32::MAX),
        TraceMode::All,
    ];
    for mode in &modes {
        let json = serde_json::to_string(mode).unwrap();
        let deserialized: TraceMode = serde_json::from_str(&json).unwrap();
        assert_eq!(*mode, deserialized, "roundtrip failed for {:?}", mode);
    }
}

#[test]
fn test_serde_specific_json_values() {
    assert_eq!(serde_json::to_string(&TraceMode::Off).unwrap(), r#""off""#);
    assert_eq!(serde_json::to_string(&TraceMode::Api).unwrap(), r#""api""#);
    assert_eq!(serde_json::to_string(&TraceMode::BB).unwrap(), r#""bb""#);
    assert_eq!(
        serde_json::to_string(&TraceMode::ApiPlusBB).unwrap(),
        r#""api+bb""#
    );
    assert_eq!(
        serde_json::to_string(&TraceMode::Lines).unwrap(),
        r#""lines""#
    );
    assert_eq!(
        serde_json::to_string(&TraceMode::All).unwrap(),
        r#""all""#
    );
}

#[test]
fn test_serde_lines_around_bb() {
    let mode = TraceMode::LinesAroundBB(42);
    let json = serde_json::to_string(&mode).unwrap();
    let back: TraceMode = serde_json::from_str(&json).unwrap();
    assert_eq!(back, TraceMode::LinesAroundBB(42));

    // Zero parameter
    let mode0 = TraceMode::LinesAroundBB(0);
    let json0 = serde_json::to_string(&mode0).unwrap();
    let back0: TraceMode = serde_json::from_str(&json0).unwrap();
    assert_eq!(back0, TraceMode::LinesAroundBB(0));
}

#[test]
fn test_deserialize_rejects_invalid() {
    assert!(serde_json::from_str::<TraceMode>(r#""unknown""#).is_err());
    assert!(serde_json::from_str::<TraceMode>(r#""OFF""#).is_err());
    assert!(serde_json::from_str::<TraceMode>("42").is_err());
    assert!(serde_json::from_str::<TraceMode>("null").is_err());
}

#[test]
fn test_debug_impl() {
    let modes = vec![
        TraceMode::Off,
        TraceMode::Api,
        TraceMode::BB,
        TraceMode::ApiPlusBB,
        TraceMode::Lines,
        TraceMode::LinesAroundBB(7),
        TraceMode::All,
    ];
    for mode in &modes {
        let dbg = format!("{:?}", mode);
        assert!(!dbg.is_empty(), "Debug output was empty for {:?}", mode);
    }
}
